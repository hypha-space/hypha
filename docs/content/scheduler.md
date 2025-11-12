# Scheduler

The scheduler orchestrates distributed training jobs by coordinating workers, managing resource allocation, and controlling DiLoCo synchronization. This section covers scheduler configuration, deployment, and operational details.

## Role and Responsibilities

The scheduler serves as the coordination point for distributed training:

**Task Advertisement**: Publishing resource requirements to workers via Gossipsub. Workers subscribe to the `hypha/worker` topic and evaluate incoming advertisements against their capabilities.

**Worker Selection**: Collecting offers from workers and selecting optimal matches. The scheduler implements greedy selection (lowest price first) to allocate resources efficiently.

**Lease Management**: Maintaining resource reservations through periodic renewal. Leases prevent resource double-booking while ensuring automatic cleanup if schedulers fail.

**DiLoCo Synchronization Coordination**: Tracking worker progress and determining when to trigger weight synchronization. The scheduler uses performance-aware scheduling to minimize stragglers by simulating future completion times.

**Data Distribution Tracking**: Assigning dataset slices to workers and ensuring complete coverage. The scheduler tracks slice states (AVAILABLE → ASSIGNED → USED) and reassigns slices when epochs complete.

**Performance-Aware Scheduling**: Allocating heterogeneous batch sizes based on worker capabilities. Faster workers process more data per synchronization round, improving overall efficiency.

**Parameter Server Lifecycle Management**: Coordinating parameter server startup, informing it which workers will participate, and setting collection timeouts.

**Metrics Aggregation**: Optionally forwarding training metrics to AIM for visualization and analysis.

## Installation and Setup

Install the scheduler binary following the [Installation](installation.md) guide.

For quick reference:

```bash
# From pre-built binary
curl -fsSL https://github.com/hypha-space/hypha/releases/latest/download/install.sh | sh

# Or build from source
cargo build --release -p hypha-scheduler
```

## Configuration Parameters

Scheduler configuration uses TOML format with two main sections: network/security settings and job specification.

### Network and Security Settings

**TLS Certificates** (required for mTLS):
- `cert_pem`: Path to scheduler's certificate (PEM format)
- `key_pem`: Path to scheduler's private key (PKCS#8 format)
- `trust_pem`: Path to CA certificate bundle
- `crls_pem`: Optional certificate revocation list

**Network Addresses**:
- `gateway_addresses`: List of gateway nodes for bootstrap (Multiaddr format)
- `listen_addresses`: Local addresses to bind (e.g., `/ip4/0.0.0.0/tcp/0`)
- `external_addresses`: Publicly reachable addresses to advertise
- `exclude_cidr`: CIDR ranges to exclude from peer discovery (defaults to reserved ranges)

**Relay Configuration**:
- `relay_circuit`: Enable relay via gateways (default: `true`)

**Telemetry** (OpenTelemetry):
- `telemetry_endpoint`: OTLP endpoint URL (e.g., `http://localhost:4318`)
- `telemetry_protocol`: Protocol type (`grpc`, `http/protobuf`, `http/json`)
- `telemetry_headers`: Authentication headers for OTLP endpoint
- `telemetry_attributes`: Resource attributes for service identification
- `telemetry_sampler`: Trace sampling strategy
- `telemetry_sample_ratio`: Sampling probability (0.0-1.0)

**AIM Integration**:
- `status_bridge`: Endpoint for AIM metrics bridge (e.g., `0.0.0.0:61000`)

### Job Specification

The `[scheduler.job]` section defines the training job:

**Model Configuration**:
```toml
[scheduler.job.model]
repository = "l45k/Resnet50"              # HuggingFace repository
revision = "main"                          # Optional branch/tag
filenames = ["config.json", "model.safetensors"]
token = "hf_..."                           # Optional auth token
type = "vision-classification"             # Model type
```

Supported model types:
- `vision-classification`: Image classification models (AutoModelForImageClassification)
- `causal-lm`: Causal language models (AutoModelForCausalLM)
- `torch`: Generic PyTorch models

**Preprocessor Configuration** (optional):
```toml
[scheduler.job.preprocessor]
repository = "l45k/Resnet50"
filenames = ["preprocessor_config.json"]
token = "hf_..."
```

**Dataset Configuration**:
```toml
[scheduler.job.dataset]
dataset = "imagenet"  # Name advertised by data nodes
```

**Inner Optimizer (AdamW)**:
```toml
[scheduler.job.inner_optimizer]
learning_rate = 0.001
# Optional: betas = [0.9, 0.999]
# Optional: epsilon = 1e-8
```

**Outer Optimizer (Nesterov Momentum)**:
```toml
[scheduler.job.outer_optimizer]
learning_rate = 0.7
momentum = 0.9
```

**Resource Requirements**:
```toml
[scheduler.job.resources]
num_workers = 4

worker = [
    { resource = { gpu = { min = 10.0 } } },
    { resource = { cpu = { min = 2.0 } } },
    { resource = { memory = { min = 8.0 } } },
]

parameter_server = [
    { resource = { cpu = { min = 4.0 } } },
    { resource = { memory = { min = 16.0 } } },
]
```

## Example Configuration

Complete example for image classification training:

```toml
# Network and Security
cert_pem = "/etc/hypha/scheduler-cert.pem"
key_pem = "/etc/hypha/scheduler-key.pem"
trust_pem = "/etc/hypha/ca-bundle.pem"

gateway_addresses = [
    "/ip4/203.0.113.10/tcp/4001",
    "/ip4/203.0.113.11/tcp/4001"
]

listen_addresses = [
    "/ip4/0.0.0.0/tcp/0",
]

external_addresses = [
    "/ip4/198.51.100.50/tcp/4001"
]

relay_circuit = true

# Telemetry
telemetry_endpoint = "http://otel-collector:4318"
telemetry_protocol = "http/protobuf"
telemetry_sampler = "traceidratio"
telemetry_sample_ratio = 0.1

# AIM Integration
status_bridge = "0.0.0.0:61000"

# Job Configuration
[scheduler.job]
type = "diloco"

[scheduler.job.model]
repository = "microsoft/resnet-50"
filenames = ["config.json", "model.safetensors"]
type = "vision-classification"

[scheduler.job.preprocessor]
repository = "microsoft/resnet-50"
filenames = ["preprocessor_config.json"]

[scheduler.job.dataset]
dataset = "imagenet-1k"

[scheduler.job.inner_optimizer]
learning_rate = 0.001

[scheduler.job.outer_optimizer]
learning_rate = 0.7
momentum = 0.9

[scheduler.job.resources]
num_workers = 8

worker = [
    { resource = { gpu = { min = 11.0 } } },
    { resource = { cpu = { min = 4.0 } } },
    { resource = { memory = { min = 16.0 } } },
]

parameter_server = [
    { resource = { cpu = { min = 8.0 } } },
    { resource = { memory = { min = 32.0 } } },
]
```

## CLI Reference

Full command-line reference will be available when [PR #118](https://github.com/hypha-space/hypha/pull/118) merges. For now, basic usage:

**Start Scheduler**:
```bash
hypha-scheduler --config /path/to/config.toml
```

**Common Options**:
- `--config <FILE>`: Path to configuration file (required)
- `--help`: Display help information
- `--version`: Show version information

**Environment Variables**:

Configuration can be overridden via environment variables:
```bash
HYPHA_CERT_PEM=/path/to/cert.pem \
HYPHA_KEY_PEM=/path/to/key.pem \
HYPHA_TRUST_PEM=/path/to/ca.pem \
hypha-scheduler --config config.toml
```

## AIM Integration

AIM (Aim Integration Module) provides visualization and tracking for training metrics.

### Configuring Status Bridge

The status bridge forwards metrics to AIM:

```toml
status_bridge = "0.0.0.0:61000"
```

This creates an HTTP endpoint on port 61000 that AIM can connect to.

### Metrics Reported

The scheduler forwards these metrics from workers:
- Loss values per batch
- Number of data points processed
- Batch processing times
- Synchronization events
- Worker utilization statistics

### Connecting to AIM Dashboard

Start AIM server:
```bash
aim server --host 0.0.0.0 --port 43800
```

Configure AIM to connect to scheduler's status bridge endpoint. Access dashboard at `http://localhost:43800`.

## OpenTelemetry (OTEL)

OpenTelemetry provides comprehensive observability through traces, metrics, and logs.

### OTLP Endpoint Configuration

```toml
telemetry_endpoint = "http://localhost:4318"
telemetry_protocol = "http/protobuf"
```

**Supported Protocols**:
- `grpc`: gRPC protocol (default port 4317)
- `http/protobuf`: HTTP with protobuf encoding (default port 4318)
- `http/json`: HTTP with JSON encoding (default port 4318)

### Authentication

For authenticated endpoints:

```toml
[telemetry_headers]
"Authorization" = "Bearer <token>"
"X-Custom-Header" = "value"
```

### Service Identification

```toml
[telemetry_attributes]
"service.name" = "hypha-scheduler"
"service.version" = "0.1.0"
"deployment.environment" = "production"
```

### Sampling Configuration

Control trace volume:

```toml
telemetry_sampler = "traceidratio"
telemetry_sample_ratio = 0.1  # Sample 10% of traces
```

**Available Samplers**:
- `always_on`: Sample all traces
- `always_off`: Sample no traces
- `traceidratio`: Sample percentage of traces
- `parentbased_traceidratio`: Respect parent span sampling decisions

### Data Exported

**Traces**:
- Worker allocation operations
- Lease renewal cycles
- DHT operations for peer discovery
- Gossipsub message propagation
- Job dispatch events

**Metrics**:
- Active worker count
- Lease renewal success/failure rates
- Data slice allocation statistics
- Network bandwidth usage
- Job completion times

**Logs**:
- Structured logs with trace context
- Worker state transitions
- Configuration validation results
- Error conditions and warnings

## Operational Notes

**Resource Discovery**: The scheduler discovers workers through Gossipsub topic subscriptions. Workers must subscribe to the same topic the scheduler publishes to.

**Lease Timeouts**: Default lease renewal interval is configurable per job. Adjust based on network latency and failure detection requirements.

**State Persistence**: Current implementation maintains state in memory. For production deployments with failover requirements, consider implementing state persistence (planned feature).

**Scaling**: A single scheduler can coordinate dozens of workers. For larger deployments, consider multiple independent schedulers managing separate job pools.

**Performance-Aware Scheduling**: The scheduler tracks average batch processing times per worker and adjusts synchronization timing to minimize wait time for stragglers.

## Troubleshooting

**Workers not responding to advertisements**:
- Verify workers are connected to same network (check gateway connectivity)
- Confirm Gossipsub topic configuration matches between scheduler and workers
- Check worker pricing thresholds aren't excluding scheduler's bid

**Lease renewals failing**:
- Check network connectivity between scheduler and workers
- Review lease timeout configuration
- Verify worker hasn't failed or restarted

**DiLoCo updates not triggering**:
- Confirm parameter server allocation succeeded
- Check scheduler logs for simulation errors
- Verify workers are reporting metrics correctly

**OTLP connection failures**:
- Test endpoint connectivity: `curl http://localhost:4318/v1/traces`
- Verify protocol configuration matches collector setup
- Check authentication headers if required

## Next Steps

- Configure [Workers](worker.md) to execute training tasks
- Set up [Data Nodes](data-node.md) to serve datasets
- Review [Training](training.md) documentation for DiLoCo details
- Deploy [Gateway](gateway.md) nodes for network stability
