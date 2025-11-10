+++
title = "Scheduler Nodes"
description = "Task coordination and workload distribution"
weight = 3
+++

# Scheduler Nodes

Scheduler nodes coordinate the distribution of machine learning tasks across available worker nodes, matching workloads to worker capabilities while optimizing for performance and resource utilization.

## Overview

The scheduler is responsible for:

- **Worker discovery**: Finding available workers via the DHT
- **Capability matching**: Pairing tasks with suitable workers
- **Load balancing**: Distributing work efficiently
- **Task monitoring**: Tracking execution progress
- **Fault handling**: Detecting and recovering from failures

## Architecture

```
      ┌─────────────────┐
      │   Client API    │
      └────────┬────────┘
               │
      ┌────────▼────────┐
      │   Scheduler     │
      │  ┌───────────┐  │
      │  │  Task     │  │
      │  │  Queue    │  │
      │  └───────────┘  │
      │  ┌───────────┐  │
      │  │  Worker   │  │
      │  │ Discovery │  │
      │  └───────────┘  │
      │  ┌───────────┐  │
      │  │ Matching  │  │
      │  │  Engine   │  │
      │  └───────────┘  │
      └────────┬────────┘
               │
    ┌──────────┼──────────┐
    │          │          │
┌───▼───┐  ┌──▼───┐  ┌───▼───┐
│Worker │  │Worker│  │Worker │
│   1   │  │  2   │  │   3   │
└───────┘  └──────┘  └───────┘
```

## Task Lifecycle

### 1. Task Submission

Clients submit tasks to the scheduler:

```rust
// Example task submission
let task = Task {
    id: TaskId::new(),
    model: "llama-2-7b",
    input: TaskInput::Text(prompt),
    requirements: Requirements {
        min_memory: 16_000_000_000,  // 16 GB
        gpu_required: true,
        cpu_cores: 4,
    },
};

scheduler.submit_task(task).await?;
```

### 2. Worker Discovery

Scheduler queries the DHT for workers:

1. Query for workers with required capabilities
2. Receive worker advertisements
3. Verify worker availability and health
4. Build candidate list

### 3. Worker Selection

Selection algorithm considers:

- **Capability match**: Worker meets task requirements
- **Current load**: Worker's active task count
- **Historical performance**: Past task completion metrics
- **Network latency**: RTT to worker
- **Resource availability**: Current resource utilization

### 4. Task Assignment

Once a worker is selected:

1. Send task assignment message
2. Wait for acknowledgment
3. Monitor task execution
4. Handle progress updates

### 5. Result Collection

When task completes:

1. Receive result from worker
2. Validate result integrity
3. Return result to client
4. Update worker metrics

## Scheduling Algorithms

### Basic Round-Robin

Simple load distribution:

```rust
fn select_worker(&self, workers: &[Worker]) -> Worker {
    let index = self.counter.fetch_add(1) % workers.len();
    workers[index].clone()
}
```

### Capability-Based

Match task requirements to worker capabilities:

```rust
fn select_worker(&self, task: &Task, workers: &[Worker]) -> Option<Worker> {
    workers.iter()
        .filter(|w| w.meets_requirements(&task.requirements))
        .min_by_key(|w| w.current_load())
        .cloned()
}
```

### Performance-Aware

Consider historical performance:

```rust
fn select_worker(&self, task: &Task, workers: &[Worker]) -> Option<Worker> {
    workers.iter()
        .filter(|w| w.meets_requirements(&task.requirements))
        .max_by_key(|w| w.performance_score())
        .cloned()
}
```

## Worker Discovery

### DHT-Based Discovery

Workers advertise capabilities via DHT:

```rust
// Worker publishes capabilities
let record = WorkerRecord {
    peer_id: self.peer_id,
    capabilities: Capabilities {
        models: vec!["llama-2-7b", "stable-diffusion-xl"],
        memory: 32_000_000_000,
        gpus: vec![GpuInfo { model: "A100", memory: 40_000_000_000 }],
    },
};

dht.put_record(record_key, record.to_bytes()).await?;
```

Scheduler queries for workers:

```rust
// Scheduler discovers workers
let query = CapabilityQuery {
    min_memory: 16_000_000_000,
    gpu_required: true,
};

let workers = dht.get_records_matching(query).await?;
```

### Heartbeat Monitoring

Continuous health checking:

```rust
// Periodic heartbeat to workers
loop {
    for worker in &self.workers {
        if let Err(e) = self.ping_worker(worker).await {
            // Mark worker as unavailable
            self.mark_unavailable(worker);
        }
    }
    tokio::time::sleep(Duration::from_secs(30)).await;
}
```

## Fault Tolerance

### Task Reassignment

When a worker fails:

1. Detect failure (timeout or error)
2. Mark task as failed
3. Select new worker
4. Reassign task
5. Notify client of delay

### Worker Health Tracking

Track worker reliability:

```rust
struct WorkerHealth {
    success_rate: f64,
    avg_completion_time: Duration,
    last_seen: Instant,
    consecutive_failures: u32,
}
```

### Retry Policies

Configurable retry behavior:

```toml
[scheduler.retry]
max_attempts = 3
backoff_base = 2  # seconds
backoff_max = 60  # seconds
retry_on_timeout = true
retry_on_worker_failure = true
```

## Configuration

### Basic Scheduler Configuration

```toml
[scheduler]
# Maximum concurrent task assignments
max_concurrent_tasks = 100

# Worker selection algorithm
selection_algorithm = "performance-aware"  # or "round-robin", "capability-based"

# Task timeout (seconds)
task_timeout = 300

# Worker discovery interval (seconds)
discovery_interval = 60

[scheduler.api]
# REST API configuration
bind_address = "0.0.0.0:8080"
max_request_size = "10MB"
```

### Advanced Configuration

```toml
[scheduler.load_balancing]
# Load balancing strategy
strategy = "least-loaded"  # or "random", "performance"

# Load calculation method
load_metric = "task-count"  # or "cpu-usage", "memory-usage"

# Worker selection preferences
prefer_local_workers = true
max_network_latency = 100  # milliseconds

[scheduler.fault_tolerance]
# Health check configuration
health_check_interval = 30  # seconds
health_check_timeout = 5    # seconds
max_consecutive_failures = 3

# Task retry configuration
enable_retries = true
max_retries = 3
retry_backoff = "exponential"
```

## API Reference

### REST API

#### Submit Task

```http
POST /api/v1/tasks
Content-Type: application/json

{
  "model": "llama-2-7b",
  "input": "Explain quantum computing",
  "requirements": {
    "min_memory": 16000000000,
    "gpu_required": true
  }
}
```

#### Get Task Status

```http
GET /api/v1/tasks/{task_id}
```

#### Cancel Task

```http
DELETE /api/v1/tasks/{task_id}
```

#### List Workers

```http
GET /api/v1/workers
```

## Monitoring

### Metrics

Key scheduler metrics:

- **Task queue depth**: Pending tasks
- **Task completion rate**: Tasks/second
- **Average task latency**: Queue to completion
- **Worker pool size**: Available workers
- **Task success rate**: Successful vs. failed

### Example Prometheus Metrics

```
# Task metrics
hypha_scheduler_tasks_queued 15
hypha_scheduler_tasks_completed_total 1523
hypha_scheduler_tasks_failed_total 12

# Worker metrics
hypha_scheduler_workers_available 8
hypha_scheduler_workers_busy 3
hypha_scheduler_worker_selection_duration_seconds 0.002
```

## Next Steps

- [Worker Nodes](/architecture/worker/) - Task execution
- [Network Layer](/architecture/network/) - P2P communication
- [Deployment Guide](/deployment/) - Production deployment
