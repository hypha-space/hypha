# Installation

This guide covers installing Hypha components on your systems. Hypha supports Linux and macOS platforms, with binaries available for common architectures.

## System Requirements

### Supported Platforms

- **Linux**: x86_64, aarch64 (tested on Ubuntu 20.04+, Debian 11+, RHEL 8+)
- **macOS**: x86_64 (Intel), aarch64 (Apple Silicon)

Windows support is not currently available.

### Runtime Requirements

**For Running Binaries**
- No additional runtime dependencies
- Binaries are statically linked and self-contained

**For Building from Source**
- Rust toolchain 1.75 or later
- Standard build tools (gcc, make, etc.)

**For Python Executors**
- Python 3.12 or later
- uv package manager (recommended)

### Hardware Requirements

Requirements vary by component and workload:

**Gateway Nodes**
- Minimal compute requirements
- Stable network connectivity essential
- Public IP address recommended

**Scheduler Nodes**
- Moderate CPU for coordination logic
- Memory proportional to worker pool size (typically 1-4 GB)

**Worker Nodes**
- CPU and memory depend on workload
- GPU support requires appropriate drivers
- Disk space for model weights and datasets

**Data Nodes**
- Storage proportional to dataset size
- Network bandwidth for serving data slices

## Installing from Pre-Built Binaries

Pre-built binaries are available from GitHub releases for quick deployment.

### Quick Install Script

The fastest way to install all Hypha binaries:

```bash
curl -fsSL https://github.com/hypha-space/hypha/releases/latest/download/install.sh | sh
```

This script:
1. Detects your platform and architecture
2. Downloads the latest release binaries
3. Installs them to `~/.local/bin`
4. Verifies installation

To install to a different location:

```bash
curl -fsSL https://github.com/hypha-space/hypha/releases/latest/download/install.sh | sh -s -- --prefix=/usr/local
```

### Manual Binary Installation

For more control, download specific binaries from the [releases page](https://github.com/hypha-space/hypha/releases).

**1. Download the binary for your platform:**

```bash
# Linux x86_64 example
wget https://github.com/hypha-space/hypha/releases/latest/download/hypha-scheduler-linux-x86_64

# macOS aarch64 (Apple Silicon) example
wget https://github.com/hypha-space/hypha/releases/latest/download/hypha-worker-darwin-aarch64
```

**2. Make it executable:**

```bash
chmod +x hypha-scheduler-linux-x86_64
```

**3. Move to your PATH:**

```bash
mv hypha-scheduler-linux-x86_64 ~/.local/bin/hypha-scheduler
```

**4. Verify installation:**

```bash
hypha-scheduler --version
```

### Available Binaries

- `hypha-scheduler`: Coordinates training jobs and manages worker pools
- `hypha-worker`: Executes training and inference tasks
- `hypha-gateway`: Provides network entry points and relay functionality
- `hypha-data`: Serves dataset slices to workers
- `hypha-certutil`: Generates certificates for mTLS (development/testing)

## Building from Source

Building from source allows customization and access to the latest development features.

### Prerequisites

**Install Rust Toolchain**

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Follow the prompts to complete installation. Verify:

```bash
rustc --version  # Should show 1.75 or later
```

**Clone the Repository**

```bash
git clone https://github.com/hypha-space/hypha.git
cd hypha
```

### Building All Components

```bash
cargo build --release
```

Binaries will be in `target/release/`:

- `target/release/hypha-scheduler`
- `target/release/hypha-worker`
- `target/release/hypha-gateway`
- `target/release/hypha-data`
- `target/release/hypha-certutil`

### Building Specific Components

To build only what you need:

```bash
# Scheduler only
cargo build --release -p hypha-scheduler

# Worker only
cargo build --release -p hypha-worker

# Gateway only
cargo build --release -p hypha-gateway

# Data node only
cargo build --release -p hypha-data
```

### Installing Built Binaries

```bash
cargo install --path crates/scheduler
cargo install --path crates/worker
cargo install --path crates/gateway
cargo install --path crates/data
```

This installs binaries to `~/.cargo/bin`, which should be in your PATH.

## Python Dependencies for Executors

Worker nodes executing training jobs require Python executors.

### Installing uv (Recommended)

The uv package manager provides fast, reliable dependency management:

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

Verify installation:

```bash
uv --version
```

### Alternative: pip and virtualenv

If you prefer traditional Python tools:

```bash
python3 -m pip install --user virtualenv
```

### Accelerate Executor

The Accelerate executor enables multi-GPU training through HuggingFace Accelerate. See the [Worker Node](worker.md#accelerate-executor) documentation for detailed setup.

For quick reference:

```bash
# Test installation (downloads from latest release)
uv run --python 3.12 \
    --with 'https://github.com/hypha-space/hypha/releases/download/v0.1.0/hypha_accelerate_executor-0.1.0-py3-none-any.whl' \
    -- python -c "import hypha.accelerate_executor; print('Success')"
```

## Verifying Installation

After installation, verify components are working:

```bash
# Check scheduler
hypha-scheduler --version
hypha-scheduler --help

# Check worker
hypha-worker --version
hypha-worker --help

# Check gateway
hypha-gateway --version
hypha-gateway --help

# Check data node
hypha-data --version
hypha-data --help
```

## Next Steps

With Hypha installed, you can:

1. Review the [Architecture Overview](architecture.md) to understand component relationships
2. Configure and start a [Gateway](gateway.md) node
3. Set up a [Scheduler](scheduler.md) for job coordination
4. Add [Worker](worker.md) nodes to execute tasks
5. Deploy [Data Nodes](data-node.md) to serve datasets

For production deployments, review the [Security](security.md) documentation to set up mTLS authentication.
