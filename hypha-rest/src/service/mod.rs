use std::convert::Infallible;

use axum::{
    Router,
    body::Body,
    extract::{Json, Path, Query, State as AppState},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::get,
};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    api::v0::{Task, TaskSpec},
    service::error::ServiceError,
    state::State,
};

mod error;

type Result<T> = std::result::Result<T, ServiceError>;

pub fn service(state: State) -> Router {
    Router::new()
        .route("/api/v0/tasks", get(handle_tasks).post(create_task))
        .route(
            "/api/v0/tasks/{id}",
            get(get_task).put(update_task).delete(delete_task),
        )
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct Params {
    watch: Option<bool>,
}

async fn get_task(
    Path(task_id): Path<Uuid>,
    AppState(state): AppState<State>,
) -> Result<Json<Task>> {
    Ok(Json(state.read_task(&task_id).await?))
}

async fn update_task(
    Path(task_id): Path<Uuid>,
    AppState(state): AppState<State>,
    Json(task): Json<Task>,
) -> Result<Json<Task>> {
    Ok(Json(state.update_task(&task_id, task).await?))
}

async fn delete_task(
    Path(task_id): Path<Uuid>,
    AppState(state): AppState<State>,
) -> Result<Json<Task>> {
    Ok(Json(state.delete_task(&task_id).await?))
}

async fn create_task(
    AppState(state): AppState<State>,
    Json(spec): Json<TaskSpec>,
) -> Result<Json<Task>> {
    Ok(Json(state.create_task(spec).await?))
}

async fn handle_tasks(
    Query(params): Query<Params>,
    AppState(state): AppState<State>,
) -> Response<Body> {
    if let Some(true) = params.watch {
        watch_tasks(state).await.into_response()
    } else {
        list_tasks(state).await.into_response()
    }
}

async fn list_tasks(state: State) -> Result<Json<Vec<Task>>> {
    Ok(Json(state.list_tasks().await?))
}

async fn watch_tasks(
    state: State,
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>> {
    let stream = state.watch_tasks().await?;
    Ok(Sse::new(stream.map(|task| {
        Ok(Event::default().json_data(task.unwrap()).unwrap())
    })))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{self, Request, StatusCode},
    };
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use tower::{Service, ServiceExt};

    use crate::state::persistence::InMemoryPersistence;

    use super::*;

    #[tokio::test]
    async fn test_task_lifecycle() {
        let state = State::new(InMemoryPersistence::new());
        let mut app = service(state).into_service();

        let request = Request::builder()
            .method("GET")
            .uri("/api/v0/tasks")
            .body(Body::empty())
            .unwrap();
        let response = ServiceExt::<Request<Body>>::ready(&mut app)
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"[]");

        let request = Request::builder()
            .method("POST")
            .uri("/api/v0/tasks")
            .header(http::header::CONTENT_TYPE, mime::APPLICATION_JSON.as_ref())
            .body(Body::from("{\"driver\": \"test\"}"))
            .unwrap();
        let response = ServiceExt::<Request<Body>>::ready(&mut app)
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let task: Task = serde_json::from_slice(&body).unwrap();
        let task_id = task.id;

        let request = Request::builder()
            .method("GET")
            .uri("/api/v0/tasks")
            .body(Body::empty())
            .unwrap();
        let response = ServiceExt::<Request<Body>>::ready(&mut app)
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let tasks_json = Bytes::from(serde_json::to_vec(&vec![task]).unwrap());
        assert_eq!(&body[..], &tasks_json);

        let request = Request::builder()
            .method("DELETE")
            .uri(format!("/api/v0/tasks/{}", task_id))
            .body(Body::empty())
            .unwrap();
        let response = ServiceExt::<Request<Body>>::ready(&mut app)
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let request = Request::builder()
            .method("GET")
            .uri("/api/v0/tasks")
            .body(Body::empty())
            .unwrap();
        let response = ServiceExt::<Request<Body>>::ready(&mut app)
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"[]");
    }
}
