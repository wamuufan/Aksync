#!/usr/bin/env bash
# ==============================================================================
# MyCloud: Pure Native Production Build Script
# ==============================================================================
set -euo pipefail

export PATH="${HOME}/.cargo/bin:${HOME}/.local/bin:${PATH}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PKG_CONFIG_PATH="${HOME}/.pkgconfig:${ROOT_DIR}/.pkgconfig:/usr/lib64/pkgconfig:/usr/share/pkgconfig:${PKG_CONFIG_PATH:-}"

# ANSI Colors
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# ------------------------------------------------------------------------------
# 1. Compile Leptos WASM Frontend via Trunk
# ------------------------------------------------------------------------------
log_info "Step 1/2: Compiling Leptos WASM frontend via Trunk..."
cd "${ROOT_DIR}/crates/mycloud-gui"
trunk build --config Trunk.toml --release

# ------------------------------------------------------------------------------
# 2. Compile Workspace & Native GUI Binary
# ------------------------------------------------------------------------------
log_info "Step 2/2: Compiling workspace and Native Desktop GUI binaries..."
cd "${ROOT_DIR}"
cargo build --workspace --release

log_success "Build complete! Native release binaries are ready in ${ROOT_DIR}/target/release/"
log_info "  - Daemon: ${ROOT_DIR}/target/release/mycloud-daemon"
log_info "  - FUSE:   ${ROOT_DIR}/target/release/mycloud-fuse"
log_info "  - GUI:    ${ROOT_DIR}/target/release/mycloud-gui"
