# Data Node

Data nodes store prepared datasets and serve slices to workers on demand. They announce available datasets via the DHT and stream data efficiently using the SafeTensors format. This section covers data node deployment, dataset preparation, and configuration.

**CLI Reference**: See the [hypha-data CLI Reference](../reference/hypha-data-cli.md) for complete command-line documentation.

## Role and Responsibilities

Data nodes serve three primary functions in the training workflow:

**Dataset Storage**: Maintaining prepared datasets in SafeTensors format, organized as slices suitable for data-parallel training.

**DHT Announcement**: Publishing available datasets to the Kademlia DHT. Schedulers query the DHT to discover which data nodes provide required datasets.

**Slice Serving**: Streaming dataset slices to workers via libp2p stream protocols. Data nodes support parallel serving to multiple workers simultaneously.

Data nodes do not participate in training coordination, resource allocation, or compute tasks. They focus exclusively on efficient data distribution.

## Installation and Setup

Install the data node binary following the [Installation](installation.md) guide.

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

* **Safety**: Safe deserialization without arbitrary code execution
* **Performance**: Memory-mapped file access for efficient loading
* **Streaming**: Efficient sequential access for network serving
* **Portability**: Simple binary format with minimal dependencies

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

Data node configuration uses TOML format with network, security, and storage settings. Generate an example configuration file using the [`hypha-data init`](../reference/hypha-data-cli.md#hypha-data-init) command. You will need to provide paths to TLS certificates and configure basic network settings (gateway addresses, listen addresses).

### Dataset Path

Specify the directory containing dataset files:

```toml
dataset_path = "/var/hypha/datasets/imagenet-1k"
```

The data node announces the directory name ("imagenet-1k" in this example) to the DHT.

### OpenTelemetry

OpenTelemetry enables distributed tracing and metrics collection for debugging and monitoring your data nodes in production. Configure telemetry either via the TOML configuration file or using standard `OTEL_*` environment variables.

**Configuration File (Example uses Grafana Cloud)**: Specify telemetry settings in your `data.toml`:

```toml
telemetry_attributes = "service.name=<Node Name>,service.namespace=<Namespace>>,deployment.environment=<Environment>"
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
export OTEL_SERVICE_NAME="data-node-01"
export OTEL_RESOURCE_ATTRIBUTES="service.namespace=production,deployment.environment=prod"

hypha-data run --config /etc/hypha/data.toml
```

Environment variables take precedence over configuration file settings, allowing flexible per-deployment customization.

## DHT Announcement

Data nodes announce available datasets to the Kademlia DHT by publishing provider records. Each provider record associates a dataset name (e.g., "imagenet-1k") with the data node's peer ID, making the dataset discoverable across the network.

When schedulers need a dataset for training, they query the DHT for provider records matching the required dataset name. The DHT returns peer IDs of all data nodes serving that dataset. Multiple data nodes can serve the same dataset simultaneously, allowing schedulers to distribute load across multiple sources or select providers based on network proximity.

Provider records include time-to-live (TTL) values to ensure freshness. Data nodes periodically refresh their announcements to maintain availability in the DHT, ensuring that schedulers always have current information about dataset availability.

## Slice Serving

Data nodes stream SafeTensors files to workers via libp2p stream protocols, supporting efficient data distribution during training. Each data node handles multiple concurrent streams, serving different slices to different workers simultaneously to maximize parallelism and throughput.

The serving implementation leverages memory-mapped file access for SafeTensors files, streaming data sequentially to minimize memory usage while maximizing network throughput. libp2p stream protocols provide TCP-like flow control, preventing data nodes from overwhelming slower workers and ensuring stable data delivery across heterogeneous network conditions.
