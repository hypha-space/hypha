use std::{collections::HashMap, future::Future, pin::Pin};

use futures_util::{Stream, StreamExt, stream::SelectAll};
use hypha_telemetry::otel::{
    KeyValue,
    metrics::{Gauge, Meter},
};
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metrics {
    pub round: u32,
    pub metrics: HashMap<String, f32>,
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
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    break;
                }
                item = self.streams.next() => {
                    match item {
                        Some((per_id, metrics)) => {
                            tracing::debug!("Forwarding metric");
                            if let Err(e) = self.connector.forward_metrics(per_id, metrics).await {
                                tracing::warn!("Failed to forward metrics: {}", e);
                            }
                        }
                        None => {
                            break;
                        }
                    }
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

#[derive(Clone)]
pub struct OtelConnector {
    gauge: Gauge<f64>,
    job_id: String,
}

impl OtelConnector {
    pub fn new(meter: Meter, job_id: String) -> Self {
        let gauge = meter
            .f64_gauge("hypha.scheduler.metric")
            .with_description("Training metrics reported by workers")
            .build();

        OtelConnector { gauge, job_id }
    }
}

impl Connector for OtelConnector {
    fn forward_metrics<'a>(
        &'a self,
        peer_id: PeerId,
        metrics: Metrics,
    ) -> Pin<Box<dyn Future<Output = Result<(), MetricsError>> + Send + 'a>> {
        Box::pin(async move {
            for (metric_name, value) in metrics.metrics {
                let value = f64::from(value);
                if !value.is_finite() {
                    tracing::debug!(
                        %peer_id,
                        round = metrics.round,
                        metric_name,
                        value,
                        "Skipped non-finite training metric"
                    );
                    continue;
                }

                let attrs = [
                    KeyValue::new("job_id", self.job_id.clone()),
                    KeyValue::new("peer_id", peer_id.to_string()),
                    KeyValue::new("round", metrics.round.to_string()),
                    KeyValue::new("metric_name", metric_name.clone()),
                ];

                self.gauge.record(value, &attrs);
                tracing::trace!(
                    %peer_id,
                    round = metrics.round,
                    metric_name,
                    value,
                    "Recorded training metric"
                );
            }

            Ok(())
        })
    }
}
