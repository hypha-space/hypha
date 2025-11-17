# Zip and Upload Action

A simple GitHub Action that zips specified files/folders and uploads them as artifacts.

## Inputs

| Input | Description | Required |
|-------|-------------|----------|
| `paths` | Space-separated list of paths to zip | ✅ |
| `zip-name` | Name for the zip file (without extension) | ✅ |

## Usage

### Basic Example

```yaml
- name: Zip build artifacts
  uses: ./.github/actions/zip-and-upload
  with:
    paths: "target/release/hypha-gateway target/release/hypha-worker"
    zip-name: "release-binaries"
```

### Multiple Files and Folders

```yaml
- name: Package documentation
  uses: ./.github/actions/zip-and-upload
  with:
    paths: "docs/ README.md LICENSE"
    zip-name: "project-docs"
```

### Configuration Files

```yaml
- name: Bundle configs
  uses: ./.github/actions/zip-and-upload
  with:
    paths: "gateway-config.toml scheduler-config.toml worker1-config.toml"
    zip-name: "config-bundle"
```

## What it does

1. Creates a zip file containing all specified paths
2. Generates a SHA256 checksum file
3. Uploads both files as GitHub artifacts

## Output

The action creates:
- `{zip-name}.zip` - The compressed archive
- `{zip-name}.zip.sha256` - SHA256 checksum file

Both files are uploaded as artifacts with the name specified in `zip-name`.