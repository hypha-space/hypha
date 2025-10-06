use opentelemetry_otlp::ExporterBuildError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    /// OTEL exporter (pipeline) construction failed
    #[error("failed to build telemetry pipeline: {0}")]
    Build(#[from] ExporterBuildError),

    /// OpenTelemetry SDK surfaced an error during shutdown or lifecycle
    #[error("Open Telemetry failed: {0}")]
    Otel(#[from] opentelemetry_sdk::error::OTelSdkError),
}
