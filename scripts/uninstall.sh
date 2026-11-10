#!/usr/bin/env bash
# ==============================================================================
# AkSync: Uninstallation Script
# ==============================================================================
set -u

FUSE_MOUNT="${HOME}/.local/share/mycloud/vault"
CACHE_RAW_MOUNT="${HOME}/.cache/mycloud/raw_vault"
RUNTIME_RAW_MOUNT=""
if [ -n "${XDG_RUNTIME_DIR:-}" ]; then
    RUNTIME_RAW_MOUNT="${XDG_RUNTIME_DIR}/mycloud/raw_vault"
fi

INSTALL_DIR="${HOME}/.local/bin/Aksync"
SYMLINK_PATH="${HOME}/.local/bin/aksync"
DESKTOP_FILE="${HOME}/.local/share/applications/aksync.desktop"
CONFIG_DIR="${HOME}/.config/mycloud"
CACHE_DIR="${HOME}/.cache/mycloud"

echo "[INFO] Stopping running AkSync processes..."
pkill -TERM -x "mycloud-gui" >/dev/null 2>&1 || true
pkill -TERM -x "mycloud-daemon" >/dev/null 2>&1 || true
pkill -TERM -x "mycloud-fuse" >/dev/null 2>&1 || true
pkill -TERM -f "rclone serve webdav" >/dev/null 2>&1 || true
pkill -TERM -f "rclone mount" >/dev/null 2>&1 || true

sleep 1

pkill -KILL -x "mycloud-gui" >/dev/null 2>&1 || true
pkill -KILL -x "mycloud-daemon" >/dev/null 2>&1 || true
pkill -KILL -x "mycloud-fuse" >/dev/null 2>&1 || true
pkill -KILL -f "rclone serve webdav" >/dev/null 2>&1 || true
pkill -KILL -f "rclone mount" >/dev/null 2>&1 || true

echo "[INFO] Unmounting FUSE mount points..."
fusermount3 -u -z "${FUSE_MOUNT}" >/dev/null 2>&1 || true
fusermount3 -u -z "${CACHE_RAW_MOUNT}" >/dev/null 2>&1 || true
if [ -n "${RUNTIME_RAW_MOUNT}" ]; then
    fusermount3 -u -z "${RUNTIME_RAW_MOUNT}" >/dev/null 2>&1 || true
fi

echo "[INFO] Removing desktop shortcut and symlink..."
rm -f "${DESKTOP_FILE}"
rm -f "${SYMLINK_PATH}"

echo "[INFO] Removing installation directory (${INSTALL_DIR})..."
rm -rf "${INSTALL_DIR}"

echo ""
echo "[NOTE] Your encrypted vault stored on Google Drive is NEVER touched by this uninstaller."
read -r -p "Do you also want to delete local configuration and cache directories (${CONFIG_DIR} and ${CACHE_DIR})? [y/N]: " REPLY || REPLY="N"

case "${REPLY}" in
    [yY]|[yY][eE][sS])
        rm -rf "${CONFIG_DIR}" "${CACHE_DIR}"
        echo "[INFO] Local configuration and cache directories removed."
        ;;
    *)
        echo "[INFO] Local configuration and cache directories preserved."
        ;;
esac

echo "[SUCCESS] AkSync has been uninstalled."
