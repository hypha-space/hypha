+++
title = "Worker Nodes"
description = "Task execution and resource management"
weight = 4
+++

# Worker Nodes

Worker nodes are the computational backbone of Hypha, executing machine learning tasks assigned by schedulers while managing local resources efficiently and securely.

## Overview

Worker nodes are responsible for:

- **Capability advertisement**: Publishing available resources
- **Task execution**: Running ML inference and training
- **Resource management**: Managing CPU, GPU, and memory
- **Progress reporting**: Updating task status
- **Result delivery**: Returning outputs to requesters

## Architecture

```
┌────────────────────────────────┐
│       Worker Node              │
│                                │
│  ┌──────────────────────────┐  │
│  │   Network Interface      │  │
│  └──────────┬───────────────┘  │
│             │                  │
│  ┌──────────▼───────────────┐  │
│  │   Task Manager           │  │
│  │  ┌────────────────────┐  │  │
│  │  │   Task Queue       │  │  │
│  │  └────────────────────┘  │  │
│  └──────────┬───────────────┘  │
│             │                  │
│  ┌──────────▼───────────────┐  │
│  │   Execution Engine       │  │
│  │  ┌────────────────────┐  │  │
│  │  │  Model Runtime     │  │  │
│  │  └────────────────────┘  │  │
│  │  ┌────────────────────┐  │  │
│  │  │  Resource Manager  │  │  │
│  │  └────────────────────┘  │  │
│  └──────────────────────────┘  │
│                                │
│  ┌──────────────────────────┐  │
│  │   Hardware               │  │
│  │   CPU │ GPU │ Memory     │  │
│  └──────────────────────────┘  │
└────────────────────────────────┘
```

## Capability Advertisement

### Publishing Capabilities

Workers advertise their computational resources:

```rust
pub struct WorkerCapabilities {
    // Hardware resources
    pub cpu_cores: u32,
    pub total_memory: u64,
    pub available_memory: u64,
    pub gpus: Vec<GpuInfo>,

    // Supported models
    pub models: Vec<String>,

    // Performance characteristics
    pub max_concurrent_tasks: u32,
    pub avg_inference_time: HashMap<String, Duration>,
}

pub struct GpuInfo {
    pub model: String,
    pub memory: u64,
    pub compute_capability: String,
}
```

### Registration

Workers register with the network:

```rust
// Publish worker record to DHT
let capabilities = detect_capabilities().await?;
let record = WorkerRecord {
    peer_id: self.peer_id,
    capabilities,
    last_updated: SystemTime::now(),
};

dht.put_record(
    format!("worker:{}", self.peer_id),
    record.to_bytes()
).await?;
```

### Periodic Updates

Capabilities are refreshed regularly:

```rust
// Update capabilities every 60 seconds
loop {
    let capabilities = detect_capabilities().await?;
    self.update_advertisement(capabilities).await?;
    tokio::time::sleep(Duration::from_secs(60)).await;
}
```

## Task Execution

### Task Reception

Worker receives tasks from scheduler:

```rust
// Handle incoming task assignment
async fn handle_task_assignment(&mut self, task: Task) -> Result<()> {
    // Validate task against capabilities
    if !self.can_execute(&task) {
        return Err(Error::InsufficientCapabilities);
    }

    // Add to queue
    self.task_queue.push(task).await?;

    // Send acknowledgment
    self.send_ack(task.id).await?;

    Ok(())
}
```

### Execution Pipeline

Tasks flow through execution stages:

1. **Validation**: Verify requirements
2. **Resource allocation**: Reserve CPU/GPU/memory
3. **Model loading**: Load ML model into memory
4. **Inference/Training**: Execute computation
5. **Result packaging**: Prepare output
6. **Cleanup**: Release resources

```rust
async fn execute_task(&mut self, task: Task) -> Result<TaskResult> {
    // Allocate resources
    let resources = self.allocate_resources(&task).await?;

    // Load model
    let model = self.model_cache
        .get_or_load(&task.model)
        .await?;

    // Execute
    let output = model.run(task.input).await?;

    // Package result
    let result = TaskResult {
        task_id: task.id,
        output,
        metrics: ExecutionMetrics {
            duration: start.elapsed(),
            memory_used: resources.memory_peak,
            gpu_utilization: resources.gpu_utilization,
        },
    };

    // Release resources
    resources.release().await?;

    Ok(result)
}
```

### Concurrent Execution

Workers can handle multiple tasks:

```rust
// Spawn task executor
let executor = TaskExecutor::new(max_concurrent);

for task in task_queue.recv() {
    executor.spawn(async move {
        match self.execute_task(task).await {
            Ok(result) => self.send_result(result).await,
            Err(e) => self.report_failure(task.id, e).await,
        }
    });
}
```

## Resource Management

### CPU Management

Track CPU utilization:

```rust
struct CpuManager {
    total_cores: u32,
    allocated_cores: AtomicU32,
}

impl CpuManager {
    async fn allocate(&self, cores: u32) -> Result<CpuAllocation> {
        let current = self.allocated_cores.load(Ordering::SeqCst);
        if current + cores > self.total_cores {
            return Err(Error::InsufficientCpu);
        }

        self.allocated_cores.fetch_add(cores, Ordering::SeqCst);
        Ok(CpuAllocation { cores })
    }
}
```

### GPU Management

Manage GPU resources:

```rust
struct GpuManager {
    devices: Vec<GpuDevice>,
}

impl GpuManager {
    async fn allocate(&self, requirements: &GpuRequirements) -> Result<GpuAllocation> {
        // Find suitable GPU
        let device = self.devices.iter()
            .find(|d| d.available_memory() >= requirements.memory)
            .ok_or(Error::InsufficientGpuMemory)?;

        // Reserve memory
        device.reserve_memory(requirements.memory).await?;

        Ok(GpuAllocation {
            device_id: device.id,
            memory: requirements.memory,
        })
    }
}
```

### Memory Management

Track and limit memory usage:

```rust
struct MemoryManager {
    total: u64,
    allocated: AtomicU64,
    limit: u64,
}

impl MemoryManager {
    async fn allocate(&self, size: u64) -> Result<MemoryAllocation> {
        let current = self.allocated.load(Ordering::SeqCst);
        if current + size > self.limit {
            return Err(Error::MemoryLimitExceeded);
        }

        self.allocated.fetch_add(size, Ordering::SeqCst);
        Ok(MemoryAllocation { size })
    }
}
```

## Model Management

### Model Caching

Cache frequently used models:

```rust
struct ModelCache {
    cache: LruCache<String, Arc<Model>>,
    loader: ModelLoader,
}

impl ModelCache {
    async fn get_or_load(&mut self, model_id: &str) -> Result<Arc<Model>> {
        if let Some(model) = self.cache.get(model_id) {
            return Ok(Arc::clone(model));
        }

        // Load model
        let model = self.loader.load(model_id).await?;
        let model = Arc::new(model);

        // Cache it
        self.cache.put(model_id.to_string(), Arc::clone(&model));

        Ok(model)
    }
}
```

### Model Storage

Store models locally or fetch on demand:

```toml
[worker.models]
# Local model directory
model_dir = "/var/lib/hypha/models"

# Model sources
[[worker.models.sources]]
type = "local"
path = "/var/lib/hypha/models"

[[worker.models.sources]]
type = "http"
url = "https://models.hypha.example.com"

[[worker.models.sources]]
type = "s3"
bucket = "hypha-models"
region = "us-east-1"
```

## Configuration

### Basic Worker Configuration

```toml
[worker]
# Maximum concurrent tasks
max_concurrent_tasks = 4

# Resource limits
max_memory_per_task = "8GB"
max_cpu_cores_per_task = 4

# Task timeout
task_timeout = 300  # seconds

# Model cache size
model_cache_size = "20GB"

[worker.network]
# Connection to gateway
gateway_addresses = [
    "/ip4/203.0.113.1/tcp/4001/p2p/12D3KooW..."
]

# Advertisement interval
advertisement_interval = 60  # seconds
```

### Advanced Configuration

```toml
[worker.execution]
# Execution environment
isolation_mode = "process"  # or "container", "vm"
sandbox_enabled = true

# Resource prioritization
cpu_priority = "normal"  # or "high", "low"
gpu_sharing = false

[worker.monitoring]
# Resource monitoring interval
monitor_interval = 5  # seconds

# Metrics collection
collect_metrics = true
metrics_port = 9091

[worker.security]
# Task validation
verify_task_signatures = true
allowed_schedulers = ["scheduler1-peer-id", "scheduler2-peer-id"]

# Resource limits
enforce_memory_limits = true
enforce_cpu_limits = true
```

## Monitoring

### Metrics

Worker metrics to monitor:

- **Active tasks**: Currently executing tasks
- **Task throughput**: Completed tasks/second
- **Resource utilization**: CPU/GPU/memory usage
- **Model cache hit rate**: Cache effectiveness
- **Task latency**: Average execution time

### Example Prometheus Metrics

```
# Task metrics
hypha_worker_tasks_active 2
hypha_worker_tasks_completed_total 523
hypha_worker_tasks_failed_total 5

# Resource metrics
hypha_worker_cpu_utilization 0.65
hypha_worker_memory_utilization 0.72
hypha_worker_gpu_utilization{device="0"} 0.85

# Model cache
hypha_worker_model_cache_hits_total 450
hypha_worker_model_cache_misses_total 73
```

## Security

### Task Isolation

Isolate task execution:

- **Process isolation**: Each task in separate process
- **Resource limits**: Enforce CPU/memory/network limits
- **Filesystem isolation**: Restricted file access
- **Network isolation**: Optional network sandboxing

### Input Validation

Validate all task inputs:

```rust
fn validate_task(&self, task: &Task) -> Result<()> {
    // Check signature
    if !task.verify_signature(&self.trusted_keys) {
        return Err(Error::InvalidSignature);
    }

    // Validate model ID
    if !self.supported_models.contains(&task.model) {
        return Err(Error::UnsupportedModel);
    }

    // Check resource requirements
    if task.requirements.memory > self.max_memory_per_task {
        return Err(Error::ExcessiveMemoryRequest);
    }

    Ok(())
}
```

## Next Steps

- [Scheduler Nodes](/architecture/scheduler/) - Task coordination
- [Network Layer](/architecture/network/) - P2P communication
- [Deployment Guide](/deployment/) - Production deployment
