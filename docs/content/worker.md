# Worker Node

Worker nodes execute training and inference tasks in the Hypha network. They evaluate task advertisements, bid for work, and run jobs through configurable executors. This section covers worker deployment, configuration, and executor setup.

## Role and Responsibilities

Workers implement several subsystems to participate in the distributed training workflow:

**Task Subscription**: Workers subscribe to the `hypha/worker` Gossipsub topic to receive task advertisements from schedulers. Each advertisement specifies resource requirements, executor types, and pricing.

**Request Evaluation**: The RequestEvaluator scores incoming advertisements against worker capabilities and pricing thresholds. Only requests matching resource availability and price expectations receive offers.

**Offer Submission**: Qualified requests trigger WorkerOffer messages sent directly to schedulers. Offers specify the worker's counter-price and include a temporary lease timeout.

**Lease Management**: The LeaseManager handles time-bounded resource reservations. Temporary leases prevent double-booking during offer negotiation. Accepted offers transition to renewable leases maintained through periodic scheduler renewal.

**Job Execution**: The JobManager spawns executor processes, provides isolated work directories, and manages job lifecycles. Each job receives a Unix socket for communication via the job bridge.

**Data Fetching**: Workers request data slices from schedulers, retrieve them from data nodes via stream protocols, and prefetch to minimize wait times.

**Metric Reporting**: Workers report training metrics (loss, batch processing times, data points processed) to schedulers for performance-aware scheduling decisions.

**Gradient Communication**: For DiLoCo training, workers send pseudo-gradients to parameter servers at synchronization points and receive updated model weights.

## Installation and Setup

Install the worker binary following the [Installation](installation.md) guide.

Quick reference:

```bash
# From pre-built binary
curl -fsSL https://github.com/hypha-space/hypha/releases/latest/download/install.sh | sh

# Or build from source
cargo build --release -p hypha-worker
```

### Work Directory Setup

Workers require a base directory for job isolation:

```bash
# Create work directory with appropriate permissions
sudo mkdir -p /var/hypha/work
sudo chown $(whoami):$(whoami) /var/hypha/work
chmod 750 /var/hypha/work
```

Each job creates a subdirectory `hypha-{uuid}` under the base path. Jobs have full access to their subdirectories but cannot access parent directories or other jobs' data.

## Configuration Parameters

Worker configuration uses TOML format with network, resource, and executor sections.

### Network and Security

**TLS Certificates** (required for mTLS):
```toml
cert_pem = "/etc/hypha/worker-cert.pem"
key_pem = "/etc/hypha/worker-key.pem"
trust_pem = "/etc/hypha/ca-bundle.pem"
crls_pem = "/etc/hypha/crls.pem"  # Optional
```

**Network Addresses**:
```toml
gateway_addresses = [
    "/ip4/203.0.113.50/tcp/4001",
    "/ip4/198.51.100.25/tcp/4001"
]

listen_addresses = [
    "/ip4/0.0.0.0/tcp/0",
]

external_addresses = []  # Optional for workers

exclude_cidr = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16"
]

relay_circuit = true
```

### Resource Advertisement

Specify available resources in GB (memory, storage, GPU) or cores (CPU):

```toml
[resources]
cpu = 16        # cores
memory = 64     # GB
storage = 500   # GB
gpu = 24        # GB (video memory)
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

```toml
telemetry_endpoint = "http://otel-collector:4318"
telemetry_protocol = "http/protobuf"
telemetry_sampler = "traceidratio"
telemetry_sample_ratio = 0.1

[telemetry_attributes]
"service.name" = "hypha-worker"
"worker.hostname" = "worker-gpu-01"
"worker.region" = "us-west-2"
```

## Complete Example Configuration

```toml
# TLS Configuration
cert_pem = "/etc/hypha/certs/worker-cert.pem"
key_pem = "/etc/hypha/certs/worker-key.pem"
trust_pem = "/etc/hypha/certs/ca-bundle.pem"

# Network
gateway_addresses = [
    "/ip4/203.0.113.50/tcp/4001",
    "/ip4/198.51.100.25/tcp/4001"
]

listen_addresses = [
    "/ip4/0.0.0.0/tcp/0"
]

relay_circuit = true

# Resources
[resources]
cpu = 32
memory = 128
storage = 1000
gpu = 48  # 2x A6000 GPUs

# Work Directory
work_dir = "/var/hypha/work"

# Executors
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
    "--extra", "cu128",
    "--",
    "accelerate",
    "launch",
    "--config_file", "/etc/hypha/accelerate-2gpu.yaml",
    "-m", "hypha.accelerate_executor.training",
    "--socket", "{SOCKET_PATH}",
    "--work-dir", "{WORK_DIR}",
    "--job", "{JOB_JSON}",
]

[[executors]]
class = "aggregate"
name = "parameter-server"
runtime = "parameter-server"

# Telemetry
telemetry_endpoint = "http://otel-collector:4318"
telemetry_protocol = "http/protobuf"

[telemetry_attributes]
"service.name" = "hypha-worker"
"worker.instance" = "worker-gpu-01"
```

## CLI Reference

Full command-line reference will be available when [PR #118](https://github.com/hypha-space/hypha/pull/118) merges.

**Basic Usage**:
```bash
hypha-worker --config /path/to/config.toml
```

**Common Options**:
- `--config <FILE>`: Path to configuration file (required)
- `--help`: Display help information
- `--version`: Show version information

## Executors

Executors handle actual computation. Workers support multiple executor types simultaneously.

### Executor Types

**Process Executor**: Runs training or inference as a subprocess. The worker spawns the configured command with template-substituted arguments. Process executors communicate via the job bridge HTTP API.

**Aggregate Executor** (Parameter Server): Built-in implementation for DiLoCo parameter server functionality. Receives pseudo-gradients from workers, averages them, applies Nesterov momentum, and broadcasts updated weights.

### Accelerate Executor

The Accelerate executor enables multi-GPU training through HuggingFace Accelerate, which provides tensor parallelism across multiple devices.

#### Purpose

Without tensor parallelism, each GPU must hold the entire model in memory. Accelerate distributes model layers across devices, enabling training of models larger than a single GPU's capacity. A worker with 4 GPUs can train models 4x larger than possible on a single GPU.

#### Installation

The Accelerate executor is distributed as a Python wheel. It uses **uv** for dependency management and execution.

**Installing uv**:
```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

**Testing Installation**:
```bash
uv run --python 3.12 \
    --with 'https://github.com/hypha-space/hypha/releases/download/v0.1.0/hypha_accelerate_executor-0.1.0-py3-none-any.whl' \
    -- python -c "import hypha.accelerate_executor; print('Success')"
```

#### Configuration

Add the Accelerate executor to your worker configuration:

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
    "--extra", "cu128",  # PyTorch variant (optional)
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

**Parameters**:

- `--python "3.12"`: Python version to use
- `--with "<wheel-url>"`: Accelerate executor wheel URL (replace `<version>` with actual release version)
- `--extra "<variant>"` (optional): PyTorch build variant
- `--config_file "<path>"`: Accelerate configuration file matching your hardware
- Module path `hypha.accelerate_executor.training`: Standard entrypoint (do not change)
- Template variables `{SOCKET_PATH}`, `{WORK_DIR}`, `{JOB_JSON}`: Automatically substituted by worker

#### PyTorch Variant Selection

Choose the PyTorch build matching your hardware via `--extra`:

- `mps_cu128` (default): Apple Silicon (MPS) + CUDA 12.8
- `cpu`: CPU-only environments
- `cu126`: NVIDIA GPUs with CUDA 12.6
- `cu129`: NVIDIA GPUs with CUDA 12.9
- `rocm64`: AMD GPUs with ROCm 6.4

Omit `--extra` to use the default (`mps_cu128`).

#### Accelerate Configuration File

Accelerate requires a configuration file describing the distributed training setup. For single-GPU setups (including Apple Silicon with MPS), this minimal configuration works:

```yaml
# accelerate.yaml
compute_environment: LOCAL_MACHINE
distributed_type: "NO"
mixed_precision: fp16
use_cpu: false
```

For multi-GPU setups, generate a configuration:

```bash
accelerate config
```

Refer to the [Accelerate CLI documentation](https://huggingface.co/docs/accelerate/en/package_reference/cli#accelerate-config) for details.

#### Troubleshooting

**"Module not found" errors**: Verify the wheel URL matches an actual release at https://github.com/hypha-space/hypha/releases.

**Training doesn't start**: Check that `--config_file` path is correct and the Accelerate configuration matches your hardware (correct number of GPUs, appropriate mixed precision settings).

**uv command not found**: Ensure uv is installed and available in `PATH`. Restart your shell after installation.

**PyTorch device mismatch**: Ensure `--extra` variant matches your hardware. For NVIDIA GPUs, check CUDA version with `nvidia-smi`. For AMD, verify ROCm version. For Apple Silicon, use `mps_cu128` (despite the name, it includes MPS support).

## Resource Handling

Workers implement market-based resource allocation:

### Arbiter Component

The Arbiter subscribes to task advertisements and manages the offer process:

1. Buffers incoming advertisements within time windows
2. Evaluates all requests in a batch for fair consideration
3. Sends offers for requests meeting resource and price requirements
4. Manages lease lifecycle for accepted offers

### RequestEvaluator

Scores and ranks competing requests when multiple schedulers bid simultaneously. Configurable strategies include:

- **Profit Maximization**: Prioritize highest bid prices
- **Utilization Optimization**: Favor requests that efficiently fill resource gaps
- **Affinity-Based**: Prefer certain scheduler types or workload characteristics

The current implementation uses simple price-based selection.

### LeaseManager

Implements time-bounded resource reservations:

**Temporary Leases**: When a worker sends an offer, it creates a short-lived lease (typically seconds) preventing duplicate offers for the same resources.

**Renewable Leases**: Accepted offers transition to renewable leases maintained through periodic scheduler renewal. Leases expire if schedulers fail to renew, automatically releasing resources.

**Cleanup**: Lease expiration triggers resource release without requiring explicit failure messages.

### JobManager

Executes confirmed jobs with isolation:

**Process Spawning**: JobManager spawns executor processes with configured commands and arguments.

**Work Directory Creation**: Each job receives an isolated subdirectory with 0700 permissions.

**Socket Creation**: Unix domain sockets (0600 permissions) enable job bridge communication.

**Lifecycle Management**: JobManager monitors process status, handles termination, and cleans up resources.

**Limitation**: Current implementation does not provide isolation between jobs on the same worker. A malicious job could potentially access other jobs' data. Future versions will address this through proper job scoping.

## Job Bridge Interface

The job bridge provides a language-agnostic HTTP API over Unix sockets, enabling executors to interact with the Hypha network without understanding P2P protocols.

### API Overview

The bridge exposes REST endpoints for resource operations. All responses return filesystem paths and metadata rather than raw data, maintaining separation between control and data planes.

### Resource Fetching

Fetch resources from HuggingFace, HTTP sources, or peer nodes:

**Request**:
```http
POST /resources/fetch
Content-Type: application/json

{
  "fetch": {
    "type": "HuggingFace",
    "repository": "microsoft/resnet-50",
    "revision": "main",
    "filenames": ["model.safetensors", "config.json"]
  },
  "out_dir": "models"
}
```

**Response**:
```json
[
  {
    "path": "models/model.safetensors",
    "size": 16534288384,
    "checksum": "sha256:abc123...",
    "metadata": {
      "source": "huggingface",
      "timestamp": "2025-01-15T10:30:00Z"
    }
  }
]
```

Files are downloaded to `{WORK_DIR}/{out_dir}`.

### Sending Resources to Peers

Send files to specific peers:

**Request**:
```http
POST /resources/send
Content-Type: application/json

{
  "send": {
    "type": "Peers",
    "peers": ["12D3KooWA...", "QmX7d..."],
    "strategy": "All"
  },
  "path": "output/gradients.safetensors"
}
```

**Strategies**:
- `All`: Send to all specified peers
- `One`: Send to any one peer
- `Random`: Send to a randomly selected peer

### Receiving Resources (Streaming)

Receive streaming data from peers:

**Request**:
```http
POST /resources/receive
Content-Type: application/json

{
  "receive": {
    "type": "Peers",
    "peers": [],  # Empty accepts from any peer
    "strategy": "All"
  },
  "out_dir": "incoming"
}
```

**Server-Sent Event Stream**:
```
data: {"path": "incoming/chunk-0.bin", "size": 1048576, "from_peer": "12D3Ko...", "sequence": 0}
data: {"path": "incoming/chunk-1.bin", "size": 1048576, "from_peer": "12D3Ko...", "sequence": 1}
```

### Security Model

The bridge enforces strict security policies:

**Path Validation**:
- No absolute paths allowed
- Parent directory traversal blocked (`../` rejected)
- All paths confined to job work directory

**Process Isolation**:
- Each job runs as a separate process
- Unix sockets and files use 0600 permissions
- Socket only accessible to spawning user

**Limitations**:
- No isolation between jobs on same worker
- Executors are trusted (configured by worker owner)
- Malicious schedulers/peers cannot escape work directory but may send invalid data

## OpenTelemetry Integration

Workers export comprehensive telemetry:

### Worker-Specific Metrics

**Job Execution**:
- Active job count
- Job start/completion rates
- Job execution duration
- Executor process status

**Resource Utilization**:
- CPU usage per job
- Memory consumption
- GPU utilization
- Storage I/O rates

**Lease Management**:
- Active lease count
- Lease renewal success/failure
- Lease expiration events

**Data Operations**:
- Data slice fetch rates
- Bytes transferred
- Prefetch queue depth

### Trace Context Propagation

Workers propagate trace context across network operations:

- Task advertisement evaluation traces
- Offer submission traces
- Job execution spans
- Data fetch operations
- Gradient send/receive operations

### Configuration

```toml
telemetry_endpoint = "http://otel-collector:4318"
telemetry_protocol = "http/protobuf"
telemetry_sampler = "parentbased_traceidratio"
telemetry_sample_ratio = 0.1

[telemetry_attributes]
"service.name" = "hypha-worker"
"worker.instance" = "worker-gpu-01"
"worker.region" = "us-west-2"
"worker.gpu.model" = "NVIDIA-A6000"
```

## Troubleshooting

**Worker not receiving task advertisements**:
- Verify connection to gateways (check logs for bootstrap success)
- Confirm Gossipsub topic subscription
- Check network connectivity to other peers

**Offers rejected by scheduler**:
- Review pricing threshold configuration
- Verify resource advertisement matches job requirements
- Check executor class/name matches scheduler's request

**Jobs failing to start**:
- Verify executor command is in PATH
- Check work directory permissions
- Review executor process logs
- Ensure template substitutions are correct

**Job bridge connection failures**:
- Verify socket path is within work directory
- Check socket file permissions (should be 0600)
- Ensure executor has access to work directory

**Resource fetch failures**:
- Test network connectivity to fetch sources
- Verify HuggingFace tokens if fetching private models
- Check disk space in work directory

## Next Steps

With workers deployed:

1. Configure [Schedulers](scheduler.md) to coordinate training jobs
2. Set up [Data Nodes](data-node.md) to serve datasets
3. Review [Training](training.md) for DiLoCo workflow details
4. Consult [Security](security.md) for production hardening
