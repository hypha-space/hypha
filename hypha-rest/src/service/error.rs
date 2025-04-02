use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::state::error::StateError;

/// Error type for the axum service.
/// Provides an 'IntoResponse' implementation to be able to use 'Result'
/// as responses in axum.
/// See https://docs.rs/axum/latest/axum/error_handling/index.html
#[derive(Debug)]
pub enum ServiceError {
    NotFound(String),
    InternalServerError(String),
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        match self {
            ServiceError::NotFound(resource) => {
                (StatusCode::NOT_FOUND, format!("Not Found: {}", resource)).into_response()
            }
            ServiceError::InternalServerError(message) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Internal Server Error: {}", message),
            )
                .into_response(),
        }
    }
}

impl From<StateError> for ServiceError {
    fn from(error: StateError) -> Self {
        match error {
            StateError::NotFound(uuid) => ServiceError::NotFound(uuid.to_string()),
            StateError::Internal(message) => ServiceError::InternalServerError(message.to_string()),
        }
    }
}
