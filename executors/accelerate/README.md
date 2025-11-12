# Hypha Accelerate Executor

The Accelerate executor enables Hypha workers to execute training tasks that use [🤗 Accelerate](https://huggingface.co/docs/accelerate) for tensor parallelism across multiple devices. This allows workers with _multi-GPU_ setups to efficiently train large models that wouldn't fit on a single device.

## Overview

Hypha's architecture consists of multiple components working together: schedulers coordinate work, workers execute tasks, data peers store information, and so on. Workers can be equipped with different **executors**, specialized _handlers_ that determine what types of tasks a worker can run.

This particular executor adds Accelerate support to workers, which is valuable when you have multiple devices available on a single machine. Accelerate provides tensor parallelism, allowing you to split large models across devices and train them as if they were on a single, larger device. Without Accelerate, each device would need to hold the entire model in memory. With Accelerate, a worker with e.g. 4 GPUs can train models 4x larger by distributing the model's layers across all available devices than one with a single GPU.

The executor is distributed as a Python wheel, making it straightforward to install and deploy across your infrastructure.

## Installation

### Prerequisites

This executor uses **uv** as the recommended Python package and project manager to handle dependencies and execution. While you could use alternatives like pip or Poetry, uv is particularly convenient because it can install dependencies and run the executor in a single command without requiring a separate installation step.

If you don't have uv installed yet, you can get it by following the instructions at:

https://docs.astral.sh/uv/getting-started/installation/

### Configuring Your Worker

To enable the Accelerate executor on a Hypha worker, you'll need to add an executor configuration to your worker's TOML configuration file. This tells the worker how to launch training jobs when they're assigned by the scheduler.

Here's the configuration template:

```toml
[[executors]]
class = "train"
name = "diloco-transformer"
runtime = "process"
cmd = "uv"
args = [
    "run",
    "--python", "3.12",
    "--no-project",
    "--with", "https://github.com/hypha-space/hypha/releases/download/v<version>/hypha_accelerate_executor-<version without semver channel or metadata>-py3-none-any.whl",
    # Optional: add `--extra`, "<variant>" here to pin a specific torch build (see docs below)
    "--", # N.B. this standalone `--` is the separator between `uv` opts and the cmd to be executed
    "accelerate",
    "launch",
    "--config_file", "<path/to/accelerate.yaml>",
    "-m", "hypha.accelerate_executor.training",
    "--socket", "{SOCKET_PATH}",
    "--work-dir", "{WORK_DIR}",
    "--job", "{JOB_JSON}",
]
```

#### Configuration Parameters

Let's break down the important parts of this configuration:

**Executor metadata**:
- `class = "train"` — Identifies this executor as handling training workloads
- `name = "diloco-transformer"` — A descriptive name for this specific executor configuration
- `runtime = "process"` — Tells Hypha to launch this as a subprocess using the defined command along with the provided arguments.

**Python and dependencies**:
- `--python "3.12"` — Specifies the Python version to use (adjust as needed for your environment)
- `--with "https://github.com/..."` — The URL to the executor wheel. Replace `<version>` with the actual release version you want to use (e.g., `0.1.0`). Find available releases at https://github.com/hypha-space/hypha/releases the pages also lists the release assets the correct URL for the executor wheel.

**PyTorch variant**
- `--extra` — via this flag you can choose which PyTorch variant to installto match the executor to
your hardware. Insert it after `--with` argument in the args list. Available options:

- `mps_cu128` (default) — Apple Silicon with Metal (MPS) acceleration and CUDA 12.8 drivers
- `cpu` — CPU-only environments
- `cu126` — NVIDIA GPUs with CUDA 12.6 drivers
- `cu129` — NVIDIA GPUs with CUDA 12.9 drivers
- `rocm64` — AMD GPUs with ROCm 6.4

If you omit `--extra`, uv installs the default `mps_cu128` build.

**Accelerate configuration**:
- `--config_file "<path/to/accelerate.yaml>"` — Path to your Accelerate configuration file that defines the distributed training setup. This must match your machine's hardware configuration (number of GPUs, distributed training strategy, etc.)

**Hypha integration** (these are automatically populated by Hypha):
- `{SOCKET_PATH}` — Communication channel between the executor and worker
- `{WORK_DIR}` — Working directory for the training job
- `{JOB_JSON}` — Job specification passed from the scheduler

**Important note**: The module path `hypha.accelerate_executor.training` should remain unchanged—this is the standard entrypoint of the executor used by Hypha to launch training jobs via *accelerate*.

### Setting Up Accelerate Configuration

Accelerate requires a configuration file that describes your training environment. For single-GPU setups (including Apple Silicon with MPS), you can use the provided [`test.yaml`](./test.yaml) as a starting point. For multi-GPU or distributed training across multiple machines, you'll need to adjust the configuration accordingly.

Refer to the [Accelerate CLI documentation](https://huggingface.co/docs/accelerate/en/package_reference/cli#accelerate-config) for guidance on creating configuration files for different hardware setups.

## Next Steps

Once you've configured your worker with the *accelerate* executor:

1. Start your Hypha worker with the updated configuration
2. The worker will automatically match its capabilities with the scheduler when a worker is requested
3. Submit training jobs to your Hypha scheduler—they'll be routed to workers with matching capabilities
4. Monitor job progress through Hypha's standard interfaces

For more information about Hypha's architecture and job submission, see the main Hypha documentation.

## Troubleshooting

**"Module not found" errors**: Ensure the wheel URL in your configuration matches an actual release. Check https://github.com/hypha-space/hypha/releases for valid versions.

**Training doesn't start**: Verify that your `--config_file` path is correct and that the Accelerate configuration matches your hardware (correct number of GPUs, appropriate mixed precision settings, etc.).

**uv command not found**: Make sure uv is installed and available in your `PATH`. Restart your shell after installation if needed.

## Development

If you're developing or customizing the Accelerate executor, there are a few workflows that will make your life easier.

**Working with local sources**: During development, you can skip building wheels by pointing `--with` directly at your local source directory. For example, `--with "./executors/accelerate"` will use the code in your working tree. This lets you iterate quickly without repeatedly building and installing wheels.

**Testing built wheels**: When you do want to test a built wheel (created with `uv build`), add the `--reinstall` flag to ensure uv doesn't use a cached version:

```bash
uv run --reinstall --with ./dist/hypha_accelerate_executor-0.0.0-py3-none-any.whl -- ...
```

**Matching your environment**: Remember to keep your Accelerate configuration file synchronized with your test environment. If you switch between machines with different GPU counts or move from single-node to multi-node setups, you'll need to update the config accordingly.
