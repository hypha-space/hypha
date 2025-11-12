#!/bin/sh
# Copyright (c) 2025 Hypha Space
#
# Permission is hereby granted, free of charge, to any person obtaining a copy
# of this software and associated documentation files (the "Software"), to deal
# in the Software without restriction, including without limitation the rights
# to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
# copies of the Software, and to permit persons to whom the Software is
# furnished to do so, subject to the following conditions:
#
# The above copyright notice and this permission notice shall be included in all
# copies or substantial portions of the Software.
#
# THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
# IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
# FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
# AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
# LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
# OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
# SOFTWARE.
#
# NOTE: This installation script is inspired by the uv installer from Astral
# (https://astral.sh/uv) and follows similar patterns for robust cross-platform
# installation. We're grateful to the Astral team for their excellent work on
# tooling for the OSS community.

set -e

# Configuration
REPO="hypha-space/hypha"
INSTALL_DIR="${HYPHA_INSTALL_DIR:-$HOME/.local/bin}"
BINARIES="hypha-gateway hypha-worker hypha-data hypha-scheduler hypha-certutil"

# Detect platform
detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux)
            case "$ARCH" in
                x86_64|amd64)
                    PLATFORM="x86_64-unknown-linux-gnu"
                    ;;
                *)
                    echo "Error: Unsupported architecture: $ARCH"
                    echo "Supported architectures: x86_64"
                    exit 1
                    ;;
            esac
            ;;
        Darwin)
            case "$ARCH" in
                arm64|aarch64)
                    PLATFORM="aarch64-apple-darwin"
                    ;;
                *)
                    echo "Error: Unsupported architecture: $ARCH"
                    echo "Supported architectures: arm64/aarch64"
                    exit 1
                    ;;
            esac
            ;;
        *)
            echo "Error: Unsupported operating system: $OS"
            echo "Supported operating systems: Linux, macOS"
            exit 1
            ;;
    esac
}

# Get the latest release version or use specified version
get_version() {
    if [ -n "$HYPHA_VERSION" ]; then
        VERSION="$HYPHA_VERSION"
    else
        echo "Fetching latest release version..."
        VERSION=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | sed -E 's/.*"([^"]+)".*/\1/')
        if [ -z "$VERSION" ]; then
            echo "Error: Failed to fetch latest version"
            exit 1
        fi
    fi
    echo "Installing Hypha $VERSION"
}

# Download and extract archive
download_and_install() {
    ARCHIVE="hypha-$PLATFORM.tar.gz"
    URL="https://github.com/$REPO/releases/download/$VERSION/$ARCHIVE"

    echo "Downloading from: $URL"

    # Create temporary directory
    TMP_DIR="$(mktemp -d)"
    trap 'rm -rf "$TMP_DIR"' EXIT

    # Download archive
    if ! curl -fsSL "$URL" -o "$TMP_DIR/$ARCHIVE"; then
        echo "Error: Failed to download archive"
        exit 1
    fi

    # Extract archive
    echo "Extracting archive..."
    if ! tar -xzf "$TMP_DIR/$ARCHIVE" -C "$TMP_DIR"; then
        echo "Error: Failed to extract archive"
        exit 1
    fi

    # Create install directory if it doesn't exist
    mkdir -p "$INSTALL_DIR"

    # Install binaries
    echo "Installing binaries to $INSTALL_DIR..."
    for binary in $BINARIES; do
        if [ -f "$TMP_DIR/$binary" ]; then
            chmod +x "$TMP_DIR/$binary"
            mv "$TMP_DIR/$binary" "$INSTALL_DIR/"
            echo "  ✓ Installed $binary"
        else
            echo "  ⚠ Warning: $binary not found in archive"
        fi
    done
}

# Check if install directory is in PATH
check_path() {
    case ":$PATH:" in
        *":$INSTALL_DIR:"*)
            ;;
        *)
            echo ""
            echo "⚠️  Installation complete, but $INSTALL_DIR is not in your PATH."
            echo ""
            echo "To add it to your PATH, add this line to your shell profile:"
            echo ""
            echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
            echo ""
            return
            ;;
    esac
}

# Main installation flow
main() {
    echo "Hypha Installer"
    echo "==============="
    echo ""

    detect_platform
    echo "Detected platform: $PLATFORM"
    echo ""

    get_version
    echo ""

    download_and_install
    echo ""

    echo "✅ Installation complete!"
    echo ""

    check_path

    echo "Get started by running:"
    echo "  hypha-gateway --help"
}

main
