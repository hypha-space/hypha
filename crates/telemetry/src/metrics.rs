use std::time::Duration;

use opentelemetry::metrics::Meter;
use opentelemetry_otlp::{ExportConfig, WithExportConfig, WithHttpConfig, WithTonicConfig};
use opentelemetry_sdk::metrics::{SdkMeterProvider, Temporality};

use crate::{
    attributes::Attributes,
    errors::Error,
    otlp::{Endpoint, Headers, Protocol},
};

pub mod global {
    use super::*;

    /// Return the global Meter for hypha-telemetry without initializing OTLP.
    /// If no SDK provider is installed, this is a no-op meter provided by the SDK.
    pub fn meter() -> Meter {
        opentelemetry::global::meter("hypha-telemetry")
    }

    pub fn set_provider(provider: Option<&SdkMeterProvider>) {
        if let Some(p) = provider {
            opentelemetry::global::set_meter_provider(p.clone());
        }
    }
}

/// Thin wrapper around an optional `SdkMeterProvider`
pub struct Metrics(Option<SdkMeterProvider>);

impl Metrics {
    pub fn new(
        endpoint: Option<Endpoint>,
        headers: Option<Headers>,
        protocol: Option<Protocol>,
        attributes: Option<Attributes>,
        interval: Duration,
    ) -> Result<Metrics, Error> {
        if endpoint.is_none() {
            return Ok(Metrics(None));
        }

        let exporter = match protocol {
            Some(Protocol::Grpc) => opentelemetry_otlp::MetricExporter::builder()
                .with_tonic()
                // NOTE: Attach gRPC metadata for auth (e.g., Authorization).
                .with_metadata(headers.unwrap_or_default().into())
                .with_export_config(ExportConfig {
                    endpoint: endpoint.map(Into::into),
                    protocol: protocol.unwrap_or_default().into(),
                    timeout: Some(interval),
                })
                .with_temporality(Temporality::default())
                .build()?,
            _ => opentelemetry_otlp::MetricExporter::builder()
                .with_http()
                // NOTE: Attach headers for auth (e.g., Authorization).
                .with_headers(headers.unwrap_or_default().into())
                .with_export_config(ExportConfig {
                    endpoint: endpoint.map(|endpoint| format!("{endpoint}/v1/metrics")),
                    protocol: protocol.unwrap_or_default().into(),
                    timeout: Some(interval),
                })
                .with_temporality(Temporality::default())
                .build()?,
        };

        let provider = SdkMeterProvider::builder()
            .with_resource(attributes.unwrap_or_default().into())
            .with_periodic_exporter(exporter)
            .build();

        Ok(Metrics(Some(provider)))
    }

    pub fn provider(&self) -> Option<&SdkMeterProvider> {
        self.0.as_ref()
    }

    pub fn shutdown(&self) -> Result<(), Error> {
        if let Some(p) = &self.0 {
            p.shutdown()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_meter_without_initializing_otlp() {
        let meter = crate::metrics::global::meter();
        let counter = meter.u64_counter("test.counter").build();
        counter.add(1, &[]);
    }

    #[test]
    fn returns_no_provider_and_allows_shutdown_with_none_endpoint() {
        let m = Metrics::new(None, None, None, None, Duration::from_millis(10)).unwrap();
        assert!(m.provider().is_none());
        m.shutdown().unwrap();
    }
}
