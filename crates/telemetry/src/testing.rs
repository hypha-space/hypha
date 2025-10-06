//! Test utilities for OpenTelemetry metrics and logs.
//! Only compiled for tests.
#![cfg(test)]

use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider, data};

/// Build a test `SdkMeterProvider` with an in-memory exporter.
pub fn build_test_provider() -> (SdkMeterProvider, InMemoryMetricExporter) {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    (provider, exporter)
}

/// Sum all u64 Sum datapoints for a metric by name.
///
/// NOTE: Our tests record a single datapoint per instrument; summing keeps this generic
/// without depending on specific attributes.
pub fn read_sum_u64(rms: &[data::ResourceMetrics], instrument: &str) -> Option<u64> {
    let mut total: u64 = 0;
    let mut found = false;
    for rm in rms.iter() {
        for scope in rm.scope_metrics() {
            for metric in scope.metrics() {
                if metric.name() != instrument {
                    continue;
                }
                match metric.data() {
                    data::AggregatedMetrics::U64(md) => match md {
                        data::MetricData::Sum(sum) => {
                            for dp in sum.data_points() {
                                total = total.saturating_add(dp.value());
                                found = true;
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }
    if found { Some(total) } else { None }
}
