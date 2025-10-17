use std::pin::Pin;

use futures_util::{Stream, StreamExt};
use hypha_messages::Status;
use libp2p::PeerId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StatusError {
    #[error("Connection to status receiver lost")]
    ConnectionLost,
    #[error("Error when sending request: {0}")]
    Request(#[from] reqwest::Error),
}

pub fn with_id<S: Stream>(id: PeerId, stream: S) -> impl Stream<Item = (PeerId, S::Item)> {
    stream.map(move |item| (id, item))
}

pub struct StatusBridge {
    pub connector: Box<dyn Connector>,
}

impl StatusBridge {
    pub fn new(connector: Box<dyn Connector>) -> Self {
        StatusBridge { connector }
    }
}

pub trait Connector: Send + Sync {
    fn forward_status<'a>(
        &'a self,
        peer_id: PeerId,
        status: Status,
    ) -> Pin<Box<dyn Future<Output = Result<(), StatusError>> + Send + 'a>>;
}

#[derive(Clone)]
pub struct NoOpConnector;

impl NoOpConnector {
    pub fn new() -> Self {
        NoOpConnector {}
    }
}

impl Default for NoOpConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl Connector for NoOpConnector {
    fn forward_status<'a>(
        &'a self,
        _peer_id: PeerId,
        _status: Status,
    ) -> Pin<Box<dyn Future<Output = Result<(), StatusError>> + Send + 'a>> {
        Box::pin(async move { Ok(()) })
    }
}

#[derive(Clone)]
pub struct AimConnector {
    connect_string: String,
    client: reqwest::Client,
    // peer_id: PeerId
}

impl AimConnector {
    pub fn new(connect_string: String) -> Self {
        AimConnector {
            connect_string,
            client: reqwest::Client::new(),
        }
    }
}

impl Connector for AimConnector {
    fn forward_status<'a>(
        &'a self,
        peer_id: PeerId,
        status: Status,
    ) -> Pin<Box<dyn Future<Output = Result<(), StatusError>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("http://{}/status", self.connect_string);
            for metric in status.metrics{
                let aim_status = hypha_messages::aim_messages::Status {
                    worker_id: peer_id,
                    round: status.round,
                    metric_name: metric.0,
                    value: metric.1,
                };
                let _ = self.client.post(&url).json(&aim_status).send().await?;
            }
            Ok(())
        })
    }
}
