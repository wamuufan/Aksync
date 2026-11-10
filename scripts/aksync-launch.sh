#!/usr/bin/env bash
# ==============================================================================
# AkSync: Unified Application Launcher & Lifecycle Wrapper
# ==============================================================================
set -u

SCRIPT_PATH="$(readlink -f "${BASH_SOURCE[0]}")"
BIN_DIR="$(dirname "${SCRIPT_PATH}")"

SOCK_PATH="${HOME}/.cache/mycloud/mycloud.sock"
FUSE_MOUNT="${HOME}/.local/share/mycloud/vault"
CACHE_RAW_MOUNT="${HOME}/.cache/mycloud/raw_vault"
RUNTIME_RAW_MOUNT=""
if [ -n "${XDG_RUNTIME_DIR:-}" ]; then
    RUNTIME_RAW_MOUNT="${XDG_RUNTIME_DIR}/mycloud/raw_vault"
fi

STARTED_DAEMON=0
DAEMON_PID=""
CLEANED_UP=0

unmount_fuse_mounts() {
    fusermount3 -u -z "${FUSE_MOUNT}" >/dev/null 2>&1 || true
    fusermount3 -u -z "${CACHE_RAW_MOUNT}" >/dev/null 2>&1 || true
    if [ -n "${RUNTIME_RAW_MOUNT}" ]; then
        fusermount3 -u -z "${RUNTIME_RAW_MOUNT}" >/dev/null 2>&1 || true
    fi
}

cleanup() {
    if [ "${CLEANED_UP}" -eq 1 ]; then
        return
    fi
    CLEANED_UP=1

    if [ "${STARTED_DAEMON}" -eq 1 ] || pgrep -x "mycloud-daemon" >/dev/null 2>&1; then
        # Gracefully stop daemon first, then FUSE and rclone child processes
        if [ -n "${DAEMON_PID}" ] && kill -0 "${DAEMON_PID}" >/dev/null 2>&1; then
            kill -TERM "${DAEMON_PID}" >/dev/null 2>&1 || true
        fi
        pkill -TERM -x "mycloud-daemon" >/dev/null 2>&1 || true
        pkill -TERM -x "mycloud-fuse" >/dev/null 2>&1 || true
        pkill -TERM -f "rclone serve webdav" >/dev/null 2>&1 || true
        pkill -TERM -f "rclone mount" >/dev/null 2>&1 || true

        # Brief wait for graceful shutdown
        local i=0
        while [ "${i}" -lt 20 ]; do
            if ! pgrep -x "mycloud-daemon" >/dev/null 2>&1 \
               && ! pgrep -x "mycloud-fuse" >/dev/null 2>&1 \
               && ! pgrep -f "rclone serve webdav" >/dev/null 2>&1 \
               && ! pgrep -f "rclone mount" >/dev/null 2>&1; then
                break
            fi
            sleep 0.1
            i=$((i + 1))
        done

        # Force kill any remaining stubborn processes
        pkill -KILL -x "mycloud-daemon" >/dev/null 2>&1 || true
        pkill -KILL -x "mycloud-fuse" >/dev/null 2>&1 || true
        pkill -KILL -f "rclone serve webdav" >/dev/null 2>&1 || true
        pkill -KILL -f "rclone mount" >/dev/null 2>&1 || true

        unmount_fuse_mounts
    fi
}

trap cleanup INT TERM

# Start daemon if it is not already running
if ! pgrep -x "mycloud-daemon" >/dev/null 2>&1; then
    # Clean up any stale or hanging FUSE mounts before starting
    unmount_fuse_mounts

    "${BIN_DIR}/mycloud-daemon" >/dev/null 2>&1 &
    DAEMON_PID=$!
    STARTED_DAEMON=1

    # Wait up to 5 seconds (50 * 0.1s) for the daemon IPC socket to appear
    waited=0
    while [ "${waited}" -lt 50 ]; do
        if [ -S "${SOCK_PATH}" ]; then
            break
        fi
        if ! kill -0 "${DAEMON_PID}" >/dev/null 2>&1; then
            break
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
fi

# Launch the desktop GUI in the foreground
"${BIN_DIR}/mycloud-gui" || true

# Clean up daemon, FUSE, and rclone processes when the GUI exits
cleanup
