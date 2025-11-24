+++
title = "Installation"
description = "Explains every supported method for installing or removing Hypha binaries and where to go next."
[taxonomies]
track = ["reference"]
+++

# Installing Hypha

Hypha provides prebuilt binaries for Linux and macOS, plus a source build path when you need to test unreleased commits. This page explains each option, when to use it, and how to verify the download before running it. For first-time users, install the binaries on your workstation and then follow the [Quick Start Guide](quickstart.md).

## Choosing an installation method

- **Installer** — Recommended for Linux/macOS. Fetches the correct binary bundle for your platform and updates the PATH automatically.
- **Cargo (from Git)** — Use only when testing changes that have not been released yet. Requires the Rust toolchain and takes several minutes to compile.

## Installer

The installer supports modern Linux distributions (Debian, Ubuntu, Fedora, Amazon Linux) and macOS 13+ on Apple Silicon. Replace `v<VERSION>` with the version you want to install.

```bash
curl -fsSL https://github.com/hypha-space/hypha/releases/download/v<VERSION>/install.sh -o /tmp/hypha-install.sh
shasum -a 256 /tmp/hypha-install.sh
sh /tmp/hypha-install.sh
```
```text
...
installing to ~/.local/bin
  hypha-gateway
  hypha-worker
  hypha-data
  hypha-scheduler
  hypha-certutil
everything's installed!
```
For a list of available versions, visit the [GitHub Releases](https://github.com/hypha-space/hypha/releases) page.

> [!TIP]
> To inspect the installer script before use, download it using `curl` and inspect it using `less`:
> ```sh
> curl -LsSf https://github.com/hypha-space/hypha/releases/download/v<VERSION>/install.sh | less
> ```

Add `$HOME/.local/bin` to the `PATH` for service users or shells that do not source `.profile` automatically. On macOS, open a new terminal so login shells reload the updated `PATH`.

## Building from source (Cargo)

Only build from source when you need unreleased changes or are modifying the code. The crates are not published on crates.io, so install directly from Git:

```bash
rustup toolchain install stable
cargo install --git https://github.com/hypha-space/hypha \
    hypha-certutil \
    hypha-data \
    hypha-gateway \
    hypha-scheduler \
    hypha-worker
```

> [!NOTE]
> This method builds the Hypha binaries from source, which requires a compatible Rust toolchain.

## Uninstalling

Remove the binaries created by the installer:

```bash
rm -f ~/.local/bin/hypha-certutil \
    ~/.local/bin/hypha-data \
    ~/.local/bin/hypha-gateway \
    ~/.local/bin/hypha-scheduler \
    ~/.local/bin/hypha-worker
```

## Next steps

1. Follow the [Quick Start Guide](quickstart.md) to configure certificates, nodes, and run your first DiLoCo job.
2. Study the [Architecture](architecture.md) and individual component docs ([Gateway](gateway.md), [Scheduler](scheduler.md), [Worker](worker.md), [Data Node](data.md)) before deploying production clusters.
