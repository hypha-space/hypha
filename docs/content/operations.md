# Operations

This section covers operational aspects of running Hypha in production, including fault tolerance, resource management, and comprehensive monitoring through OpenTelemetry.

## Fault Tolerance

Hypha handles failures through lease-based mechanisms and automatic detection, with ongoing work to improve recovery capabilities.

### How Faults Are Handled

**Lease Expiration on Failure**: Workers and schedulers maintain resource reservations through periodic lease renewals. When a node fails, it stops renewing leases, and resources are automatically released when leases expire.

**Automatic Detection**: Failure detection is implicit through missed renewals rather than explicit failure messages. This approach avoids requiring perfect failure detectors or consensus protocols.

**No Explicit Failure Messages**: Nodes do not send failure notifications. The absence of lease renewal indicates failure.

**Timeout-Based Detection**: Lease timeout values determine failure detection latency. Shorter timeouts detect failures faster but increase network overhead. Typical values: 30-60 seconds.

### Current Fault Handling Capabilities

**Worker Failures**:
- Worker stops renewing lease
- Scheduler detects expiration
- Resource marked as available for reallocation
- Job state lost (no automatic rescheduling yet)

**Scheduler Failures**:
- Workers detect missed lease renewals
- Workers release reserved resources
- Job state lost
- Requires manual restart and job resubmission

**Gateway Failures**:
- Nodes use alternate gateways from bootstrap list
- DHT routing adapts to gateway absence
- Relay circuits through failed gateway are dropped

**Data Node Failures**:
- Workers retry data fetches
- Scheduler can reassign data slices to workers
- If dataset unavailable, training stalls

**Parameter Server Failures**:
- Training round fails
- Requires manual restart
- Workers wait for new parameter server allocation

### Ongoing Work

Several enhancements are under development to improve fault tolerance:

**Automatic Rescheduling**:
- Detect lease failures during training
- Automatically reallocate failed workers
- Continue training from last synchronization point
- Status: Planned

**Checkpoint and Resume**:
- Periodic model checkpoints
- Job state persistence
- Resume from checkpoint after failures
- Status: Planned

**Graceful Degradation with Partial Worker Loss**:
- Continue training with reduced worker pool
- Dynamically adjust batch allocation
- Maintain minimum worker threshold
- Status: Under consideration

**Stream Failure Detection and Failover**:
- Detect broken data streams
- Automatic retry with exponential backoff
- Failover to alternate data nodes if available
- Status: Partially implemented

**Scheduler State Persistence**:
- Persist worker pool state to disk
- Serialize data slice assignments
- Enable scheduler failover
- Status: Planned

## Resource Management

Hypha uses lease-based resource reservations to manage allocation and prevent conflicts.

### Lease-Based Reservations

**Temporary Leases**: When workers send offers, they create short-lived leases (typically 5-10 seconds) to prevent double-booking during scheduler selection.

**Renewable Leases**: Accepted offers transition to renewable leases maintained through periodic renewal (typically every 30-60 seconds).

**Automatic Expiration**: Leases expire if not renewed, automatically releasing resources without explicit cleanup messages.

**Lease Parameters**:
- Initial timeout: 5-10 seconds (temporary leases)
- Renewal interval: 30-60 seconds (renewable leases)
- Renewal window: typically 75% of lease lifetime (renew at 22-45 seconds for 30-60s lease)

### Dynamic Pricing and Load Balancing

**Worker Pricing Thresholds**: Workers maintain configurable minimum prices for resource offers. Only requests meeting or exceeding thresholds receive offers.

**Scheduler Bidding**: Schedulers advertise willingness to pay via task advertisements. Pricing signals help distribute load across workers.

**Market-Based Allocation**: Higher scheduler bids receive priority when multiple schedulers compete for resources.

**Load Balancing**: Pricing naturally balances load:
- Overloaded workers can increase thresholds
- Underutilized workers can lower thresholds
- System reaches equilibrium without central coordination

**Future Enhancement**: More sophisticated pricing strategies (utilization-based, affinity-based) are planned.

### Resource Limits

**Current State**: Resource advertisement (CPU, memory, GPU, storage) informs scheduler decisions but is not enforced by the worker.

**Planned Enhancement**: Actual resource limiting through cgroups or similar mechanisms to prevent resource oversubscription.

**Workaround**: Deploy workers in containers or VMs with resource limits to enforce quotas at infrastructure level.

## Monitoring and Observability

Hypha integrates with OpenTelemetry for comprehensive observability across traces, metrics, and logs.

### OpenTelemetry Integration

All Hypha components (gateway, scheduler, worker, data node) support OTLP (OpenTelemetry Protocol) export.

**Configuration** (applies to all components):
```toml
telemetry_endpoint = "http://otel-collector:4318"
telemetry_protocol = "http/protobuf"
telemetry_sampler = "traceidratio"
telemetry_sample_ratio = 0.1

[telemetry_attributes]
"service.name" = "hypha-{component}"
"service.version" = "0.1.0"
"deployment.environment" = "production"
```

**OTLP Endpoint**: Where to send telemetry data. Typically an OpenTelemetry Collector.

**Protocol Options**:
- `grpc`: gRPC transport (default port 4317)
- `http/protobuf`: HTTP with protobuf encoding (default port 4318)
- `http/json`: HTTP with JSON encoding (default port 4318)

**Metadata and Authentication**: Headers for endpoint authentication and routing.
```toml
[telemetry_headers]
"Authorization" = "Bearer <api-token>"
"X-Scope-OrgID" = "<tenant-id>"
```

### Traces

Distributed tracing tracks operations across Hypha components.

**Network Operation Tracing**:
- DHT lookups and peer discovery
- Gossipsub message propagation
- Request/response protocol exchanges
- Stream establishment and data transfer

**Training Operation Tracing**:
- Task advertisement publication
- Worker offer evaluation and submission
- Lease renewal cycles
- Job dispatch and execution
- Data slice fetching
- Gradient transmission and aggregation
- Model weight broadcasting

**Cross-Component Traces**: Trace context propagates across network boundaries, enabling end-to-end visibility.

**Example Trace**:
```
[Scheduler] Publish task advertisement
  ├─ [Network] Gossipsub publish
  │   └─ [Gateway] Relay message
  │       └─ [Worker] Receive advertisement
  └─ [Worker] Evaluate request
      └─ [Worker] Send offer
          └─ [Scheduler] Receive offer
              └─ [Scheduler] Accept offer (lease renewal)
```

### Metrics

Hypha exports operational metrics for monitoring system health and performance.

**Network Metrics**:
- Active connection count per component
- Connection establishment/failure rates
- Bandwidth utilization (bytes sent/received)
- DHT routing table size
- Gossipsub mesh peer count
- Relay circuit count (gateways)

**Scheduler Metrics**:
- Active worker pool size
- Lease renewal success/failure rates
- Data slice allocation statistics
- Synchronization event frequency
- Job completion times

**Worker Metrics**:
- Active job count
- Job execution duration
- Resource utilization (CPU, memory, GPU)
- Data slice fetch latency
- Gradient transmission times

**Gateway Metrics**:
- Relayed connection count
- Relay bandwidth usage
- DHT query success rate

**Data Node Metrics**:
- Active stream count
- Bytes served per dataset
- Slice request rate
- Serving latency

### Logs

Structured logging with contextual information supports debugging and auditing.

**Log Levels**:
- `error`: Failures requiring attention
- `warn`: Unusual conditions that may indicate problems
- `info`: Normal operational events
- `debug`: Detailed information for troubleshooting
- `trace`: Very detailed execution flow

**Configuration**:
Set via environment variable:
```bash
RUST_LOG=hypha=info,hypha_network=debug
```

**Structured Fields**: Logs include contextual information:
- Component name
- Trace ID (when applicable)
- Peer IDs
- Job IDs
- Operation types

**Example Log Entry**:
```json
{
  "timestamp": "2025-01-15T14:32:10.123Z",
  "level": "info",
  "component": "hypha-scheduler",
  "message": "Lease renewed successfully",
  "trace_id": "a1b2c3d4e5f6...",
  "worker_peer_id": "12D3KooW...",
  "lease_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

### Deployment Example: OpenTelemetry Collector

Deploy an OTLP collector to aggregate telemetry:

**Collector Configuration** (otel-collector-config.yaml):
```yaml
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: 0.0.0.0:4317
      http:
        endpoint: 0.0.0.0:4318

processors:
  batch:
    timeout: 10s

exporters:
  prometheus:
    endpoint: 0.0.0.0:9090

  jaeger:
    endpoint: jaeger:14250
    tls:
      insecure: true

  loki:
    endpoint: http://loki:3100/loki/api/v1/push

service:
  pipelines:
    traces:
      receivers: [otlp]
      processors: [batch]
      exporters: [jaeger]

    metrics:
      receivers: [otlp]
      processors: [batch]
      exporters: [prometheus]

    logs:
      receivers: [otlp]
      processors: [batch]
      exporters: [loki]
```

**Starting Collector**:
```bash
docker run -d \
  -v $(pwd)/otel-collector-config.yaml:/etc/otel-collector-config.yaml \
  -p 4317:4317 \
  -p 4318:4318 \
  -p 9090:9090 \
  otel/opentelemetry-collector:latest \
  --config /etc/otel-collector-config.yaml
```

**Hypha Configuration**:
```toml
telemetry_endpoint = "http://otel-collector:4318"
telemetry_protocol = "http/protobuf"
```

### Visualization

**Traces**: View in Jaeger UI (`http://localhost:16686`)
- Search traces by service, operation, tags
- Visualize trace timelines
- Identify latency bottlenecks

**Metrics**: Query via Prometheus (`http://localhost:9090`)
- PromQL queries for analysis
- Grafana dashboards for visualization
- Alerting rules for operational issues

**Logs**: Search in Grafana Loki
- LogQL queries for log analysis
- Correlation with traces via trace IDs
- Real-time log streaming

### Common Monitoring Queries

**Lease Renewal Success Rate**:
```promql
sum(rate(hypha_lease_renewals_total{status="success"}[5m])) /
sum(rate(hypha_lease_renewals_total[5m]))
```

**Training Throughput** (samples per second):
```promql
sum(rate(hypha_training_samples_total[5m]))
```

**Data Fetch Latency** (p95):
```promql
histogram_quantile(0.95, rate(hypha_data_fetch_duration_bucket[5m]))
```

**Active Worker Count**:
```promql
hypha_active_workers
```

**Network Bandwidth Usage**:
```promql
sum(rate(hypha_network_bytes_sent_total[5m])) +
sum(rate(hypha_network_bytes_received_total[5m]))
```

### Alerting

Define alerts for operational issues:

**Lease Renewal Failures**:
```yaml
alert: HighLeaseRenewalFailures
expr: |
  (sum(rate(hypha_lease_renewals_total{status="failure"}[5m])) /
   sum(rate(hypha_lease_renewals_total[5m]))) > 0.1
for: 5m
annotations:
  summary: "Lease renewal failure rate above 10%"
```

**Worker Pool Depletion**:
```yaml
alert: LowWorkerCount
expr: hypha_active_workers < 2
for: 10m
annotations:
  summary: "Active worker count below threshold"
```

**Data Fetch Latency**:
```yaml
alert: SlowDataFetch
expr: |
  histogram_quantile(0.95,
    rate(hypha_data_fetch_duration_bucket[5m])) > 60
for: 5m
annotations:
  summary: "P95 data fetch latency above 60 seconds"
```

**Gateway Connectivity**:
```yaml
alert: GatewayDown
expr: up{job="hypha-gateway"} == 0
for: 2m
annotations:
  summary: "Gateway node is down"
```

## Performance Tuning

### Network Performance

**TCP Buffer Sizes**: Increase for high-bandwidth scenarios.
```bash
sysctl -w net.core.rmem_max=134217728
sysctl -w net.core.wmem_max=134217728
sysctl -w net.ipv4.tcp_rmem="4096 87380 134217728"
sysctl -w net.ipv4.tcp_wmem="4096 65536 134217728"
```

**Connection Limits**: Increase file descriptor limits.
```bash
ulimit -n 65536
```

**Gossipsub Tuning**: Adjust mesh size for large networks (modify defaults in code).

### Data Transfer Optimization

**Prefetching**: Workers prefetch next data slice before completing current one.

**Parallel Streams**: Data nodes serve multiple workers concurrently.

**Compression**: Future enhancement to compress data streams.

### Scheduler Optimization

**Synchronization Timing**: Performance-aware scheduling automatically optimizes sync points.

**Batch Size Allocation**: Heterogeneous allocation adapts to worker capabilities.

**Slice Size**: Adjust dataset slice sizes based on network bandwidth (see [Data Node](data-node.md#choosing-slice-sizes)).

## Backup and Recovery

### State to Preserve

**Certificates**: Critical for network access.
- CA certificates (root and intermediates)
- Node certificates and private keys
- CRLs

**Configuration Files**: Node configurations.
- TOML configuration files
- Accelerate configuration files

**Datasets**: Prepared training data.
- SafeTensors slice files
- Dataset metadata

**Model Checkpoints** (when implemented):
- Periodic model snapshots
- Optimizer state

### Backup Procedures

**Certificate Backup**:
```bash
tar -czf certs-backup.tar.gz /etc/hypha/certs/
```

Store offline in secure location. Root CA keys should never be on networked systems.

**Configuration Backup**:
```bash
tar -czf config-backup.tar.gz /etc/hypha/*.toml
```

Version control configuration files for change tracking.

**Dataset Backup**:
```bash
rsync -av /var/hypha/datasets/ backup-server:/hypha-datasets-backup/
```

Datasets are large; consider incremental backups or cloud storage.

**Checkpoint Backup** (future):
Automated periodic snapshots to durable storage (S3, network filesystem, etc.).

### Recovery Procedures

**Certificate Recovery**:
1. Restore certificates from backup
2. Verify file permissions (keys: 0600, certs: 0644)
3. Restart nodes

**Configuration Recovery**:
1. Restore configuration files
2. Validate configuration: `hypha-{component} --config config.toml --validate`
3. Restart nodes

**Dataset Recovery**:
1. Restore datasets from backup
2. Verify SafeTensors file integrity
3. Restart data nodes

**Job Recovery** (manual currently):
1. Identify last successful synchronization point
2. Restart scheduler with same job configuration
3. Workers reallocate and resume training

## Capacity Planning

### Gateway Capacity

**Connections**: Plan for 1 gateway per 50-100 nodes.

**Bandwidth**: Gateways relay traffic for NAT traversal. Estimate 10-20% of total network traffic.

**Compute**: Minimal (1-2 CPU cores, 2-4 GB RAM).

### Scheduler Capacity

**Worker Pool Size**: Single scheduler handles 10-100 workers depending on synchronization frequency.

**Memory**: Approximately 100 MB + 10 MB per worker.

**Compute**: Moderate (2-4 CPU cores).

**Network**: Minimal bandwidth (mostly control messages).

### Worker Capacity

**Jobs per Worker**: Typically 1 training job + 1 parameter server job.

**GPU Utilization**: Aim for 80-90% GPU utilization during training.

**Memory**: Model size + optimizer state + 2-3 batches worth of data.

**Storage**: Local cache for 5-10 data slices per job.

### Data Node Capacity

**Concurrent Streams**: Plan for 1 stream per active worker.

**Bandwidth**: Estimate based on slice size and fetch frequency.
  - Example: 10 workers, 1 GB slices, fetching every 5 minutes = 10 * 1GB / 5min = 2 GB/min = 33 MB/s

**Storage**: Dataset size + headroom for future datasets.

**Compute**: Minimal (1-2 CPU cores for memory mapping and streaming).

## Troubleshooting

### Connection Issues

**Nodes cannot connect to network**:
- Verify gateway addresses are correct and reachable
- Check firewall rules allow traffic on configured ports
- Confirm certificates are valid and trusted
- Review logs for TLS handshake failures

**Intermittent disconnections**:
- Check network stability (packet loss, latency)
- Verify lease renewal intervals accommodate network conditions
- Monitor gateway health and load

### Performance Issues

**Slow training progress**:
- Monitor straggler workers (check batch processing times)
- Verify data fetch latency is acceptable
- Check network bandwidth utilization
- Review GPU utilization on workers

**High synchronization overhead**:
- Increase inner steps H (automatic via performance-aware scheduling)
- Check parameter server capacity
- Monitor gradient transmission times

### Resource Issues

**Workers not receiving offers**:
- Verify worker connected to network
- Check resource advertisement matches scheduler requirements
- Review pricing thresholds

**Lease failures**:
- Check network connectivity between nodes
- Verify lease timeout configuration
- Review logs for renewal errors

### Data Issues

**Dataset not found**:
- Confirm data node announced dataset to DHT
- Verify dataset name matches scheduler configuration exactly
- Check data node connectivity to network

**Slow data fetches**:
- Monitor data node bandwidth usage
- Check network path to data node
- Consider replicating dataset to additional data nodes

## Next Steps

- Set up comprehensive monitoring via [OpenTelemetry](#opentelemetry-integration)
- Configure alerting for critical failures
- Establish backup and recovery procedures
- Plan capacity for expected workloads
- Review [Security](security.md) for production hardening
