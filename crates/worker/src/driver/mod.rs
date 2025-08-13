use std::{
    collections::HashMap,
    ffi::OsStr,
    fs::Permissions,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use hypha_network::{request_response::RequestResponseInterface, stream::StreamSenderInterface};
use libp2p::{PeerId, request_response::cbor::codec::Codec};
use nix::{
    libc::pid_t,
    sys::signal::{self, Signal},
    unistd::Pid,
};
use tokio::{
    fs::set_permissions,
    io::{AsyncBufReadExt, BufReader},
    net::UnixListener,
    process::Command,
    sync::{Mutex, broadcast, mpsc},
    time::sleep,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

use crate::{driver::router::TrainingConfig, network::Network};

pub use crate::driver::{
    parameter_server::try_new_parameter_server,
    router::{Parameters, TrainingInputs},
};

mod parameter_server;
mod router;

/// Create a new driver instance.
///
/// The driver instance will create a socket at `socket_path` and execute the command
/// specified by `command` with the given `arguments`.
/// This command will be executed in the background and will be terminated when the driver is dropped.
/// It communicates through the driver API provided by the socket.
pub async fn try_new_command<P, S, I>(
    network: Network,
    socket_path: P,
    command: S,
    arguments: I,
    token: CancellationToken,
) -> std::io::Result<Driver>
where
    P: AsRef<Path>,
    S: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
{
    tracing::info!("Starting driver");

    let (message_tx, message_rx) = mpsc::channel(8);

    let (tasks_channel, _) = broadcast::channel(8);
    let (inputs_channel, _) = broadcast::channel(8);
    let (outputs_tx, outputs_rx) = mpsc::channel(8);
    let server_tracker = TaskTracker::new();
    let driver_tracker = TaskTracker::new();

    let tasks = Arc::new(Mutex::new(HashMap::new()));

    let message_dispatcher = {
        let tasks_tx = tasks_channel.clone();
        let inputs_tx = inputs_channel.clone();

        tokio::select! {
            Some(message) = message_rx.recv() => {
                match message {
                    Message::Send(peer_id, path) => {
                        inputs_tx.send(TrainingInputs::Parameters(Parameters {
                            version: 0,
                            path: path.to_path_buf(),
                        }));
                    }
                    Message::StartTraining(scheduler_peer_id, parameter_server_peer_id, task_id) => {
                        tasks_tx.send(TrainingConfig {
                            scheduler_peer_id: scheduler_peer_id,
                            parameter_server_peer_id: parameter_server_peer_id,
                            task_id: task_id,
                            model: "EleutherAI/gpt-neo-125m".to_string(),
                            dataset: "datablations/c4-filter-small".to_string(),
                            epochs: 2,
                            batch_size: 4,
                            learning_rate: 1e-5,
                            learning_rate_scheduler: "".to_string(),
                            optimizer: "AdamW".to_string(),
                            checkpointing: 1,
                        });
                    }
                }
            },
            _ = outputs_rx.recv() => {

            }
        };
    };

    let app = router::new(
        network,
        tasks.clone(),
        tasks_channel.clone(),
        inputs_channel.clone(),
    );

    // This will create a file that we later delete as part of 'wait'.
    let listener = UnixListener::bind(&socket_path)?;
    // Access is restricted to the current user.
    set_permissions(&socket_path, Permissions::from_mode(0o600)).await?;

    let shutdown = token.clone();
    server_tracker.spawn(
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown.cancelled().await;
            })
            .into_future(),
    );
    server_tracker.close();

    let command = command.as_ref().to_owned();
    let arguments = arguments
        .into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect::<Vec<_>>();

    let mut driver_process = Command::new(command)
        .args(arguments)
        .stdout(Stdio::piped())
        .spawn()?;

    let socket_path = PathBuf::from(socket_path.as_ref());

    driver_tracker.spawn(async move {
        let stdout = driver_process.stdout.take().expect("stdout is available");

        // Stream output.
        let mut lines = BufReader::new(stdout).lines();

        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    tracing::trace!("Received shutdown signal. Stopping driver process");

                    // Send SIGTERM to process
                    // TODO: This is only available in UNIX environment.
                    //       We need to have a Windows-specific code-path
                    //       if we want it to work there as well.
                    if let Some(pid) = driver_process.id() {
                        if let Err(e) = signal::kill(Pid::from_raw(pid as pid_t), Signal::SIGTERM) {
                            tracing::warn!(error = ?e, "Failed to send SIGTERM to driver process");
                        }
                    } else {
                        tracing::trace!("Driver process already exited");
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
                _ = driver_process.wait() => {
                    tracing::debug!("Driver task terminated");
                    // TODO: Decide what to do if the process failed.
                    //       We could, e.g., restart it.
                    break
                }
            }
        }

        tokio::select! {
            status = driver_process.wait() => {
                tracing::trace!(status = ?status, "Driver task exited");
            }
            // If the driver process does not exit in time, send SIGKILL.
            _ = sleep(Duration::from_secs(5)) => {
                tracing::trace!("Driver task didn't exit in time, sending SIGKILL");
                if let Err(e) = driver_process.kill().await {
                    tracing::warn!(error = ?e, "Failed to send SIGKILL to driver process");
                }
            }
        }

        server_tracker.wait().await;

        // If for some reason we can't remove the socket, ignore the error.;
        let _ = std::fs::remove_file(&socket_path);
    });

    driver_tracker.close();

    Ok(Driver::new(network, tasks, message_tx, driver_tracker))
}

// TODO: Remove this hardcoded driver, instead add configuration options
// to specify the driver configuration there.
pub async fn try_new_accelerate<P1, P2>(
    network: Network,
    socket_path: P1,
    work_dir: P2,
    cancel: CancellationToken,
) -> std::io::Result<Driver>
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
{
    let path = PathBuf::from(socket_path.as_ref());
    let work_path = PathBuf::from(work_dir.as_ref());

    try_new_command(
        network,
        socket_path,
        "uv",
        vec![
            "run",
            "--directory",
            "drivers/hypha-accelerate-driver",
            "accelerate",
            "launch",
            "--config-file",
            "test.yaml",
            "src/training.py",
            "--socket",
            path.to_str().expect("socket path is valid UTF-8"),
            "--work-dir",
            work_path
                .to_str()
                .expect("work directory path is valid UTF-8"),
        ],
        cancel,
    )
    .await
}

#[derive(Debug, Clone)]
pub struct Task {
    id: Uuid,
    scheduler_peer_id: PeerId,
    parameter_server_peer_id: PeerId,
}

enum Message {
    StartTraining(PeerId, PeerId, Uuid),
    Send(PeerId, PathBuf),
}

// TODO: remove channels, only have mpsc for messages to the driver loop
// Driver uses direct access to network for all messages on the network
#[derive(Clone)]
pub struct Driver {
    tasks: Arc<Mutex<HashMap<Uuid, Task>>>,
    message_sender: mpsc::Sender<Message>,

    driver_tracker: TaskTracker,
}

impl Driver {
    pub fn new(
        tasks: Arc<Mutex<HashMap<Uuid, Task>>>,
        message_sender: mpsc::Sender<Message>,
        driver_tracker: TaskTracker,
    ) -> Driver {
        Driver {
            tasks,
            message_sender,
            driver_tracker,
        }
    }

    pub async fn start_training(
        &mut self,
        scheduler_peer_id: PeerId,
        parameter_server_peer_id: PeerId,
        task_id: Uuid,
    ) {
        self.tasks.lock().await.insert(
            task_id,
            Task {
                id: task_id,
                scheduler_peer_id,
                parameter_server_peer_id,
            },
        );
        let _ = self.tasks_channel.send(TrainingConfig {
            scheduler_peer_id: scheduler_peer_id,
            parameter_server_peer_id: parameter_server_peer_id,
            task_id: task_id,
            model: "EleutherAI/gpt-neo-125m".to_string(),
            dataset: "datablations/c4-filter-small".to_string(),
            epochs: 2,
            batch_size: 4,
            learning_rate: 1e-5,
            learning_rate_scheduler: "".to_string(),
            optimizer: "AdamW".to_string(),
            checkpointing: 1,
        });
    }

    /// Wait for all driver tasks to be completed.
    /// This will also delete the API socket.
    /// To be called after the cancellation token has been cancelled.
    pub async fn wait(&self) {
        // The driver process is accessing the API provided by axum.
        // Wait for it to terminate before stopping axum.
        self.driver_tracker.wait().await;
    }

    pub async fn send_data(&self, peer_id: PeerId, path: &Path) {
        let _ = self.receive_channel.send((
            peer_id,
            TrainingInputs::Parameters(Parameters {
                version: 0,
                path: path.to_path_buf(),
            }),
        ));
    }

    pub fn receive_data(&self) -> broadcast::Receiver<(PeerId, TrainingInputs)> {
        self.send_channel.subscribe()
    }
}
