use std::{future::Future, pin::Pin};

use libp2p::PeerId;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

mod bridge;
mod parameter_server;
mod process;

pub use parameter_server::ParameterServerExecutor;
pub use process::ProcessExecutor;
use uuid::Uuid;

use crate::{connector::ConnectorError, executor::parameter_server::TensorOpError};

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Running,
    Success {
        description: Option<String>,
        out: Option<String>,
        err: Option<String>,
    },
    Failed {
        description: Option<String>,
        out: Option<String>,
        err: Option<String>,
    },
    Cancelled {
        description: Option<String>,
        out: Option<String>,
        err: Option<String>,
    },
}

impl Status {
    pub fn success(description: Option<String>) -> Self {
        Status::Success {
            description,
            out: None,
            err: None,
        }
    }

    pub fn failed(description: Option<String>) -> Self {
        Status::Failed {
            description,
            out: None,
            err: None,
        }
    }

    pub fn cancelled(description: Option<String>) -> Self {
        Status::Cancelled {
            description,
            out: None,
            err: None,
        }
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Bridge error: {0}")]
    Bridge(#[from] bridge::Error),
    // NOTE: Bridge::try_new returns std::io::Result; map it here for `?` ergonomics
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Unsupported job spec")]
    UnsupportedJobSpec(),
    #[error("Unsupported optimizer")]
    UnsupportedOptimizer(),
    #[error("Tensor error: {0}")]
    Tensor(#[from] TensorOpError),
    #[error("Executor configuration invalid: {0}")]
    InvalidExecutorConfig(String),
    #[error("Request/Response error: {0}")]
    RequestResponse(#[from] hypha_network::request_response::RequestResponseError),
    #[error("Connector error: {0}")]
    Connector(#[from] ConnectorError),
}

pub trait JobExecutor {
    fn execute(
        &self,
        job: hypha_messages::JobSpec,
        cancel: CancellationToken,
        job_id: Uuid,
        scheduler: PeerId,
    ) -> impl Future<Output = Result<impl Execution, Error>> + Send;
}

pub trait Execution {
    // NOTE: Make object-safe by returning a boxed future. This allows storing heterogeneous Execution handles behind trait objects.
    fn wait<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<Status, Error>> + Send + 'a>>;
}
