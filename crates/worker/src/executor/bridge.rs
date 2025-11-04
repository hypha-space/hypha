use std::{
    fs::Permissions,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::{StreamExt, stream};
use hypha_messages::{
    Fetch, Receive, Reference, Send,
    progress::{self, Request},
};
use hypha_network::request_response::RequestResponseError;
use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    fs::{self, set_permissions},
    io::{self, AsyncWriteExt},
    net::UnixListener,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use utoipa::OpenApi;
use uuid::Uuid;

use crate::{
    connector::{BoxAsyncRead, Connector, ConnectorError, ReadItem},
    network::Network,
};

#[derive(Error, Debug)]
pub enum Error {
    #[error("Network error")]
    Network(#[from] RequestResponseError),
    #[error("Connector error")]
    Connector(#[from] ConnectorError),
    #[error("I/O error")]
    Io(#[from] std::io::Error),
    #[error("Invalid job status: {0}")]
    InvalidStatus(String),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        #[derive(serde::Serialize)]
        struct ApiError<'a> {
            error: &'a str,
            detail: String,
        }
        match self {
            Error::Network(e) => {
                tracing::error!(error = %e, "bridge error: network");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError {
                        error: "network_error",
                        detail: e.to_string(),
                    }),
                )
                    .into_response()
            }
            Error::InvalidStatus(msg) => {
                tracing::warn!(detail = %msg, "bridge error: invalid_request");
                (
                    StatusCode::BAD_REQUEST,
                    Json(ApiError {
                        error: "invalid_request",
                        detail: msg,
                    }),
                )
                    .into_response()
            }
            Error::Connector(e) => {
                tracing::error!(error = %e, "bridge error: connector");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError {
                        error: "connector_error",
                        detail: e.to_string(),
                    }),
                )
                    .into_response()
            }
            Error::Io(e) => {
                tracing::error!(error = %e, "bridge error: io");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError {
                        error: "io_error",
                        detail: e.to_string(),
                    }),
                )
                    .into_response()
            }
        }
    }
}

struct SockState {
    work_dir: PathBuf,
    connector: Connector<Network>,
    network: Network,
    job_id: Uuid,
    // task_id: Uuid,
    scheduler: PeerId,
    task_tracker: TaskTracker,
    cancel: CancellationToken,
}

pub struct Bridge {
    task_tracker: TaskTracker,
    socket_path: PathBuf,
    cancel: CancellationToken,
}

impl Bridge {
    pub async fn try_new<P1, P2>(
        connector: Connector<Network>,
        network: Network,
        work_dir: P1,
        socket_path: P2,
        cancel: CancellationToken,
        job_id: Uuid,
        scheduler: PeerId,
    ) -> std::io::Result<Bridge>
    where
        P1: AsRef<std::path::Path>,
        P2: AsRef<std::path::Path>,
    {
        let task_tracker = TaskTracker::new();
        let cancel_token = cancel;

        let state = Arc::new(SockState {
            work_dir: PathBuf::from(work_dir.as_ref()),
            connector,
            network,
            job_id,
            scheduler,
            task_tracker: task_tracker.clone(),
            cancel: cancel_token.clone(),
        });

        let router = Router::new()
            .route("/openapi.json", get(openapi))
            .route("/resources/fetch", post(fetch_resource))
            .route("/resources/send", post(send_resource))
            .route("/resources/receive", post(receive_subscribe))
            .route("/status/send", post(send_status))
            .with_state(state);

        // This will create a file that we later delete as part of 'wait'.
        let listener = UnixListener::bind(&socket_path)?;

        // Access is restricted to the current user.
        set_permissions(&socket_path, Permissions::from_mode(0o600)).await?;

        let shutdown = cancel_token.clone();
        task_tracker.spawn(
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    shutdown.cancelled().await;
                })
                .into_future(),
        );

        Ok(Bridge {
            task_tracker,
            socket_path: PathBuf::from(socket_path.as_ref()),
            cancel: cancel_token,
        })
    }

    pub(crate) async fn wait(&self) -> Result<(), Error> {
        self.cancel.cancel();
        self.task_tracker.close();
        self.task_tracker.wait().await;

        // If for some reason we can't remove the socket, ignore the error.
        let _ = std::fs::remove_file(&self.socket_path);
        Ok(())
    }
}

#[derive(OpenApi)]
#[openapi(paths(openapi))]
struct SockApiDoc;

#[utoipa::path(
    get,
    path = "/openapi.json",
    responses(
        (status = OK, description = "JSON file", body = ())
    )
)]
async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(SockApiDoc::openapi())
}

#[derive(Debug, Serialize)]
struct FileResponse {
    path: String,
    size: u64,
}

async fn fetch_resource(
    State(state): State<Arc<SockState>>,
    Json(resource): Json<Fetch>,
) -> Result<Json<Vec<FileResponse>>, Error> {
    validate_fetch(&resource)?;

    let dir_rel = "artifacts".to_string();
    let dir_abs = safe_join(&state.work_dir, &dir_rel)?;
    fs::create_dir_all(&dir_abs).await?;

    let mut out: Vec<FileResponse> = Vec::new();
    let mut items = state.connector.fetch(resource).await?;
    let mut idx: usize = 0;
    while let Some(item) = items.next().await.transpose().map_err(Error::Io)? {
        let (file_name, mut reader) = derive_name_and_reader(item, idx);
        let rel = format!("{}/{}", dir_rel, file_name);
        let abs = safe_join(&state.work_dir, &rel)?;
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).await?;
        }

        let mut file = fs::File::create(&abs).await?;
        let size = tokio::io::copy(&mut reader, &mut file).await?;
        file.sync_all().await?;
        tracing::info!(size, file = %abs.display(), "Copied resource");
        set_permissions(&abs, Permissions::from_mode(0o600)).await?;

        out.push(FileResponse { path: rel, size });
        idx += 1;
    }

    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
struct SendRequest {
    resource: Send,
    path: String,
}

async fn send_resource(
    State(state): State<Arc<SockState>>,
    Json(req): Json<SendRequest>,
) -> Result<(), Error> {
    let abs = safe_join(&state.work_dir, &req.path)?;
    let mut writers = match state.connector.send(req.resource).await {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = %e, file = %abs.display(), "send_resource: failed to open writers");
            return Err(Error::Connector(e));
        }
    };

    let cancel = state.cancel.clone();
    let file_path = abs.clone();
    let task_tracker = state.task_tracker.clone();

    // Copy the resource in the background to avoid blocking.
    task_tracker.spawn(async move {
        loop {
            let next_item = tokio::select! {
                _ = cancel.cancelled() => None,
                item = writers.next() => item,
            };

            let Some(item_result) = next_item else {
                if cancel.is_cancelled() {
                    tracing::debug!(file = %file_path.display(), "send_resource: task cancelled");
                }
                break;
            };

            let item = match item_result {
                Ok(item) => item,
                Err(err) => {
                    tracing::error!(error = %err, file = %file_path.display(), "send_resource: writer stream error");
                    break;
                }
            };

            let peer_id = item.meta.name.clone();
            let mut writer = item.writer;

            let mut reader = match fs::File::open(&file_path).await {
                Ok(file) => file,
                Err(err) => {
                    tracing::error!(error = %err, file = %file_path.display(), peer_id = %peer_id, "send_resource: failed to open file");
                    break;
                }
            };

            tracing::info!(peer_id = %peer_id, file = %file_path.display(), "Sending resource");

            let sent_bytes = match io::copy(&mut reader, &mut writer).await {
                Ok(bytes) => bytes,
                Err(err) => {
                    tracing::error!(error = %err, file = %file_path.display(), peer_id = %peer_id, "send_resource: failed to copy resource");
                    break;
                }
            };

            if let Err(err) = writer.shutdown().await {
                tracing::error!(error = %err, file = %file_path.display(), peer_id = %peer_id, "send_resource: failed to shutdown writer");
                break;
            }

            tracing::info!(size = sent_bytes, file = %file_path.display(), peer_id = %peer_id, "Sent resource");
        }
    });

    Ok(())
}

/// Validate and join relative path it under work_dir.
fn safe_join(work_dir: &Path, rel: &str) -> Result<PathBuf, Error> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(Error::InvalidStatus(
            "absolute paths are not allowed".into(),
        ));
    }

    // NOTE: Reject any parent directory components to avoid traversal
    if rel_path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(Error::InvalidStatus("path traversal is not allowed".into()));
    }
    Ok(work_dir.join(rel_path))
}

// TODO: We should not only validate the URI, but also check it against an allow list
// to restrict access to _trusted_ sources.
fn validate_fetch(resource: &Fetch) -> Result<(), Error> {
    match resource.as_ref() {
        Reference::Uri { value } => {
            if !(value.starts_with("http://") || value.starts_with("https://")) {
                return Err(Error::InvalidStatus(format!(
                    "invalid URI: expected http(s)://..., got `{}`",
                    value
                )));
            }
            Ok(())
        }
        Reference::HuggingFace {
            repository,
            filenames,
            ..
        } => {
            if repository.trim().is_empty() {
                return Err(Error::InvalidStatus("repository must not be empty".into()));
            }
            if filenames.is_empty() {
                return Err(Error::InvalidStatus("filenames must not be empty".into()));
            }
            Ok(())
        }
        Reference::Scheduler { .. } => Ok(()),
        _ => Err(Error::InvalidStatus("unsupported strategy".into())),
    }
}

#[derive(Debug, Deserialize)]
struct ReceiveSubscribeRequest {
    resource: Receive,
    path: Option<String>,
}

#[derive(Debug, Serialize)]
struct UpdatePointer {
    path: String,
    size: u64,
    from_peer: String,
}

async fn receive_subscribe(
    State(state): State<Arc<SockState>>,
    Json(req): Json<ReceiveSubscribeRequest>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>>, Error> {
    let dir_rel = req.path.unwrap_or_else(|| "incoming".to_string());
    let dir_abs = safe_join(&state.work_dir, &dir_rel)?;
    fs::create_dir_all(&dir_abs).await?;

    // Channel to push events to the SSE stream
    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(64);
    let connector = state.connector.clone();
    let work_dir = state.work_dir.clone();
    let resource = req.resource.clone();
    let cancel = state.cancel.clone();
    let task_tracker = state.task_tracker.clone();
    let dir_rel_clone = dir_rel.clone();

    // Background task: receive loops until the client disconnects or an error occurs
    task_tracker.spawn(async move {
        let mut incoming = match connector.receive(resource).await {
            Ok(s) => s,
            Err(err) => {
                tracing::error!(error = %err, path = %dir_rel_clone, "receive_subscribe: failed to start stream");
                return;
            }
        };
        let mut index = 0usize;
        while let Some(item_result) = tokio::select! {
            _ = cancel.cancelled() => {
                tracing::debug!(path = %dir_rel_clone, "receive_subscribe: task cancelled");
                None
            }
            item = incoming.next() => item,
        } {
            let item = match item_result {
                Ok(item) => item,
                Err(err) => {
                    tracing::error!(error = %err, path = %dir_rel_clone, "receive_subscribe: stream error");
                    break;
                }
            };
            let (file_name, mut reader) = derive_name_and_reader(item, index);
            let file_rel = format!("{}/{}", dir_rel_clone, file_name);
            let file_abs = match safe_join(&work_dir, &file_rel) {
                Ok(p) => p,
                Err(err) => {
                    tracing::error!(error = %err, file = %file_rel, "receive_subscribe: invalid target path");
                    break;
                }
            };
            if let Some(parent) = file_abs.parent() {
                match fs::create_dir_all(parent).await {
                    Ok(()) => (),
                    Err(err) => {
                        tracing::error!(error = %err, directory = %parent.display(), "receive_subscribe: failed to create directory");
                        break;
                    }
                }
            }
            let mut file = match fs::File::create(&file_abs).await {
                Ok(f) => f,
                Err(err) => {
                    tracing::error!(error = %err, file = %file_abs.display(), "receive_subscribe: failed to create file");
                    break;
                }
            };
            let size = match tokio::io::copy(&mut reader, &mut file).await {
                Ok(n) => n,
                Err(err) => {
                    tracing::error!(error = %err, file = %file_abs.display(), "receive_subscribe: failed to copy resource");
                    break;
                }
            };
            if let Err(err) = file.sync_all().await {
                tracing::warn!(error = %err, file = %file_abs.display(), "receive_subscribe: failed to sync file");
            }
            if let Err(err) = set_permissions(&file_abs, Permissions::from_mode(0o600)).await {
                tracing::warn!(error = %err, file = %file_abs.display(), "receive_subscribe: failed to set permissions");
            }

            tracing::info!(size, file = %file_abs.display(), "Received resource");

            let from_peer = file_name.split('.').next().unwrap_or("").to_string();
            let pointer = UpdatePointer {
                path: file_rel,
                size,
                from_peer,
            };
            let ev = match serde_json::to_string(&pointer) {
                Ok(data) => Event::default().data(data),
                Err(err) => {
                    tracing::error!(error = %err, "receive_subscribe: failed to serialize pointer");
                    Event::default().data(r#"{"error":"serialize"}"#)
                }
            };
            if tx.send(ev).await.is_err() {
                tracing::debug!(path = %dir_rel_clone, "receive_subscribe: client disconnected");
                break;
            }
            index += 1;
        }
    });

    let stream = stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|ev| (Ok(ev), rx))
    });

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(5))
            .text("keepalive"),
    ))
}

async fn send_status(
    State(state): State<Arc<SockState>>,
    Json(req): Json<progress::Progress>,
) -> Result<Json<progress::Response>, Error> {
    tracing::info!("{:?}", req);
    let response =
        hypha_network::request_response::RequestResponseInterface::<progress::Codec>::request(
            &state.network,
            state.scheduler,
            Request {
                job_id: state.job_id,
                progress: req,
            },
        )
        .await?;

    Ok(axum::Json(response))
}

fn derive_name_and_reader(item: ReadItem, idx: usize) -> (String, BoxAsyncRead) {
    let name = if item.meta.name.is_empty() {
        format!("part-{}.bin", idx)
    } else {
        item.meta.name
    };
    (name, item.reader)
}
