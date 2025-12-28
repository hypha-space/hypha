use std::{
    collections::HashMap,
    fs::OpenOptions,
    future::Future,
    io::Write,
    path::PathBuf,
    pin::Pin,
    time::{SystemTime, UNIX_EPOCH},
};

use futures_util::{Stream, StreamExt, stream::SelectAll};
use hypha_telemetry::otel::{
    KeyValue,
    metrics::{Gauge, Meter},
};
use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum MetricsError {
    #[error("Connection to status receiver lost")]
    ConnectionLost,
    #[error("Error when sending request: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Error when writing metrics: {0}")]
    Io(#[from] std::io::Error),
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
    pub connectors: Vec<Box<dyn Connector>>,
    streams: SelectAll<Pin<Box<dyn Stream<Item = PeerMetrics> + Send>>>,
}

impl MetricsBridge
where
    PeerId: Send + 'static,
{
    pub fn new(connectors: Vec<Box<dyn Connector>>) -> Self {
        let mut connectors = connectors;
        if connectors.is_empty() {
            connectors.push(Box::new(NoOpConnector::new()));
        }

        MetricsBridge {
            connectors,
            streams: SelectAll::new(),
        }
    }

    pub fn add_connector(&mut self, connector: Box<dyn Connector>) {
        self.connectors.push(connector);
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
                            for connector in &self.connectors {
                                if let Err(e) = connector.forward_metrics(per_id, metrics.clone()).await {
                                    tracing::warn!("Failed to forward metrics: {}", e);
                                }
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
pub struct JsonlConnector {
    path: PathBuf,
}

impl JsonlConnector {
    pub fn new(path: PathBuf) -> Self {
        JsonlConnector { path }
    }
}

impl Connector for JsonlConnector {
    fn forward_metrics<'a>(
        &'a self,
        peer_id: PeerId,
        metrics: Metrics,
    ) -> Pin<Box<dyn Future<Output = Result<(), MetricsError>> + Send + 'a>> {
        Box::pin(async move {
            let timestamp_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();

            #[allow(clippy::disallowed_methods)]
            let record = json!({
                "timestamp": timestamp_ms,
                "peer_id": peer_id.to_string(),
                "round": metrics.round,
                "metrics": metrics.metrics,
            });

            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            writeln!(file, "{}", record)?;

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

#[derive(Clone)]
pub struct CsvConnector {
    path: PathBuf,
}

impl CsvConnector {
    pub fn new(path: PathBuf) -> Self {
        CsvConnector { path }
    }
}

impl Connector for CsvConnector {
    fn forward_metrics<'a>(
        &'a self,
        peer_id: PeerId,
        metrics: Metrics,
    ) -> Pin<Box<dyn Future<Output = Result<(), MetricsError>> + Send + 'a>> {
        Box::pin(async move {
            let timestamp_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();

            let mut keys: Vec<&String> = metrics.metrics.keys().collect();
            keys.sort();

            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;

            // NOTE: Write header if file is empty for compatibility with HF Trackio CSV import
            if file.metadata()?.len() == 0 {
                let mut header = String::from("step,timestamp,peer_id");
                for key in &keys {
                    header.push(',');
                    header.push_str(key);
                }
                writeln!(file, "{}", header)?;
            }

            let mut row = format!("{},{},{}", metrics.round, timestamp_ms, peer_id);
            for key in keys {
                let val = metrics.metrics.get(key).copied().unwrap_or_default();
                row.push(',');
                row.push_str(&val.to_string());
            }
            writeln!(file, "{}", row)?;

            Ok(())
        })
    }
}
