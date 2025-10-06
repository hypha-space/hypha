pub mod attributes;
pub mod bandwidth;
pub mod errors;
pub mod layer;
pub mod logging;
pub mod metrics;
pub mod otlp;
pub mod tracing;

#[cfg(test)]
pub mod testing;

use std::time::Duration;

pub fn metrics(
    endpoint: Option<otlp::Endpoint>,
    headers: Option<otlp::Headers>,
    protocol: Option<otlp::Protocol>,
    attributes: Option<attributes::Attributes>,
    interval: Duration,
) -> Result<metrics::Metrics, errors::Error> {
    metrics::Metrics::new(endpoint, headers, protocol, attributes, interval)
}

pub fn logging(
    endpoint: Option<otlp::Endpoint>,
    headers: Option<otlp::Headers>,
    protocol: Option<otlp::Protocol>,
    attributes: Option<attributes::Attributes>,
) -> Result<logging::Logging, errors::Error> {
    logging::Logging::new(endpoint, headers, protocol, attributes)
}

pub fn tracing(
    endpoint: Option<otlp::Endpoint>,
    headers: Option<otlp::Headers>,
    protocol: Option<otlp::Protocol>,
    attributes: Option<attributes::Attributes>,
    sampler: Option<tracing::SamplerKind>,
    sample_ratio: Option<f64>,
) -> Result<tracing::Tracing, errors::Error> {
    tracing::Tracing::new(
        endpoint,
        headers,
        protocol,
        attributes,
        sampler,
        sample_ratio,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_wrapper_none_endpoint() {
        let m = metrics(None, None, None, None, Duration::from_millis(10)).unwrap();
        assert!(m.provider().is_none());
        m.shutdown().unwrap();
    }

    #[test]
    fn logging_wrapper_none_endpoint_and_layer() {
        let l = logging(None, None, None, None).unwrap();
        assert!(l.provider().is_none());
        // Building a layer should not panic
        let _layer = l.layer::<tracing_subscriber::Registry>();
        l.shutdown().unwrap();
    }

    #[test]
    fn tracing_wrapper_none_endpoint_and_layer() {
        let t = tracing(None, None, None, None, None, None).unwrap();
        assert!(t.provider().is_none());
        // Building a layer should not panic
        let _layer = t.layer::<tracing_subscriber::Registry>();
        t.shutdown().unwrap();
    }
}
