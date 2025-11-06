use std::{ffi::OsStr, future::Future, path::PathBuf, pin::Pin, process::Stdio, time::Duration};

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
    connector::Connector,
    executor::{Error, Execution, JobExecutor, bridge::Bridge},
    network::Network,
};

pub struct ProcessExecutor {
    connector: Connector<Network>,
    network: Network,
    work_dir_base: PathBuf,
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

impl ProcessExecutor {
    pub(crate) fn new(
        connector: Connector<Network>,
        network: Network,
        work_dir_base: PathBuf,
    ) -> Self {
        ProcessExecutor {
            connector,
            network,
            work_dir_base,
        }
    }
}

#[allow(refining_impl_trait)]
impl JobExecutor for ProcessExecutor {
    async fn execute(
        &self,
        job: hypha_messages::JobSpec,
        cancel: CancellationToken,
        _task_id: Uuid,
        scheduler: PeerId,
    ) -> Result<ProcessExecution, Error> {
        let id = Uuid::new_v4();
        // NOTE: Place per-job workdir under configured base and keep socket within it
        let work_dir = self.work_dir_base.join(format!("hypha-{}", id));
        let sock_path = work_dir.join("bridge.sock");
        fs::create_dir_all(&work_dir).await?;

        // NOTE: Create a bridge to allow for process <> network communication
        let bridge = Bridge::try_new(
            self.connector.clone(),
            self.network.clone(),
            work_dir.clone(),
            sock_path.clone(),
            cancel.clone(),
            job.job_id,
            // task_id,
            scheduler,
        )
        .await?;

        let job_json = serde_json::to_string(&job).expect("valid JobSpec JSON");

        // This should come from the config and should be bassed when crearting the executor
        let run_future = try_run(
            get_process_call(&job.executor),
            get_process_args(&job.executor).into_iter().chain(
                vec![
                    "--socket".to_string(),
                    sock_path.to_str().expect("a valid path").to_string(),
                    "--work-dir".to_string(),
                    work_dir.to_str().expect("a valid path").to_string(),
                    "--job".to_string(),
                    job_json,
                ]
                .into_iter(),
            ),
            cancel.clone(),
        )?;

        let task_tracker = TaskTracker::new();
        let shutdown = cancel.clone();
        task_tracker.spawn(async move {
            run_future.await;

            // At this point the process is no longer running but the bridge still is.
            // By cancelling here, the bridge will stop serving and terminate.
            shutdown.cancel();

            // NOTE: Bridge::wait returns Result; we currently ignore errors here
            // since we cannot propagate them.
            let _ = bridge.wait().await;

            // Clean up.
            let _ = fs::remove_file(&sock_path).await;
            let _ = fs::remove_dir_all(&work_dir).await;
        });

        task_tracker.close();

        // TODO: spawn and manage the actual process once the bridge is ready
        Ok(ProcessExecution { task_tracker })
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

fn try_run<S, I>(
    command: S,
    arguments: I,
    token: CancellationToken,
) -> Result<impl Future<Output = ()> + 'static, std::io::Error>
where
    S: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
{
    // This should come from the config and should be bassed when crearting the executor
    let mut process = Command::new(command)
        .args(arguments) // serialized JobSpec JSON
        .stdout(Stdio::piped())
        .spawn()?;

    Ok(async move {
        let stdout = process.stdout.take().expect("stdout is available");

        // Stream output.
        let mut lines = BufReader::new(stdout).lines();

        loop {
            tokio::select! {
                _ = token.cancelled() => {
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
    })
}

#[cfg(test)]
mod tests {
    use tokio::time;

    use super::*;

    #[tokio::test]
    async fn test_run_graceful_termination() {
        let token = CancellationToken::new();

        let process = tokio::spawn(
            try_run(
                "bash",
                vec!["-c", "while true; do sleep 60; done"],
                token.clone(),
            )
            .unwrap(),
        );

        // We need to wait a while for the process to actually run.
        sleep(Duration::from_secs(1)).await;

        token.cancel();
        process.await.unwrap();
    }

    #[tokio::test]
    async fn test_run_sigkill_termination() {
        let token = CancellationToken::new();

        // This command doesn't react to SIGTERM, only to SIGKILL.
        let process = tokio::spawn(
            try_run(
                "bash",
                vec!["-c", "trap \"\" SIGTERM; while true; do sleep 60; done"],
                token.clone(),
            )
            .unwrap(),
        );

        // We need to wait a while for the process to actually run.
        sleep(Duration::from_secs(1)).await;

        token.cancel();

        time::pause();
        time::advance(Duration::from_secs(5)).await;

        process.await.unwrap();
    }
}
