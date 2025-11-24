+++
title = "Deploying Hypha"
description = "Hypha documentation."
template = "index.html"
+++

# Deploying Hypha – Overview & Planning Guide

This guide is aimed at DevOps and platform engineers who need to run Hypha across multiple machines (cloud, on-prem, or hybrid). It highlights the architectural decisions, references detailed component docs, and points you to deeper deployment guides such as [Deploying a small AWS GPU Worker](aws_worker.md). If you only want to experiment on a single machine, start with the [Quick Start Guide](../quickstart.md).

## 1. Define the goal

Before creating instances or opening ports, document what you are trying to accomplish:

- Are you running a short-lived DiLoCo proof of concept or a cluster that must tolerate days of training without manual intervention?
- Are nodes co-located inside a single VPC, or do you need to span regions, private networks, or the public internet?
- Do you control the PKI and automation stack, or do you need to interoperate with an existing security team and config-management system?

These answers determine how many machines you need, which roles you can colocate, how aggressive the firewall rules must be, and whether you can rely on `hypha-certutil` for certificates.

## 2. Understand the components

A DiLoCo deployment uses four main services:

- [**Gateway**](../gateway.md) — stable entry point, DHT anchor, libp2p relay.
- [**Scheduler**](../scheduler.md) — publishes requirements, leases resources, orchestrates DiLoCo rounds.
- [**Workers**](../worker.md) — run training executors or parameter servers and bid on jobs.
- [**Data nodes**](../data.md) — announce datasets and stream SafeTensors slices.

Small clusters often colocate roles (for example, run the scheduler and parameter server on the same host as the gateway). Larger deployments dedicate machines to each role and add multiple gateways or data nodes for redundancy.

> [!TIP]
> Hypha does not require a one-to-one mapping between “node” and “machine.” You can run multiple Hypha daemons on a single host as long as you provision enough CPU, memory, and disk. Resource numbers in configs are advisory—they guide the scheduler but do not enforce OS-level limits.

## 3. Certificates and trust model

Hypha uses mutual TLS everywhere. Every process presents an Ed25519-backed certificate that chains to a trusted CA bundle. Follow the hierarchy described in [Security](../security.md):

1. Offline **root CA**
2. Per-tenant **intermediate/organization CA**
3. Individual **node certificates** for gateways, schedulers, workers, and data nodes

`hypha-certutil` is acceptable for local testing, but production deployments should integrate with your corporate PKI and CRL/OCSP processes. Plan how you will distribute and rotate certificates (manual copy for pilots, Ansible/Salt for staging, automated secret delivery for production). Never reuse the same certificate across multiple nodes because PeerIDs are derived from the leaf key.

## 4. Install binaries and tooling

Each machine typically needs:

1. The Hypha CLI suite (`hypha-*`) — install via the methods in the [Installation](../installation.md) guide.
2. `uv` plus any executor-specific dependencies (only required on workers running Python-based executors like the Accelerate-based DiLoCo runner).

Automate both steps via cloud-init, Ansible, or your preferred provisioning system so new workers can join the network quickly.

## 5. Networking and gateway configuration

Gateways must be directly reachable by every node. Configure them with accurate listen/external addresses and CIDR filters so the DHT never advertises unusable IP ranges.

**Listen addresses** — prefer binding to specific interfaces:

```
/ip4/10.0.1.100/tcp/55000
/ip4/10.0.1.100/udp/55001/quic-v1
```

Use `/ip4/0.0.0.0/...` only when infrastructure-as-code cannot predict the private IP.

**External addresses** — advertise the address other peers should dial (`/ip4/203.0.113.10/tcp/55000` for public gateways, `/ip4/10.0.1.100/tcp/55000` for intra-VPC deployments, or `/dns4/gateway.example.com/tcp/55000`).

**exclude_cidr** — filter out unroutable ranges (loopback, link-local, internal ranges you do not want propagated). Without these filters, peers may announce `127.0.0.1`, causing other nodes to dial themselves.

Open both the TCP and UDP ports you configure, since Hypha can operate over TCP or QUIC. Workers, schedulers, and data nodes usually need only outbound connectivity plus SSH for administration.

> [!TIP]
> Enable QUIC (UDP) whenever possible. It offers better performance on high-latency links and significantly improves connection success rates through restrictive firewalls. If you are forced to run TCP-only, explicitly open each worker’s listening port in your security groups.

## 6. Capacity planning

- **Gateway** — 1–2 vCPUs, 1–2 GB RAM, minimal disk. Focus on reliable network connectivity.
- **Scheduler** — CPU-only instance with 2–4 vCPUs and 4 GB RAM is sufficient for most jobs. Co-locate the parameter server only if you have spare memory.
- **Parameter server worker** — allocate enough RAM to hold model weights plus optimizer buffers (2× a single layer is a good baseline) and provide fast local SSD for checkpoint staging.
- **Training workers** — size primarily for GPU VRAM. Plan for root disk ≥50 GB plus a dedicated data volume for Hugging Face caches, `uv` environments, and Hypha work directories. For multi-GPU nodes, ensure PCIe bandwidth and cooling are adequate.
- **Data nodes** — prioritize disk capacity and network throughput. Provision storage equal to the datasets you need plus future growth, and place them close (network-wise) to the workers they will serve.

Document these choices so you can justify cloud spend and replicate the environment later.

## 7. Generate and edit configs

Use the `init` subcommands to scaffold configs, then edit them to match your topology:

- `hypha-gateway init -n gateway-a -o /etc/hypha/gateway.toml`
- `hypha-scheduler init ... --gateway /dns4/gateway.example.com/tcp/55000`
- `hypha-worker init ... --work-dir /mnt/data/work`
- `hypha-data init ... --dataset-path /var/hypha/datasets/imagenet`

Wire in certificate paths, adjust `listen_addresses`/`external_addresses`, and define scheduler job specs and worker executors. Keep configs in version control where possible so infrastructure changes are auditable.

## 8. Bring the cluster online

Start services in this order, verifying each step before moving on:

1. **Gateway** — `hypha-gateway run -c gateway.toml`
2. **Data node(s)** — ensures schedulers can resolve datasets.
3. **Workers** — `hypha-worker probe` before `run` to confirm mTLS and connectivity.
4. **Scheduler** — launches jobs once enough workers have leases.

Probe commands (`hypha-worker probe <gateway>` or `hypha-data probe <gateway>`) reuse the same certificates and addresses as production services and act as smoke tests. During training, watch scheduler logs for `Job is completed` plus data handler shutdown messages.

## 9. Hardening and operations

- **Telemetry** — configure OpenTelemetry on every node and forward metrics to Grafana, Honeycomb, etc. (see each component doc for examples).
- **Secrets** — rotate certificates regularly and publish CRLs referenced by every node.
- **Automation** — run Hypha binaries under systemd or another supervisor, and template configs via your CM tool. The [AWS GPU Worker guide](aws_worker.md) includes system prep patterns (cache directories, driver installation) you can reuse elsewhere.
- **Incident response** — document troubleshooting steps for mTLS failures, DHT discovery problems, and dataset availability. Start with the symptom tables in [Security](../security.md) and [Data Node](../data.md).

---

Next, follow a platform-specific runbook such as [Deploying a small Hypha GPU Worker on AWS](aws_worker.md) or create your own by cloning this template and linking back to the relevant component references.
