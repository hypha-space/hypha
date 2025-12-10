+++
title = "AWS CPU Worker (Parameter Server)"
description = "Guide for deploying a lightweight CPU worker for Parameter Server tasks on AWS"
[taxonomies]
track = ["onboarding"]
+++

# Deploying a Hypha CPU Worker on AWS

This guide walks you through deploying a **CPU-only worker** on AWS EC2, specifically optimized to run as a **Parameter Server**.

**Goal:** A lightweight worker (t2.small) for aggregating and storing model updates in a Hypha DiLoCo setup.

**Prerequisites:**
- A running **Gateway** reachable by the worker.
- A **Node Certificate** and Key for the worker (see [Security](@/security.md)).
- An [aws](https://aws.amazon.com) account.

## 1. Infrastructure Specification

Provision an EC2 instance with the following specifications. For detailed steps, refer to the [AWS User Guide: Launching an Instance](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/launching-instance.html).

| Component | Specification | Rationale |
|---|---|---|
| AMI | Amazon Linux 2023 (x86_64) | Standard, stable, optimized for EC2. |
| Instance | t2.small | Sufficient for aggregating layer-wise updates (~2GB memory). |
| Storage | Root: 50GB+ (EBS) | Parameter servers store results from all workers and multiple iterations. |
| Network | Security Group:<br/>- Inbound: SSH (22)<br/>- Outbound: All<br/><br/>Optional: Inbound TCP/UDP | Worker initiates connections to the Gateway. |

> [!NOTE]
> A `t2.small` instance provides 2GB of RAM, which should be sufficient for most smaller models as the implementation uses a memory-efficient algorithm to aggregate results layer-wise (requiring memory for approx. 2 layers + overhead).

## 2. Install & Configure Hypha

Connect to your instance via SSH. For instructions, see [AWS Guide: Connect to your Linux instance](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/connecting_to_your_instance.html) or the information provided by when clicking "Connect" in the AWS Management Console.

### 3.1 Install Hypha

You can find detailed instructions in the [installation guide](@/installation.md), or use the following command to install the latest version:

```bash
curl -LsSf https://hypha-space.org/install.sh | sh
```

### 3.2 Setup Configuration

Begin by placing your worker's node certificates. Upload your `cert.pem`, `key.pem`, and `ca.pem` to `/etc/hypha/certs/` on the instance and secure them with appropriate permissions (`chmod 600` for private keys).

Next, initialize a base configuration file using `hypha-worker init`. Ensure you specify the worker's name, the gateway address, and the designated work directory on your data volume:

```bash
hypha-worker init \
  -n param-server-1 \
  --gateway <GATEWAY_MULTIADDR> \
  --work-dir /mnt/data/work
```

After generating the configuration, you will need to edit the `config.toml` file to fine-tune it. Critically, update the `cert_pem`, `key_pem`, and `trust_pem` paths to point to the certificate files you placed earlier. Additionally, it's important to adjust the `[resources]` section to accurately reflect your instance's capabilities, such as for a `t2.small`:

```toml
[resources]
cpu = 1
memory = 2    # GB
gpu = 0       # No GPU
storage = 50  # GB (Data Volume)
```

The `init` command added a default _diloco-transformer_ `[[executors]]` block. Since this is a non-training node, **remove** or comment out the entire section related to executor to prevent unnecessary offer matches.

## 4. Observability (Optional)

Hypha supports OpenTelemetry (OTEL) for metrics and tracing. You can export telemetry to any OTLP-compatible backend, such as Grafana Cloud.

To configure this, set the following environment variables:

```bash
# Get these values from your Grafana Cloud "OpenTelemetry" details page
export OTEL_EXPORTER_OTLP_ENDPOINT="https://otlp-gateway-prod-eu-west-2.grafana.net/otlp"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Basic <API_TOKEN>"
```

## 5. Start

With everything configured, you can start the worker:

```bash
hypha-worker run -c config.toml
```
