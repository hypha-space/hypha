use std::{future::Future, path::PathBuf, pin::Pin, process::Stdio, time::Duration};

use hypha_messages::Executor;
use libp2p::PeerId;
use nix::{
    libc::pid_t,
    sys::signal::{self, Signal},
    unistd::Pid,
};
use tokio::{
    fs,
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    time::sleep,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

use crate::{
    config::{ExecutorConfig, ExecutorRuntime},
    connector::Connector,
    executor::{Error, Execution, JobExecutor, bridge::Bridge},
    network::Network,
};

pub struct ProcessExecutor {
    connector: Connector<Network>,
    network: Network,
    work_dir_base: PathBuf,
    executor_config: ExecutorConfig,
}

pub struct ProcessExecution {
    task_tracker: TaskTracker,
}

impl Execution for ProcessExecution {
    fn wait<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.task_tracker.wait().await;
        })
    }
}

struct RuntimeContext {
    socket_path: String,
    work_dir: String,
    job_json: String,
}

impl RuntimeContext {
    fn new(socket_path: String, work_dir: String, job_json: String) -> Self {
        Self {
            socket_path,
            work_dir,
            job_json,
        }
    }
}

impl ProcessExecutor {
    pub(crate) fn new(
        connector: Connector<Network>,
        network: Network,
        work_dir_base: PathBuf,
        executor_config: ExecutorConfig,
    ) -> Self {
        ProcessExecutor {
            connector,
            network,
            work_dir_base,
            executor_config,
        }
    }
}

#[allow(refining_impl_trait)]
impl JobExecutor for ProcessExecutor {
    async fn execute(
        &self,
        job: hypha_messages::JobSpec,
        cancel: CancellationToken,
        _job_id: Uuid,
        scheduler: PeerId,
    ) -> Result<ProcessExecution, Error> {
        if !matches!(&job.executor, Executor::Train(_)) {
            return Err(Error::UnsupportedJobSpec());
        }

        if !matches!(
            self.executor_config.runtime(),
            ExecutorRuntime::Process { .. }
        ) {
            return Err(Error::InvalidExecutorConfig(
                "executor config must be process type".into(),
            ));
        }

        let id = Uuid::new_v4();
        let work_dir = self.work_dir_base.join(format!("hypha-{}", id));
        let sock_path = work_dir.join("bridge.sock");
        fs::create_dir_all(&work_dir).await?;

        let bridge = Bridge::try_new(
            self.connector.clone(),
            self.network.clone(),
            work_dir.clone(),
            sock_path.clone(),
            cancel.clone(),
            job.job_id,
            scheduler,
        )
        .await?;

        let cmd = self.executor_config.cmd().ok_or_else(|| {
            Error::InvalidExecutorConfig("missing command for process executor".into())
        })?;
        let runtime = RuntimeContext::new(
            sock_path.to_string_lossy().into_owned(),
            work_dir.to_string_lossy().into_owned(),
            serde_json::to_string(&job).expect("valid JobSpec JSON"),
        );

        let mut process = Command::new(cmd);
        let args = self
            .executor_config
            .args()
            .iter()
            .map(|arg| replace_placeholders(arg, &runtime))
            .collect::<Vec<_>>();

        process
            .args(args)
            .env("SOCKET_PATH", &runtime.socket_path)
            .env("WORK_DIR", &runtime.work_dir)
            .env("JOB_JSON", &runtime.job_json)
            .stdout(Stdio::piped());
        let mut process = process.spawn()?;

        let task_tracker = TaskTracker::new();
        let shutdown = cancel.clone();
        task_tracker.spawn(async move {
            let stdout = process.stdout.take().expect("stdout is available");
            let mut lines = BufReader::new(stdout).lines();

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        tracing::trace!("Received shutdown signal. Stopping executor process");
                        if let Some(pid) = process.id()
                            && let Err(e) = signal::kill(Pid::from_raw(pid as pid_t), Signal::SIGTERM)
                        {
                            tracing::warn!(error = ?e, "Failed to send SIGTERM to executor process");
                        }
                        break;
                    }
                    line = lines.next_line() => {
                        match line {
                            Ok(Some(line)) => println!("{line}"),
                            Ok(None) => {
                                tracing::debug!("Executor stdout stream exhausted");
                                break;
                            }
                            Err(e) => {
                                tracing::warn!(error = ?e, "Failed to read executor stdout");
                                break;
                            }
                        }
                    }
                    _ = process.wait() => {
                        tracing::debug!("Executor process task terminated");
                        break;
                    }
                }
            }

            tokio::select! {
                status = process.wait() => {
                    tracing::trace!(status = ?status, "Executor task exited");
                }
                _ = sleep(Duration::from_secs(5)) => {
                    tracing::trace!("Executor task didn't exit in time, sending SIGKILL");
                    if let Err(e) = process.kill().await {
                        tracing::warn!(error = ?e, "Failed to send SIGKILL to executor process");
                    }
                }
            }

            shutdown.cancel();
            let _ = bridge.wait().await;
            let _ = fs::remove_file(&sock_path).await;
            let _ = fs::remove_dir_all(&work_dir).await;
        });

        task_tracker.close();

        Ok(ProcessExecution { task_tracker })
    }
}

fn replace_placeholders(arg: &str, ctx: &RuntimeContext) -> String {
    arg.replace("{SOCKET_PATH}", &ctx.socket_path)
        .replace("{WORK_DIR}", &ctx.work_dir)
        .replace("{JOB_JSON}", &ctx.job_json)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod replace_placeholders {
        use super::RuntimeContext;

        #[test]
        fn substitutes_all_known_tokens() {
            let ctx = RuntimeContext::new(
                "/tmp/socket".into(),
                "/tmp/work".into(),
                "{\"id\":1}".into(),
            );
            let arg = "run --socket {SOCKET_PATH} --dir {WORK_DIR} --job {JOB_JSON}";

            let replaced = super::replace_placeholders(arg, &ctx);

            assert_eq!(
                replaced,
                "run --socket /tmp/socket --dir /tmp/work --job {\"id\":1}"
            );
        }

        #[test]
        fn leaves_unknown_tokens_intact() {
            let ctx = RuntimeContext::new("socket".into(), "work".into(), "job".into());
            let arg = "noop {UNKNOWN} literal";

            let replaced = super::replace_placeholders(arg, &ctx);

            assert_eq!(replaced, arg);
        }
    }
}
