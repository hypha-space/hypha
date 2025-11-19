# Scheduler

The scheduler orchestrates distributed training jobs by coordinating workers, managing resource allocation, and controlling DiLoCo synchronization. This section covers scheduler configuration, deployment, and operational details.

**CLI Reference**: See the [hypha-scheduler CLI Reference](../reference/hypha-scheduler-cli.md) for complete command-line documentation.

## Role and Responsibilities

The scheduler serves as the coordination point for distributed training:

**Task Advertisement**: Publishing resource requirements to workers via Gossipsub. Workers subscribe to the `hypha/worker` topic and evaluate incoming advertisements against their capabilities.

**Worker Selection**: Collecting offers from workers and selecting optimal matches. The scheduler evaluates offers based on a configurable price range and picks the best price/performance ratio rather than simply the lowest absolute bid.

**Lease Management**: Maintaining resource reservations through periodic renewal. Leases prevent resource double-booking while ensuring automatic cleanup if schedulers fail.

**DiLoCo Synchronization Coordination**: Tracking worker progress and determining when to trigger weight synchronization. The scheduler uses performance-aware scheduling to minimize stragglers by simulating future completion times.

**Data Distribution Tracking**: Assigning dataset slices to workers and ensuring complete coverage. The scheduler tracks slice states (AVAILABLE → ASSIGNED → USED) and reassigns slices when epochs complete.

**Performance-Aware Scheduling**: Allocating heterogeneous batch sizes based on worker capabilities. Faster workers process more data per synchronization round, improving overall efficiency.

**Parameter Server Lifecycle Management**: Coordinating parameter server startup, informing it which workers will participate, and setting collection timeouts.

**Metrics Aggregation**: Optionally forwarding training metrics to AIM for visualization and analysis.


## Installation

Install the data node binary following the [Installation](installation.md) guide.

## Configuration Parameters

Scheduler configuration uses TOML format with network, security, and job specification settings. Generate an example configuration file using the [`hypha-scheduler init`](../reference/hypha-scheduler-cli.md#hypha-scheduler-init) command. You will need to provide paths to TLS certificates, configure network settings (gateway addresses, listen addresses), and define your training job specification.

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

**AIM Integration**:
- `status_bridge`: Endpoint for AIM metrics bridge (e.g., `0.0.0.0:61000`)

### Job Specification

The `[scheduler.job]` section defines the training job:

**Model Configuration**:
```toml
[scheduler.job.model]
repository = "owner/repo"                  # HuggingFace repository
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

[[scheduler.job.resources.worker]]
type = "GPU"
min = 10.0

[[scheduler.job.resources.worker]]
type = "CPU"
min = 2.0

[[scheduler.job.resources.worker]]
type = "Memory"
min = 8.0

[[scheduler.job.resources.worker]]
kind = "diloco-transformer"   # Require workers that advertise this executor

[[scheduler.job.resources.parameter_server]]
type = "CPU"
min = 4.0

[[scheduler.job.resources.parameter_server]]
type = "Memory"
min = 16.0

[[scheduler.job.resources.parameter_server]]
kind = "parameter-server"
```
Each `[[...worker]]` or `[[...parameter_server]]` table serializes directly into a woreker`Requirement`.

> [!NOTE]
> Make sure that the GPU requirements for the worker match the required GPU memory for training with a batch size of 1 on a GPU. A good estimat in GB is #number of parameters * 24 * 9e-10. This is equal to the model memory (1 fp32) + AdamW state (4 fp32) + gradient (1 fp32) and activation (1 fp32). For a better estimation consult [Transformer math](https://blog.eleuther.ai/transformer-math/). The number of paramters can be easily computed in `torch` with `sum(v.shape.numel() for v in model.state_dict().values())`.

**Price Ranges**: Configure bid/maximum pairs for workers and parameter servers to express how far the scheduler is willing to counter-offer without revealing the cap to workers:

```toml
[scheduler.job.resources.worker_price]
bid = 110.0   # published bid workers see
max = 160.0   # private cap used for filtering

[scheduler.job.resources.parameter_server_price]
bid = 80.0
max = 120.0
```

The bid is broadcast to workers, while the max remains local to the scheduler. Received offers whose price exceeds the configured `max` are ignored. Within that range, offers are ranked by the scheduler's resource evaluator to prioritize the most capacity per token spent.

### OpenTelemetry

OpenTelemetry enables distributed tracing and metrics collection for debugging and monitoring your scheduler in production. Configure telemetry either via the TOML configuration file or using standard `OTEL_*` environment variables.

**Configuration File (Example uses Grafana Cloud)**: Specify telemetry settings in your `scheduler.toml`:

```toml
telemetry_attributes = "service.name=<Node Name>,service.namespace=<Namespace>,deployment.environment=<Environment>"
telemetry_endpoint = "https://otlp-gateway-prod-eu-west-2.grafana.net/otlp"
telemetry_headers = "Authorization=Basic <Api Key>"
telemetry_protocol = "http/protobuf"
telemetry_sampler = "parentbased_traceidratio"
telemetry_sample_ratio = 0.1
```

**Environment Variables (Example uses Grafana Cloud)**: Alternatively, use standard OpenTelemetry environment variables:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://otlp-gateway-prod-eu-west-2.grafana.net/otlp"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Basic <Api Key>"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_SERVICE_NAME="scheduler-01"
export OTEL_RESOURCE_ATTRIBUTES="service.namespace=production,deployment.environment=prod"

hypha-scheduler run --config /etc/hypha/scheduler.toml
```

Environment variables take precedence over configuration file settings, allowing flexible per-deployment customization.


## AIM Integration

AIM (Aim Integration Module) provides visualization and tracking for training metrics.

### Configuring Status Bridge

Set `status_bridge` to the HTTP endpoint that should receive worker metrics:

```toml
status_bridge = "aim-bridge.internal:61000"
```

The scheduler **pushes** JSON payloads (`POST http://<status_bridge>/status`) using the configured address. Point this at the provided AIM bridge (`drivers/aim-driver`) or any service that can accept the `AimMetrics` schema. The scheduler itself does not start an HTTP listener.


### Connecting to AIM Dashboard

1. Start an AIM server (`aim server --host 0.0.0.0 --port 43800`).
2. Run the AIM bridge so it listens on `status_bridge` and forwards incoming metrics to AIM.
3. Visit `http://localhost:43800` to explore runs (loss curves, throughput, worker utilization, etc.).
