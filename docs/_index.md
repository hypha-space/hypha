+++
title = "Hypha"
sort_by = "weight"

[extra]
subtitle = "A self-managing, enterprise-ready ML system that democratizes access to large-scale machine learning"

[[extra.features]]
title = "Democratize Large-Scale ML"
description = "Make cutting-edge machine learning accessible to organizations by efficiently utilizing heterogeneous compute resources."

[[extra.features]]
title = "Minimize Operational Complexity"
description = "Self-managing system that automatically handles resource discovery, workload distribution, fault tolerance, and scaling."

[[extra.features]]
title = "Power Production Applications"
description = "Reliable, high-performance inference backbone supporting real-world ML applications with enterprise-grade requirements."

[[extra.features]]
title = "Real-World Ready"
description = "Secure, maintainable, and observable system meeting enterprise requirements for encryption, resilience, and compliance."

[[extra.features]]
title = "Heterogeneous Support"
description = "Compatible with diverse hardware setups commonly found in enterprises and research institutions."

[[extra.features]]
title = "Performant & Secure"
description = "Efficient and optimized capabilities without compromising security for performance gains."
+++

## Welcome to Hypha

Hypha is a distributed machine learning infrastructure designed to democratize access to large-scale ML by efficiently orchestrating heterogeneous compute resources across decentralized environments.

### Quick Start

Get started with Hypha in minutes:

```bash
# Clone the repository
git clone https://github.com/hypha-space/hypha.git
cd hypha

# Build the project
cargo build --release

# Run a gateway node
cargo run --bin hypha-gateway

# Run a worker node (in another terminal)
cargo run --bin hypha-worker
```

For detailed setup instructions, see the [Getting Started Guide](/getting-started/).

### Key Components

- **[Network](/architecture/network/)**: P2P networking foundation using libp2p
- **[Gateway](/architecture/gateway/)**: Stable entry point and relay for network peers
- **[Scheduler](/architecture/scheduler/)**: Discovers workers and dispatches ML tasks
- **[Worker](/architecture/worker/)**: Advertises capabilities and executes assigned tasks

### Learn More

- [Architecture Overview](/architecture/) - Understand how Hypha works
- [Deployment Guide](/deployment/) - Deploy Hypha in your environment
- [Contributing](/contributing/) - Help improve Hypha

### Community

Join the Hypha community:

- [GitHub](https://github.com/hypha-space/hypha) - Source code and issue tracking
- [Contributing Guide](/contributing/) - How to contribute to the project
