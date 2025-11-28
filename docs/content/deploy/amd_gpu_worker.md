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
> Ensure the ROCm version matches is set to 6.4, matching the executor variant (`rocm64`) we will configure later.

## 2. Install & Configure Hypha

### 2.1 Install Dependencies

Install `uv` for Python package management by running the official install script. Subsequently, install the `hypha-worker` binary, remembering to replace `<VERSION>` with your target version in the install script URL.

```bash
# Install uv
curl -LsSf https://astral.sh/uv/install.sh | sh

# Install Hypha Worker (replace <VERSION> with your target version)
curl -fsSL https://github.com/hypha-space/hypha/releases/download/v<VERSION>/install.sh | sh
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

Within the `[[executors]]` block for `diloco-transformer`, explicitly set the `[rocm64]` variant. This ensures `uv` installs the correct PyTorch/ROCm compatible packages.

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
    "--with", "hypha-accelerate-executor[rocm64] @ https://github.com/hypha-space/hypha/releases/download/v<VERSION>/hypha_accelerate_executor-<VERSION without semver channel or metadata>-py3-none-any.whl",
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
