#!/bin/bash
#
# fatx-rs setup script for macOS
# Installs Rust (if needed), builds fatx-cli, and optionally installs it.
#

set -e

BOLD='\033[1m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo ""
echo -e "${BOLD}========================================${NC}"
echo -e "${BOLD}  fatx-rs setup — Xbox FATX for macOS   ${NC}"
echo -e "${BOLD}========================================${NC}"
echo ""

# ---------------------------------------------------------------------------
# Step 1: Check / install Rust
# ---------------------------------------------------------------------------
echo -e "${BOLD}[1/4] Checking for Rust toolchain...${NC}"

if command -v cargo &> /dev/null; then
    RUST_VER=$(rustc --version)
    echo -e "  ${GREEN}Found: $RUST_VER${NC}"
else
    echo -e "  ${YELLOW}Rust not found. Installing via rustup...${NC}"
    echo ""
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
    echo ""
    echo -e "  ${GREEN}Installed: $(rustc --version)${NC}"
fi
echo ""

# ---------------------------------------------------------------------------
# Step 2: Build
# ---------------------------------------------------------------------------
echo -e "${BOLD}[2/4] Building fatx-cli (release mode)...${NC}"
echo ""

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

cargo build --release 2>&1

BINARY="$SCRIPT_DIR/target/release/fatx-cli"
echo ""
echo -e "  ${GREEN}Built successfully: $BINARY${NC}"
echo ""

# ---------------------------------------------------------------------------
# Step 3: Optional install to /usr/local/bin
# ---------------------------------------------------------------------------
echo -e "${BOLD}[3/4] Install to /usr/local/bin? (makes 'fatx-cli' available system-wide)${NC}"
read -p "  Install? (y/n) [n]: " INSTALL_CHOICE

if [[ "$INSTALL_CHOICE" == "y" || "$INSTALL_CHOICE" == "Y" ]]; then
    if [[ -w /usr/local/bin ]]; then
        cp "$BINARY" /usr/local/bin/fatx-cli
    else
        echo "  Need sudo to copy to /usr/local/bin..."
        sudo cp "$BINARY" /usr/local/bin/fatx-cli
    fi
    echo -e "  ${GREEN}Installed to /usr/local/bin/fatx-cli${NC}"
else
    echo "  Skipped. You can run it directly:"
    echo "    sudo $BINARY"
fi
echo ""

# ---------------------------------------------------------------------------
# Step 4: Quick help
# ---------------------------------------------------------------------------
echo -e "${BOLD}[4/4] Quick start${NC}"
echo ""
echo "  Interactive mode (guided — prompts for everything):"
echo -e "    ${GREEN}sudo fatx-cli${NC}"
echo ""
echo "  Or use subcommands directly:"
echo "    sudo fatx-cli scan /dev/rdisk4"
echo "    sudo fatx-cli ls /dev/rdisk4 --partition \"Data (E)\" / -l"
echo "    sudo fatx-cli read /dev/rdisk4 --partition \"Data (E)\" /saves/game.sav -o game.sav"
echo ""
echo "  For full help:"
echo "    fatx-cli --help"
echo ""
echo -e "${BOLD}Important notes:${NC}"
echo "  - Use /dev/rdiskN (raw device) for best performance"
echo "  - Unmount the disk first: diskutil unmountDisk /dev/diskN"
echo "  - sudo is required for raw device access"
echo ""
echo -e "${GREEN}Setup complete!${NC}"
