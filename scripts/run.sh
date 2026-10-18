#!/usr/bin/env bash
# ==============================================================================
# MyCloud: Pure Native Local Development & Run Script
# ==============================================================================
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="${HOME}/.cargo/bin:${HOME}/.local/bin:${PATH}"
export PKG_CONFIG_PATH="/home/user-0/.pkgconfig:${ROOT_DIR}/.pkgconfig:/usr/lib64/pkgconfig:/usr/share/pkgconfig:${PKG_CONFIG_PATH:-}"

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
# 0. Ensure Native Binaries Exist (Auto-build if missing)
# ------------------------------------------------------------------------------
if [ ! -f "${ROOT_DIR}/target/release/mycloud-daemon" ] || [ ! -f "${ROOT_DIR}/target/release/mycloud-gui" ]; then
    log_warn "Release binaries not found. Running ./scripts/build.sh first..."
    "${ROOT_DIR}/scripts/build.sh"
fi

# Clean up any leftover mounts from prior sessions before starting
fusermount3 -u -z "${HOME}/.local/share/mycloud/vault" 2>/dev/null || true
fusermount3 -u -z "${HOME}/.cache/mycloud/raw_vault" 2>/dev/null || true
fusermount3 -u -z "${HOME}/MyCloud" 2>/dev/null || true
fusermount3 -u -z "${HOME}/.cache/mycloud/raw_mount" 2>/dev/null || true
if [ -n "${XDG_RUNTIME_DIR:-}" ]; then
    fusermount3 -u -z "${XDG_RUNTIME_DIR}/mycloud/raw_vault" 2>/dev/null || true
    fusermount3 -u -z "${XDG_RUNTIME_DIR}/mycloud/raw_mount" 2>/dev/null || true
fi

# ------------------------------------------------------------------------------
# 1. Start mycloud-daemon in background
# ------------------------------------------------------------------------------
log_info "Starting mycloud-daemon process in background..."
"${ROOT_DIR}/target/release/mycloud-daemon" &
DAEMON_PID=$!

# Guarantee cleanup of daemon upon script exit, interrupt or termination
cleanup() {
    log_warn "Stopping background mycloud-daemon (PID: ${DAEMON_PID})..."
    kill "${DAEMON_PID}" 2>/dev/null || true
    wait "${DAEMON_PID}" 2>/dev/null || true
    # Lazily unmount so file managers (e.g. Dolphin) never cause 'Device or resource busy'
    fusermount3 -u -z "${HOME}/.local/share/mycloud/vault" 2>/dev/null || true
    fusermount3 -u -z "${HOME}/.cache/mycloud/raw_vault" 2>/dev/null || true
    fusermount3 -u -z "${HOME}/MyCloud" 2>/dev/null || true
    fusermount3 -u -z "${HOME}/.cache/mycloud/raw_mount" 2>/dev/null || true
    if [ -n "${XDG_RUNTIME_DIR:-}" ]; then
        fusermount3 -u -z "${XDG_RUNTIME_DIR}/mycloud/raw_vault" 2>/dev/null || true
    fi
    # Clean up WebDAV bookmarks from KDE Places so no stale links remain when stopped
    if [ -f "${HOME}/.local/share/user-places.xbel" ]; then
        python3 -c "
import re
path = '${HOME}/.local/share/user-places.xbel'
try:
    with open(path, 'r', encoding='utf-8') as f:
        c = f.read()
    c = re.sub(r' *<bookmark href=\"webdav://127\.0\.0\.1:876[56]/\">.*?</bookmark>\n?', '', c, flags=re.DOTALL)
    with open(path, 'w', encoding='utf-8') as f:
        f.write(c)
except Exception:
    pass
" 2>/dev/null || true
    fi
    log_success "Daemon process terminated cleanly."
}
trap cleanup EXIT INT TERM

log_success "mycloud-daemon active (PID: ${DAEMON_PID}). Waiting for IPC readiness..."
for i in {1..100}; do
    [ -S "${HOME}/.cache/mycloud/mycloud.sock" ] && break
    sleep 0.02
done

# ------------------------------------------------------------------------------
# 2. Launch Native Desktop GUI
# ------------------------------------------------------------------------------
log_info "Launching Native MyCloud Desktop GUI..."

# WebKitGTK Rendering & Injected Bundle Configuration
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1

if [ -d "/home/user-0/.cache/rpms/extracted/usr/lib64/webkit2gtk-4.1/injected-bundle" ]; then
    export WEBKIT_INJECTED_BUNDLE_PATH="/home/user-0/.cache/rpms/extracted/usr/lib64/webkit2gtk-4.1/injected-bundle"
fi

export LD_LIBRARY_PATH="/home/user-0/.pkgconfig/lib:${LD_LIBRARY_PATH:-}"

if [ -d "/usr/libexec/libwebkit2gtk-4_1-0" ]; then
    "${ROOT_DIR}/target/release/mycloud-gui"
elif command -v bwrap >/dev/null 2>&1; then
    mkdir -p /tmp/my_libexec
    cp -rs /usr/libexec/* /tmp/my_libexec/ 2>/dev/null || true
    if [ -d "/home/user-0/.cache/rpms/extracted/usr/libexec/libwebkit2gtk-4_1-0" ]; then
        cp -rs /home/user-0/.cache/rpms/extracted/usr/libexec/libwebkit2gtk-4_1-0 /tmp/my_libexec/ 2>/dev/null || true
    fi
    bwrap --ro-bind / / \
          --ro-bind /tmp/my_libexec /usr/libexec \
          --bind /run /run \
          --bind /home/user-0 /home/user-0 \
          --bind /tmp /tmp \
          --dev-bind /dev /dev \
          --proc /proc \
          env WEBKIT_DISABLE_COMPOSITING_MODE=1 \
              WEBKIT_DISABLE_DMABUF_RENDERER=1 \
              WEBKIT_INJECTED_BUNDLE_PATH="${WEBKIT_INJECTED_BUNDLE_PATH:-}" \
              LD_LIBRARY_PATH="/home/user-0/.pkgconfig/lib:${LD_LIBRARY_PATH:-}" \
          "${ROOT_DIR}/target/release/mycloud-gui"
else
    "${ROOT_DIR}/target/release/mycloud-gui"
fi
