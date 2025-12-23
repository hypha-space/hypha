use std::time::Duration;

use libp2p::identity::PeerId;
use opentelemetry::{
    KeyValue,
    metrics::{Gauge, Meter},
};

/// Records ping RTT measurements with peer labels.
#[derive(Clone, Debug)]
pub struct RttMetrics {
    rtt_gauge: Gauge<f64>,
}

impl RttMetrics {
    /// Create a new RTT gauge using the provided meter.
    pub fn new(meter: &Meter) -> Self {
        let rtt_gauge = meter
            .f64_gauge("hypha.rtt.ms")
            .with_description("Ping round-trip time by peer")
            .with_unit("ms")
            .build();

        Self { rtt_gauge }
    }

    /// Record a ping RTT for the given peer.
    pub fn record(&self, peer: &PeerId, rtt: Duration) {
        let attrs = [KeyValue::new("peer_id", peer.to_string())];
        self.rtt_gauge.record(rtt.as_secs_f64() * 1000.0, &attrs);
    }
}
