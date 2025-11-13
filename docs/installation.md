# Installing Hypha

## Installation methods

Install Hypha using the standalone installer script or using one of the orher methods.

### Standalone installer

Hypha provides a standalone installer script to download and install the Hypha binaries. Use `curl` to download the script and execute it with sh:

```sh
curl -fsSL https://github.com/hypha-space/hypha/releases/download/v<VERSION>/install.sh | sh
```

> [!TIP]
> To inspect the installer script before use, download it using `curl` and inspect it using `less`:
> ```sh
> curl -LsSf https://github.com/hypha-space/hypha/releases/download/v<VERSION>/install.sh | less
> ```

For a list of available versions, visit the [GitHub Releases](https://github.com/hypha-space/hypha/releases) page.

### GitHub Releases

Hypha release artifacts can be downloaded directly from [GitHub Releases](https://github.com/hypha-space/hypha/releases).

Each release page includes the binaries for all supported platforms.

### Cargo

Hypha is available via Cargo, but must be built from Git rather than crates.io due to unpublished crates.

```bash
cargo install --git https://github.com/hypha-space/hypha \
    hypha-certutil \
    hypha-data \
    hypha-gateway \
    hypha-scheduler \
    hypha-worker
```

> [!NOTE]
> This method builds the Hypha binaries from source, which requires a compatible Rust toolchain.

## Uninstallation

Remove the Hypha binaries:

```bash
rm -f ~/.local/bin/hypha-certutil \
    ~/.local/bin/hypha-data \
    ~/.local/bin/hypha-gateway \
    ~/.local/bin/hypha-scheduler \
    ~/.local/bin/hypha-worker
```
## Next Steps

Follow the [Quick Start Guide](quickstart.md) to get Hypha running locally, or review the [Architecture](architecture.md) to understand how components work together before configuring them individually.
