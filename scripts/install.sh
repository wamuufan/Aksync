#!/usr/bin/env bash
# ==============================================================================
# AkSync: Distribution Package Installer
# ==============================================================================
set -euo pipefail

SCRIPT_PATH="$(readlink -f "${BASH_SOURCE[0]}")"
SCRIPT_DIR="$(dirname "${SCRIPT_PATH}")"

SKIP_DEPS=0

for arg in "$@"; do
    case "${arg}" in
        --skip-deps)
            SKIP_DEPS=1
            ;;
        -h|--help)
            echo "Usage: $0 [--skip-deps]"
            echo ""
            echo "Options:"
            echo "  --skip-deps    Skip OS detection and automated package installation"
            echo "  -h, --help     Show this help message"
            exit 0
            ;;
        *)
            echo "[ERROR] Unknown option: ${arg}"
            echo "Usage: $0 [--skip-deps]"
            exit 1
            ;;
    esac
done

# ------------------------------------------------------------------------------
# 1. Locate Distribution Binaries (Aksync-bin or development target/release)
# ------------------------------------------------------------------------------
BIN_SOURCE_DIR=""
SCRIPT_SOURCE_DIR=""
DEV_FALLBACK=0

if [ -d "${SCRIPT_DIR}/Aksync-bin" ]; then
    BIN_SOURCE_DIR="${SCRIPT_DIR}/Aksync-bin"
    SCRIPT_SOURCE_DIR="${SCRIPT_DIR}/Aksync-bin"
elif [ -x "${SCRIPT_DIR}/target/release/mycloud-daemon" ]; then
    BIN_SOURCE_DIR="${SCRIPT_DIR}/target/release"
    SCRIPT_SOURCE_DIR="${SCRIPT_DIR}/scripts"
    DEV_FALLBACK=1
elif [ -x "${SCRIPT_DIR}/../target/release/mycloud-daemon" ]; then
    BIN_SOURCE_DIR="${SCRIPT_DIR}/../target/release"
    SCRIPT_SOURCE_DIR="${SCRIPT_DIR}"
    DEV_FALLBACK=1
else
    echo "[ERROR] Could not find Aksync-bin directory in ${SCRIPT_DIR}."
    echo "[INFO] If running from a source checkout, please build release binaries first via ./scripts/build.sh."
    exit 1
fi

if [ "${DEV_FALLBACK}" -eq 1 ]; then
    echo "[INFO] Aksync-bin not found; using development binaries from ${BIN_SOURCE_DIR}."
fi

for bin in mycloud-daemon mycloud-fuse mycloud-gui; do
    if [ ! -f "${BIN_SOURCE_DIR}/${bin}" ]; then
        echo "[ERROR] Missing required binary: ${BIN_SOURCE_DIR}/${bin}"
        exit 1
    fi
done

for helper in aksync-launch.sh uninstall.sh; do
    if [ ! -f "${SCRIPT_SOURCE_DIR}/${helper}" ]; then
        echo "[ERROR] Missing required script: ${SCRIPT_SOURCE_DIR}/${helper}"
        exit 1
    fi
done

# ------------------------------------------------------------------------------
# 2. OS Distribution Check & Automated Dependency Setup (/etc/os-release)
# ------------------------------------------------------------------------------
if [ "${SKIP_DEPS}" -eq 0 ]; then
    ID=""
    PRETTY_NAME=""
    if [ -r /etc/os-release ]; then
        # shellcheck source=/dev/null
        source /etc/os-release
    fi

    if [ "${ID:-}" != "opensuse-tumbleweed" ]; then
        echo "[ERROR] Your distribution (${PRETTY_NAME:-Unknown}) is not officially supported by the automated dependency installer."
        echo "[INFO] Currently, automatic dependency setup only supports openSUSE Tumbleweed."
        echo "[INFO] Please install the required runtime dependencies manually by checking INSTALLATION.md, then re-run:"
        echo "       ./install.sh --skip-deps"
        exit 1
    fi

    echo "[INFO] Detected openSUSE Tumbleweed. Checking runtime dependencies..."
    REQUIRED_PKGS=(rclone fuse3 libwebkit2gtk-4_1-0 libayatana-appindicator3-1)
    MISSING_PKGS=()

    for pkg in "${REQUIRED_PKGS[@]}"; do
        if ! rpm -q "${pkg}" >/dev/null 2>&1; then
            MISSING_PKGS+=("${pkg}")
        fi
    done

    if [ "${#MISSING_PKGS[@]}" -gt 0 ]; then
        echo "[INFO] Installing missing runtime packages: ${MISSING_PKGS[*]}"
        sudo zypper install -y "${MISSING_PKGS[@]}"
    else
        echo "[INFO] All required runtime dependencies are already installed."
    fi
else
    echo "[INFO] Skipping OS detection and dependency checks (--skip-deps)."
fi

# ------------------------------------------------------------------------------
# 3. Install Files & Permissions
# ------------------------------------------------------------------------------
INSTALL_DIR="${HOME}/.local/bin/Aksync"
LOCAL_BIN_DIR="${HOME}/.local/bin"

echo "[INFO] Installing AkSync binaries and scripts to ${INSTALL_DIR}..."
mkdir -p "${INSTALL_DIR}"

cp "${BIN_SOURCE_DIR}/mycloud-daemon" "${INSTALL_DIR}/mycloud-daemon"
cp "${BIN_SOURCE_DIR}/mycloud-fuse"   "${INSTALL_DIR}/mycloud-fuse"
cp "${BIN_SOURCE_DIR}/mycloud-gui"    "${INSTALL_DIR}/mycloud-gui"
cp "${SCRIPT_SOURCE_DIR}/aksync-launch.sh" "${INSTALL_DIR}/aksync-launch.sh"
cp "${SCRIPT_SOURCE_DIR}/uninstall.sh"     "${INSTALL_DIR}/uninstall.sh"

chmod 755 \
    "${INSTALL_DIR}/mycloud-daemon" \
    "${INSTALL_DIR}/mycloud-fuse" \
    "${INSTALL_DIR}/mycloud-gui" \
    "${INSTALL_DIR}/aksync-launch.sh" \
    "${INSTALL_DIR}/uninstall.sh"

echo "[INFO] Creating terminal launcher symlink at ${LOCAL_BIN_DIR}/aksync..."
ln -sf "${INSTALL_DIR}/aksync-launch.sh" "${LOCAL_BIN_DIR}/aksync"

# ------------------------------------------------------------------------------
# 4. Desktop Application Menu Entry
# ------------------------------------------------------------------------------
APPLICATIONS_DIR="${HOME}/.local/share/applications"
DESKTOP_FILE="${APPLICATIONS_DIR}/aksync.desktop"

echo "[INFO] Creating application menu shortcut at ${DESKTOP_FILE}..."
mkdir -p "${APPLICATIONS_DIR}"

cat <<EOF > "${DESKTOP_FILE}"
[Desktop Entry]
Type=Application
Name=AkSync
Comment=Encrypted Cloud Drive
Exec=${HOME}/.local/bin/Aksync/aksync-launch.sh
Icon=folder-cloud
Terminal=false
Categories=Utility;Network;
EOF

# ------------------------------------------------------------------------------
# 5. Summary
# ------------------------------------------------------------------------------
echo ""
echo "[SUCCESS] AkSync has been installed successfully!"
echo "[INFO] You can launch AkSync from your Application Menu (AkSync) or from the terminal with:"
echo "       aksync"
echo "[INFO] Make sure ${HOME}/.local/bin is included in your PATH if launching from the terminal."
echo "[INFO] To uninstall AkSync at any time, run:"
echo "       ~/.local/bin/Aksync/uninstall.sh"
