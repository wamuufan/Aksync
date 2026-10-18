#!/usr/bin/env python3
"""
MyCloud Files On-Demand Nautilus Extension
Provides real-time file synchronization status emblems and metadata columns
for GNOME Nautilus by communicating with mycloud-daemon via Unix Domain Socket (UDS).
"""

import json
import os
import socket
import urllib.parse
from gi.repository import GObject, Nautilus

SOCKET_DEFAULT_PATH = os.path.expanduser("~/.cache/mycloud/mycloud.sock")
TARGET_MOUNT_DEFAULT = os.path.expanduser("~/MyCloud")

class MyCloudExtension(GObject.GObject, Nautilus.InfoProvider, Nautilus.ColumnProvider):
    def __init__(self):
        super().__init__()
        self.socket_path = os.environ.get("MYCLOUD_SOCKET_PATH", SOCKET_DEFAULT_PATH)
        self.target_mount = os.path.realpath(os.environ.get("MYCLOUD_TARGET_MOUNT", TARGET_MOUNT_DEFAULT))

    def _query_daemon_status(self):
        """Queries mycloud-daemon status via Unix Domain Socket."""
        if not os.path.exists(self.socket_path):
            return None

        try:
            client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            client.settimeout(0.5)
            client.connect(self.socket_path)
            
            cmd = json.dumps({"cmd": "GetStatus"}) + "\n"
            client.sendall(cmd.encode("utf-8"))

            response_data = b""
            while True:
                chunk = client.recv(4096)
                if not chunk:
                    break
                response_data += chunk
                if b"\n" in chunk:
                    break

            client.close()
            payload = json.loads(response_data.decode("utf-8").strip())
            return payload.get("status")
        except Exception:
            return None

    def update_file_info(self, file):
        """Called by Nautilus to populate emblems and string attributes for files."""
        if file.get_uri_scheme() != "file":
            return

        file_path = os.path.realpath(urllib.parse.unquote(file.get_uri()[7:]))

        # Only apply emblems to files within the user-facing MyCloud mount directory
        if not file_path.startswith(self.target_mount):
            return

        daemon_info = self._query_daemon_status()
        if not daemon_info:
            file.add_emblem("emblem-unreadable")
            file.add_string_attribute("mycloud_status", "Offline")
            return

        state = daemon_info.get("state", "offline")

        if state == "syncing":
            file.add_emblem("emblem-synchronizing")
            file.add_string_attribute("mycloud_status", "Syncing...")
        elif state == "idle":
            file.add_emblem("emblem-default")
            file.add_string_attribute("mycloud_status", "Synced (E2EE)")
        elif state == "paused":
            file.add_emblem("emblem-readonly")
            file.add_string_attribute("mycloud_status", "Sync Paused")
        elif "error" in str(state).lower():
            file.add_emblem("emblem-important")
            file.add_string_attribute("mycloud_status", "Sync Error")
        else:
            file.add_emblem("emblem-default")
            file.add_string_attribute("mycloud_status", "Cloud")

    def get_columns(self):
        """Registers a custom 'MyCloud Status' column in Nautilus detailed list view."""
        return (
            Nautilus.Column(
                name="MyCloudExtension::mycloud_status",
                attribute="mycloud_status",
                label="MyCloud Status",
                description="End-to-End Encrypted Cloud Synchronization Status",
            ),
        )
