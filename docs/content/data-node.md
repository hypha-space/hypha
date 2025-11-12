# Data Node

Data nodes store prepared datasets and serve slices to workers on demand. They announce available datasets via the DHT and stream data efficiently using the SafeTensors format. This section covers data node deployment, dataset preparation, and configuration.

## Role and Responsibilities

Data nodes serve three primary functions in the training workflow:

**Dataset Storage**: Maintaining prepared datasets in SafeTensors format, organized as slices suitable for data-parallel training.

**DHT Announcement**: Publishing available datasets to the Kademlia DHT. Schedulers query the DHT to discover which data nodes provide required datasets.

**Slice Serving**: Streaming dataset slices to workers via libp2p stream protocols. Data nodes support parallel serving to multiple workers simultaneously.

Data nodes do not participate in training coordination, resource allocation, or compute tasks. They focus exclusively on efficient data distribution.

## Installation and Setup

Install the data node binary following the [Installation](installation.md) guide.

Quick reference:

```bash
# From pre-built binary
curl -fsSL https://github.com/hypha-space/hypha/releases/latest/download/install.sh | sh

# Or build from source
cargo build --release -p hypha-data
```

### Storage Setup

Create a directory for datasets:

```bash
sudo mkdir -p /var/hypha/datasets
sudo chown $(whoami):$(whoami) /var/hypha/datasets
chmod 755 /var/hypha/datasets
```

Ensure sufficient disk space for your datasets. Large vision datasets (e.g., ImageNet) require hundreds of GB. Language model datasets can require TB of storage.

## Dataset Preparation

Data nodes serve SafeTensors files containing preprocessed training data. You must prepare datasets before deployment.

### SafeTensors Format

SafeTensors provides several advantages for dataset storage:

**Safety**: Safe deserialization without arbitrary code execution
**Performance**: Memory-mapped file access for efficient loading
**Streaming**: Efficient sequential access for network serving
**Portability**: Simple binary format with minimal dependencies

While originally designed for model parameters, SafeTensors works well for preprocessed training data (tokenized text, normalized images, etc.).

### Converting Datasets

Dataset conversion typically involves:

1. Loading raw data (images, text, etc.)
2. Preprocessing (tokenization, normalization, augmentation)
3. Batching into appropriate slice sizes
4. Serializing to SafeTensors files

**Example: Image Dataset Conversion**

```python
import torch
from safetensors.torch import save_file
from PIL import Image
import os

def prepare_image_slices(image_dir, output_dir, slice_size=1000):
    """Convert image directory to SafeTensors slices."""
    images = []
    labels = []

    for idx, img_path in enumerate(sorted(os.listdir(image_dir))):
        img = Image.open(os.path.join(image_dir, img_path))
        # Preprocess: resize, normalize, etc.
        tensor = preprocess(img)
        images.append(tensor)
        labels.append(get_label(img_path))

        # Save slice when batch is full
        if (idx + 1) % slice_size == 0:
            slice_idx = idx // slice_size
            save_slice(images, labels, output_dir, slice_idx)
            images, labels = [], []

    # Save remaining data
    if images:
        slice_idx = len(images) // slice_size
        save_slice(images, labels, output_dir, slice_idx)

def save_slice(images, labels, output_dir, slice_idx):
    """Save batch as SafeTensors file."""
    tensors = {
        "images": torch.stack(images),
        "labels": torch.tensor(labels)
    }
    save_file(tensors, os.path.join(output_dir, f"slice-{slice_idx:04d}.safetensors"))
```

### Choosing Slice Sizes

Slice size affects training efficiency and network overhead:

**Smaller Slices** (100-500 samples):
- Lower latency (workers start training sooner)
- More network requests (higher overhead)
- Better for networks with unstable connections
- Enables finer-grained prefetching

**Larger Slices** (1000-10000 samples):
- Fewer network requests (lower overhead)
- Higher latency (workers wait longer for downloads)
- Better for stable, high-bandwidth networks
- Requires more local storage per worker

Typical recommendation: 500-2000 samples per slice for image datasets, fewer for large language model training where individual samples are large.

### Dataset Organization

Organize datasets by name and version:

```
/var/hypha/datasets/
├── imagenet-1k/
│   ├── slice-0000.safetensors
│   ├── slice-0001.safetensors
│   ├── ...
│   └── slice-1280.safetensors
├── coco-2017/
│   ├── slice-0000.safetensors
│   └── ...
└── wikitext-103/
    ├── slice-0000.safetensors
    └── ...
```

The data node announces the directory name (e.g., "imagenet-1k") to the DHT. Schedulers specify this name in job configurations.

## Configuration Parameters

Data node configuration uses TOML format with network, security, and storage settings.

### TLS Certificates

```toml
cert_pem = "/etc/hypha/data-cert.pem"
key_pem = "/etc/hypha/data-key.pem"
trust_pem = "/etc/hypha/ca-bundle.pem"
crls_pem = "/etc/hypha/crls.pem"  # Optional
```

### Network Configuration

```toml
gateway_addresses = [
    "/ip4/203.0.113.50/tcp/4001",
    "/ip4/198.51.100.25/tcp/4001"
]

listen_addresses = [
    "/ip4/0.0.0.0/tcp/0"
]

exclude_cidr = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16"
]
```

### Dataset Path

Specify the directory containing dataset files:

```toml
dataset_path = "/var/hypha/datasets/imagenet-1k"
```

The data node announces the directory name ("imagenet-1k" in this example) to the DHT.

### OpenTelemetry

```toml
telemetry_endpoint = "http://otel-collector:4318"
telemetry_protocol = "http/protobuf"

[telemetry_attributes]
"service.name" = "hypha-data"
"data.dataset" = "imagenet-1k"
"data.region" = "us-west-2"
```

## Complete Example Configuration

```toml
# TLS Configuration
cert_pem = "/etc/hypha/certs/data-cert.pem"
key_pem = "/etc/hypha/certs/data-key.pem"
trust_pem = "/etc/hypha/certs/ca-bundle.pem"

# Network
gateway_addresses = [
    "/ip4/203.0.113.50/tcp/4001",
    "/ip4/198.51.100.25/tcp/4001"
]

listen_addresses = [
    "/ip4/0.0.0.0/tcp/6000"
]

exclude_cidr = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "127.0.0.0/8"
]

# Dataset
dataset_path = "/var/hypha/datasets/imagenet-1k"

# Telemetry
telemetry_endpoint = "http://otel-collector:4318"
telemetry_protocol = "http/protobuf"
telemetry_sampler = "traceidratio"
telemetry_sample_ratio = 0.1

[telemetry_attributes]
"service.name" = "hypha-data"
"service.version" = "0.1.0"
"data.dataset" = "imagenet-1k"
"data.location" = "us-west-2a"
```

## CLI Reference

Full command-line reference will be available when [PR #118](https://github.com/hypha-space/hypha/pull/118) merges.

**Basic Usage**:
```bash
hypha-data --config /path/to/config.toml
```

**Common Options**:
- `--config <FILE>`: Path to configuration file (required)
- `--help`: Display help information
- `--version`: Show version information

**Environment Variables**:

Override configuration via environment:
```bash
HYPHA_DATASET_PATH=/var/hypha/datasets/imagenet-1k \
hypha-data --config config.toml
```

## DHT Announcement

Data nodes announce available datasets to the Kademlia DHT:

**Provider Records**: The data node publishes a provider record for its dataset name (e.g., "imagenet-1k"). This record associates the dataset name with the data node's peer ID.

**Discovery**: When schedulers need a dataset, they query the DHT for provider records matching the dataset name. The DHT returns peer IDs of data nodes serving that dataset.

**Multiple Providers**: Multiple data nodes can serve the same dataset. Schedulers receive all provider records and can distribute load across multiple sources or select based on proximity.

**TTL and Refresh**: Provider records have time-to-live (TTL) values. Data nodes periodically refresh their announcements to maintain availability in the DHT.

## Slice Serving

Data nodes stream SafeTensors files to workers via libp2p stream protocols:

**Parallel Serving**: Data nodes handle multiple concurrent streams, serving different slices to different workers simultaneously.

**Efficient Streaming**: SafeTensors files are memory-mapped and streamed sequentially, minimizing memory usage and maximizing throughput.

**Flow Control**: libp2p stream protocols implement TCP-like flow control, preventing overwhelm of slower workers.

**Fault Tolerance**: Stream failures are handled gracefully. Workers can retry failed slice fetches without impacting training.

## Deployment Patterns

### Single Data Node

Simplest deployment for small-scale training:

```
┌──────────┐
│ Data Node│
│ ImageNet │
└────┬─────┘
     │
     ├─────> Worker 1
     ├─────> Worker 2
     └─────> Worker 3
```

Suitable for:
- Small worker pools (< 10 workers)
- Moderate dataset sizes (< 500 GB)
- Networks with good bandwidth

### Replicated Data Nodes

Multiple nodes serve the same dataset for redundancy and load distribution:

```
┌──────────┐  ┌──────────┐  ┌──────────┐
│ Data A   │  │ Data B   │  │ Data C   │
│ ImageNet │  │ ImageNet │  │ ImageNet │
└────┬─────┘  └────┬─────┘  └────┬─────┘
     │             │             │
     ├─────────────┼─────────────┤
     │             │             │
   Worker 1    Worker 2      Worker 3
```

Benefits:
- Load distribution across servers
- Redundancy if one data node fails
- Regional deployment reduces latency

### Multiple Datasets

Different data nodes serve different datasets:

```
┌──────────┐  ┌──────────┐  ┌──────────┐
│ Data A   │  │ Data B   │  │ Data C   │
│ ImageNet │  │   COCO   │  │WikiText  │
└────┬─────┘  └────┬─────┘  └────┬─────┘
     │             │             │
     └─────────────┼─────────────┘
                   │
              Scheduler
```

Enables:
- Multi-dataset training experiments
- Dataset specialization per node
- Isolation of large datasets

## Monitoring and Observability

Data nodes export metrics via OpenTelemetry:

**Serving Metrics**:
- Active stream count
- Bytes transferred per dataset
- Slice request rate
- Average serving latency

**Storage Metrics**:
- Disk space usage
- Available slice count
- Dataset size

**Network Metrics**:
- Bandwidth utilization
- Connection count
- DHT query rate

**Configuration**:

```toml
telemetry_endpoint = "http://otel-collector:4318"
telemetry_protocol = "http/protobuf"

[telemetry_attributes]
"service.name" = "hypha-data"
"data.dataset" = "imagenet-1k"
"data.instance" = "data-01"
"data.region" = "us-west-2"
```

## Troubleshooting

**Workers cannot find dataset**:
- Verify data node connected to network (check logs)
- Confirm DHT announcement succeeded (look for provider record messages)
- Check dataset name matches scheduler configuration exactly
- Verify gateway connectivity for DHT participation

**Slow slice serving**:
- Monitor network bandwidth utilization
- Check disk I/O performance (SafeTensors should use memory mapping)
- Verify concurrent stream count isn't overwhelming node
- Consider replicating dataset to additional data nodes

**DHT announcement failures**:
- Ensure connection to gateways succeeded
- Check network connectivity to other DHT participants
- Verify no network partitions isolating data node
- Review logs for Kademlia-related errors

**Slice files not found**:
- Verify `dataset_path` points to correct directory
- Check file permissions (data node must have read access)
- Ensure SafeTensors files are properly formatted
- Look for file corruption

**High memory usage**:
- SafeTensors should use memory mapping (low memory footprint)
- Check for memory leaks in streaming code
- Monitor concurrent stream count
- Verify proper cleanup of completed streams

## Next Steps

With data nodes deployed:

1. Configure [Schedulers](scheduler.md) to reference dataset names
2. Prepare workers to fetch data slices (see [Worker](worker.md) documentation)
3. Review [Training](training.md) for data distribution workflows
4. Set up monitoring via [Operations](operations.md#monitoring-and-observability)
5. Consult [Security](security.md) for production hardening
