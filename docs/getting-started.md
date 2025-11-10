+++
title = "Getting Started"
description = "Quick start guide for installing and running Hypha"
weight = 1
+++

# Getting Started with Hypha

This guide will help you get Hypha up and running on your system.

## Prerequisites

Before you begin, ensure you have the following installed:

- **Rust** (1.75 or later): [Install Rust](https://rustup.rs/)
- **Git**: For cloning the repository
- **Build essentials**: C compiler and other build tools

### System Requirements

- **Operating System**: Linux, macOS, or Windows (with WSL2)
- **Memory**: Minimum 2GB RAM (4GB+ recommended)
- **Network**: Internet connectivity for peer discovery

## Installation

### 1. Clone the Repository

```bash
git clone https://github.com/hypha-space/hypha.git
cd hypha
```

### 2. Build the Project

Build all components using Cargo:

```bash
cargo build --release
```

This will compile all Hypha components:
- `hypha-gateway`: Gateway node
- `hypha-scheduler`: Scheduler node
- `hypha-worker`: Worker node

Build artifacts will be located in `target/release/`.

### 3. Verify Installation

Check that the binaries were built successfully:

```bash
./target/release/hypha-gateway --version
./target/release/hypha-worker --version
./target/release/hypha-scheduler --version
```

## Running Hypha

### Starting a Gateway Node

Gateway nodes provide stable entry points for the network:

```bash
./target/release/hypha-gateway
```

The gateway will start and display its peer ID and listening addresses.

### Starting a Worker Node

Worker nodes execute ML tasks. In a new terminal:

```bash
./target/release/hypha-worker
```

The worker will:
1. Connect to the gateway
2. Advertise its capabilities
3. Wait for task assignments

### Starting a Scheduler Node

Scheduler nodes coordinate task distribution:

```bash
./target/release/hypha-scheduler
```

## Configuration

Hypha components can be configured using:

1. **Configuration files**: TOML files in the config directory
2. **Environment variables**: Prefixed with `HYPHA_`
3. **Command-line arguments**: Run with `--help` for options

### Example Configuration

Create a configuration file `config.toml`:

```toml
[network]
listen_addresses = ["/ip4/0.0.0.0/tcp/0"]
bootstrap_peers = []

[gateway]
relay_enabled = true

[worker]
max_concurrent_tasks = 4
```

Load the configuration:

```bash
./target/release/hypha-worker --config config.toml
```

## Next Steps

- [Architecture Overview](/architecture/) - Learn about Hypha's architecture
- [Deployment Guide](/deployment/) - Deploy to production
- [Contributing](/contributing/) - Contribute to the project

## Troubleshooting

### Common Issues

**Problem**: Build fails with dependency errors

**Solution**: Update Rust and try again:
```bash
rustup update
cargo clean
cargo build --release
```

**Problem**: Worker cannot connect to gateway

**Solution**: Check network connectivity and ensure the gateway is running and accessible.

**Problem**: Permission denied errors

**Solution**: Ensure you have write permissions for log directories and data storage.

## Getting Help

If you encounter issues:

1. Check the [GitHub Issues](https://github.com/hypha-space/hypha/issues)
2. Review the [Architecture Documentation](/architecture/)
3. Open a new issue with details about your problem
