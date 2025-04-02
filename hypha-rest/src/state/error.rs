use thiserror::Error;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum StateError {
    #[error("task `{0}` not found")]
    NotFound(Uuid),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}
