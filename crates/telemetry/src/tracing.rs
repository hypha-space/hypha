use opentelemetry_otlp::{
    ExportConfig, Protocol as OtlpProtocol, WithExportConfig, WithHttpConfig, WithTonicConfig,
};
use opentelemetry_sdk::{
    Resource,
    trace::{Sampler, SdkTracerProvider},
};
use serde::{Deserialize, Serialize};
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;

use crate::{
    attributes::Attributes,
    errors::Error,
    layer::NoopLayer,
    otlp::{Endpoint, Headers, Protocol},
};

/// Supported sampler kinds for configuring tracing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SamplerKind {
    AlwaysOn,
    AlwaysOff,
    /// Use a fixed ratio without considering the parent decision.
    #[serde(rename = "traceidratio")]
    TraceIdRatio,
    /// Parent-based sampler using a trace-id ratio for root spans.
    #[serde(rename = "parentbased_traceidratio")]
    ParentBasedTraceIdRatio,
}

/// Wrapper around an optional `SdkTracerProvider` with convenience methods.
pub struct Tracing(Option<SdkTracerProvider>);

impl Tracing {
    pub fn new(
        endpoint: Option<Endpoint>,
        headers: Option<Headers>,
        protocol: Option<Protocol>,
        attributes: Option<Attributes>,
        sampler: Option<SamplerKind>,
        sample_ratio: Option<f64>,
    ) -> Result<Tracing, Error> {
        if endpoint.is_none() {
            return Ok(Tracing(None));
        }

        let endpoint: Option<String> = endpoint.map(Into::into);
        let otlp_protocol: OtlpProtocol = protocol.unwrap_or_default().into();
        let resource: Resource = attributes.unwrap_or_default().into();

        let exporter = match protocol.unwrap_or_default() {
            Protocol::Grpc => opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                // NOTE: Attach gRPC metadata for auth (e.g., Authorization).
                .with_metadata(headers.unwrap_or_default().into())
                .with_export_config(ExportConfig {
                    endpoint,
                    protocol: otlp_protocol,
                    timeout: None,
                })
                .build()?,
            _ => opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                // NOTE: Attach headers for auth (e.g., Authorization).
                .with_headers(headers.unwrap_or_default().into())
                .with_export_config(ExportConfig {
                    endpoint: endpoint.map(|e| format!("{e}/v1/traces")),
                    protocol: otlp_protocol,
                    timeout: None,
                })
                .build()?,
        };

        let sampler = match sampler {
            Some(SamplerKind::AlwaysOff) => Sampler::AlwaysOff,
            Some(SamplerKind::AlwaysOn) => Sampler::AlwaysOn,
            Some(SamplerKind::TraceIdRatio) => {
                let r = sample_ratio.unwrap_or(1.0).clamp(0.0, 1.0);
                Sampler::TraceIdRatioBased(r)
            }
            Some(SamplerKind::ParentBasedTraceIdRatio) => {
                let r = sample_ratio.unwrap_or(1.0).clamp(0.0, 1.0);
                Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(r)))
            }
            None => match sample_ratio {
                Some(r) => {
                    Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(r.clamp(0.0, 1.0))))
                }
                None => Sampler::AlwaysOn,
            },
        };

        let provider = SdkTracerProvider::builder()
            .with_resource(resource)
            .with_sampler(sampler)
            .with_batch_exporter(exporter)
            .build();

        Ok(Tracing(Some(provider)))
    }

    pub fn provider(&self) -> Option<&SdkTracerProvider> {
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
            use opentelemetry::trace::TracerProvider as _;
            Box::new(OpenTelemetryLayer::new(provider.tracer("tracing")))
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
        let tracing = Tracing::new(None, None, None, None, None, None).unwrap();
        assert!(tracing.provider().is_none());
    }

    #[test]
    fn returns_noop_layer_when_endpoint_is_none() {
        let tracing = Tracing::new(None, None, None, None, None, None).unwrap();
        // Constructing a layer for a registry should not panic when provider is None.
        let _layer = tracing.layer::<tracing_subscriber::Registry>();
    }

    #[test]
    fn shutdown_succeeds_when_endpoint_is_none() {
        let tracing = Tracing::new(None, None, None, None, None, None).unwrap();
        tracing.shutdown().unwrap();
    }
}
