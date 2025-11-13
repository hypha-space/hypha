# Hypha

Hypha is a self-managing "Kubernetes for AI" designed for distributed machine learning training and inference. Built on libp2p for decentralized networking, Hypha enables organizations to train and serve massive models across heterogeneous, poorly-connected infrastructure—from HPC GPU farms to commodity hardware—without requiring centralized coordination.

The system implements DiLoCo (Distributed Low-Communication) style training, reducing network communication by approximately 500x compared to traditional data-parallel approaches. With automatic resource discovery, workload distribution, and fault tolerance, Hypha maintains enterprise-grade security and reliability while eliminating single points of failure.

## Installation

Install Hypha using the standalone installer script:

```sh
curl -fsSL https://github.com/hypha-space/hypha/releases/download/v<VERSION>/install.sh | sh
```

For alternative installation methods (GitHub releases, Cargo), see the [Installation Guide](docs/installation.md).

## Getting Started

Follow the [Quick Start Guide](docs/quickstart.md) to set up your first end-to-end decentralized training system.

## Goals

### Democratize Large-Scale ML

Make cutting-edge machine learning accessible to organizations by efficiently utilizing heterogeneous compute resources.

### Minimize Operational Complexity

Develop a _self_-managing system that automatically handles resource discovery, workload distribution, fault tolerance, and scaling with minimal configuration requirements and administrative overhead.

### Power Production Applications

Provide a reliable, high-performance inference backbone to support real-world ML applications with the scale, latency, and reliability requirements of production systems.

### Real-World Ready

Build a secure, maintainable, and observable system that meets enterprise requirements for encryption, resilience, repeatable deployment, and comprehensive logging.

## Components

<!-- TODO: Add short overview describing the main components purpose . -->

## Contributing

Want to help improve Hypha and its capabilities for distributed training and inference? We encourage contributions of all kinds, from bug fixes and feature enhancements to documentation improvements. Hypha aims to provide a robust platform for efficient and scalable machine learning workflows, and your contributions can help make it even better. Consult [CONTRIBUTING.md](CONTRIBUTING.md) for detailed instructions on how to contribute effectively.
