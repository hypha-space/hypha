# Quick Start Guide

## Overview

**Hypha** is an open-source tool for democratizing large-scale machine learning across heterogeneous compute resources. It helps discover and utilize diverse compute resources (CPUs, GPUs, and specialized accelerators) to run distributed training and inference with minimal operational overhead.

This guide walks through installation, configuration, and running a first distributed training job.

---

## Prerequisites

Make sure you have the following installed:

| Requirement | Version | Check Command    | Install Documentation                                                                                              |
| ----------- | ------- | ---------------- | ------------------------------------------------------------------------------------------------------------------ |
| uv          | ≥ 0.9.7 | `uv --version`   | [https://docs.astral.sh/uv/getting-started/installation/](https://docs.astral.sh/uv/getting-started/installation/) |
| git         | any     | `git --version`  | https://git-scm.com/book/en/v2/Getting-Started-Installing-Git                                                      |
| git-lfs     | any     | `git-lfs --version` | https://git-lfs.github.com/                                                                                       |
| curl        | any     | `curl --version` | https://curl.se/docs/install.html                                                                                  |

Also if you want to build from source, you need your rust toolchain.

---

## Installation

Install Hypha using the standalone installer script:

```sh
curl -fsSL https://github.com/hypha-space/hypha/releases/download/v<VERSION>/install.sh | sh
```

For alternative installation methods (GitHub releases, Cargo), see the [Installation Guide](installation.md).

---

## Configuration

All Hypha components require setup, and certificates need to be prepared.

---

### Certificates

Create the necessary certificates first. See `hypha-certutil --help` for details.

For this quickstart guide, execute the following commands:

```bash
hypha-certutil root --organization root && \
hypha-certutil org --root-cert ./root-ca-cert.pem --root-key ./root-ca-key.pem -o quickstart && \
hypha-certutil node --ca-cert ./quickstart-ca-cert.pem --ca-key ./quickstart-ca-key.pem -n gateway && \
hypha-certutil node --ca-cert ./quickstart-ca-cert.pem --ca-key ./quickstart-ca-key.pem -n scheduler && \
hypha-certutil node --ca-cert ./quickstart-ca-cert.pem --ca-key ./quickstart-ca-key.pem -n worker1 && \
hypha-certutil node --ca-cert ./quickstart-ca-cert.pem --ca-key ./quickstart-ca-key.pem -n worker2 && \
hypha-certutil node --ca-cert ./quickstart-ca-cert.pem --ca-key ./quickstart-ca-key.pem -n worker3 && \
hypha-certutil node --ca-cert ./quickstart-ca-cert.pem --ca-key ./quickstart-ca-key.pem -n data1
```

---

### Setting Up Nodes

Set up the nodes next. This guide assumes the following configuration:

---

#### Gateway Node

Generate a configuration for local development using `hypha-gateway init`:

```bash
hypha-gateway init -n gateway -o gateway-config.toml --exclude-cidr 192.0.2.0/24
```

#### Scheduler Node

Generate the necessary configuration using `hypha-scheduler init`:

```bash
hypha-scheduler init -n scheduler -o scheduler-config.toml --exclude-cidr 192.0.2.0/24 --gateway /ip4/127.0.0.1/tcp/8080
```

#### Worker Nodes

Generate worker configuration using `hypha-worker init`:

```bash
hypha-worker init -n worker1 -o worker1-config.toml --exclude-cidr 192.0.2.0/24 --gateway /ip4/127.0.0.1/tcp/8080 && \
hypha-worker init -n worker2 -o worker2-config.toml --exclude-cidr 192.0.2.0/24 --gateway /ip4/127.0.0.1/tcp/8080 && \
hypha-worker init -n worker3 -o worker3-config.toml --exclude-cidr 192.0.2.0/24 --gateway /ip4/127.0.0.1/tcp/8080
```

#### Accelerate Configuration

Create an `accelerate.yaml` file with the following configuration:

```bash
cat <<EOL > accelerate.yaml
compute_environment: LOCAL_MACHINE
debug: false
distributed_type: "NO"
downcast_bf16: "no"
enable_cpu_affinity: false
machine_rank: 0
main_training_function: main
mixed_precision: "no"
num_machines: 1
num_processes: 1
rdzv_backend: static
same_network: true
tpu_env: []
tpu_use_cluster: false
tpu_use_sudo: false
use_cpu: false
EOL
```

#### Data Node

Generate a data configuration using `hypha-data init`:

```bash
hypha-data init -n data1 --dataset-path="mnist" -o data-config.toml --exclude-cidr 192.0.2.0/24 --gateway /ip4/127.0.0.1/tcp/8080
```

### Download the Training Dataset

In the project folder (where all configuration files and certificates are), clone the HuggingFace dataset. To ensure a clean folder structure afterwards, use this quick workaround:

```bash
git clone https://huggingface.co/datasets/hypha-space/mnist mnist.git && \
mv mnist.git/mnist . && \
rm -rf mnist.git
```

Congratulations! Hypha is now set up correctly.

---

## Running Hypha for Training

With everything set up, start the Hypha nodes and begin training.

In different terminals, start the gateway first:

1. `RUST_LOG=info hypha-gateway run -c gateway-config.toml`

Then start the worker and data nodes (the gateway must be running first):

1. `RUST_LOG=info hypha-worker run -c worker1-config.toml`
1. `RUST_LOG=info hypha-worker run -c worker2-config.toml`
1. `RUST_LOG=info hypha-worker run -c worker3-config.toml`
1. `RUST_LOG=info hypha-data run -c data-config.toml`

Finally, once all the nodes above are running properly, start the scheduler to begin training:

1. `RUST_LOG=info hypha-scheduler run -c scheduler-config.toml`

### Expected Output

After successfully running training, the scheduler terminal should display these messages:

```bash
2025-11-13T15:59:07.446178Z  INFO hypha_scheduler::scheduling::batch_scheduler: Job is completed.
2025-11-13T15:59:07.446185Z  INFO hypha_scheduler: Batch Scheduler finished
2025-11-13T15:59:07.446455Z ERROR hypha_network::request_response: Failed to unregister handler error=Other("Failed to send action")
2025-11-13T15:59:07.446471Z ERROR hypha_network::request_response: Failed to unregister handler error=Other("Failed to send action")
2025-11-13T15:59:07.446482Z  INFO hypha_scheduler::scheduling::data_scheduler: Data handler finished.
```

**NOTE**: We are aware of the error messages and they are expected (for now). As long as these are the only errors, everything worked as expected.
