#!/usr/bin/env bash
# ==============================================================================
# MyCloud: Workspace Deep Clean Script
# ==============================================================================
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# ANSI Colors
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[CLEAN]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

cd "${ROOT_DIR}"

log_info "Cleaning Cargo build artifacts..."
cargo clean

log_info "Removing Trunk distribution artifacts..."
rm -rf "${ROOT_DIR}/crates/mycloud-gui/dist"

log_info "Removing Tauri build bundles..."
rm -rf "${ROOT_DIR}/crates/mycloud-gui/src-tauri/target"

log_success "Workspace thoroughly cleaned."
