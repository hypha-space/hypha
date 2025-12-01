# Hypha

Hypha is a self-managing "Kubernetes for AI" (but simpler) for distributed machine learning. Train and serve models across heterogeneous infrastructure—from GPU farms to _commodity_ hardware.

[Get started in minutes following the quick start guide.](https://hypha-space.org/quickstart/)

Built on the battle-tested libp2p network stack with additional security features, Hypha maintains high security and reliability while making it simple to set up. The system implements DiLoCo (Distributed Low-Communication) style training, an approach that dramatically reduces communication overhead compared to traditional data-parallel training making it feasable to train across data centers.

## Key Features

- **Distributed Training** — Run DiLoCo-style training across workers with infrequent synchronization, ideal for bandwidth-constrained or geographically distributed setups. [Learn more →](docs/content/training.md)
- **Production Inference (in development)** — The same decentralized architecture supports scalable, resilient inference serving with automatic load balancing.
- **Security** — End-to-end encryption via mTLS, certificate revocation for immediate access control, and a permissioned network model. [Security guide →](docs/content/security.md)

## Installation

Install Hypha using the standalone installer script:

```sh
curl -LsSf https://hypha-space.org/install.sh | sh
```

For alternative installation methods (GitHub releases, Cargo), see the [Installation Guide](docs/content/installation.md).

## Next Steps

New to Hypha? Start with the Quick Start to get a local cluster running in minutes, then explore the architecture and deployment guides to get into production.

- **[Quick Start](https://hypha-space.org/quickstart/)** — Set up a local cluster and run your first training job
- **[Architecture](https://hypha-space.org/architecture/)** — How Gateways, Schedulers, Workers, and Data Nodes fit together
- **[Deployment](https://hypha-space.org/deploy/)** — Deploy Hypha on cloud infrastructure

## Contributing

Want to help improve Hypha and its capabilities for distributed training and inference? We encourage contributions of all kinds, from bug fixes and feature enhancements to documentation improvements. Hypha aims to provide a robust platform for efficient and scalable machine learning workflows, and your contributions can help make it even better. Consult [CONTRIBUTING.md](CONTRIBUTING.md) for detailed instructions on how to contribute effectively.
