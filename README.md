# AkSync

**Zero-Knowledge, End-to-End Encrypted Cloud Drive & Two-Tier Virtual Filesystem (VFS) for Linux**

[![License: BSD-2-Clause](https://img.shields.io/badge/License-BSD--2--Clause-blue.svg)](./LICENSE)
[![Language: Pure Rust](https://img.shields.io/badge/Language-100%25%20Rust-orange.svg)](https://www.rust-lang.org/)
[![Cryptography: XChaCha20-Poly1305](https://img.shields.io/badge/AEAD-XChaCha20--Poly1305%20%2B%20Argon2id-success.svg)](#3-cryptographic-architecture-format-v3--aks3)
[![Desktop: KDE Plasma & GNOME](https://img.shields.io/badge/Linux-KDE%20Plasma%20%7C%20GNOME-informational.svg)](#4-desktop-environment-aware-native-ui-kde-breeze--gnome-adwaita)

**AkSync** is a **100% pure Rust** zero-knowledge, end-to-end encrypted (E2EE) cloud filesystem and desktop orchestration suite for Linux. It layers a high-concurrency cryptographic FUSE filesystem (`mycloud-fuse`) directly on top of an isolated `rclone` VFS transport layer, providing transparent POSIX file access over Google Drive without ever exposing plaintext data, filenames, or cryptographic keys to the cloud provider or the underlying transport process.

From the kernel-facing FUSE driver (`mycloud-fuse`) and self-healing orchestration daemon (`mycloud-daemon`) to the WebAssembly reactive desktop frontend (`Leptos` + `Tauri`), the entire codebase contains **zero lines of JavaScript and no `node_modules`**.

---

## Table of Contents
1. [Architectural Overview: The Two-Tier VFS Pipeline](#1-architectural-overview-the-two-tier-vfs-pipeline)
2. [Systems Engineering: How AkSync Orchestrates the Underlying Stack](#2-systems-engineering-how-aksync-orchestrates-the-underlying-stack)
3. [Cryptographic Architecture (Format v3 / `AKS3`)](#3-cryptographic-architecture-format-v3--aks3)
4. [Desktop-Environment-Aware Native UI (KDE Breeze & GNOME Adwaita)](#4-desktop-environment-aware-native-ui-kde-breeze--gnome-adwaita)
5. [Installation & Building from Source](#5-installation--building-from-source)
6. [Repository Layout](#6-repository-layout)
7. [License & Legal Notice](#7-license--legal-notice)

---

## 1. Architectural Overview: The Two-Tier VFS Pipeline

AkSync deliberately avoids reinventing cloud storage REST API clients. Cloud provider rate-limiting, exponential pacing, chunked multipart uploads, connection pooling, and sparse VFS caching are already solved at production scale by **`rclone`**.

Instead, AkSync treats `rclone` strictly as an **untrusted ciphertext transport layer** and mounts its own multi-threaded cryptographic FUSE engine (**`mycloud-fuse`**) on top of it:

```text
[User Applications / KDE Dolphin / GNOME Files / LibreOffice]
       │
       │ (1. Plaintext POSIX VFS syscalls: open, read, pwrite, rename, symlink, statfs)
       ▼
~/.local/share/mycloud/vault          ◄── [mycloud-fuse] (Rust `fuser` + `/dev/fuse`)
       │
       │ (2. On-the-fly 64 KiB AEAD chunking + Base32 filename encryption + .aksync_diriv)
       ▼
$XDG_RUNTIME_DIR/mycloud/raw_vault    ◄── [rclone mount] (VFS Cache Mode: writes)
       │
       │ (3. Observes ONLY opaque AKS3 ciphertext blocks & Base32 names; syncs to cloud)
       ▼
[Google Drive (`MyCloud_<user>_<host>` remote vault)]
```

### Why This Separation of Concerns Matters
- **Strict Zero-Knowledge Boundary:** `rclone` never possesses your master passphrase, Key Encryption Key (`KEK`), Data Master Key (`DMK`), or plaintext files. The lower mount point at `$XDG_RUNTIME_DIR/mycloud/raw_vault` (backed by RAM-resident `tmpfs`) exposes exclusively authenticated `AKS3` ciphertext frames and Base32-encoded encrypted filenames.
- **Transparent POSIX Semantics:** When an application writes to `~/.local/share/mycloud/vault`, the Linux kernel routes the request through `/dev/fuse` to `mycloud-fuse`. The FUSE worker pool encrypts the stream into 64 KiB authenticated chunks in memory and commits them to `raw_vault`, where `rclone`'s write-back VFS cache (`--vfs-cache-mode writes`) asynchronously flushes the ciphertext to Google Drive.

---

## 2. Systems Engineering: How AkSync Orchestrates the Underlying Stack

Naive wrapper utilities spawn background processes blindly (`Command::new("rclone").spawn()`) and inevitably lock up the Linux desktop when network latency spikes or a mount drops. AkSync enforces five kernel- and OS-level invariants across its sub-processes:

### A. Two-Phase Mount Verification (`/proc/self/mountinfo` + RC Handshake + `FUSE_SUPER_MAGIC`)
If `mycloud-fuse` were mounted before `rclone` finished establishing the remote VFS mount, encrypted writes would silently fall through onto the local underlying directory rather than the cloud transport.
- [`crates/mycloud-daemon/src/process.rs`](crates/mycloud-daemon/src/process.rs) (`wait_for_mount_ready`) prevents split-brain mounts via a three-stage verification gate:
  1. **Kernel Mount Table Inspection:** Parses `/proc/self/mountinfo` (`is_mountpoint_ready`), handling octal escape sequences (such as `\040` for spaces), to confirm the kernel has registered `raw_vault` as an active `fuse.rclone` mount.
  2. **Authenticated RC Readiness Probe:** Polls `rclone`'s loopback Remote Control HTTP API (`127.0.0.1:5572`) using ephemeral `Basic Auth` credentials (`wait_for_rclone_rc_ready`) until `vfs/stats` returns a healthy operational payload.
  3. **Superblock Magic Verification:** Inside [`crates/mycloud-fuse/src/main.rs`](crates/mycloud-fuse/src/main.rs), `mycloud-fuse` issues `libc::statfs` against the backing store and verifies `f_type == FUSE_SUPER_MAGIC (0x65735546)` before serving I/O.

### B. Desktop Freeze Prevention (`WorkerPool` + `RawMountHealth` Circuit Breaker + Dolphin Rules)
Network-backed FUSE filesystems are notorious on Linux for freezing file managers (`D` state / uninterruptible sleep) when upstream connections stall or when desktop indexers aggressively generate thumbnails.
1. **8-Thread Bounded Worker Pool & Timeout Scheduler (`mycloud-fuse`):** Incoming kernel FUSE requests are dispatched via `crossbeam-channel` across an 8-thread `WorkerPool`. A dedicated `TimeoutScheduler` tracks every in-flight operation; if the backing store stalls beyond the configured deadline (15s), an atomic `ReplyGuard` immediately returns `EIO` to the kernel, guaranteeing that every FUSE request is answered exactly once and preventing desktop hangs.
2. **Circuit Breaker (`RawMountHealth`):** If the underlying `rclone` mount disconnects or returns `ENOTCONN`, `RawMountHealth` trips a 15-second cooldown circuit breaker, short-circuiting subsequent I/O with immediate `EIO` responses instead of queueing doomed disk operations.
3. **KDE Dolphin Thumbnail & Indexer Throttling:** `apply_folder_freeze_prevention_rules` injects `.directory` (`Preview=false`) and `.nomedia` metadata into the mounted vault so KDE `kio-extras` / `thumbnail.so` workers do not saturate cloud bandwidth attempting to preview gigabytes of encrypted media.
4. **Self-Healing Process Supervisor:** `mycloud-daemon` continuously monitors child PIDs and mount health, automatically respawning crashed `rclone` or `mycloud-fuse` instances using bounded exponential backoff (`2s -> 4s -> 8s ... max 60s`).

### C. Zero-Leak Key Transport & Process Hardening (`--key-stdin` + `mlock` + `PR_SET_DUMPABLE`)
Passing cryptographic keys via CLI arguments (`--key-hex`) or environment variables exposes secrets to any local unprivileged process reading `/proc/<PID>/cmdline` or `/proc/<PID>/environ`.
- **Anonymous Pipe Key Handoff:** `spawn_fuse_mount` launches `mycloud-fuse` strictly with `--key-stdin`, writing the 32-byte Data Master Key across an anonymous kernel pipe (`Stdio::piped()`) and immediately closing the descriptor.
- **Kernel Memory Locking & Dump Protection:** Upon startup, both `mycloud-daemon` and `mycloud-fuse` invoke:
  - `libc::prctl(libc::PR_SET_DUMPABLE, 0)` to disable core dumps and block unprivileged `ptrace` attachment.
  - `libc::mlock()` to pin key material pages in physical RAM (`VmLck` in `/proc/<PID>/status`), preventing the kernel from paging sensitive keys out to unencrypted disk swap.
  - Automatic memory scrubbing via `zeroize::Zeroizing<[u8; 32]>` whenever the vault is locked (`LockVault`) or the daemon terminates.

### D. Native KDE Plasma & GNOME Integration (`KWallet` + `user-places.xbel` + Live Theme Sync)
- **KDE KWallet & Loopback WebDAV Sync:** `read_kwallet_webdav_password` queries KDE's wallet subsystem (`kwallet-query`) to synchronize Dolphin's cached WebDAV credentials with a `0600`-permissioned SHA-1 `.htpasswd` file bound strictly to `127.0.0.1`.
- **Dolphin Places Integration:** `update_kde_places` dynamically registers the mounted vault in `~/.local/share/user-places.xbel` with native `folder-cloud` iconography.
- **Live System Palette Extraction:** Reads `~/.config/kdeglobals` and GTK `gsettings` at runtime to extract the active desktop environment, dark/light scheme, and system accent color.

### E. One-Way Encrypted Home Directory Backup Bridge (`home_sync.rs`)
When you select user directories (e.g., `Documents`, `Desktop`, `Pictures`) for automatic backup:
- `get_user_home_folders` parses `~/.config/user-dirs.dirs` (FreeDesktop XDG User Directories specification) to resolve localized directory paths automatically.
- `execute_home_sync` synchronizes selected folders directly into the **mounted FUSE vault (`~/.local/share/mycloud/vault/home/...`)**, ensuring every file passes through `mycloud-fuse` for chunked `AKS3` encryption before touching the cloud cache, followed by an authenticated `vfs/refresh` RC notification.

---

## 3. Cryptographic Architecture (Format v3 / `AKS3`)

The cryptographic engine ([`crates/mycloud-crypto`](crates/mycloud-crypto)) implements an authenticated, misuse-resistant AEAD construction:

| Layer | Primitive / Construction | Specification |
| :--- | :--- | :--- |
| **Key Derivation (KDF)** | `Argon2id` | Derives a 256-bit Key Encryption Key (`KEK`) from the user's master passphrase and a 128-bit random salt. |
| **Envelope Encryption (DEK/KEK)** | `XChaCha20-Poly1305` | A randomly generated 256-bit Data Master Key (`DMK`) is wrapped (`wrapped_dmk`) under both the passphrase-derived `KEK` and a 52-character Base32 **Recovery Key**. Enables **instant (~0.01s) passphrase rotation** without re-encrypting stored files. |
| **Subkey Domain Separation** | `BLAKE2b-MAC-512` | Derives four independent 256-bit subkeys from the `DMK`: `k_data` (chunk payload), `k_hdr` (header authentication), `k_name` (filenames & `DirIV`), and `k_canary` (vault verification token). |
| **Authenticated File Header (64 B)** | `AKS3` + `BLAKE2b-256` | `Magic ("AKS3", 4 B)` + `Version (0x03, 1 B)` + `Flags (3 B)` + `Random FileID (24 B)` + `Keyed BLAKE2b-256 MAC (32 B)`. Verified in constant time. |
| **Chunk AEAD (64 KiB Plaintext)** | `XChaCha20-Poly1305` | Each 64 KiB chunk is encrypted with a fresh 24-byte `OsRng` nonce and a 16-byte Poly1305 authentication tag (`65,576 B` on disk). Partial writes (*Read-Modify-Write*) always regenerate a fresh random chunk nonce. |
| **Contextual AAD (33 B)** | Per-Chunk Binding | Every chunk's Poly1305 tag authenticates `file_id (24 B) \|\| LE64(chunk_index) (8 B) \|\| is_last (1 B)`. Defeats in-file block reordering, cross-file chunk splicing, and truncation attacks, returning `libc::EIO` on any mismatch. |
| **Directory-Bound Filenames** | `DirIV` + `Base32` | Each directory contains an encrypted `.aksync_diriv` storing a unique 16-byte `dir_id`. Filenames are encrypted with `k_name` using `dir_id` as AAD, enabling `O(1)` atomic directory renames. Filenames exceeding 119 bytes (up to POSIX `255 B`) use `AKS_LN_<hash>` + `.name` sidecars. |

---

## 4. Desktop-Environment-Aware Native UI (KDE Breeze & GNOME Adwaita)

AkSync features a clean, edge-to-edge split-view native utility interface that adapts to the active Linux desktop environment:
- **KDE Plasma (`.de-kde`):** Crisp `4px` widget radii, `1px` bordered Breeze buttons, left-accented navigation indicators, and native `kdeglobals` color synchronization.
- **GNOME / GTK (`.de-gnome`):** `6px`/`8px` Libadwaita widget radii, flat filled button styling, and pill-shaped sidebar navigation items.
- **Live Interface Scaling:** Supports instant `90%`, `100%`, `110%`, `125%`, and `150%` UI scaling via the Settings tab or `Ctrl + +`, `Ctrl + -`, and `Ctrl + 0` keyboard shortcuts.

---

## 5. Installation & Building from Source

- 📦 **[Installation Guide (`INSTALLATION.md`)](./INSTALLATION.md)** — Instructions for installing prebuilt release archives.
- 🛠️ **[Build Guide (`HOW-TO-BUILD.md`)](./HOW-TO-BUILD.md)** — System dependencies, `wasm32-unknown-unknown` / `trunk` setup, and building native release binaries from source.

```bash
# One-step production release build:
./scripts/build.sh
```

---

## 6. Repository Layout

```text
AkSync/
├── crates/
│   ├── mycloud-crypto/        # Format v3 (AKS3) XChaCha20-Poly1305, Argon2id, DirIV, DEK/KEK envelope encryption
│   ├── mycloud-fuse/          # Multi-threaded FUSE (/dev/fuse) cryptographic filesystem driver
│   ├── mycloud-daemon/        # Background orchestrator, Unix IPC server, rclone/WebDAV/KWallet/HomeSync supervisor
│   └── mycloud-gui/           # Leptos (WASM) reactive frontend + src-tauri/ native desktop & system tray shell
├── scripts/
│   └── build.sh               # Automated production build script (Trunk WASM + Cargo workspace release)
├── optimizations.toml         # Compile-time embedded rclone VFS, caching, and network tuning parameters
├── INSTALLATION.md            # End-user installation guide
├── HOW-TO-BUILD.md            # Developer build instructions
└── LICENSE                    # BSD 2-Clause Simplified License
```

---

## 7. License & Legal Notice

Distributed under the **[BSD 2-Clause "Simplified" License](./LICENSE)**.
- All Rust crate dependencies (`chacha20poly1305`, `argon2`, `blake2`, `fuser`, `tokio`, `tauri`, `leptos`) are licensed under permissive open-source licenses (`MIT`, `Apache-2.0`, `BSD`).
- External system utilities (`rclone` and `fusermount3`) are invoked strictly as separate user-space OS processes via standard CLI/IPC interfaces without static linking.
