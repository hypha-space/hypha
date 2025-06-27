use std::{
    collections::HashMap,
    fs::Permissions,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use libp2p::PeerId;
use tokio::{
    fs::set_permissions,
    net::UnixListener,
    sync::{Mutex, mpsc},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

use crate::Network;

mod router;

/// Create a new driver instance.
///
/// The driver instance will create a socket at `socket_path` and execute the command
/// specified by `command` with the given `arguments`.
/// This command will be executed in the background and will be terminated when the driver is dropped.
/// It communicates through the driver API provided by the socket.
pub async fn try_new<P1, P2>(
    network: Network,
    socket_path: P1,
    work_dir: P2,
    token: CancellationToken,
) -> std::io::Result<Driver>
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
{
    tracing::info!("Starting driver");

    let (tx, rx) = mpsc::channel(8);
    let server_tracker = TaskTracker::new();

    let tasks = Arc::new(Mutex::new(HashMap::new()));

    let app = router::new(network, work_dir, tasks.clone(), rx);

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

    Ok(Driver {
        tasks,
        tx,
        socket_path: PathBuf::from(socket_path.as_ref()),
        server_tracker,
    })
}

#[derive(Debug, Clone)]
struct Task {
    id: Uuid,
    scheduler_peer_id: PeerId,
    parameter_server_peer_id: PeerId,
}

#[derive(Clone)]
pub struct Driver {
    tasks: Arc<Mutex<HashMap<Uuid, Task>>>,
    tx: mpsc::Sender<Task>,
    socket_path: PathBuf,
    server_tracker: TaskTracker,
}

impl Driver {
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
        let _ = self
            .tx
            .send(Task {
                id: task_id,
                scheduler_peer_id,
                parameter_server_peer_id,
            })
            .await;
    }

    /// Wait for all driver tasks to be completed.
    /// This will also delete the API socket.
    /// To be called after the cancellation token has been cancelled.
    pub async fn wait(&self) {
        self.server_tracker.wait().await;

        // If for some reason we can't remove the socket, ignore the error.
        let _ = std::fs::remove_file(&self.socket_path);
    }
}
