use std::{future::Future, path::PathBuf, pin::Pin, process::Stdio, time::Duration};

use hypha_messages::Executor;
use nix::{
    libc::pid_t,
    sys::signal::{self, Signal},
    unistd::Pid,
};
use tokio::{
    fs::create_dir_all,
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    time::sleep,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

use crate::{
    connector::Connector,
    executor::{Error, Execution, JobExecutor, bridge::Bridge},
    network::Network,
};

pub struct ProcessExecutor {
    connector: Connector<Network>,
}

pub struct ProcessExecution {
    bridge: Bridge,
    task_tracker: TaskTracker,
}

impl Execution for ProcessExecution {
    fn wait<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.task_tracker.wait().await;
            // NOTE: Bridge::wait returns Result; we currently ignore errors here
            // since the Execution trait does not propagate them.
            let _ = self.bridge.wait().await;
        })
    }
}

impl ProcessExecutor {
    pub(crate) fn new(connector: Connector<Network>) -> Self {
        ProcessExecutor { connector }
    }
}

#[allow(refining_impl_trait)]
impl JobExecutor for ProcessExecutor {
    async fn execute(
        &self,
        job: hypha_messages::JobSpec,
        cancel: CancellationToken,
    ) -> Result<ProcessExecution, Error> {
        let id = Uuid::new_v4();
        let sock_path = PathBuf::from(format!("/tmp/hypha-{}.sock", id));
        let work_dir = PathBuf::from(format!("/tmp/hypha-{}", id));
        create_dir_all(&work_dir).await?;

        // NOTE: Create a bridge to allow for process <> network communication
        let bridge = Bridge::try_new(
            self.connector.clone(),
            work_dir.clone(),
            sock_path.clone(),
            cancel.clone(),
        )
        .await?;

        let job_json = serde_json::to_string(&job).expect("valid JobSpec JSON");

        // This should come from the config and should be bassed when crearting the executor
        let mut process = Command::new(get_process_call(&job.executor))
            .args(get_process_args(&job.executor))
            .arg("--socket")
            .args([&sock_path]) // passes the actual path
            .args(["--work-dir"])
            .arg(&work_dir) // passes the actual path
            .args(["--job"])
            .arg(job_json) // serialized JobSpec JSON
            .stdout(Stdio::piped())
            .spawn()?;

        let task_tracker = TaskTracker::new();
        let shutdown = cancel.clone();
        task_tracker.spawn(async move {
            let stdout = process.stdout.take().expect("stdout is available");

            // Stream output.
            let mut lines = BufReader::new(stdout).lines();

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        tracing::trace!("Received shutdown signal. Stopping executor process");

                        // Send SIGTERM to process
                        // TODO: This is only available in UNIX environment.
                        //       We need to have a Windows-specific code-path
                        //       if we want it to work there as well.
                        if let Some(pid) = process.id() {
                            if let Err(e) = signal::kill(Pid::from_raw(pid as pid_t), Signal::SIGTERM) {
                                tracing::warn!(error = ?e, "Failed to send SIGTERM to executor process");
                            }
                        } else {
                            tracing::trace!("Executor process already exited");
                        }
                        break;
                    }
                    line = lines.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                println!("{line}")
                            }
                            Ok(None) => {
                                // TODO
                            }
                            Err(_) => {
                                // TODO
                            }
                        }
                    }
                    // Received if the driver stopped.
                    _ = process.wait() => {
                        tracing::debug!("Executor process task terminated");
                        // TODO: Decide what to do if the process failed.
                        //       We could, e.g., restart it.
                        break
                    }
                }
            }

            tokio::select! {
                status = process.wait() => {
                    tracing::trace!(status = ?status, "Executor task exited");
                }
                // If the driver process does not exit in time, send SIGKILL.
                _ = sleep(Duration::from_secs(5)) => {
                    tracing::trace!("Executor task didn't exit in time, sending SIGKILL");
                    if let Err(e) = process.kill().await {
                        tracing::warn!(error = ?e, "Failed to send SIGKILL to executor process");
                    }
                }
            }
        });

        task_tracker.close();

        // TODO: spawn and manage the actual process once the bridge is ready
        Ok(ProcessExecution {
            bridge,
            task_tracker,
        })
    }
}

fn get_process_args(executor: &Executor) -> Vec<String> {
    match executor {
        Executor::DiLoCoTransformer { .. } => {
            vec![
                "run".into(),
                "--directory".into(),
                "drivers/hypha-accelerate-driver/src".into(),
                "accelerate".into(),
                "launch".into(),
                "--config-file".into(),
                "test.yaml".into(),
                "training.py".into(),
            ]
        }
        _ => {
            vec![".".into()]
        }
    }
}

fn get_process_call(executor: &Executor) -> String {
    match executor {
        Executor::DiLoCoTransformer { .. } => "uv".into(),
        _ => ".".into(),
    }
}
