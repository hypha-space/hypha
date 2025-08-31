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
use hypha_messages::{Fetch, Receive, Reference, Send};
use hypha_network::request_response::RequestResponseError;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    fs::{self, set_permissions},
    io::{self, AsyncWriteExt},
    net::UnixListener,
};
use tokio_util::{
    compat::{FuturesAsyncReadCompatExt, FuturesAsyncWriteCompatExt},
    sync::CancellationToken,
    task::TaskTracker,
};
use tokio::io::{AsyncReadExt as TokioAsyncReadExt, AsyncWriteExt as TokioAsyncWriteExt};
use utoipa::OpenApi;

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
            Error::Network(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError {
                    error: "network_error",
                    detail: e.to_string(),
                }),
            )
                .into_response(),
            Error::InvalidStatus(msg) => (
                StatusCode::BAD_REQUEST,
                Json(ApiError {
                    error: "invalid_request",
                    detail: msg,
                }),
            )
                .into_response(),
            Error::Connector(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError {
                    error: "connector_error",
                    detail: e.to_string(),
                }),
            )
                .into_response(),
            Error::Io(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError {
                    error: "io_error",
                    detail: e.to_string(),
                }),
            )
                .into_response(),
        }
    }
}

struct SockState {
    work_dir: PathBuf,
    connector: Connector<Network>,
}

pub struct Bridge {
    task_tracker: TaskTracker,
    socket_path: PathBuf,
}

impl Bridge {
    pub async fn try_new<P1, P2>(
        connector: Connector<Network>,
        work_dir: P1,
        socket_path: P2,
        cancel: CancellationToken,
    ) -> std::io::Result<Bridge>
    where
        P1: AsRef<std::path::Path>,
        P2: AsRef<std::path::Path>,
    {
        let state = Arc::new(SockState {
            work_dir: PathBuf::from(work_dir.as_ref()),
            connector,
        });

        let router = Router::new()
            .route("/openapi.json", get(openapi))
            .route("/resources/fetch", post(fetch_resource))
            .route("/resources/send", post(send_resource))
            .route("/resources/receive", post(receive_subscribe))
            .with_state(state);

        // This will create a file that we later delete as part of 'wait'.
        let listener = UnixListener::bind(&socket_path)?;

        // Access is restricted to the current user.
        set_permissions(&socket_path, Permissions::from_mode(0o600)).await?;
        let task_tracker = TaskTracker::new();
        let shutdown = cancel.clone();
        task_tracker.spawn(
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    shutdown.cancelled().await;
                })
                .into_future(),
        );
        task_tracker.close();

        Ok(Bridge {
            task_tracker,
            socket_path: PathBuf::from(socket_path.as_ref()),
        })
    }

    pub(crate) async fn wait(&self) -> Result<(), Error> {
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
        let (file_name, reader) = derive_name_and_reader(item, idx);
        let rel = format!("{}/{}", dir_rel, file_name);
        let abs = safe_join(&state.work_dir, &rel)?;
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).await?;
        }

        let mut file = fs::File::create(&abs).await?;
        let size = tokio::io::copy(&mut reader.compat(), &mut file).await?;
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
    let mut writers = state.connector.send(req.resource).await?;

    // Copy the resource in the background to avoid blocking.
    tokio::spawn(async move {
        while let Some(item) = writers.next().await.transpose().expect("stream") {
            let peer_id = item.meta.name.clone();
            let mut file = fs::File::open(&abs).await.expect("file exists");
            tracing::info!(peer_id = %peer_id, file = %abs.display(), "Sending resource");
            let mut reader = file;
            let mut writer = item.writer.compat_write();
            let sent_bytes = io::copy(&mut reader, &mut writer).await.expect("copy");
            writer.shutdown().await.expect("shutdown");
            tracing::info!(size = sent_bytes, file = %abs.display(), "Sent resource");
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
        _ => Ok(()),
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

    // Background task: receive loops until the client disconnects or an error occurs
    tokio::spawn(async move {
        let mut incoming = match connector.receive(resource).await {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut index = 0usize;
        while let Some(item) = incoming.next().await.transpose().ok().flatten() {
            let (file_name, reader) = derive_name_and_reader(item, index);
            let file_rel = format!("{}/{}", dir_rel, file_name);
            let file_abs = match safe_join(&work_dir, &file_rel) {
                Ok(p) => p,
                Err(_) => break,
            };
            if let Some(parent) = file_abs.parent()
                && fs::create_dir_all(parent).await.is_err()
            {
                break;
            }
            let mut file = match fs::File::create(&file_abs).await {
                Ok(f) => f,
                Err(_) => break,
            };
            // Auto-detect framed (chunked) vs raw; support both for compatibility.
            let size = match recv_auto(&mut reader.compat(), &mut file).await {
                Ok(n) => n,
                Err(_) => break,
            };
            let _ = file.sync_all().await;
            let _ = set_permissions(&file_abs, Permissions::from_mode(0o600)).await;

            tracing::info!(size, file = %file_abs.display(), "Received resource");

            let from_peer = file_name.split('.').next().unwrap_or("").to_string();
            let pointer = UpdatePointer {
                path: file_rel,
                size,
                from_peer,
            };
            let ev = serde_json::to_string(&pointer)
                .map(|s| Event::default().data(s))
                .unwrap_or_else(|_| Event::default().data("{\\\"error\\\":\\\"serialize\\\"}"));
            if tx.send(ev).await.is_err() {
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

fn derive_name_and_reader(item: ReadItem, idx: usize) -> (String, BoxAsyncRead) {
    let name = if item.meta.name.is_empty() {
        format!("part-{}.bin", idx)
    } else {
        format!("{}-{}.bin", item.meta.name, idx)
    };
    (name, item.reader)
}

// Shared with parameter server framing: accept magic+chunks or raw.
const MAGIC: &[u8; 4] = b"HPCH";
const MAX_CHUNK: usize = 64 * 1024 * 1024;
const CHUNK_SIZE: usize = 10 * 1024 * 1024; // 10 MiB

async fn send_in_chunks<R, W>(r: &mut R, w: &mut W) -> std::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut total: u64 = 0;
    let mut buf = vec![0u8; CHUNK_SIZE];
    // Write magic header once per stream
    TokioAsyncWriteExt::write_all(w, MAGIC).await?;
    let mut idx: u64 = 0;
    loop {
        let n = TokioAsyncReadExt::read(r, &mut buf).await?;
        if n == 0 {
            break;
        }
        let len = (n as u64).to_le_bytes();
        TokioAsyncWriteExt::write_all(w, &len).await?;
        TokioAsyncWriteExt::write_all(w, &buf[..n]).await?;
        total += n as u64;
        tracing::info!(chunk_index = idx, chunk_bytes = n, total_sent = total, "Sent chunk");
        idx += 1;
    }
    Ok(total)
}

async fn recv_auto<R, W>(r: &mut R, w: &mut W) -> std::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut total: u64 = 0;
    let mut magic = [0u8; 4];
    match read_exact_bytes(r, &mut magic).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(0),
        Err(e) => return Err(e),
    }
    if &magic == MAGIC {
        let mut hdr = [0u8; 8];
        let mut buf: Vec<u8> = Vec::new();
        let mut idx: u64 = 0;
        loop {
            match read_exact_bytes(r, &mut hdr).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            }
            let len = u64::from_le_bytes(hdr) as usize;
            if len == 0 || len > MAX_CHUNK {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid chunk len {}", len),
                ));
            }
            if buf.len() < len {
                buf.resize(len, 0);
            }
            read_exact_bytes(r, &mut buf[..len]).await?;
            TokioAsyncWriteExt::write_all(w, &buf[..len]).await?;
            total += len as u64;
            tracing::info!(chunk_index = idx, chunk_bytes = len, total_received = total, "Received chunk");
            idx += 1;
        }
        Ok(total)
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing HPCH magic header",
        ));
    }
}

async fn read_exact_bytes<R: tokio::io::AsyncRead + Unpin>(r: &mut R, mut buf: &mut [u8]) -> std::io::Result<()> {
    while !buf.is_empty() {
        match TokioAsyncReadExt::read(r, buf).await? {
            0 => return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "early EOF")),
            n => {
                let tmp = buf;
                buf = &mut tmp[n..];
            }
        }
    }
    Ok(())
}
