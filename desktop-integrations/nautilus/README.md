# MyCloud Nautilus Extension

Provides GNOME Nautilus file manager emblems and synchronization metadata columns for MyCloud files on Linux.

## Requirements
- GNOME Nautilus (v3 or v4)
- `nautilus-python` (`python3-nautilus` / `nautilus-python`)
- `mycloud-daemon` running with UDS socket active

## Installation

1. Copy `mycloud_extension.py` to the Nautilus extensions directory:
   ```bash
   mkdir -p ~/.local/share/nautilus-python/extensions
   cp mycloud_extension.py ~/.local/share/nautilus-python/extensions/
   ```

2. Restart Nautilus to load the extension:
   ```bash
   nautilus -q
   ```

## Functionality
- Automatically detects files located inside `~/MyCloud/` (or configured mount point).
- Queries `~/.cache/mycloud/mycloud.sock` in real-time via non-blocking Unix Domain Socket communication.
- Displays synchronization emblems:
  - `emblem-synchronizing`: File is currently syncing to/from cloud VFS.
  - `emblem-default`: File is synchronized and encrypted with E2EE.
  - `emblem-readonly`: Sync is paused.
  - `emblem-important`: Sync error.
  - `emblem-unreadable`: Daemon offline.
- Adds custom "MyCloud Status" column in detailed list view.
