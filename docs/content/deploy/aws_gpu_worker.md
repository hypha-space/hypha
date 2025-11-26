+++
title = "AWS GPU Worker"
description = "Step-by-step guide for deploying a small GPU Worker on AWS (G4dn)"
[taxonomies]
track = ["onboarding"]
+++

# Deploying a small Hypha GPU Worker on AWS


This guide walks you through deploying a **small GPU worker** on AWS EC2.

**Goal:** A small NVIDIA GPU worker (g4dn) for running Hypha DiLoCo training jobs.

**Prerequisites:**
- A running **Gateway** reachable by the worker.
- A **Node Certificate** and Key for the worker (see [Security](@/security.md)).

## 1. Infrastructure Specification

Provision an EC2 instance with the following specifications. For detailed steps, refer to the [AWS User Guide: Launching an Instance](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/launching-instance.html).

| Component | Specification | Rationale |
|---|---|---|
| AMI | Amazon Linux 2023 (x86_64) | Standard, stable, optimized for EC2. |
| Instance | g4dn.xlarge (or larger) | Contains NVIDIA T4 (16GB). |
| Storage | Root: 30GB+<br>Data: 125GB+ (included) | Root fills quickly with drivers and etc. Models, datasets, and caches go on the Data volume. |
| Network | Security Group:<br/>- Inbound: SSH (22)<br/>- Outbound: All<br/><br/>Optional: Inbound TCP/UDP | Worker initiates connections to the Gateway. |
| IAM Role | AmazonS3ReadOnlyAccess | Required to download NVIDIA drivers from AWS S3 buckets. |

> [!IMPORTANT]
> Do not use `g4ad.*` (AMD) instances as these instances are not supported for trainign by AMD. 

## 2. System Setup

Connect to your instance via SSH. For instructions, see [AWS Guide: Connect to your Linux instance](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/connecting_to_your_instance.html) or the information provided by when clicking "Connect" in the AWS Management Console.

### 2.1 Install NVIDIA Drivers

Follow the [AWS Guide: Install NVIDIA drivers on Linux instances](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/nvidia-GRID-driver.html).

> [!NOTE]
> Ensure `nvidia-smi` returns valid GPU status before proceeding.

### 2.2 Mount Data Volume

Mount the **local NVMe instance store** (fast, ephemeral) or an EBS volume to `/mnt/data`.
See **[AWS Guide: Instance Store Volumes](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/InstanceStorage.html)**.

First identify the ephemeral disk (usually the one NOT mounted at /) using `lsblk`. Then format (if using instance store) & mount the volume.

```bash
sudo mkfs.ext4 -E nodiscard /dev/nvme1n1
sudo mkdir -p /mnt/data
sudo mount /dev/nvme1n1 /mnt/data
sudo chown -R ec2-user:ec2-user /mnt/data
```
> [!IMPORTANT]
> Adjust the disk paths to match your instance's configuration!

> [!TIP]
> Remember to add an entry to /etc/fstab with 'nofail'  for persistence. 

### 2.3 Configure Cache Paths

Prevent the root disk from filling up by pointing caches to the data volume.

```bash
mkdir -p /mnt/data/{hf,uv}
```

```bash
export HF_HOME=/mnt/data/hf
export UV_CACHE_DIR=/mnt/data/uv
```

## 3. Install & Configure Hypha

### 3.1 Install Hypha along with its dependencies

First, install `uv` using the [official installation guide](https://astral.sh/docs/uv/install/). A quick way to do this is:

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

Next, install the `hypha-worker` binary. You can find detailed instructions in the [installation guide](@/installation.md), or use the following command (remember to adapt `<VERSION>`):

```bash
curl -fsSL https://github.com/hypha-space/hypha/releases/download/v<VERSION>/install.sh | sh
```

### 3.2 Setup Configuration

Begin by placing your worker's node certificates. Upload your `cert.pem`, `key.pem`, and `ca.pem` to `/etc/hypha/certs/` on the instance and secure them with appropriate permissions (`chmod 600` for private keys).

Next, initialize a base configuration file using `hypha-worker init`. This command generates a `config.toml` file with essential settings. Ensure you specify the worker's name, the gateway address, and the designated work directory on your data volume:

```bash
hypha-worker init \
  -n worker-1 \
  --gateway <GATEWAY_MULTIADDR>\
  --work-dir /mnt/data/work 
```

After generating the configuration, you will need to edit the `config.toml` file to fine-tune it. Critically, update the `cert_pem`, `key_pem`, and `trust_pem` paths to point to the certificate files you placed earlier. Additionally, it's important to adjust the `[resources]` section to accurately reflect your instance's capabilities, such as for a `g4dn.xlarge`:

```toml
[resources]
cpu = 4
memory = 16  # GB
gpu = 16     # GB (T4)
storage = 200 # GB (Data Volume)
```

**Create Accelerate Configuration:**
The `diloco-transformer` executor requires an Accelerate configuration file. For a single-GPU `g4dn.xlarge` instance, you can generate a default configuration by running `accelerate config` on the instance and selecting "no" for distributed training, then placing the `accelerate.yaml` file at `/etc/hypha/accelerate.yaml`. For more complex setups, refer to the [Accelerate CLI documentation](https://huggingface.co/docs/accelerate/en/package_reference/cli#accelerate-config).

The generated configuration automatically includes a default `diloco-transformer` executor and parameter server setup. For standard `g4dn` instances, these defaults work out-of-the-box. You can optionally verify or customize these settings as detailed in the [Training Docs](@/training.md).

## 4. Observability (Optional)

Hypha supports OpenTelemetry (OTEL) for metrics and tracing. You can export telemetry to any OTLP-compatible backend, such as Grafana Cloud.

To configure this, set the standard OTEL env or the respectice configutation options. For Grafana Cloud, set the following environment variables: 

```bash
# Get these values from your Grafana Cloud "OpenTelemetry" details page
export OTEL_EXPORTER_OTLP_ENDPOINT="https://otlp-gateway-prod-eu-west-2.grafana.net/otlp"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Basic <API_TOKEN>"
```

## 5. Start

With everything configured, you can start the worker:

```bash
hypha-worker run -c worker.toml
```
