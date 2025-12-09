+++
title = "AMD GPU Worker (ROCm)"
description = "Guide for deploying a high-performance AMD GPU Worker (MI300X) on AMD Developer Cloud"
[taxonomies]
track = ["onboarding"]
+++

# Deploying an AMD GPU Worker (MI300X)

This guide walks you through deploying a high-performance **AMD GPU worker** using the **AMD Developer Cloud**.

**Goal:** A powerful worker (MI300X) for running Hypha DiLoCo training jobs.

To follow this guide, ensure you have a running **Gateway** reachable by the worker, a **Node Certificate** and Key for the worker (refer to [Security](@/security.md) for details), and an account on [amd.digitalocean.com](https://amd.digitalocean.com).

## 1. Infrastructure Specification

We will use the AMD Developer Cloud to provision a "GPU Droplet" with the following key specifications:

| Component | Specification | Rationale |
|---|---|---|
| Image | **ROCm 6.4** | Select the base "ROCm Software" image. Do not use the "AI/ML" or "GPT-OSS" pre-configured images as they contain unnecessary bloat. |
| Instance | MI300X 1 GPU | 20 vCPU, 240GB RAM, 192GB VRAM. |
| Storage | Boot: 720GB NVMe<br>Scratch: 5TB NVMe | Massive storage for large models and datasets. |

Begin by logging in to the [AMD Developer Cloud Console](https://amd.digitalocean.com). Navigate to **GPU Droplets** and select **Create GPU Droplet**. Choose the **MI300X** (1 GPU) plan and then select the preinstalled **ROCm™ Software** package, with version **6.4**. Before creating the droplet, add your public SSH key. Once configured, click **Create**.

> [!IMPORTANT]
> Ensure the ROCm version matches is set to 6.4, matching the PyTorch index URL we will configure later.

## 2. Install & Configure Hypha

### 2.1 Install Dependencies

First, install `uv` using the [official installation guide](https://astral.sh/docs/uv/install/). A quick way to do this is:

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

Next, install the `hypha-worker` binary. You can find detailed instructions in the [installation guide](@/installation.md), or use the following command to install the latest version:

```bash
curl -LsSf https://hypha-space.org/install.sh | sh
```

### 2.2 Configuration

First, upload your node certificates (`cert.pem`, `key.pem`, and `ca.pem`) to `/etc/hypha/certs/`. Then, initialize a base configuration file using `hypha-worker init`, specifying a name for your worker, the gateway's multiaddr, and the designated work directory.

```bash
hypha-worker init \
  -n amd-worker-1 \
  --gateway <GATEWAY_MULTIADDR> \
  --work-dir /mnt/data/work
```

After generating `config.toml`, you must adjust the resources to reflect the MI300X's capabilities. Critically, configure the executor for ROCm.

**Resources:**

```toml
[resources]
cpu = 20
memory = 240   # GB
gpu = 192      # GB (MI300X)
storage = 5000 # GB (Scratch Disk)
```

**Executor Configuration (ROCm):**

Within the `[[executors]]` block for `diloco-transformer`, configure the `args` to use the ROCm PyTorch index. This ensures `uv` installs the correct PyTorch/ROCm compatible packages.

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
    "--with", "https://github.com/hypha-space/hypha/releases/download/v<version>/hypha_accelerate_executor-<PEP 440-ish version derived from the release version (e.g., `1.0.0a19` for `v1.0.0-alpha.19`)>-py3-none-any.whl",
    "--index", "https://download.pytorch.org/whl/rocm6.4",
    "--index-strategy", "unsafe-best-match",
    "--",
    "accelerate",
    "launch",
    "--config_file", "<path/to/accelerate.yaml>",
    "-m", "hypha.accelerate_executor.training",
    "--socket", "{SOCKET_PATH}",
    "--work-dir", "{WORK_DIR}",
    "--job", "{JOB_JSON}",
]
```

## 3. Start

Once all configurations are complete, start the worker using the command:

```bash
hypha-worker run -c worker.toml
```
