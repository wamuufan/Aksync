#!/usr/bin/env bash
# ==============================================================================
# MyCloud: Files On-Demand Cloud Synchronization Client
# Production Installation & Deployment Script (Idempotent)
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${HOME}/.local/bin"
SYSTEMD_USER_DIR="${HOME}/.config/systemd/user"
NAUTILUS_EXT_DIR="${HOME}/.local/share/nautilus-python/extensions"
CACHE_DIR="${HOME}/.cache/mycloud/raw_mount"
MOUNT_DIR="${HOME}/MyCloud"

# ANSI Colors
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

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

echo "=========================================================="
echo "          MyCloud Client Production Installer             "
echo "=========================================================="

# ------------------------------------------------------------------------------
# 1. Compile Workspace in Release Mode
# ------------------------------------------------------------------------------
log_info "Compiling MyCloud backend binaries in release mode..."
cd "${SCRIPT_DIR}"
cargo build --release -p mycloud-daemon -p mycloud-fuse

log_success "Cargo build complete."

# ------------------------------------------------------------------------------
# 2. Install Executables into ~/.local/bin/
# ------------------------------------------------------------------------------
log_info "Installing binaries to ${BIN_DIR}..."
mkdir -p "${BIN_DIR}"
install -m 755 "${SCRIPT_DIR}/target/release/mycloud-daemon" "${BIN_DIR}/mycloud-daemon"
install -m 755 "${SCRIPT_DIR}/target/release/mycloud-fuse" "${BIN_DIR}/mycloud-fuse"

log_success "Binaries installed: mycloud-daemon, mycloud-fuse"

# Ensure runtime directories exist
mkdir -p "${CACHE_DIR}"
mkdir -p "${MOUNT_DIR}"

# ------------------------------------------------------------------------------
# 3. Configure and Enable User Systemd Service
# ------------------------------------------------------------------------------
log_info "Configuring user systemd service..."
mkdir -p "${SYSTEMD_USER_DIR}"
install -m 644 "${SCRIPT_DIR}/deploy/mycloud-daemon.service" "${SYSTEMD_USER_DIR}/mycloud-daemon.service"

if command -v systemctl >/dev/null 2>&1; then
    log_info "Reloading systemd user daemon and enabling service..."
    systemctl --user daemon-reload
    systemctl --user enable --now mycloud-daemon.service || log_warn "Systemd enable/start returned non-zero (session may be non-interactive)"
    log_success "Systemd user service installed and enabled."
else
    log_warn "systemctl command not found. Skipping automatic service activation."
fi

# ------------------------------------------------------------------------------
# 4. Install GNOME Nautilus Python Extension
# ------------------------------------------------------------------------------
log_info "Installing Nautilus extension..."
mkdir -p "${NAUTILUS_EXT_DIR}"
install -m 644 "${SCRIPT_DIR}/desktop-integrations/nautilus/mycloud_extension.py" "${NAUTILUS_EXT_DIR}/mycloud_extension.py"
log_success "Nautilus extension copied to ${NAUTILUS_EXT_DIR}/mycloud_extension.py"

# ------------------------------------------------------------------------------
# 5. Restart Nautilus (if running)
# ------------------------------------------------------------------------------
log_info "Reloading GNOME Nautilus to apply emblems..."
if command -v nautilus >/dev/null 2>&1; then
    nautilus -q || true
    log_success "Nautilus reloaded."
else
    log_info "Nautilus not found in PATH; skipping reload."
fi

echo "=========================================================="
log_success "MyCloud installation completed successfully!"
echo "User mount available at: ${MOUNT_DIR}"
echo "Encrypted cache at:     ${CACHE_DIR}"
echo "=========================================================="
