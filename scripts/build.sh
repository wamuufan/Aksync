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
log_info "Step 1/3: Compiling Leptos WASM frontend via Trunk..."
cd "${ROOT_DIR}/crates/mycloud-gui"
trunk build --config Trunk.toml --release

# ------------------------------------------------------------------------------
# 2. Compile Workspace & Native GUI Binary
# ------------------------------------------------------------------------------
log_info "Step 2/3: Compiling workspace and Native Desktop GUI binaries..."
cd "${ROOT_DIR}"
cargo build --workspace --release

# ------------------------------------------------------------------------------
# 3. Package Standalone Release Archive (aksync-linux-x86_64.tar.gz)
# ------------------------------------------------------------------------------
log_info "Step 3/3: Packaging standalone release archive (aksync-linux-x86_64.tar.gz)..."
PKG_NAME="aksync-linux-x86_64"
PKG_STAGE="${ROOT_DIR}/target/dist-pkg/${PKG_NAME}"
rm -rf "${ROOT_DIR}/target/dist-pkg"
mkdir -p "${PKG_STAGE}/Aksync-bin"

cp "${ROOT_DIR}/target/release/mycloud-daemon" "${PKG_STAGE}/Aksync-bin/"
cp "${ROOT_DIR}/target/release/mycloud-fuse"   "${PKG_STAGE}/Aksync-bin/"
cp "${ROOT_DIR}/target/release/mycloud-gui"    "${PKG_STAGE}/Aksync-bin/"
cp "${ROOT_DIR}/scripts/aksync-launch.sh"      "${PKG_STAGE}/Aksync-bin/"
cp "${ROOT_DIR}/scripts/uninstall.sh"          "${PKG_STAGE}/Aksync-bin/"
cp "${ROOT_DIR}/scripts/install.sh"            "${PKG_STAGE}/install.sh"
chmod 755 "${PKG_STAGE}/install.sh" "${PKG_STAGE}/Aksync-bin/"*

for doc in LICENSE README.md INSTALLATION.md; do
    if [ -f "${ROOT_DIR}/${doc}" ]; then
        cp "${ROOT_DIR}/${doc}" "${PKG_STAGE}/"
    fi
done

tar -czf "${ROOT_DIR}/${PKG_NAME}.tar.gz" -C "${ROOT_DIR}/target/dist-pkg" "${PKG_NAME}"
sha256sum "${ROOT_DIR}/${PKG_NAME}.tar.gz" > "${ROOT_DIR}/${PKG_NAME}.tar.gz.sha256"
rm -rf "${ROOT_DIR}/target/dist-pkg"

log_success "Build and packaging complete!"
log_info "  - Daemon:  ${ROOT_DIR}/target/release/mycloud-daemon"
log_info "  - FUSE:    ${ROOT_DIR}/target/release/mycloud-fuse"
log_info "  - GUI:     ${ROOT_DIR}/target/release/mycloud-gui"
log_info "  - Archive: ${ROOT_DIR}/${PKG_NAME}.tar.gz"
log_info "  - SHA256:  ${ROOT_DIR}/${PKG_NAME}.tar.gz.sha256"
