+++
title = "AWS Gateway"
description = "Guide for deploying a Hypha Gateway on AWS"
[taxonomies]
track = ["onboarding"]
+++

# Deploying a Hypha Gateway on AWS

This guide walks you through deploying a **Hypha Gateway** on AWS EC2.

**Goal:** An entry point for your Hypha network using a lightweight `t3.micro` instance.

**Prerequisites:**
- A **Node Certificate** and Key for the gateway (see [Security](@/security.md)).
- A static public IP (AWS Elastic IP) is strongly recommended to keep the entry point stable.
- An [aws](https://aws.amazon.com) account.

## 1. Infrastructure Specification

Provision an EC2 instance with the following specifications. For detailed steps, refer to the [AWS User Guide: Launching an Instance](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/launching-instance.html).

| Component | Specification | Rationale |
|---|---|---|
| AMI | Amazon Linux 2023 (x86_64) | Standard, stable, optimized for EC2. |
| Instance | t3.micro | Sufficient for routing traffic; the Gateway is I/O bound, not CPU bound. |
| Storage | Root: 8GB+ | Minimal storage required; Gateway does not store models or data. |
| Network | Security Group:<br/>- Inbound: SSH (22)<br/>- Inbound: TCP 55000<br/>- Inbound: UDP 55001<br/>- Outbound: All | The Gateway must be reachable on specific ports from the public internet. |

> [!IMPORTANT]
> **No Proxies or Load Balancers:** The Gateway functions similarly to a STUN server to aid in automatic network discovery and NAT traversal. It requires a direct public IP address. Do not place it behind a load balancer (ALB/NLB) or reverse proxy, as this breaks peer identification.

## 2. Network Configuration

### 2.1 Elastic IP (Recommended)

Allocating an **Elastic IP** (EIP) and associating it with your instance is strongly recommended for production. This ensures your Gateway's public address (`/ip4/203.0.113.10/...`) remains static even if the instance stops or reboots. 

For test clusters, the default "Auto-assign public IP" is sufficient, but be aware that this IP will change if the instance is stopped, requiring configuration updates on all peers.

See **[AWS Guide: Elastic IP addresses](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/elastic-ip-addresses-eip.html)**.

### 2.2 Security Groups

The Gateway requires specific inbound ports to be open to the world to function as the network anchor. See **[AWS Guide: Work with security groups](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/working-with-security-groups.html)** for details on how to configure security groups.

1.  Navigate to the **Security Groups** section in the EC2 Console.
2.  Create a new Security Group or edit the one assigned to your instance.
3.  Add the following **Inbound Rules**:

| Type | Protocol | Port Range | Source | Description |
|---|---|---|---|---|
| SSH | TCP | 22 | My IP / Admin IP | SSH Access |
| Custom TCP | TCP | 55000 | 0.0.0.0/0 | Hypha TCP Transport |
| Custom UDP | UDP | 55001 | 0.0.0.0/0 | Hypha QUIC Transport |

## 3. Install & Configure Hypha

Connect to your instance via SSH. For instructions, see [AWS Guide: Connect to your Linux instance](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/connecting_to_your_instance.html) or the information provided by when clicking "Connect" in the AWS Management Console.

### 3.1 Install Hypha

Install the `hypha-gateway` binary using the official installer:

```bash
curl -LsSf https://hypha-space.org/install.sh | sh
```

### 3.2 Configuration

Begin by placing your gateway's node certificates. Upload your `cert.pem`, `key.pem`, and `ca.pem` to `/etc/hypha/certs/` on the instance and secure them (`chmod 600` for private keys).

Next, initialize a base configuration file:

```bash
hypha-gateway init \
  -n gateway-1 \
  --cert-path /etc/hypha/certs/cert.pem \
  --key-path /etc/hypha/certs/key.pem \
  --ca-path /etc/hypha/certs/ca.pem
```

You must edit `gateway.toml` to explicitly define how the Gateway listens and advertises itself.

Bind the gateway to your instance's private IP address (e.g., `10.0.0.5`). While binding to `0.0.0.0` works, restricting it to the specific network interface is a security best practice.

```toml
[networking]
listen_addresses = ["/ip4/10.0.0.5/tcp/55000", "/ip4/10.0.0.5/udp/55001/quic-v1"]
```

Next, configure the `external_addresses` to match your **Elastic IP** or "Auto-assign public IP" (e.g. 203.0.113.10). This is the address other peers use to dial your gateway. If this does not match your actual public IP, connection attempts may fail.

```toml
external_addresses = ["/ip4/203.0.113.10/tcp/55000", "/ip4/203.0.113.10/udp/55001/quic-v1"]
```

To prevent the gateway from advertising internal private IPs or link-local addresses to the DHT, a set of CIDRs is excluded by default. This ensures peers only attempt to connect via the reachable addresses. The defaults should work fine in most cases, but make sure to review and potentially adjusting them before deploying.

## 4. Observability (Optional)

Configure OpenTelemetry (OTEL) to monitor connection counts and bandwidth.

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://otlp-gateway-prod-eu-west-2.grafana.net/otlp"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Basic <API_TOKEN>"
```

## 5. Start

Start the gateway:

```bash
hypha-gateway run -c gateway.toml
```

Verify it is working by checking the logs for "Listening on..." and ensuring no errors regarding binding ports.
