use std::{future::Future, pin::Pin};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

mod bridge;
mod parameter_server;
mod process;

pub use parameter_server::ParameterServerExecutor;
pub use process::ProcessExecutor;

use crate::executor::parameter_server::TensorOpError;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Bridge error")]
    Bridge(#[from] bridge::Error),
    // NOTE: Bridge::try_new returns std::io::Result; map it here for `?` ergonomics
    #[error("I/O error")]
    Io(#[from] std::io::Error),
    #[error("Unsupported job spec")]
    UnsupportedJobSpec(),
    #[error("Unsupported optimizer")]
    UnsupportedOptimizer(),
    #[error("Tensor error")]
    Tensor(#[from] TensorOpError),
}

pub trait JobExecutor {
    // NOTE: Avoid `async fn` in traits; return a boxed Future instead
    fn execute(
        &self,
        job: hypha_messages::JobSpec,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<impl Execution, Error>> + Send;
}

pub trait Execution {
    // NOTE: Make object-safe by returning a boxed future.
    // This allows storing heterogeneous Execution handles behind trait objects.
    fn wait<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}
