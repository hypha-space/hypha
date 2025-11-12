# Training

Hypha implements DiLoCo (Distributed Low-Communication), a federated learning approach that enables efficient training across geographically distributed and poorly-connected infrastructure. This section covers the training algorithm, data distribution, hyperparameters, and model management.

## DiLoCo-Based Training Overview

DiLoCo is a variant of federated averaging specifically designed for heterogeneous, distributed environments with limited bandwidth or high latency. It achieves comparable performance to traditional synchronous training while reducing communication by approximately 500x.

### Key Characteristics

**Two-Level Optimization**: DiLoCo separates optimization into inner loops (local training on workers) and outer loops (global aggregation via parameter server).

**Large Inner Step Counts**: Workers perform hundreds of local optimization steps (H ≈ 500) before synchronizing. This dramatically reduces communication frequency compared to traditional data-parallel training which synchronizes after every batch.

**Heterogeneous Hardware Support**: Performance-aware scheduling adapts to varying worker capabilities by allocating different batch sizes and synchronization frequencies.

**Communication Efficiency**: By synchronizing only pseudo-gradients (weight deltas) rather than per-batch gradients, DiLoCo reduces network traffic by ~500x compared to standard data-parallel approaches.

**Fault Tolerance**: The algorithm naturally tolerates stragglers and transient failures through its asynchronous local training and lease-based coordination.

### When to Use DiLoCo

**Ideal Scenarios**:
- Multi-region deployments with high WAN latency
- Networks with limited or unreliable bandwidth
- Heterogeneous hardware pools (mixed GPU types, varying capacities)
- Edge computing scenarios
- Hybrid cloud/on-premise infrastructure

**Less Suitable For**:
- Single datacenter with fast interconnect (traditional data-parallel may be simpler)
- Homogeneous hardware pools with guaranteed availability
- Applications requiring strict batch-synchronized updates

## Algorithm Flow and Pseudocode

DiLoCo operates in two nested loops: inner optimization on workers and outer optimization on the parameter server.

### Inner Optimization Loop

Workers train locally for H steps using their assigned data slices:

```
For each worker w in parallel:
  Initialize model θ_w from parameter server

  For each inner step h in [1, H]:
    batch = fetch_data_from_data_node()
    loss = forward_pass(model, batch)
    gradients = backward_pass(loss)
    θ_w = inner_optimizer.step(θ_w, gradients)  # AdamW update

    report metrics(loss, batch_size) to scheduler

  # Compute pseudo-gradient (weight delta)
  Δθ_w = θ_w - θ_initial

  send_to_parameter_server(Δθ_w)
```

**Key Points**:
- Each worker maintains its own model copy
- Inner optimizer (AdamW) applies standard gradient descent
- Workers process different data (data-parallel)
- Metrics reported to scheduler for performance-aware scheduling
- Pseudo-gradient is the total weight change over H steps

### Outer Optimization (Parameter Server)

The parameter server aggregates worker updates and applies Nesterov momentum:

```
# Wait for pseudo-gradients from all workers
Δθ = {Δθ_1, Δθ_2, ..., Δθ_N}

# Average gradients
Δθ_avg = mean(Δθ)

# Apply Nesterov momentum
velocity = momentum * velocity_prev + Δθ_avg
update = learning_rate * (momentum * velocity + Δθ_avg)

# Update global model
θ_global = θ_global + update

# Broadcast to workers for next round
broadcast_to_workers(θ_global)
```

**Key Points**:
- Parameter server waits for all workers (configurable timeout)
- Simple averaging provides unweighted aggregation
- Nesterov momentum accelerates convergence
- Updated model becomes starting point for next inner loop

## Hyperparameters

DiLoCo has two sets of optimizer hyperparameters:

### Inner Optimizer (AdamW)

Applied during local training on workers:

**learning_rate**: Step size for inner updates (typical: 1e-4 to 1e-3)
```toml
[scheduler.job.inner_optimizer]
learning_rate = 0.001
```

**betas** (optional): Exponential decay rates for moment estimates
```toml
betas = [0.9, 0.999]  # Default PyTorch values
```

**epsilon** (optional): Small constant for numerical stability
```toml
epsilon = 1e-8  # Default PyTorch value
```

If `betas` or `epsilon` are omitted, PyTorch defaults apply.

### Outer Optimizer (Nesterov Momentum)

Applied during global aggregation:

**learning_rate**: Step size for outer updates (typical: 0.5 to 0.9)
```toml
[scheduler.job.outer_optimizer]
learning_rate = 0.7
```

**momentum**: Momentum coefficient (typical: 0.9)
```toml
momentum = 0.9
```

### Synchronization Frequency

**Inner Steps (H)**: Number of local updates before synchronization

This is not directly configured but emerges from performance-aware scheduling. The scheduler tracks worker progress and triggers synchronization when efficient. Typical values: H ≈ 500 for vision models, H ≈ 100-200 for language models.

### Tuning Recommendations

**Starting Point**:
- Inner LR: 1e-3 (vision), 1e-4 (language models)
- Outer LR: 0.7
- Outer Momentum: 0.9

**Convergence Issues**:
- Reduce outer learning rate if training diverges
- Increase inner learning rate if progress is too slow
- Adjust momentum if oscillations occur

**Performance Optimization**:
- Let performance-aware scheduling determine H automatically
- Monitor straggler impact (check sync wait times)
- Adjust worker batch sizes if some workers are consistently slow

## Data Distribution & Synchronization

Hypha separates data loading from training synchronization, allowing independent management of data availability and model updates.

### Data Loading Loop

Data distribution runs continuously, independent of synchronization:

**1. Worker Requests Data Slice**:
```
worker -> scheduler: "Need data for training"
```

**2. Scheduler Assigns Slice**:
```
scheduler checks slice states:
  - AVAILABLE: not yet assigned
  - ASSIGNED: sent to worker but not confirmed used
  - USED: processed during current epoch

scheduler finds AVAILABLE slice:
  slice.state = AVAILABLE -> ASSIGNED
  scheduler -> worker: "Fetch slice-042 from data_node_peer_id"
```

**3. Worker Fetches from Data Node**:
```
worker -> data_node: open_stream(slice-042)
data_node -> worker: stream SafeTensors file
worker: save to local cache
```

**4. Worker Reports Completion**:
```
worker -> scheduler: "Slice-042 processed"
scheduler: slice-042.state = ASSIGNED -> USED
```

**5. Epoch Boundary**:
```
if all slices in USED state:
  scheduler: reset all slices to AVAILABLE
  epoch += 1
```

**Prefetching**: Workers can request next slice before finishing current one, minimizing wait times.

### Training Loop

Training synchronization is controlled by the scheduler based on progress:

**1. Worker Processes Batches**:
```
for batch in current_slice:
  loss = train_step(batch)
  worker -> scheduler: report(loss, batch_size, processing_time)
```

**2. Scheduler Tracks Progress**:
```
scheduler maintains:
  - avg_batch_time per worker
  - total_data_points_processed
  - target_data_points_for_update
```

**3. Scheduler Simulates Future**:
```python
def should_schedule_update(workers, target_remaining):
    """Determine if update should be scheduled now."""
    # Project each worker's completion time
    projected_completions = []
    for worker in workers:
        batches_remaining = worker.batches_until_target()
        time_remaining = batches_remaining * worker.avg_batch_time
        projected_completions.append(time_remaining)

    # Schedule if slowest worker is close
    slowest_time = max(projected_completions)
    return slowest_time < threshold  # e.g., 5 minutes
```

**4. Scheduler Triggers Update**:
```
if should_schedule_update():
  for each worker:
    batches_to_go = calculate_remaining(worker)
    scheduler -> worker: ScheduleUpdate(counter=batches_to_go)

  scheduler -> param_server: ExpectUpdates(
    worker_peer_ids=[...],
    timeout=estimated_completion + buffer
  )
```

**5. Worker Sends Gradient**:
```
worker processes 'counter' batches:
  for i in range(counter):
    batch = next_batch()
    train_step(batch)
    counter -= 1

  if counter == 0:
    pseudo_gradient = current_model - initial_model
    worker -> param_server: send_gradient(pseudo_gradient)
```

**6. Parameter Server Aggregates**:
```
param_server waits for all workers (or timeout)
gradients = collect_from_workers()
avg_gradient = mean(gradients)
new_model = apply_nesterov(avg_gradient)
param_server -> workers: broadcast(new_model)
```

## Performance-Aware Scheduling

Traditional distributed training assumes homogeneous workers with uniform batch sizes. Hypha adapts to heterogeneous capabilities.

### Heterogeneous Batch Size Allocation

**Problem**: In heterogeneous environments, fast workers wait for slow workers at synchronization points.

**Solution**: Allocate more work to fast workers, less to slow workers. All workers finish approximately simultaneously.

**Implementation**:

```python
# Scheduler tracks per-worker statistics
worker_stats = {
    worker_id: {
        "avg_batch_time": 0.5,  # seconds
        "batch_size": 32,
        "recent_completions": [0.48, 0.52, 0.49]
    }
}

# When scheduling update, allocate based on capacity
def allocate_batches(workers, total_target_data):
    """Allocate batches inversely proportional to processing time."""
    # Calculate weights (faster workers get more data)
    weights = {w: 1.0 / w.avg_batch_time for w in workers}
    total_weight = sum(weights.values())

    for worker in workers:
        fraction = weights[worker] / total_weight
        worker_data = total_target_data * fraction
        worker_batches = worker_data / worker.batch_size
        assign_batches(worker, worker_batches)
```

### Simulation-Based Synchronization

**Tracking Progress**:
- Average batch processing time per worker (exponential moving average)
- Number of data points processed
- Batches remaining to target

**Projecting Completion**:
```python
def project_completions(workers, target_data_points):
    """Simulate future to find optimal sync point."""
    updates = {w: 0 for w in workers}
    next_update = {w: w.avg_batch_time for w in workers}
    time = 0
    done = 0

    while done < target_data_points and time < timeout:
        # Find worker finishing next batch
        next_worker = min(workers, key=lambda w: next_update[w])
        time = next_update[next_worker]

        # Record completion
        done += next_worker.batch_size
        updates[next_worker] += 1
        next_update[next_worker] += next_worker.avg_batch_time

    return time, updates
```

**Decision Logic**:
- Trigger update when slowest worker is within threshold
- Threshold balances synchronization overhead vs straggler wait time
- Typical threshold: 5 minutes or 5 batches remaining

### Benefits

- Minimizes idle time for fast workers
- Better utilization of heterogeneous resources
- Automatic adaptation to changing conditions (network, load)
- No manual tuning of per-worker batch sizes

## Metrics, Logging, and AIM Integration

Hypha collects comprehensive training metrics for monitoring and analysis.

### Metrics Collected

**Per-Batch Metrics** (from workers):
- Loss value
- Number of data points processed
- Batch processing time (including data loading)
- Learning rate (if using schedules)

**Synchronization Metrics** (from scheduler):
- Synchronization trigger timestamp
- Worker participation (which workers contributed)
- Gradient aggregation time
- Weight broadcast time

**Data Distribution Metrics** (from scheduler):
- Slice assignment rate
- Slice fetch completion time
- Prefetch queue depth
- Epoch progress

### AIM Integration

AIM (Aim) provides visualization and tracking for ML experiments.

**Configuration**:
```toml
status_bridge = "0.0.0.0:61000"
```

The scheduler exposes metrics on this endpoint for AIM to consume.

**Starting AIM Server**:
```bash
aim server --host 0.0.0.0 --port 43800
```

**Connecting to Scheduler**:
Configure AIM to connect to the scheduler's `status_bridge` endpoint (port 61000).

**Dashboard Access**:
Open `http://localhost:43800` to view:
- Loss curves over time
- Throughput (samples/second)
- Synchronization frequency
- Worker utilization

### OpenTelemetry

For production monitoring, use OpenTelemetry:

**Metrics**:
- Training throughput
- Synchronization latency
- Worker straggler time
- Data fetch latency

**Traces**:
- End-to-end training iteration spans
- Data fetch operations
- Gradient aggregation traces
- Weight broadcast traces

**Logs**:
- Synchronization events with context
- Worker failures or timeouts
- Configuration changes

## Data Preparation

Efficient training requires properly prepared datasets.

### Dataset Partitioning & Slicing

**Slice Size Considerations**:

*Small Slices (100-500 samples)*:
- Workers start training faster
- Better for unreliable networks (easier to retry)
- Higher overhead (more fetch requests)

*Large Slices (1000-10000 samples)*:
- Lower overhead
- Better bandwidth utilization
- Requires more local storage

**Recommendation**: 500-2000 samples per slice for typical vision datasets.

### Conversion to SafeTensors

SafeTensors format provides safety, speed, and memory mapping.

**Example: Vision Data**:
```python
from safetensors.torch import save_file
import torch

def save_vision_slice(images, labels, output_path):
    """Save preprocessed images and labels."""
    tensors = {
        "images": torch.stack(images),  # [N, C, H, W]
        "labels": torch.tensor(labels),  # [N]
    }
    save_file(tensors, output_path)
```

**Example: Language Data**:
```python
def save_language_slice(input_ids, attention_masks, output_path):
    """Save tokenized text."""
    tensors = {
        "input_ids": torch.tensor(input_ids),  # [N, seq_len]
        "attention_mask": torch.tensor(attention_masks),  # [N, seq_len]
    }
    save_file(tensors, output_path)
```

**Format Advantages**:
- Safe deserialization (no pickle vulnerabilities)
- Fast loading via memory mapping
- Efficient streaming over network
- Cross-platform compatibility

### Choosing Slice Sizes

**Network-Constrained Environments**:
- Smaller slices (500-1000 samples)
- Enable prefetching
- Tolerate incomplete downloads

**High-Bandwidth Environments**:
- Larger slices (2000-5000 samples)
- Reduce fetch overhead
- Maximize throughput

**Heterogeneous Workers**:
- Vary slice sizes by worker capability
- Fast workers fetch large slices
- Slow workers fetch smaller slices

## Model Management

### Supported Model Types

Hypha supports models loadable via HuggingFace Transformers:

**Image Classification**:
```toml
[scheduler.job.model]
repository = "microsoft/resnet-50"
type = "vision-classification"
```

Uses `AutoModelForImageClassification.from_pretrained()`.

**Causal Language Models**:
```toml
[scheduler.job.model]
repository = "meta-llama/Llama-3.1-8B"
type = "causal-lm"
```

Uses `AutoModelForCausalLM.from_pretrained()`.

**Custom PyTorch Models** (future):
```toml
[scheduler.job.model]
type = "torch"
```

Generic PyTorch model loading (planned).

### Loading Models via HuggingFace

**Repository Specification**:
```toml
repository = "l45k/Resnet50"  # user/model format
revision = "main"              # branch, tag, or commit
filenames = [
    "config.json",
    "model.safetensors"
]
```

**Required Files**:
- `config.json`: Model configuration
- `model.safetensors`: Model weights in SafeTensors format
- Additional files depending on model type

**Authentication**:
```toml
token = "hf_..."  # HuggingFace API token for private models
```

For private models, provide a HuggingFace API token with read access.

**Compatibility**: Hypha supports all HuggingFace models loadable via the Auto classes. See [HuggingFace documentation](https://huggingface.co/docs/transformers/model_doc/auto#transformers.AutoModelForImageClassification.from_pretrained) for model compatibility.

### Preprocessor Configuration

Vision models require preprocessors:

**Vision Preprocessor**:
```toml
[scheduler.job.preprocessor]
repository = "microsoft/resnet-50"
filenames = ["preprocessor_config.json"]
```

Uses `AutoImageProcessor.from_pretrained()`.

**Tokenizers** (language models):
Tokenizer loading is automatic for language models via `AutoTokenizer.from_pretrained()`.

**Custom Preprocessing**:
For custom preprocessing pipelines, implement within executor code.

## Gradient Extraction and Application

DiLoCo exchanges weight deltas (pseudo-gradients) rather than per-batch gradients.

### Computing Pseudo-Gradients

```python
# Worker: after H inner steps
initial_weights = model_at_start_of_round
current_weights = model_after_H_steps

pseudo_gradient = {}
for name, param in current_weights.items():
    pseudo_gradient[name] = param - initial_weights[name]

# Serialize to SafeTensors for transmission
save_file(pseudo_gradient, "pseudo_gradient.safetensors")
```

### Applying Updates

```python
# Parameter server: after aggregation
new_weights = {}
for name, param in current_model.items():
    new_weights[name] = param + gradient_update[name]

# Broadcast to workers
save_file(new_weights, "updated_model.safetensors")
```

### SafeTensors Serialization

SafeTensors provides efficient serialization:
- Compact binary format
- Fast serialization/deserialization
- Memory-mapped loading
- Network streaming support

## Learning Rate Scheduling

Control learning rate decay over training:

### Supported Schedules

**cosine-with-warmup**: Cosine decay with linear warmup
```toml
schedule = "cosine-with-warmup"
warmup_steps = 1000
total_steps = 100000
```

**linear-with-warmup**: Linear decay with warmup
```toml
schedule = "linear-with-warmup"
warmup_steps = 1000
total_steps = 100000
```

**wsd**: Warmup-stable-decay schedule
```toml
schedule = "wsd"
warmup_steps = 1000
stable_steps = 50000
total_steps = 100000
```

**constant**: Fixed learning rate
```toml
schedule = "constant"
# No additional parameters
```

### Configuration

Schedules apply to inner optimizer (worker-side):

```toml
[scheduler.job.inner_optimizer]
learning_rate = 0.001
schedule = "cosine-with-warmup"
warmup_steps = 2000
total_steps = 500000
```

Outer optimizer (parameter server) typically uses constant learning rate.

## Next Steps

- Review [Scheduler Configuration](scheduler.md) for job specification
- Configure [Workers](worker.md) for training execution
- Set up [Data Nodes](data-node.md) for dataset serving
- Consult [Operations](operations.md) for monitoring and fault handling
