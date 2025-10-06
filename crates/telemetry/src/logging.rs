use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{
    ExportConfig, Protocol as OtlpProtocol, WithExportConfig, WithHttpConfig, WithTonicConfig,
};
use opentelemetry_sdk::{Resource, logs::SdkLoggerProvider};
use tracing::Subscriber;
use tracing_subscriber::registry::LookupSpan;

use crate::{
    attributes::Attributes,
    errors::Error,
    layer::NoopLayer,
    otlp::{Endpoint, Headers, Protocol},
};

/// Build an OTLP logs bridge layer for `tracing` and the corresponding `LoggerProvider`.
pub struct Logging(Option<SdkLoggerProvider>);

impl Logging {
    pub fn new(
        endpoint: Option<Endpoint>,
        headers: Option<Headers>,
        protocol: Option<Protocol>,
        attributes: Option<Attributes>,
    ) -> Result<Logging, Error> {
        if endpoint.is_none() {
            return Ok(Logging(None));
        }

        let endpoint: Option<String> = endpoint.map(Into::into);
        let otlp_protocol: OtlpProtocol = protocol.unwrap_or_default().into();
        let resource: Resource = attributes.unwrap_or_default().into();

        // Build OTLP log exporter
        let exporter = match protocol.unwrap_or_default() {
            Protocol::Grpc => opentelemetry_otlp::LogExporter::builder()
                .with_tonic()
                // NOTE: Attach gRPC metadata for auth (e.g., Authorization).
                .with_metadata(headers.unwrap_or_default().into())
                .with_export_config(ExportConfig {
                    endpoint,
                    protocol: otlp_protocol,
                    timeout: None,
                })
                .build()?,
            _ => opentelemetry_otlp::LogExporter::builder()
                .with_http()
                // NOTE: Attach header for auth (e.g., Authorization).
                .with_headers(headers.unwrap_or_default().into())
                .with_export_config(ExportConfig {
                    endpoint: endpoint.map(|e| format!("{e}/v1/logs")),
                    protocol: otlp_protocol,
                    timeout: None,
                })
                .build()?,
        };

        let provider = SdkLoggerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(exporter)
            .build();

        Ok(Logging(Some(provider)))
    }

    pub fn provider(&self) -> Option<&SdkLoggerProvider> {
        self.0.as_ref()
    }

    pub fn shutdown(&self) -> Result<(), Error> {
        if let Some(p) = &self.0 {
            p.shutdown()?;
        }
        Ok(())
    }

    pub fn layer<S>(&self) -> Box<dyn tracing_subscriber::Layer<S> + Send + Sync>
    where
        S: Subscriber + for<'span> LookupSpan<'span> + Send + Sync,
    {
        if let Some(provider) = &self.0 {
            Box::new(OpenTelemetryTracingBridge::new(provider))
        } else {
            Box::new(NoopLayer)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_no_provider_when_endpoint_is_none() {
        let logging = Logging::new(None, None, None, None).unwrap();
        assert!(logging.provider().is_none());
    }

    #[test]
    fn returns_noop_layer_when_endpoint_is_none() {
        let logging = Logging::new(None, None, None, None).unwrap();

        // Should return a Noop layer for any subscriber type;
        // constructing the layer must not panic.
        let _layer = logging.layer::<tracing_subscriber::Registry>();
    }

    #[test]
    fn shutdown_succeeds_when_endpoint_is_none() {
        let logging = Logging::new(None, None, None, None).unwrap();
        logging.shutdown().unwrap();
    }
}
