# Building AkSync from Source (`HOW-TO-BUILD.md`)

This document provides complete instructions for compiling the AkSync workspace (`mycloud-crypto`, `mycloud-fuse`, `mycloud-daemon`, and `mycloud-gui`) from source on Linux.

---

## 1. Prerequisites & System Dependencies

AkSync links against native Linux system libraries (`fuse3`, `webkit2gtk-4.1`, `gtk3`, `openssl`, `libayatana-appindicator`) for its FUSE filesystem driver and Tauri desktop shell, while compiling its reactive frontend (`Leptos`) to WebAssembly (`wasm32-unknown-unknown`) via **Trunk**.

### A. Install Native Development Packages

#### Ubuntu / Debian (22.04 / 24.04+):
```bash
sudo apt update
sudo apt install -y \
  build-essential \
  pkg-config \
  curl \
  git \
  libfuse3-dev \
  fuse3 \
  rclone \
  libssl-dev \
  libglib2.0-dev \
  libgtk-3-dev \
  libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev
```

#### Arch Linux / CachyOS / Manjaro:
```bash
sudo pacman -S --needed \
  base-devel \
  pkgconf \
  curl \
  git \
  fuse3 \
  rclone \
  openssl \
  glib2 \
  gtk3 \
  libsoup3 \
  webkit2gtk-4.1 \
  libayatana-appindicator \
  librsvg
```

#### Fedora / openSUSE:
```bash
# Fedora:
sudo dnf install -y \
  gcc gcc-c++ make pkgconf-pkg-config \
  fuse3-devel fuse3 rclone openssl-devel \
  gtk3-devel webkit2gtk4.1-devel libayatana-appindicator-gtk3-devel librsvg2-devel

# openSUSE Tumbleweed:
sudo zypper install -y \
  gcc gcc-c++ make pkg-config \
  fuse3-devel fuse3 rclone libopenssl-devel \
  gtk3-devel webkit2gtk3-soup2-devel libayatana-appindicator3-devel librsvg-devel
```

---

### B. Install the Rust Toolchain, WASM Target, and `trunk`

1. **Install Rust (`rustup`) if not already present:**
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source "$HOME/.cargo/env"
   ```

2. **Add the `wasm32-unknown-unknown` compilation target for the Leptos UI:**
   ```bash
   rustup target add wasm32-unknown-unknown
   ```

3. **Install `trunk` (the Rust WebAssembly asset bundler):**
   ```bash
   cargo install --locked trunk
   ```

---

## 2. Automated Production Build (`scripts/build.sh`)

Clone the repository and execute the root build script:

```bash
git clone https://github.com/<YOUR_USERNAME>/AkSync.git
cd AkSync
chmod +x scripts/build.sh
./scripts/build.sh
```

### What `scripts/build.sh` Does Under the Hood
1. **Stage 1 (`crates/mycloud-gui`):** Runs `trunk build --config Trunk.toml --release` to compile the Leptos frontend into an optimized WebAssembly bundle under `crates/mycloud-gui/dist/`.
2. **Stage 2 (Cargo Workspace):** Runs `cargo build --workspace --release` (`opt-level = 3`, `lto = "thin"`, `strip = true`, `codegen-units = 1`), embedding both `optimizations.toml` and the compiled `dist/` frontend assets directly into the native binaries.

Once finished, the stripped production binaries are located in `target/release/`:
- `target/release/mycloud-daemon` — Background orchestration daemon & Unix domain socket IPC server
- `target/release/mycloud-fuse` — Multi-threaded cryptographic FUSE (`/dev/fuse`) filesystem driver
- `target/release/mycloud-gui` — Native desktop UI & system tray application

---

## 3. Manual Build, Verification, and Testing

For development, auditing, or CI verification, you can run each step individually:

```bash
# 1. Run cryptographic unit tests (Format v3 AEAD, Argon2id, DirIV, DEK/KEK wrapping, long filenames):
cargo test -p mycloud-crypto

# 2. Run the full workspace test suite (crypto, FUSE RMW/concurrency, daemon IPC & self-healing):
cargo test --workspace

# 3. Run strict static analysis with Clippy (zero warnings allowed):
cargo clippy --workspace --all-targets -- -D warnings

# 4. Build the Leptos WebAssembly frontend manually:
cd crates/mycloud-gui
trunk build --config Trunk.toml --release
cd ../..

# 5. Build all native workspace binaries in release mode:
cargo build --workspace --release
```

---

## 4. Packaging a Standalone Release Tarball (`tar.gz`)

To package the compiled binaries into a portable archive identical to the GitHub Actions release artifact:

```bash
mkdir -p dist-pkg/aksync-linux-x86_64
cp target/release/mycloud-daemon \
   target/release/mycloud-fuse \
   target/release/mycloud-gui \
   LICENSE README.md INSTALLATION.md HOW-TO-BUILD.md \
   dist-pkg/aksync-linux-x86_64/

tar -czvf aksync-linux-x86_64.tar.gz -C dist-pkg aksync-linux-x86_64
sha256sum aksync-linux-x86_64.tar.gz > aksync-linux-x86_64.tar.gz.sha256
```
