# Production Deployment Guide

Hypha is a decentralized machine learning system designed for production use across heterogeneous infrastructure. This guide provides deployment recommendations, resource requirements, network configuration, and operational best practices for running Hypha components in production environments.

## Deployment Architecture

Hypha consists of four primary components that work together in a decentralized P2P network.

- **Gateway**: Deploy on a server with a public IP address and stable connectivity for network entry and peer discovery.
- **Scheduler**: Runs temporarily on the ML engineer's machine or a compute node during job execution.
- **Workers**: Deploy on dedicated GPU hosts for GPU enabled workers. Can be behind NAT/firewalls using relay circuits through the gateway for initial discovery.
- **Data Nodes**: Deploy on servers with sufficient storage and network bandwidth to serve dataset slices directly to workers.

For detailed architectural information, including component responsibilities, protocols, and communication patterns, see the [Architecture Overview](architecture.md).

## Network Configuration

Hypha's P2P network operates on ephemeral port ranges by default, with only the Gateway requiring publicly accessible inbound ports. All port assignments are fully configurable via each node's configuration file to accommodate existing network policies and firewall rules.

### Port Requirements

**Gateway Node**:

```
TCP 49152 to 65535  - Hypha P2P protocol (must be publicly accessible)
UDP 49152 to 65535  - Hypha P2P protocol with QUIC (must be publicly accessible)
TCP 22    - SSH administration (restrict to admin IPs)
```

**Worker and Data Nodes**:

```
TCP 0     - Dynamic port assignment by OS (outbound connections only)
UDP 0     - Dynamic port assignment by OS (outbound connections only)
TCP 22    - SSH administration (restrict to admin IPs)
```

**Scheduler** (when running on a server):

```
TCP 0     - Dynamic port assignment by OS (outbound connections only)
UDP 0     - Dynamic port assignment by OS (outbound connections only)
TCP 22    - SSH administration (restrict to admin IPs)
```

### Network Topology

**Private Networking**: Use private networks between nodes in the same datacenter/region when available. This reduces bandwidth costs and improves performance.

**Cross-Region Communication**: Nodes in different regions/providers attempt direct peer-to-peer connections first, automatically falling back to the gateway's relay functionality if direct communication is not possible. Ensure the gateway has sufficient bandwidth for relay traffic.

**NAT Traversal**: Workers and data nodes behind NAT/firewalls automatically use relay circuits through the gateway. No inbound port forwarding required.

**IPv4 and IPv6**: Configure both IPv4 and IPv6 addresses on the gateway when available for broader accessibility.

## Component Setup

### Gateway Node

Gateway is primarily I/O bound with minimal compute requirements. Focus on network reliability and bandwidth.

**Resource Requirements**:

- **CPU**: 2-4 vCPUs
- **Memory**: 2-4 GB RAM
- **Storage**: 10-20 GB (primarily for logs and certificates)
- **Network**: Public IP address required, 1+ Gbps bandwidth recommended
- **Uptime**: High availability crucial (99.9%+ recommended)

**Configuration Details**:

For complete gateway configuration options, including environment variables, command-line flags, advanced networking settings, and troubleshooting, see the [Gateway documentation](gateway.md).

### Scheduler Process

Scheduler resource requirements depend on the number of workers coordinated and job complexity. For 50+ workers, consider 8+ vCPUs and 16+ GB RAM. Focus on network reliability and bandwidth.

**Resource Requirements**:

- **CPU**: 4-8 vCPUs (scales with worker count)
- **Memory**: 8-16 GB RAM (job state, worker tracking)
- **Storage**: 20-50 GB (job artifacts, metrics, checkpoints)
- **Network**: Stable connectivity to gateway required
- **Uptime**: Must remain available during active jobs. If the scheduler crashes or becomes unavailable, jobs cannot complete and must be restarted from the beginning.

**Configuration Details**:

For complete scheduler configuration options, including environment variables, command-line flags, job submission parameters, and troubleshooting, see the [Scheduler documentation](scheduler.md).

### Worker Nodes

Worker specifications should match the intended workload. Training large language models requires significantly more resources than inference or fine-tuning smaller models.

**Resource Requirements** (varies significantly by workload):

- **CPU**: 8+ vCPUs (16+ recommended for GPU workloads)
- **Memory**: 32+ GB RAM (64+ GB recommended for large models)
- **GPU**: Varies by workload
  - Entry-level: 16-24 GB VRAM (e.g., RTX 4060 Ti, RTX 4090, AMD RX 7900 XTX)
  - Mid-range: 24-48 GB VRAM (e.g., RTX 4090, AMD Instinct MI210)
  - High-end: 80+ GB VRAM (e.g., NVIDIA A100, H100, AMD MI300X, Intel Data Center GPU Max)
- **Storage**: 100+ GB fast storage (NVMe SSD recommended)
- **Network**: High bandwidth recommended (10+ Gbps for large-scale training)

**Configuration Details**:

For complete worker configuration options, including environment variables, command-line flags, executor setup, resource limits, and troubleshooting, see the [Worker documentation](worker.md).

### Data Node

Data nodes benefit from fast storage (NVMe) and high network bandwidth to serve dataset slices efficiently to multiple workers simultaneously.

**Resource Requirements**:

- **CPU**: 4-8 vCPUs
- **Memory**: 8-16 GB RAM
- **Storage**: Varies by dataset size (100 GB to multiple TB)
- **Network**: High bandwidth crucial (10+ Gbps for serving multiple workers)

**Configuration Details**:

For complete data node configuration options, including environment variables, command-line flags, storage management, dataset serving, and troubleshooting, see the [Data Node documentation](data-node.md).

## Certificate Management

For production deployments, obtain certificates from a proper Certificate Authority (CA). Hypha uses mutual TLS (mTLS) for authentication and requires a proper certificate hierarchy.

For complete details on certificate requirements, PKI infrastructure, certificate generation, rotation, and revocation, see the [Security documentation](security.md).

**Key Points for Production**:

- Use certificates from an established CA (not `hypha-certutil`, which is for development only)
- Each node needs: its own certificate, corresponding private key, and trust bundle
- Plan for certificate rotation before expiration (90 days to 1 year validity recommended)
- Maintain Certificate Revocation Lists (CRLs) for compromised or decommissioned nodes

## Operational Considerations

### Service Management

For long-running nodes (gateway, workers, data nodes), consider:

- **Process supervision**: Use systemd, supervisord, or similar
- **Automatic restarts**: Configure restart policies for failures
- **Graceful shutdowns**: Ensure clean shutdown procedures
- **Log management**: Rotate logs to prevent disk exhaustion

### Monitoring and Observability

Plan for operational visibility:

- **Metrics collection**: CPU, memory, GPU, network utilization
- **Log aggregation**: Centralized logging for debugging
- **Alerting**: Notify operators of failures or degradation
- **Health checks**: Regular validation of node connectivity
- **Certificate monitoring**: Alert on upcoming expirations

### Backup and Recovery

Establish backup procedures for:

- **Certificates and keys**: Secure offline backups
- **Configuration files**: Version control recommended
- **Job artifacts**: Based on ML workflow requirements
- **Recovery procedures**: Document and test regularly

### Security Hardening

Beyond certificate-based authentication:

- **Network segmentation**: Isolate production networks
- **SSH hardening**: Key-based auth, non-standard ports
- **OS updates**: Regular security patching
- **Audit logging**: Track administrative actions
- **Least privilege**: Minimize permissions for services

### Scaling Procedures

Plan for capacity changes:

- **Adding workers**: Generate certificates, configure, connect to gateway
- **Removing workers**: Graceful shutdown, certificate revocation
- **Regional expansion**: Deploy additional workers in new regions
- **Gateway migration**: Plan for gateway IP changes (affects all nodes)

### Cost Management

Consider operational costs:

- **Bandwidth**: Monitor gateway relay traffic
- **GPU utilization**: Track job efficiency
- **Storage**: Clean up old job artifacts
- **Power**: GPU power consumption for budgeting

This production deployment guide provides the foundation for running Hypha in production environments. Adapt these recommendations to your specific infrastructure, security requirements, and operational procedures.
