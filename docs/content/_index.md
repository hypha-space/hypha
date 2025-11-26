+++
title = "Hypha"
description = "High-level overview of Hypha's goals, capabilities, and primary use cases for decentralized ML."
template = "index.html"
+++

# Hypha

Hypha is a self-managing "Kubernetes for AI" (but simpler) for distributed machine learning. Train and serve models across heterogeneous infrastructure—from GPU farms to _commodity_ hardware.

[Get started in minutes following the quick start guide.](@/quickstart.md)

Built on the battle-tested libp2p network stack with additional security features, Hypha maintains high security and reliability while making it simple to set up. The system implements DiLoCo (Distributed Low-Communication) style training, an approach that dramatically reduces communication overhead compared to traditional data-parallel training making it feasable to train across data centers.

## Key Features

- **Distributed Training** — Run DiLoCo-style training across workers with infrequent synchronization, ideal for bandwidth-constrained or geographically distributed setups. [Learn more →](@/training.md)
- **Production Inference (in development)** — The same decentralized architecture supports scalable, resilient inference serving with automatic load balancing.
- **Security** — End-to-end encryption via mTLS, certificate revocation for immediate access control, and a permissioned network model. [Security guide →](@/security.md)

---

## Next Steps

New to Hypha? Start with the Quick Start to get a local cluster running in minutes, then explore the architecture and deployment guides to get into production.

- **[Quick Start](@/quickstart.md)** — Set up a local cluster and run your first training job
- **[Architecture](@/architecture.md)** — How Gateways, Schedulers, Workers, and Data Nodes fit together
- **[Deployment](@/deploy/_index.md)** — Deploy Hypha on cloud infrastructure

---
