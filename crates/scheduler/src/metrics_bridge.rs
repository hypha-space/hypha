use std::pin::Pin;

use futures_util::{Stream, StreamExt, stream::SelectAll};
use hypha_messages::progress::Metrics;
use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum MetricsError {
    #[error("Connection to status receiver lost")]
    ConnectionLost,
    #[error("Error when sending request: {0}")]
    Request(#[from] reqwest::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AimMetrics {
    pub worker_id: PeerId,
    pub round: u32,
    pub metric_name: String,
    pub value: f32,
}

pub fn with_id<S: Stream>(id: PeerId, stream: S) -> impl Stream<Item = (PeerId, S::Item)> {
    stream.map(move |item| (id, item))
}

type PeerMetrics = (PeerId, Metrics);

pub struct MetricsBridge
where
    PeerId: Send + 'static,
{
    pub connector: Box<dyn Connector>,
    streams: SelectAll<Pin<Box<dyn Stream<Item = PeerMetrics> + Send>>>,
}

impl MetricsBridge
where
    PeerId: Send + 'static,
{
    pub fn new(connector: Box<dyn Connector>) -> Self {
        MetricsBridge {
            connector,
            streams: SelectAll::new(),
        }
    }

    pub fn register_stream<St>(&mut self, stream: St)
    where
        St: Stream<Item = PeerMetrics> + Send + 'static,
    {
        self.streams.push(Box::pin(stream))
    }

    pub async fn run(mut self, cancel: CancellationToken) -> Result<(), MetricsError> {
        while !cancel.is_cancelled() {
            match self.streams.next().await {
                Some((per_id, metrics)) => {
                    tracing::debug!("Forwarding metric");
                    self.connector
                        .forward_metrics(per_id, metrics)
                        .await
                        .expect("Status forwarded");
                }
                None => {
                    tracing::debug!("None received")
                }
            }
        }
        Ok(())
    }
}

pub trait Connector: Send + Sync {
    fn forward_metrics<'a>(
        &'a self,
        peer_id: PeerId,
        metrics: Metrics,
    ) -> Pin<Box<dyn Future<Output = Result<(), MetricsError>> + Send + 'a>>;
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
    fn forward_metrics<'a>(
        &'a self,
        _peer_id: PeerId,
        _metrics: Metrics,
    ) -> Pin<Box<dyn Future<Output = Result<(), MetricsError>> + Send + 'a>> {
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
    fn forward_metrics<'a>(
        &'a self,
        peer_id: PeerId,
        metrics: Metrics,
    ) -> Pin<Box<dyn Future<Output = Result<(), MetricsError>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("http://{}/status", self.connect_string);
            for metric in metrics.metrics {
                let aim_status = AimMetrics {
                    worker_id: peer_id,
                    round: metrics.round,
                    metric_name: metric.0,
                    value: metric.1,
                };
                let _ = self.client.post(&url).json(&aim_status).send().await?;
            }
            Ok(())
        })
    }
}
