#!/bin/bash
#
# Quick rebuild script for fatx-cli
#

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Ensure cargo is available
if ! command -v cargo &> /dev/null; then
    if [ -f "$HOME/.cargo/env" ]; then
        source "$HOME/.cargo/env"
    else
        echo "Error: Rust not installed. Run setup.sh first."
        exit 1
    fi
fi

echo "Building fatx-cli (release)..."
cargo build --release 2>&1

echo ""
echo "Done: ./target/release/fatx-cli"
echo ""
echo "Usage:"
echo "  sudo ./target/release/fatx-cli              # interactive mode"
echo "  sudo ./target/release/fatx-cli -v scan /dev/rdisk4   # verbose scan"
