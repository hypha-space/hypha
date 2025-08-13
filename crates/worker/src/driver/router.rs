use std::{collections::HashMap, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::{get, post},
};
use futures_util::{Stream, StreamExt};
use hypha_network::{
    request_response::{RequestResponseError, RequestResponseInterface},
    stream::StreamSenderInterface,
};
use libp2p::{PeerId, request_response::cbor::codec::Codec};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{Mutex, broadcast};
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::driver::Task;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TrainingConfig {
    #[schema(value_type = String, format = "peer")]
    pub scheduler_peer_id: PeerId,
    #[schema(value_type = String, format = "peer")]
    pub parameter_server_peer_id: PeerId,
    pub task_id: Uuid,
    pub model: String,
    pub dataset: String,
    pub epochs: u32,
    pub batch_size: u32,
    pub learning_rate: f32,
    pub learning_rate_scheduler: String,
    pub optimizer: String,
    pub checkpointing: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub enum TrainingInputs {
    #[serde(rename = "parameters")]
    Parameters(Parameters),
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Parameters {
    pub version: u64,
    #[schema(value_type = String, format = Uri)]
    pub path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct TrainingStatus {
    status: String,
    result: Option<TrainingResult>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct TrainingResult {
    epoch: u64,
    #[schema(value_type = String, format = Uri)]
    path: PathBuf,
}

struct AppState<N>
where
    N: Clone + Send + 'static,
{
    tasks: Arc<Mutex<HashMap<Uuid, Task>>>,
    tasks_channel: Mutex<broadcast::Sender<TrainingConfig>>,
    send_channel: Mutex<broadcast::Sender<(PeerId, TrainingInputs)>>,
    receive_channel: Mutex<broadcast::Sender<(PeerId, TrainingInputs)>>,
    network: N,
}

#[derive(Error, Debug)]
enum Error {
    #[error("Network error")]
    Network(#[from] RequestResponseError),
    #[error("Unknown task `{0}`")]
    TaskNotFound(Uuid),
    #[error("Invalid task status: {0}")]
    InvalidStatus(String),
    #[error("Broadcast error")]
    Broadcast(#[from] BroadcastStreamRecvError),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Error::Network(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update task status",
            )
                .into_response(),
            Error::TaskNotFound(task_id) => {
                (StatusCode::NOT_FOUND, format!("unknown task `{task_id}`")).into_response()
            }
            Error::InvalidStatus(msg) => {
                (StatusCode::BAD_REQUEST, format!("invalid status: {msg}")).into_response()
            }
            Error::Broadcast(_e) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "failed to retrieve data").into_response()
            }
        }
    }
}

pub fn new<N>(
    network: N,
    tasks: Arc<Mutex<HashMap<Uuid, Task>>>,
    tasks_channel: broadcast::Sender<TrainingConfig>,
    inputs_channel: broadcast::Sender<TrainingInputs>,
) -> Router
where
    N: StreamSenderInterface
        + RequestResponseInterface<Codec<hypha_messages::Request, hypha_messages::Response>>
        + Clone
        + Sync
        + Send
        + 'static,
{
    let state = Arc::new(AppState {
        tasks,
        tasks_channel: Mutex::new(tasks_channel),
        send_channel: Mutex::new(inputs_channel),
        receive_channel: Mutex::new(receive_channel),
        network,
    });

    Router::new()
        .route("/openapi.json", get(openapi))
        .route("/tasks", get(get_tasks))
        .route("/status/{id}", post(status))
        .route("/data/{peer_id}", get(receive).post(send))
        .with_state(state)
}

#[derive(OpenApi)]
#[openapi(paths(openapi, get_tasks, status))]
struct ApiDoc;

#[utoipa::path(
    get,
    path = "/openapi.json",
    responses(
        (status = OK, description = "JSON file", body = ())
    )
)]
async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[utoipa::path(
    get,
    path = "/tasks",
    responses(
        (status = OK, description = "Training task", body = TrainingConfig)
    )
)]
async fn get_tasks<N>(
    State(state): State<Arc<AppState<N>>>,
) -> Sse<impl Stream<Item = Result<Event, Error>>>
where
    N: Clone + Send + 'static,
{
    tracing::trace!("GET /tasks");

    let tasks_rx = state.tasks_channel.lock().await.subscribe();

    Sse::new(BroadcastStream::new(tasks_rx).map(|msg| match msg {
        Ok(task) => Ok(Event::default().json_data(task).unwrap()),
        Err(e) => Err(e.into()),
    }))
}

// TODO: API to send messages to individual nodes
async fn send<N>(
    Path(peer_id): Path<PeerId>,
    State(state): State<Arc<AppState<N>>>,
    Json(data): Json<TrainingInputs>,
) -> Result<(), Error>
where
    N: Clone + Send + 'static,
{
    tracing::info!(peer_id = %peer_id, "POST /data");

    let inputs_tx = state.send_channel.lock().await;

    let _ = inputs_tx.send((peer_id, data));

    Ok(())
}

async fn receive<N>(
    Path(peer_id): Path<PeerId>,
    State(state): State<Arc<AppState<N>>>,
) -> Sse<impl Stream<Item = Result<Event, Error>>>
where
    N: Clone + Send + 'static,
{
    tracing::debug!(peer_id = %peer_id, "GET /data");

    let inputs_rx = state.receive_channel.lock().await.subscribe();

    Sse::new(BroadcastStream::new(inputs_rx).map(|msg| match msg {
        Ok((_peer_id, data)) => {
            tracing::info!("Received data");
            Ok(Event::default().json_data(data).unwrap())
        }
        Err(e) => Err(e.into()),
    }))
}

#[utoipa::path(
    post,
    path = "/status/{task_id}",
    params(("task_id", description = "Task ID")),
    request_body = TrainingStatus,
    responses(
        (status = OK, description = "Success", body = ()),
        (status = INTERNAL_SERVER_ERROR, description = "Network error"),
        (status = NOT_FOUND, description = "Unknown task")
    )
)]
async fn status<N>(
    Path(task_id): Path<Uuid>,
    State(state): State<Arc<AppState<N>>>,
    Json(status): Json<TrainingStatus>,
) -> Result<(), Error>
where
    N: StreamSenderInterface
        + RequestResponseInterface<Codec<hypha_messages::Request, hypha_messages::Response>>
        + Clone
        + Sync
        + Send
        + 'static,
{
    tracing::trace!(task_id = %task_id, "POST /status");

    if let Some(task) = state.tasks.lock().await.get(&task_id) {
        let scheduler_peer_id = task.scheduler_peer_id;

        state
            .network
            .request(
                scheduler_peer_id,
                hypha_messages::Request::Worker(hypha_messages::WorkerRequest::TaskStatus {
                    task_id,
                    status: status
                        .status
                        .parse()
                        .unwrap_or(hypha_messages::TaskStatus::Unknown),
                }),
            )
            .await?;

        Ok(())
    } else {
        Err(Error::TaskNotFound(task_id))
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{self, Request},
    };
    use http_body_util::BodyExt;
    use hypha_network::stream::StreamInterface;
    use libp2p::request_response;
    use mockall::mock;
    use tower::ServiceExt;

    use super::*;

    mock! {
        TestNetwork {}

        impl Clone for TestNetwork {
            fn clone(&self) -> Self;
        }

        impl RequestResponseInterface<Codec<hypha_messages::Request, hypha_messages::Response>> for TestNetwork {
            async fn send(&self, action: hypha_network::request_response::RequestResponseAction<Codec<hypha_messages::Request,hypha_messages::Response>>);
            fn try_send(&self, action: hypha_network::request_response::RequestResponseAction<Codec<hypha_messages::Request,hypha_messages::Response>>) -> Result<(), RequestResponseError>;
            async fn request(&self, peer_id: PeerId, request: <Codec<hypha_messages::Request, hypha_messages::Response> as request_response::Codec>::Request) -> Result<<Codec<hypha_messages::Request, hypha_messages::Response> as request_response::Codec>::Response, RequestResponseError>;
        }

        impl StreamInterface for TestNetwork {
            fn stream_control(&self) -> libp2p_stream::Control;
        }

        impl StreamSenderInterface for TestNetwork {
            async fn stream(&self, peer: PeerId) -> Result<libp2p::Stream, libp2p_stream::OpenStreamError>;
        }
    }

    /// Test the tasks endpoint.
    #[tokio::test]
    async fn test_tasks() {
        let mock_network = MockTestNetwork::new();

        let work_dir = tempfile::tempdir().unwrap();
        let (tx, rx) = broadcast::channel(1);
        let (channel, _) = broadcast::channel(1);

        let scheduler_peer_id = PeerId::random();
        let parameter_server_peer_id = PeerId::random();
        let task_id = Uuid::new_v4();
        tx.send(Task {
            id: task_id,
            scheduler_peer_id,
            parameter_server_peer_id,
        })
        .await
        .unwrap();

        let tasks = Arc::new(Mutex::new(HashMap::new()));

        let app = new(mock_network, work_dir, tasks, rx, channel);

        let response = app
            .oneshot(
                Request::builder()
                    .method(http::Method::GET)
                    .uri("/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let training_config: TrainingConfig = serde_json::from_slice(&body).unwrap();

        // We don't bother with comparing all config values for now as they're hard-coded.
        assert_eq!(training_config.scheduler_peer_id, scheduler_peer_id);
        assert_eq!(
            training_config.parameter_server_peer_id,
            parameter_server_peer_id
        );
        assert_eq!(training_config.task_id, task_id);
    }

    /// Test the inputs endpoint.
    #[tokio::test]
    async fn test_inputs() {
        let mock_network = MockTestNetwork::new();

        let work_dir = tempfile::tempdir().unwrap();
        let (_tx, rx) = mpsc::channel(1);
        let (channel, _) = broadcast::channel(1);

        let task_id = Uuid::new_v4();

        let tasks = Arc::new(Mutex::new(HashMap::new()));
        tasks.lock().await.insert(
            task_id,
            Task {
                id: task_id,
                scheduler_peer_id: PeerId::random(),
                parameter_server_peer_id: PeerId::random(),
            },
        );

        let app = new(mock_network, work_dir, tasks, rx, channel);

        let response = app
            .oneshot(
                Request::builder()
                    .method(http::Method::GET)
                    .uri(format!("/inputs/{task_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let inputs: TrainingInputs = serde_json::from_slice(&body).unwrap();

        assert!(matches!(inputs, TrainingInputs::Parameters(..)));

        let TrainingInputs::Parameters(parameters) = inputs;

        // There's not much value of testing these values yet, as they're hard-coded.
        assert_eq!(parameters.version, 0);
    }

    /// Test the status endpoint.
    #[tokio::test]
    async fn test_status_without_result() {
        let mut mock_network = MockTestNetwork::new();

        let scheduler_peer_id = PeerId::random();

        // Task status updates are forwarded to the scheduler.
        mock_network
            .expect_request()
            .times(1)
            .returning(move |peer_id, _| {
                if peer_id == scheduler_peer_id {
                    Ok(hypha_messages::Response::Worker(
                        hypha_messages::WorkerResponse::TaskStatus {},
                    ))
                } else {
                    Err(RequestResponseError::Other("wrong peer ID".to_string()))
                }
            });

        let work_dir = tempfile::tempdir().unwrap();
        let (_tx, rx) = mpsc::channel(1);
        let (channel, _) = broadcast::channel(1);

        let task_id = Uuid::new_v4();

        let tasks = Arc::new(Mutex::new(HashMap::new()));
        tasks.lock().await.insert(
            task_id,
            Task {
                id: task_id,
                scheduler_peer_id,
                parameter_server_peer_id: PeerId::random(),
            },
        );

        let app = new(mock_network, work_dir.path(), tasks, rx, channel);

        let status = TrainingStatus {
            status: "Running".to_string(),
            result: None,
        };

        let response = app
            .oneshot(
                Request::builder()
                    .method(http::Method::POST)
                    .uri(format!("/status/{task_id}"))
                    .header(http::header::CONTENT_TYPE, mime::APPLICATION_JSON.as_ref())
                    .body(Body::from(serde_json::to_string(&status).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
