use std::{collections::HashMap, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use hypha_network::{
    request_response::{RequestResponseError, RequestResponseInterface},
    stream::StreamSenderInterface,
};
use libp2p::{PeerId, request_response::cbor::codec::Codec};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{Mutex, mpsc};
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::{driver::Task, file_transfer::send_file};

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct TrainingConfig {
    #[schema(value_type = String, format = "peer")]
    scheduler_peer_id: PeerId,
    #[schema(value_type = String, format = "peer")]
    parameter_server_peer_id: PeerId,
    task_id: Uuid,
    model: String,
    dataset: String,
    epochs: u32,
    batch_size: u32,
    learning_rate: f32,
    learning_rate_scheduler: String,
    optimizer: String,
    checkpointing: i32,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
enum TrainingInputs {
    #[serde(rename = "parameters")]
    Parameters(Parameters),
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct Parameters {
    version: u64,
    #[schema(value_type = String, format = Uri)]
    path: PathBuf,
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
    rx: Mutex<mpsc::Receiver<Task>>,
    network: N,
    work_dir: PathBuf,
}

#[derive(Error, Debug)]
enum Error {
    #[error("Network error")]
    Network(#[from] RequestResponseError),
    #[error("Unknown task `{0}`")]
    TaskNotFound(Uuid),
    #[error("Invalid task status: {0}")]
    InvalidStatus(String),
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
        }
    }
}

pub fn new<N, P>(
    network: N,
    work_dir: P,
    tasks: Arc<Mutex<HashMap<Uuid, Task>>>,
    rx: mpsc::Receiver<Task>,
) -> Router
where
    N: StreamSenderInterface
        + RequestResponseInterface<Codec<hypha_messages::Request, hypha_messages::Response>>
        + Clone
        + Sync
        + Send
        + 'static,
    P: AsRef<std::path::Path>,
{
    let state = Arc::new(AppState {
        tasks,
        rx: Mutex::new(rx),
        network,
        work_dir: PathBuf::from(work_dir.as_ref()),
    });

    Router::new()
        .route("/openapi.json", get(openapi))
        .route("/tasks", get(get_tasks))
        .route("/inputs/{id}", get(inputs))
        .route("/status/{id}", post(status))
        .with_state(state)
}

#[derive(OpenApi)]
#[openapi(paths(openapi, get_tasks, inputs, status))]
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
async fn get_tasks<N>(State(state): State<Arc<AppState<N>>>) -> Result<Json<TrainingConfig>, Error>
where
    N: Clone + Send + 'static,
{
    tracing::trace!("GET /tasks");
    let task = state
        .rx
        .lock()
        .await
        .recv()
        .await
        .expect("task channel is open");
    tracing::trace!(task_id = %task.id, "Providing training config for task");
    // TODO
    Ok(Json(TrainingConfig {
        scheduler_peer_id: task.scheduler_peer_id,
        parameter_server_peer_id: task.parameter_server_peer_id,
        task_id: task.id,
        model: "EleutherAI/gpt-neo-125m".to_string(),
        dataset: "datablations/c4-filter-small".to_string(),
        epochs: 1,
        batch_size: 1,
        learning_rate: 1e-5,
        learning_rate_scheduler: "".to_string(),
        optimizer: "AdamW".to_string(),
        checkpointing: 1,
    }))
}

#[utoipa::path(
    get,
    path = "/inputs/{task_id}",
    params(("task_id", description = "Task ID")),
    responses(
        (status = OK, description = "Training inputs", body = TrainingInputs),
        (status = NOT_FOUND, description = "Unknown task"),
    )
)]
async fn inputs<N>(
    Path(task_id): Path<Uuid>,
    State(state): State<Arc<AppState<N>>>,
) -> Result<Json<TrainingInputs>, Error>
where
    N: Clone + Send + 'static,
{
    tracing::trace!(task_id = %task_id, "GET /inputs");

    if let Some(_task) = state.tasks.lock().await.get(&task_id) {
        // TODO
        Ok(Json(TrainingInputs::Parameters(Parameters {
            version: 0,
            path: state.work_dir.join("0-0"),
        })))
    } else {
        Err(Error::TaskNotFound(task_id))
    }
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
        let parameter_server_peer_id = task.parameter_server_peer_id;
        let scheduler_peer_id = task.scheduler_peer_id;

        if let Some(result) = status.result {
            tracing::trace!(
                task_id = %task_id,
                epoch = result.epoch,
                "Streaming training result",
            );
            let network = state.network.clone();

            // Some simple input validation
            // NOTE: Validate `result.path` as it MUST be with in the `work_dir` to prevent path traversal attacks.
            if !&result.path.starts_with(state.work_dir.as_path()) {
                return Err(Error::InvalidStatus(
                    "result path is not within the work directory".to_string(),
                ));
            }

            // Streaming a large file can take a while, therefore we spawn a task here.
            // TODO: Have a separate actor-like class keep track of this and handle errors.
            tokio::spawn(async move {
                let mut tensor_stream = network
                    .stream(parameter_server_peer_id)
                    .await
                    .expect("stream to parameter server can be established");

                tracing::info!(
                    peer_id = %parameter_server_peer_id,
                    "Starting file transfer"
                );

                let header = hypha_messages::ArtifactHeader {
                    task_id,
                    epoch: result.epoch,
                };

                // With the driver notifying us about a result file, we
                // have ownership of that file and a driver is no longer allowed
                // to access that file.
                // Therefore we delete it once it has been sent.
                send_file(&header, &result.path, &mut tensor_stream)
                    .await
                    .expect("training result file can be sent");
                tokio::fs::remove_file(&result.path)
                    .await
                    .expect("training result file can be removed after sending");
            });
        }

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
        let (tx, rx) = mpsc::channel(1);

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

        let app = new(mock_network, work_dir, tasks, rx);

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

        let app = new(mock_network, work_dir, tasks, rx);

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

        let app = new(mock_network, work_dir.path(), tasks, rx);

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
