# Worker Node

Worker nodes execute training and inference tasks in the Hypha network. They evaluate task advertisements, bid for work, and run jobs through configurable executors. This section covers worker deployment, configuration, and executor setup.

**CLI Reference**: See the [hypha-worker CLI Reference](../reference/hypha-worker-cli.md) for complete command-line documentation.

## Role and Responsibilities

Workers implement several subsystems to participate in the distributed training workflow:

**Request Management**: Subscribes to advertisements on the `hypha/worker` topic and evaluates incoming requests against configurable criteria. The arbiter maintains a pricing threshold and only responds to requests meeting resource requirements and price expectations. When multiple schedulers compete for resources simultaneously, the evaluation system scores and ranks requests using configurable strategies — prioritizing profit maximization. See the [Decentralized Resource Allocation Protocol RFC](../rfc/2025-08-04_decentralized_resource_allocation_protocol.md) for details on the negotiation mechanism.

**Lease Management**: The LeaseManager handles time-bounded resource reservations. Temporary leases prevent double-booking during offer negotiation. Accepted offers transition to renewable leases maintained through periodic scheduler renewal.

**Job Execution**: The JobManager spawns executor processes, provides isolated work directories, and manages job lifecycles. Each job receives a Unix socket for communication via the job bridge.

**Data Fetching**: Workers request data slices from schedulers, retrieve them from data nodes via stream protocols, and prefetch to minimize wait times.

**Metric Reporting**: Workers report training metrics (loss, batch processing times, data points processed) to schedulers for performance-aware scheduling decisions.

**Gradient Communication**: For DiLoCo training, workers send pseudo-gradients to parameter servers at synchronization points and receive updated model weights.

## Installation and Setup

Install the data node binary following the [Installation](installation.md) guide.

### Work Directory Setup

Workers require a base directory for job isolation:

```bash
# Create work directory with appropriate permissions
sudo mkdir -p /var/hypha/work
sudo chown $(whoami):$(whoami) /var/hypha/work
chmod 750 /var/hypha/work
```

Each job creates a subdirectory `hypha-{uuid}` under the base path. Jobs have full access to their subdirectories.

## Configuration Parameters

Data node configuration uses TOML format with network, security, and storage settings. Generate an example configuration file using the [`hypha-worker init`](../reference/hypha-wowker-cli.md#hypha-data-init) command. You will need to provide paths to TLS certificates and configure basic network settings (gateway addresses, listen addresses).

### Resource Advertisement

Specify available resources in GB (memory, storage, GPU) or cores (CPU):

```toml
[resources]
cpu = 16        # cores
memory = 64     # GB
storage = 500   # GB
gpu = 24        # GB (vram)
```

These values advertise capacity to schedulers. Actual resource limiting is planned but not yet implemented.

### Work Directory

```toml
work_dir = "/var/hypha/work"
```

Executors receive isolated subdirectories under this path.

### Executor Configuration

Executors define how workers handle jobs. Each executor specifies:

- **class**: Job category (e.g., "train", "inference")
- **name**: Specific executor variant (e.g., "diloco-transformer")
- **runtime**: Execution mechanism ("process" or "parameter-server")
- **cmd** (process only): Command to execute
- **args** (process only): Arguments, with template substitutions

**Example: Accelerate Executor for Training**

```toml
[[executors]]
class = "train"
name = "diloco-transformer"
runtime = "process"
cmd = "uv"
args = [
    "run",
    "--python", "3.12",
    "--no-project",
    "--with", "https://github.com/hypha-space/hypha/releases/download/v0.1.0/hypha_accelerate_executor-0.1.0-py3-none-any.whl",
    "--",
    "accelerate",
    "launch",
    "--config_file", "/etc/hypha/accelerate.yaml",
    "-m", "hypha.accelerate_executor.training",
    "--socket", "{SOCKET_PATH}",
    "--work-dir", "{WORK_DIR}",
    "--job", "{JOB_JSON}",
]
```

**Example: Parameter Server Executor**

```toml
[[executors]]
class = "aggregate"
name = "parameter-server"
runtime = "parameter-server"
```

**Template Substitutions**:
- `{SOCKET_PATH}`: Unix socket path for job bridge communication
- `{WORK_DIR}`: Job-specific work directory
- `{JOB_JSON}`: Job specification as JSON string

### OpenTelemetry

OpenTelemetry enables distributed tracing and metrics collection for debugging and monitoring your workers in production. Configure telemetry either via the TOML configuration file or using standard `OTEL_*` environment variables.

**Configuration File (Example uses Grafana Cloud)**: Specify telemetry settings in your `worker.toml`:

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
export OTEL_SERVICE_NAME="worker-01"
export OTEL_RESOURCE_ATTRIBUTES="service.namespace=production,deployment.environment=prod"

hypha-worker run --config /etc/hypha/worker.toml
```

Environment variables take precedence over configuration file settings, allowing flexible per-deployment customization.

### Environment Overrides

Workers load configuration in layers: TOML file → `HYPHA_*` env vars → CLI flags. Example:

```bash
HYPHA_CERT_PEM=/etc/hypha/certs/worker-cert.pem \
HYPHA_KEY_PEM=/etc/hypha/certs/worker-key.pem \
HYPHA_WORK_DIR=/var/hypha/work \
hypha-worker run --config /etc/hypha/worker.toml
```

Use this pattern to inject secrets or per-host tweaks (e.g., different GPUs or work directories) while sharing a single committed config.


## Executors

Executors handle actual computation. Workers support multiple executor types simultaneously.

### Accelerate Executor

The Accelerate executor enables multi-GPU training through HuggingFace Accelerate, which provides tensor parallelism across multiple devices. Without tensor parallelism, each GPU must hold the entire model in memory. Accelerate distributes model layers across devices, enabling training of models larger than a single GPU's capacity. A worker with 4 GPUs can train models 4x larger than possible on a single GPU.

The executor is distributed as a Python wheel and uses **uv** for dependency management and execution.

For detailed installation instructions, configuration examples, PyTorch variant selection, and troubleshooting, see the [Accelerate Executor README](../../executors/accelerate/README.md).

### Parameter Server Executor

The Parameter Server executor provides built-in support for DiLoCo (Distributed Low-Communication) based training, enabling workers to act as parameter servers that aggregate model updates across distributed training runs.

#### Configuration

The parameter server executor is built into Hypha workers—no additional installation required. Add it to your worker configuration:

```toml
[[executors]]
class = "aggregate"
name = "parameter-server"
runtime = "parameter-server"
```
