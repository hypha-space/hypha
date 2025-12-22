use std::{
    fs::Permissions,
    os::unix::fs::{MetadataExt, PermissionsExt},
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
use hypha_data::hash::get_file_hash;
use hypha_messages::{
    DataSlice, Fetch, Receive, Reference, Send,
    action::{self, ActionRequest},
    api, data,
};
use hypha_network::{
    request_response::{RequestResponseError, RequestResponseInterface},
    stream_pull::StreamPullSenderInterface,
};
use libp2p::PeerId;
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    fs::{self, set_permissions},
    io::{self, AsyncWriteExt},
    net::UnixListener,
};
use tokio_retry::{
    Retry,
    strategy::{ExponentialBackoff, FibonacciBackoff, jitter},
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
    #[error("Network error: {0}")]
    Network(#[from] RequestResponseError),
    #[error("Connector error: {0}")]
    Connector(#[from] ConnectorError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Hash mismatch: expected {expected}, got {found}")]
    HashMismatch { expected: u64, found: u64 },
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
            Error::HashMismatch { expected, found } => {
                tracing::warn!(expected, found, "bridge error: hash_mismatch");
                (
                    StatusCode::BAD_REQUEST,
                    Json(ApiError {
                        error: "hash_mismatch",
                        detail: format!("Expected hash {}, but got {}", expected, found),
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
            .route("/action/update", post(send_action))
            .with_state(state);

        let listener = UnixListener::bind(&socket_path)?;
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

        // NOTE: If for some reason we can't remove the socket, ignore the error.
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
    let retry_strategy = FibonacciBackoff::from_millis(100).map(jitter).take(6);
    validate_fetch(&resource)?;

    match resource.as_ref() {
        // Resolve scheduler fetches:
        // We request a data slice from the scheduler.
        // If we already downloaded that slice, we use the existing one,
        // otherwise we download it from a data provider.
        Reference::Scheduler { peer, dataset } => {
            tracing::debug!(peer_id = %peer, dataset, "Requesting data slice index from scheduler");
            let (data_providers, hash) = Retry::spawn(retry_strategy.clone(), || {
                let network = state.network.clone();
                async move {
                    match <Network as RequestResponseInterface<api::Codec>>::request(
                        &network,
                        *peer,
                        api::Request::Data(data::Request {
                            dataset: dataset.clone(),
                        }),
                    )
                    .await
                    {
                        Ok(api::Response::Data(data::Response::Success {
                            data_providers,
                            hash,
                        })) => Ok((data_providers, hash)),
                        Ok(r) => Err(Error::Io(std::io::Error::other(format!(
                            "Unexpected response \"{:?}\"",
                            r
                        )))),
                        Err(e) => Err(Error::Io(std::io::Error::other(format!(
                            "Failed to request data slice for dataset \"{}\": {}",
                            dataset, e
                        )))),
                    }
                }
            })
            .await?;

            tracing::debug!(peer_id = %peer, data_peer_ids = ?data_providers, dataset, hash, "Received slice index and data provider");

            let out = Retry::spawn(retry_strategy, || {
                let data_providers = data_providers.clone();
                let state = state.clone();
                let dir_rel = "artifacts".to_string();
                let mut out: Vec<FileResponse> = Vec::new();
                async move {
                    let dir_abs = safe_join(&state.work_dir, &dir_rel)?;
                    fs::create_dir_all(&dir_abs).await?;

                    let file_name = hash;
                    let rel = format!("{}/{}", dir_rel, file_name);
                    let abs = safe_join(&state.work_dir, &rel)?;

                    match fs::try_exists(&abs).await {
                        // Cache hit!
                        Ok(true) => {
                            tracing::debug!(
                                peer_id = %peer, data_peer_ids = ?data_providers,
                                dataset,
                                hash,
                                "File already exists, skipping data slice download"
                            );

                            let metadata = fs::metadata(&abs).await?;

                            out.push(FileResponse {
                                path: rel,
                                size: metadata.size(),
                            });
                        }
                        // Cache miss!
                        _ => {
                            tracing::debug!(peer_id = %peer, data_peer_ids = ?data_providers, dataset, hash, "Downloading data slice");

                            let data_provider = data_providers.choose(&mut rand::rng()).copied().ok_or_else(|| Error::Io(io::Error::other("no peers provided")))?;

                            let mut reader = state
                                .network
                                .open_pull_stream(
                                    data_provider,
                                    &DataSlice {
                                        dataset: dataset.clone(),
                                        hash,
                                    },
                                )
                                .await.map_err(|e| Error::Connector(ConnectorError::OpenStream(e)))?;

                            if let Some(parent) = abs.parent() {
                                fs::create_dir_all(parent).await?;
                            }

                            let mut file = fs::File::create(&abs).await?;
                            let size = tokio::io::copy(&mut reader, &mut file).await?;
                            file.sync_all().await?;

                            // Validate file against hash
                            let calculated_hash = get_file_hash(&abs)?;

                            if calculated_hash != hash {
                                // Delete file
                                fs::remove_file(&abs).await?;

                                return Err(Error::HashMismatch {
                                    expected: hash,
                                    found: calculated_hash,
                                });
                            }

                            tracing::info!(size, file = %abs.display(), "Copied resource");
                            set_permissions(&abs, Permissions::from_mode(0o600)).await?;

                            out.push(FileResponse { path: rel, size });
                        }
                    };

                    Ok::<std::vec::Vec<FileResponse>, Error>(out)
                }
            })
            .await?;

            Ok(Json(out))
        }
        _ => {
            let out = Retry::spawn(retry_strategy, || {
                let state = state.clone();
                let resource = resource.clone();
                let dir_rel = "artifacts".to_string();
                let mut out: Vec<FileResponse> = Vec::new();
                async move {
                    let dir_abs = safe_join(&state.work_dir, &dir_rel)?;
                    fs::create_dir_all(&dir_abs).await?;
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
                    Ok::<std::vec::Vec<FileResponse>, Error>(out)
                }
            })
            .await?;

            Ok(Json(out))
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
struct SendRequest {
    resource: Send,
    path: String,
}

async fn send_resource(
    State(state): State<Arc<SockState>>,
    Json(req): Json<SendRequest>,
) -> Result<(), Error> {
    let retry_strategy = ExponentialBackoff::from_millis(100)
        .map(jitter) // add jitter to delays
        .take(3); // limit to 3 retries

    Retry::spawn(retry_strategy, || {
        let state = state.clone();
        let req = req.clone();
        async move {
            let abs = safe_join(&state.work_dir, &req.path)?;
            let cancel = state.cancel.clone();
            let file_path = abs.clone();
            let metadata = fs::metadata(&file_path).await?;
            let payload_len = metadata.len();
            let mut writers = state.connector.send(req.resource, payload_len).await?;

            // Don't copy the resource in the background. We need to wait until its done.
            // If run in the background, the Python code never knows when the send
            // completed. However, it needs to inform the scheduler that it sent its
            // gradients, s.t. it can advance the state.
            // This might lead to timeouts in the Python code however, for the moment
            // this seems to be the best fix.
            {
                loop {
                    let next_item = tokio::select! {
                        _ = cancel.cancelled() => None,
                        item = writers.next() => item,
                    };

                    let Some(item_result) = next_item else {
                        if cancel.is_cancelled() {
                            tracing::debug!(file = %file_path.display(), "send_resource: task cancelled");
                        }

                        // We no longer need the file once it has been sent, so remove it.
                        fs::remove_file(&file_path).await?;
                        break;
                    };
                    let item = item_result?;
                    let peer_id = item.meta.name.clone();
                    let mut writer = item.writer;
                    let mut reader =  fs::File::open(&file_path).await?;
                    tracing::info!(peer_id = %peer_id, file = %file_path.display(), "Sending resource");
                    let sent_bytes = io::copy(&mut reader, &mut writer).await?;
                    writer.shutdown().await?;
                    tracing::info!(size = sent_bytes, file = %file_path.display(), peer_id = %peer_id, "Sent resource");
                }
            }
            Ok::<(), Error>(())
        }
    }).await?;

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
            _ = tx.closed() => {
                tracing::debug!(path = %dir_rel_clone, "receive_subscribe: client stream dropped");
                None
            }
            _ = cancel.cancelled() => {
                tracing::debug!(path = %dir_rel_clone, "receive_subscribe: task cancelled");
                None
            }
            item = incoming.next() => item,
        } {
            let item = match item_result {
                Ok(item) => item,
                Err(err) => {
                    tracing::warn!(error = %err, path = %dir_rel_clone, "receive_subscribe: stream error");
                    continue;
                }
            };
            let (file_name, mut reader) = derive_name_and_reader(item, index);
            let file_rel = format!("{}/{}", dir_rel_clone, file_name);
            let file_abs = match safe_join(&work_dir, &file_rel) {
                Ok(p) => p,
                Err(err) => {
                    tracing::error!(error = %err, file = %file_rel, "receive_subscribe: invalid target path");
                    continue;
                }
            };
            if let Some(parent) = file_abs.parent() {
                match fs::create_dir_all(parent).await {
                    Ok(()) => (),
                    Err(err) => {
                        tracing::error!(error = %err, directory = %parent.display(), "receive_subscribe: failed to create directory");
                        continue;
                    }
                }
            }
            let mut file = match fs::File::create(&file_abs).await {
                Ok(f) => f,
                Err(err) => {
                    tracing::error!(error = %err, file = %file_abs.display(), "receive_subscribe: failed to create file");
                    continue;
                }
            };
            let size = match tokio::io::copy(&mut reader, &mut file).await {
                Ok(n) => n,
                Err(err) => {
                    tracing::warn!(error = %err, file = %file_abs.display(), "receive_subscribe: failed to copy resource");
                    continue;
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

async fn send_action(
    State(state): State<Arc<SockState>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<action::ActionResponse>, Error> {
    if req.job_id != state.job_id {
        return Err(Error::InvalidStatus("job_id mismatch".to_string()));
    }

    let retry_strategy = ExponentialBackoff::from_millis(100).map(jitter).take(3);

    // TODO we should ensure that a message is not received repeatedly. Otherwise it will distort the training.
    let result = Retry::spawn(retry_strategy, || {
        let req_clone = req.clone();
        let state = state.clone();
        async move {
            hypha_network::request_response::RequestResponseInterface::<action::Codec>::request(
                &state.network,
                state.scheduler,
                req_clone,
            )
            .await
        }
    })
    .await?;

    Ok(axum::Json(result))
}

fn derive_name_and_reader(item: ReadItem, idx: usize) -> (String, BoxAsyncRead) {
    let name = if item.meta.name.is_empty() {
        format!("part-{}.bin", idx)
    } else {
        item.meta.name
    };
    (name, item.reader)
}
