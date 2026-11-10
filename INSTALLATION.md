# Installation (`INSTALLATION.md`)

## 1. Runtime Dependencies

AkSync requires the following runtime packages on your system:
- `rclone`
- `fuse3`
- `libwebkit2gtk-4_1-0`
- `libayatana-appindicator3-1`

### openSUSE Tumbleweed
```bash
sudo zypper install -y rclone fuse3 libwebkit2gtk-4_1-0 libayatana-appindicator3-1
```

---

## A. Prebuilt Release Package

### 1. Quick Installation
1. Download `aksync-linux-x86_64.tar.gz` from the [GitHub Releases](../../releases) page.
2. Extract the archive, enter the directory, and run `install.sh`:

```bash
tar -xzvf aksync-linux-x86_64.tar.gz
cd aksync-linux-x86_64
chmod +x install.sh
./install.sh
```

---

## B. Build from Source

### 1. Build Dependencies

Install the native build dependencies and `rustup` on **openSUSE Tumbleweed**:
```bash
sudo zypper install -y \
  gcc gcc-c++ make pkg-config curl git \
  fuse3-devel fuse3 rclone libopenssl-devel \
  gtk3-devel webkit2gtk3-soup2-devel libayatana-appindicator3-devel librsvg-devel \
  rustup
```

Initialize the Rust toolchain, add the WebAssembly target, and install `trunk`:
```bash
rustup default stable
rustup target add wasm32-unknown-unknown
cargo install --locked trunk
```

### 2. Compilation

Clone the repository and run the build script (which compiles the binaries and generates `aksync-linux-x86_64.tar.gz`):
```bash
git clone https://github.com/Akcali/Aksync.git
cd Aksync
chmod +x scripts/build.sh
./scripts/build.sh
```

### 3. Extract and Install the Built Package

Extract the generated `aksync-linux-x86_64.tar.gz` archive to `/tmp` and run the installer:
```bash
mkdir -p /tmp/aksync-install
tar -xzvf aksync-linux-x86_64.tar.gz -C /tmp/aksync-install
cd /tmp/aksync-install/aksync-linux-x86_64
./install.sh
```

---

## Launch & Uninstall

- **Launch:** Open **AkSync** from your Application Menu or run `aksync` in the terminal.
- **Uninstall:**
  ```bash
  ~/.local/bin/Aksync/uninstall.sh
  ```
