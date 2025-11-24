+++
title = "AWS GPU Worker"
description = "Step-by-step guide for deploying a small GPU Worker on AWS (G4dn)"
[taxonomies]
track = ["onboarding"]
+++

# Deploying a small Hypha GPU Worker on AWS

This guide walks you through deploying a **single GPU worker** on AWS EC2. The goal is:

> “I want one NVIDIA GPU worker on AWS that can run Hypha DiLoCo training jobs.”

We’ll:

1. Choose a suitable EC2 instance (G4dn).
2. Launch the instance with the right storage, security group, and IAM role.
3. Install the NVIDIA GRID driver and reboot.
4. Mount and persist a data volume (`/mnt/data`) and move caches off the root disk.
5. Install `uv` and Hypha.
6. Add certificates and generate a `hypha-worker` config.
7. Start the worker and verify it’s visible to the gateway.

This document assumes you **already have**:

- A running **gateway** somewhere reachable from the worker.
- A PKI that can issue a **node certificate** for this worker (see [Security](../security.md)).

## 1. Plan Your GPU Worker

### 1.1 AMI: Amazon Linux 2023 (x86_64)

Use **Amazon Linux 2023 (AL2023)**, 64-bit (x86_64):

- It’s the default / “standard” image in many regions.
- It’s designed for EC2, stable and cost-effective.
- x86_64v2 is supported on all x86-64 EC2 instance types; see the [Amazon Linux 2023 system requirements](https://docs.aws.amazon.com/linux/al2023/ug/system-requirements.html).  

From the EC2 console:

- Choose **Launch instance**.
- Under **Application and OS Images (Amazon Machine Image)**, select **Amazon Linux 2023** (x86_64).

You **do not** need a Deep Learning AMI or a pre-baked GPU AMI. For G4dn (NVIDIA T4) we’ll install the **NVIDIA GRID driver** ourselves following the [AWS documentation](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/nvidia-GRID-driver.html).  

### 1.2 GPU Family: Use G4dn, Avoid G4ad

For a small, general-purpose training node, **G4dn** is a good default:

- G4dn instances use **NVIDIA T4 Tensor Core GPUs** and are designed for ML and graphics workloads (see the [launch announcement](https://aws.amazon.com/blogs/aws/now-available-ec2-instances-g4-with-nvidia-t4-tensor-core-gpus/)).  
- They are cost-effective for small-scale training and experimentation.

In contrast, **G4ad** instances use **AMD Radeon Pro V520** GPUs ([deep dive](https://aws.amazon.com/blogs/compute/deep-dive-on-the-new-amazon-ec2-g4ad-instances/)).  

Hypha’s standard training executors use **CUDA/NVIDIA** tooling, not ROCm, so:

> **Do not use G4ad** for Hypha DiLoCo training workers.

Stick with the **G4dn family** (for example `g4dn.xlarge`, `g4dn.2xlarge`) or other NVIDIA-based families (G5, G6, P\*, etc.) if you need more VRAM; the [EC2 GPU catalog](https://aws.amazon.com/ec2/instance-types/g4/) highlights each option.  

For this guide we’ll assume **`g4dn.xlarge`** as an example (1× T4 GPU, moderate CPU/RAM).

### 1.3 Capacity Considerations

Rough sizing guidelines:

- **Model + optimizer state + activations** must fit in GPU memory.
- Leave headroom for:
  - data loader
  - framework overhead
  - any extra processes (metrics, logging)

If the model doesn’t fit on `g4dn.xlarge`:

- Try **`g4dn.2xlarge`** for more GPU memory and host RAM.
- For bigger models, look at G5/G6 or P instances; the [EC2 GPU comparison guide](https://www.nops.io/blog/amazon-ec2-gpu-instances-the-complete-guide/) has a detailed matrix.  

---

## 2. Launch the EC2 Instance

We’ll go through the EC2 “Launch instance” flow once in detail.

### 2.1 Name and AMI

1. In the EC2 console, click **Launch instance**.
2. **Name**: e.g. `hypha-worker-gpu-1`.
3. **Application and OS Images (AMI)**:
   - Choose **Amazon Linux 2023** (x86_64). See the [AMI selection guide](https://docs.aws.amazon.com/linux/al2023/ug/ec2.html) if you need to script this.

### 2.2 Instance Type

4. Under **Instance type**, filter for `g4dn`.
   - Start with **`g4dn.xlarge`** (1× T4 GPU).
   - If you already know your VRAM requirements, pick a larger size as needed.

**Avoid** G4ad here (AMD GPUs). Hypha’s CUDA-based executors will not run on those without a ROCm port (see the [G4 family overview](https://aws.amazon.com/ec2/instance-types/g4/)).  

### 2.3 Key Pair (for SSH)

5. Under **Key pair (login)**:
   - Choose an existing key pair **or** create a new one.
   - Download the private key (`.pem`) if you create a new pair and store it safely (see the [EC2 key pair guide](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/ec2-key-pairs.html)).  

You’ll need this to SSH in and:

- install drivers
- mount disks
- install Hypha and `uv`

### 2.4 Network & Security Group

6. Under **Network settings**:
   - VPC: pick your default or a dedicated VPC for Hypha.
   - Subnet: any subnet with outbound internet access (NAT or public).
   - **Auto-assign public IP**: enable if you want to SSH directly from the internet; optionally disable if using a bastion/SSM.

7. **Firewall (Security Group and Network ACL)**:

#### 2.4.1 Security Group

Create a security group like `hypha-worker-gpu` (or reuse a shared `hypha-cluster` SG) with:

- **Inbound**:
  - SSH: `TCP 22` from your IP or VPN.
  - Optional worker P2P port, e.g. `TCP 9091` and/or `UDP 9091`, **from**:
    - the gateway instance’s security group, or
    - your cluster security group.

- **Outbound**:
  - Allow all outbound (default). Worker needs:
    - HTTPS to fetch models, datasets, and drivers (S3, Hugging Face, etc.).
    - Connectivity to your gateway’s advertised addresses.

EC2 security groups are stateful; replies to outbound traffic are automatically allowed (see the [security group reference](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/ec2-security-groups.html)).  

#### 2.4.2 Network ACL

If any custom Network ACLs rules are in place, ensure they allow the necessary traffic to your workers instance.

Other than the workers Security Group firewall, Network ACLs are stateless. Replies to outbound traffic are not automatically allowed. 

Given that, when NACLs are in place, you have to bind your worker to a specific port and allow traffic to that port.

If there is no inbound traffic allowed for your worker, it will not be able to receive any traffic and therefore fail to connect to the gateway.

### 2.5 Storage

8. Under **Configure storage**:

- Root volume:
  - Keep the default type (gp3).
  - Bump size to at least **50–100 GB**; NVIDIA drivers + OS + logs can fill 8–30 GB quickly.
- Additional volume:
  - If the instance type includes **NVMe instance storage**, it will show up automatically after launch.
  - Otherwise, add an EBS volume (e.g. **200+ GB**) for models, datasets, and caches.

We’ll mount the larger disk at `/mnt/data`.

### 2.6 IAM Role (for driver download)

9. Under **Advanced details → IAM instance profile**:

Attach or create an IAM role with at least:

- **Read access to S3** (for NVIDIA GRID drivers and possibly your own buckets):
  - e.g. `AmazonS3ReadOnlyAccess` or a minimal policy that allows `s3:GetObject` for the driver bucket and your relevant buckets, as described in the [GRID driver prerequisites](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/nvidia-GRID-driver.html).  

This is required to download the GRID driver using the AWS docs.

### 2.7 Launch

10. Click **Launch instance**.

Once it’s running, note:

- The **public IP** (or DNS) for SSH.
- The **private IP** (you’ll often use this for the gateway address).

---

## 3. First Login and System Update

From your local machine:

```bash
ssh -i path/to/key.pem ec2-user@<public-ip>
```

Update base packages:

```bash
sudo dnf update -y
```

Amazon Linux 2023 uses dnf (not yum).  ￼

⸻

4. Install the NVIDIA GRID Driver (G4dn)

For G4dn (NVIDIA T4) you need the NVIDIA driver. AWS provides official guidance and S3-hosted drivers for:
	•	G4dn, G5, G6, etc.  ￼

Follow the AWS docs exactly for the version/instance family you’re using. High-level steps on Amazon Linux 2023:
	1.	Install kernel headers and development packages as described in NVIDIA’s Amazon Linux guide:  ￼

```bash
sudo dnf install -y \
  kernel-devel \
  kernel-headers \
  gcc make
```

	2.	Download the GRID driver using the instructions in the EC2 User Guide:  ￼
	•	This usually involves an aws s3 cp from the special driver bucket.
	•	Example shape (not exact URL — use the AWS docs):

```bash
aws s3 cp s3://ec2-linux-nvidia-drivers/latest/NVIDIA-Linux-x86_64-<version>.run .
chmod +x NVIDIA-Linux-x86_64-<version>.run
```

	3.	Install the driver (often with --silent):

```bash
sudo ./NVIDIA-Linux-x86_64-<version>.run --silent
```

	4.	(Optional) Reboot to load the kernel modules:

```bash
sudo reboot
```

	5.	SSH back in and verify:

```bash
ssh -i path/to/key.pem ec2-user@<public-ip>
nvidia-smi
```

If nvidia-smi works and shows a T4 GPU, the driver is correctly installed.

⸻

5. Prepare /mnt/data (Data & Cache Volume)

We want all heavy data (models, datasets, caches) on a data volume, not on the root disk.

5.1 Identify the Data Disk

Check the block devices:

```bash
sudo lsblk
```

You’ll typically see:
	•	nvme0n1 – root disk
	•	nvme1n1 – instance store or extra EBS disk

Use the one that is not mounted as /. Do not format the root disk.

5.2 Create a Filesystem

Example assuming /dev/nvme1n1 is your data disk:

```bash
sudo mkfs.ext4 -E nodiscard /dev/nvme1n1
```

5.3 Create and Mount /mnt/data

```bash
sudo mkdir -p /mnt/data
sudo chown -R ec2-user:ec2-user /mnt/data

sudo mount /dev/nvme1n1 /mnt/data
df -h /mnt/data
```

5.4 Persist the Mount (fstab)

Get the disk UUID:

```bash
sudo blkid /dev/nvme1n1
```

Add an entry to /etc/fstab (replace with your UUID):

```bash
echo 'UUID=<YOUR-UUID>  /mnt/data  ext4  defaults,nofail,_netdev  0  2' \
  | sudo tee -a /etc/fstab
```

Test:

```bash
sudo umount /mnt/data
sudo mount -a
findmnt /mnt/data
```

If you prefer legacy paths like /mount/data, add a symlink:

```bash
sudo ln -s /mnt/data /mount/data
```

⸻

6. Move Caches off the Root Disk

Hugging Face and Python tooling can easily fill the root disk if you leave caches under ~/.cache.

We’ll point everything at /mnt/data.

6.1 Create Cache Directories

```bash
mkdir -p /mnt/data/{hf/{datasets,transformers,hub},uv,pip,tmp}
sudo chown -R ec2-user:ec2-user /mnt/data
```

6.2 Export Environment Variables

Append to ~/.bashrc:

```bash
cat >> ~/.bashrc <<'EOF'
# Hugging Face caches
export HF_HOME=/mnt/data/hf
export HF_DATASETS_CACHE=/mnt/data/hf/datasets
export TRANSFORMERS_CACHE=/mnt/data/hf/transformers
export HUGGINGFACE_HUB_CACHE=/mnt/data/hf/hub

# uv cache
export UV_CACHE_DIR=/mnt/data/uv

# pip cache
export PIP_CACHE_DIR=/mnt/data/pip

# temp dir
export TMPDIR=/mnt/data/tmp
EOF

source ~/.bashrc
```

UV_CACHE_DIR is the standard env var for configuring uv’s cache location.  ￼

If you later run the worker under systemd, copy these environment variables into the unit via systemctl edit hypha-worker.

⸻

7. Install uv and Hypha

7.1 Install uv

Use the official installer:  ￼

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh

echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc

uv --version

uv will manage Python environments and automatically install Python versions as needed.  ￼
```

7.2 Install Hypha Binaries

Install Hypha from the release installer (adapt <VERSION>):

```bash
curl -fsSL https://github.com/hypha-space/hypha/releases/download/v<VERSION>/install.sh | sh
```

Verify:

```bash
hypha-worker --help
```

See the hypha-worker CLI reference￼ for full options.

⸻

8. Add Worker Certificates

Hypha uses mutual TLS; this worker needs its own node certificate and key, plus the CA bundle. See Security￼ for the PKI layout.

For a test environment you can use hypha-certutil to issue a node cert; in production this will usually come from your org’s CA.

On the worker, place files like:
	•	/etc/hypha/certs/worker-gpu-1-cert.pem
	•	/etc/hypha/certs/worker-gpu-1-key.pem
	•	/etc/hypha/certs/ca-bundle.pem  (trust chain)

Protect the key:

```bash
sudo chown root:root /etc/hypha/certs/*-key.pem
sudo chmod 600 /etc/hypha/certs/*-key.pem
```

You’ll point the worker config at these paths.

⸻

9. Generate the Worker Config

We’ll generate a base config with hypha-worker init and then edit it.

9.1 Run hypha-worker init

Create a working directory:

```bash
mkdir -p ~/hypha && cd ~/hypha
```

Pick your gateway address:
	•	For internal cluster traffic, prefer private IP, e.g.:

/ip4/<GATEWAY_PRIVATE_IP>/tcp/8080



Run:

```bash
hypha-worker init \
  -n worker-gpu-1 \
  -o worker-gpu-1.toml \
  --exclude-cidr 192.0.2.0/24 \
  --gateway /ip4/<GATEWAY_PRIVATE_IP>/tcp/8080
```

This creates worker-gpu-1.toml with reasonable defaults (see Worker Node￼ for structure).

9.2 Edit the Config

Open worker-gpu-1.toml in your editor.

9.2.1 Certificates
Set certificate paths:

```toml
cert_pem  = "/etc/hypha/certs/worker-gpu-1-cert.pem"
key_pem   = "/etc/hypha/certs/worker-gpu-1-key.pem"
trust_pem = "/etc/hypha/certs/ca-bundle.pem"
# Optional: CRLs if you use them
# crls_pem = "/etc/hypha/certs/crl.pem"
```

9.2.2 Working Directory
Point the worker at /mnt/data/work so all per-job directories and artifacts land on the data volume:

```toml
work_dir = "/mnt/data/work"
```

Create it:

```bash
mkdir -p /mnt/data/work
```

9.2.3 Resources
Advertise the resources of your instance. Example for g4dn.xlarge (adjust as needed):

```toml
[resources]
cpu     = 4     # vCPUs
memory  = 16    # GB RAM
storage = 200   # GB available on /mnt/data
gpu     = 16    # GB VRAM on the T4
```

These are advisory, used by the scheduler to match jobs to workers; they are not hard OS limits.

9.2.4 Network Settings (optional)
By default, workers initiate outbound connections to the gateway and rarely need inbound from the public internet.

If you want to pin a specific listen port:

```toml
listen_addresses = [
  "/ip4/0.0.0.0/tcp/9091",
  "/ip4/0.0.0.0/udp/9091/quic-v1",
]
```

Make sure your security group allows this port from the gateway (or cluster SG).

9.2.5 Executor for DiLoCo (Accelerate)
Add an executor for DiLoCo training using uv + Accelerate, similar to the example in the Worker docs:

```toml
[[executors]]
class   = "train"
name    = "diloco-transformer"
runtime = "process"
cmd     = "uv"
args = [
    "run",
    "--python", "3.12",
    "--no-project",
    "--with", "https://github.com/hypha-space/hypha/releases/download/v<VERSION>/hypha_accelerate_executor-<WHEEL_VERSION>-py3-none-any.whl",
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

Make sure /etc/hypha/accelerate.yaml exists with a configuration that matches your single-GPU setup (see the training docs for an example).

⸻

10. Quick Connectivity Check

Before running the worker for real, use the probe subcommand to verify mTLS and connectivity to the gateway:

```bash
hypha-worker probe \
  -c worker-gpu-1.toml \
  /ip4/<GATEWAY_PRIVATE_IP>/tcp/8080/
```

On success, it exits with status 0. If you get errors, check:
	•	certificate paths
	•	gateway address (private vs public)
	•	security group rules
	•	driver / network configuration

⸻

11. Start the Worker

Finally, start the worker:

```bash
hypha-worker run -c worker-gpu-1.toml
```

You should see logs indicating:
	•	successful TLS handshake with the gateway
	•	discovery and DHT join
	•	resource advertisement

Once a scheduler assigns training jobs, this worker will:
	•	bid for leases,
	•	start DiLoCo training via the diloco-transformer executor,
	•	stream metrics and status back via the Job Bridge.

From here, you can follow the DiLoCo Training tutorial to run a full training job that targets this GPU worker.

⸻

12. Where to Go Next
	•	Overall deployment: see Deploying Hypha – Overview & Planning Guide.
	•	Scheduler and job setup: see Scheduler￼ and DiLoCo Training￼.
	•	Adding more workers (extra GPUs, other regions): repeat this guide with different instance types and resource values.
	•	Hardening:
	•	move to systemd units for long-running workers
	•	tighten security groups
	•	integrate cert rotation via your config management.

If you’d like a sister doc for “GPU worker on AWS but with multiple GPUs & Accelerate multi-device” we can spin that out as a separate page that deep-dives into `accelerate.yaml`, sharding and resource advertising.
