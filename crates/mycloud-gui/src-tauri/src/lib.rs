use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    Manager,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::info;

/// High-level synchronization states matching mycloud-daemon
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    SetupRequired,
    Locked,
    Connecting,
    Idle,
    Syncing,
    Paused,
    Error(String),
    Offline,
}

/// Daemon status response payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatusPayload {
    pub state: SyncState,
    pub rclone_pid: Option<u32>,
    pub fuse_pid: Option<u32>,
    pub raw_mount_path: String,
    pub target_mount_path: String,
}

/// Setup status response payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupStatusPayload {
    pub is_complete: bool,
    pub rclone_configured: bool,
    pub vault_initialized: bool,
    pub remote_name: String,
}

/// Folder item metadata for home folders sync selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncFolderInfo {
    pub name: String,
    pub rel_path: String,
    pub abs_path: String,
    pub selected: bool,
    pub icon: String,
}

/// Custom Google Drive API credentials for rclone
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DriveApiCredentialsPayload {
    #[serde(alias = "client_id")]
    pub client_id: String,
    #[serde(alias = "client_secret")]
    pub client_secret: String,
}

/// Generic Daemon IPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum DaemonIpcResponse {
    Status(DaemonStatusPayload),
    SetupStatus(SetupStatusPayload),
    TransferStats(TransferStats),
    SyncFolders(Vec<SyncFolderInfo>),
    SyncHiddenFiles(bool),
    DriveApiCredentials(DriveApiCredentialsPayload),
    Ok(String),
    Err(String),
}

/// Transfer speeds and storage statistics for frontend dashboard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferStats {
    pub upload_speed: u64,   // Bytes per second
    pub download_speed: u64, // Bytes per second
    pub storage_used: u64,   // Bytes used
    pub storage_total: u64,  // Total quota bytes (e.g. 100 GB)
    pub files_synced: u64,
}

/// Dynamic system desktop theme & color palette info extracted from KDE Plasma / GTK themes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SystemThemeInfo {
    pub desktop_env: String,
    pub theme_name: String,
    pub is_dark: bool,
    pub accent_color: String,
    pub window_bg: String,
    pub view_bg: String,
    pub card_bg: String,
    pub text_primary: String,
    pub text_secondary: String,
    pub border_color: String,
    pub is_fallback: bool,
}

/// Shared state inside the Tauri GUI application
pub struct GuiState {
    pub socket_path: PathBuf,
    pub is_paused: AtomicBool,
    pub dashboard_item: std::sync::Mutex<Option<MenuItem<tauri::Wry>>>,
    pub folder_item: std::sync::Mutex<Option<MenuItem<tauri::Wry>>>,
    pub sync_item: std::sync::Mutex<Option<MenuItem<tauri::Wry>>>,
    pub pause_item: std::sync::Mutex<Option<MenuItem<tauri::Wry>>>,
}

impl Default for GuiState {
    fn default() -> Self {
        Self::new()
    }
}

impl GuiState {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let default_sock = PathBuf::from(home).join(".cache/mycloud/mycloud.sock");
        let socket_path = std::env::var("MYCLOUD_SOCKET_PATH")
            .map(PathBuf::from)
            .unwrap_or(default_sock);

        Self {
            socket_path,
            is_paused: AtomicBool::new(false),
            dashboard_item: std::sync::Mutex::new(None),
            folder_item: std::sync::Mutex::new(None),
            sync_item: std::sync::Mutex::new(None),
            pause_item: std::sync::Mutex::new(None),
        }
    }

    /// Dynamically updates "Open Dashboard" / "Hide Dashboard" tray menu label
    pub fn update_dashboard_item_label(&self, is_open: bool) {
        if let Ok(guard) = self.dashboard_item.lock() {
            if let Some(ref item) = *guard {
                let label = if is_open {
                    "Hide Dashboard"
                } else {
                    "Open Dashboard"
                };
                let _ = item.set_text(label);
            }
        }
    }

    /// Dynamically updates enabled/disabled state of tray actions based on vault lock status
    pub fn update_tray_state(&self, state: &SyncState) {
        let is_unlocked = matches!(state, SyncState::Idle | SyncState::Syncing | SyncState::Paused);
        if let Ok(guard) = self.folder_item.lock() {
            if let Some(ref item) = *guard {
                let _ = item.set_enabled(is_unlocked);
            }
        }
        if let Ok(guard) = self.sync_item.lock() {
            if let Some(ref item) = *guard {
                let _ = item.set_enabled(is_unlocked);
            }
        }
        if let Ok(guard) = self.pause_item.lock() {
            if let Some(ref item) = *guard {
                let _ = item.set_enabled(is_unlocked);
                let is_paused = matches!(state, SyncState::Paused);
                let label = if is_paused {
                    "Resume Sync"
                } else {
                    "Pause Sync"
                };
                let _ = item.set_text(label);
            }
        }
    }

    /// Queries the daemon via Unix Domain Socket
    pub async fn query_daemon(&self, cmd: &str) -> Result<String, String> {
        if !self.socket_path.exists() {
            return Err("Daemon socket does not exist (offline)".to_string());
        }

        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| format!("Failed to connect to UDS: {e}"))?;

        let (reader, mut writer) = stream.split();
        let mut buf_reader = BufReader::new(reader);

        let mut command_line = cmd.to_string();
        if !command_line.ends_with('\n') {
            command_line.push('\n');
        }

        writer
            .write_all(command_line.as_bytes())
            .await
            .map_err(|e| format!("UDS write error: {e}"))?;
        writer
            .flush()
            .await
            .map_err(|e| format!("UDS flush error: {e}"))?;

        let mut response = String::new();
        buf_reader
            .read_line(&mut response)
            .await
            .map_err(|e| format!("UDS read error: {e}"))?;

        Ok(response.trim().to_string())
    }
}

// ----------------- Tauri Commands -----------------

pub mod commands {
    use super::*;

    /// Returns the current synchronization status from mycloud-daemon
    #[tauri::command]
    pub async fn get_sync_state(
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<DaemonStatusPayload, String> {
        match state.query_daemon("{\"cmd\":\"GetStatus\"}").await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::Status(status) => {
                        let is_paused = matches!(status.state, SyncState::Paused);
                        state.is_paused.store(is_paused, Ordering::SeqCst);
                        state.update_tray_state(&status.state);
                        Ok(status)
                    }
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(_) => {
                state.update_tray_state(&SyncState::Offline);
                let home = std::env::var("HOME").unwrap_or_default();
                Ok(DaemonStatusPayload {
                    state: SyncState::Offline,
                    rclone_pid: None,
                    fuse_pid: None,
                    raw_mount_path: format!("{home}/.cache/mycloud/raw_mount"),
                    target_mount_path: format!("{home}/MyCloud"),
                })
            }
        }
    }

    /// Checks whether the initial setup (Drive OAuth & Vault) is complete
    #[tauri::command]
    pub async fn check_setup_status(
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<SetupStatusPayload, String> {
        match state.query_daemon("{\"cmd\":\"CheckSetupStatus\"}").await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::SetupStatus(payload) => Ok(payload),
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(e) => Err(format!("Daemon offline: {e}")),
        }
    }

    /// Triggers Google Drive OAuth authorization via rclone
    #[tauri::command]
    pub async fn trigger_drive_auth(
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<String, String> {
        match state.query_daemon("{\"cmd\":\"TriggerDriveAuth\"}").await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::Ok(msg) => Ok(msg),
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(e) => Err(format!("Failed to contact daemon: {e}")),
        }
    }

    /// Initializes cryptographic vault with master password
    #[tauri::command]
    pub async fn initialize_vault(
        password: String,
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<String, String> {
        let cmd = serde_json::to_string(&serde_json::json!({
            "cmd": "InitializeVault",
            "data": password
        }))
        .map_err(|e| e.to_string())?;

        match state.query_daemon(&cmd).await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::Ok(msg) => {
                        state.update_tray_state(&SyncState::Idle);
                        Ok(msg)
                    }
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(e) => Err(format!("Failed to contact daemon: {e}")),
        }
    }

    /// Unlocks an existing initialized cryptographic vault with master password
    #[tauri::command]
    pub async fn unlock_vault(
        password: String,
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<String, String> {
        let cmd = serde_json::to_string(&serde_json::json!({
            "cmd": "UnlockVault",
            "data": password
        }))
        .map_err(|e| e.to_string())?;

        match state.query_daemon(&cmd).await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::Ok(msg) => {
                        state.update_tray_state(&SyncState::Idle);
                        Ok(msg)
                    }
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(e) => Err(format!("Failed to contact daemon: {e}")),
        }
    }

    /// Locks cryptographic vault and drops master key from memory
    #[tauri::command]
    pub async fn lock_vault(
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<String, String> {
        match state.query_daemon("{\"cmd\":\"LockVault\"}").await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::Ok(msg) => {
                        state.update_tray_state(&SyncState::Locked);
                        Ok(msg)
                    }
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(e) => Err(format!("Failed to contact daemon: {e}")),
        }
    }

    /// Changes cryptographic vault master passphrase in 0.01s via DEK/KEK wrapping
    #[tauri::command]
    pub async fn change_passphrase(
        current_password: String,
        new_password: String,
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<String, String> {
        let cmd = serde_json::to_string(&serde_json::json!({
            "cmd": "ChangePassphrase",
            "data": {
                "current_password": current_password,
                "new_password": new_password
            }
        }))
        .map_err(|e| e.to_string())?;

        match state.query_daemon(&cmd).await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::Ok(msg) => Ok(msg),
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(e) => Err(format!("Failed to contact daemon: {e}")),
        }
    }

    /// Recovers vault using 52-character recovery key and sets new master password
    #[tauri::command]
    pub async fn recover_vault(
        recovery_key: String,
        new_password: String,
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<String, String> {
        let cmd = serde_json::to_string(&serde_json::json!({
            "cmd": "RecoverVault",
            "data": {
                "recovery_key": recovery_key,
                "new_password": new_password
            }
        }))
        .map_err(|e| e.to_string())?;

        match state.query_daemon(&cmd).await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::Ok(msg) => {
                        state.update_tray_state(&SyncState::Idle);
                        Ok(msg)
                    }
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(e) => Err(format!("Failed to contact daemon: {e}")),
        }
    }

    /// Returns live transfer speeds and storage statistics
    #[tauri::command]
    pub async fn get_transfer_stats(
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<TransferStats, String> {
        match state.query_daemon("{\"cmd\":\"GetTransferStats\"}").await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::TransferStats(stats) => Ok(stats),
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(_) => Ok(TransferStats {
                upload_speed: 0,
                download_speed: 0,
                storage_used: 0,
                storage_total: 0,
                files_synced: 0,
            }),
        }
    }

    /// Toggles sync pause / resume via daemon IPC
    #[tauri::command]
    pub async fn toggle_pause(
        paused: bool,
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<String, String> {
        state.is_paused.store(paused, Ordering::SeqCst);
        if let Ok(guard) = state.pause_item.lock() {
            if let Some(ref item) = *guard {
                let label = if paused { "Resume Sync" } else { "Pause Sync" };
                let _ = item.set_text(label);
            }
        }
        let cmd = format!("{{\"cmd\":\"SetPaused\",\"data\":{paused}}}");
        state.query_daemon(&cmd).await
    }

    /// Triggers a manual synchronization cycle via daemon IPC
    #[tauri::command]
    pub async fn trigger_sync(state: tauri::State<'_, Arc<GuiState>>) -> Result<String, String> {
        state.query_daemon("{\"cmd\":\"TriggerSync\"}").await
    }

    /// Opens the user-facing decrypted target mount directory in the desktop file manager
    #[tauri::command]
    pub async fn open_cloud_folder() -> Result<(), String> {
        let home = std::env::var("HOME").map_err(|e| e.to_string())?;
        let path = format!("{home}/.local/share/mycloud/vault");
        if !std::path::Path::new(&path).exists() {
            let _ = std::fs::create_dir_all(&path);
        }
        std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {e}"))?;
        Ok(())
    }

    /// Retrieves discoverable user home directories and their sync status
    #[tauri::command]
    pub async fn get_sync_folders(
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<Vec<SyncFolderInfo>, String> {
        match state.query_daemon("{\"cmd\":\"GetSyncFolders\"}").await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::SyncFolders(folders) => Ok(folders),
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(e) => Err(format!("Daemon offline: {e}")),
        }
    }

    /// Sets user home directories to synchronize with Google Drive
    #[tauri::command]
    pub async fn set_sync_folders(
        folders: Vec<String>,
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<String, String> {
        let cmd = serde_json::to_string(&serde_json::json!({
            "cmd": "SetSyncFolders",
            "data": folders
        }))
        .map_err(|e| e.to_string())?;

        match state.query_daemon(&cmd).await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::Ok(msg) => Ok(msg),
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(e) => Err(format!("Failed to contact daemon: {e}")),
        }
    }

    /// Retrieves whether hidden files should be synchronized
    #[tauri::command]
    pub async fn get_sync_hidden_files(
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<bool, String> {
        match state.query_daemon("{\"cmd\":\"GetSyncHiddenFiles\"}").await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::SyncHiddenFiles(val) => Ok(val),
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(_) => Ok(false),
        }
    }

    /// Sets whether hidden files should be synchronized
    #[tauri::command]
    pub async fn set_sync_hidden_files(
        enabled: bool,
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<String, String> {
        let cmd = serde_json::to_string(&serde_json::json!({
            "cmd": "SetSyncHiddenFiles",
            "data": enabled
        }))
        .map_err(|e| e.to_string())?;

        match state.query_daemon(&cmd).await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::Ok(msg) => Ok(msg),
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(e) => Err(format!("Failed to contact daemon: {e}")),
        }
    }

    /// Retrieves custom Google Drive API credentials
    #[tauri::command]
    pub async fn get_drive_api_credentials(
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<DriveApiCredentialsPayload, String> {
        match state.query_daemon("{\"cmd\":\"GetDriveApiCredentials\"}").await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::DriveApiCredentials(payload) => Ok(payload),
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(e) => Err(format!("Daemon offline: {e}")),
        }
    }

    /// Sets custom Google Drive API credentials
    #[tauri::command]
    pub async fn set_drive_api_credentials(
        client_id: String,
        client_secret: String,
        state: tauri::State<'_, Arc<GuiState>>,
    ) -> Result<String, String> {
        let cmd = serde_json::to_string(&serde_json::json!({
            "cmd": "SetDriveApiCredentials",
            "data": {
                "client_id": client_id,
                "client_secret": client_secret
            }
        }))
        .map_err(|e| e.to_string())?;

        match state.query_daemon(&cmd).await {
            Ok(raw_json) => {
                let parsed: DaemonIpcResponse = serde_json::from_str(&raw_json)
                    .map_err(|e| format!("JSON parse error: {e}"))?;
                match parsed {
                    DaemonIpcResponse::Ok(msg) => Ok(msg),
                    DaemonIpcResponse::Err(e) => Err(e),
                    _ => Err("Unexpected response type from daemon".to_string()),
                }
            }
            Err(e) => Err(format!("Failed to contact daemon: {e}")),
        }
    }

    fn parse_rgb_to_hex(val: &str) -> Option<String> {
        let clean = val.trim();
        if clean.starts_with('#') {
            return Some(clean.to_string());
        }
        let parts: Vec<&str> = clean.split(',').collect();
        if parts.len() == 3 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                parts[0].trim().parse::<u8>(),
                parts[1].trim().parse::<u8>(),
                parts[2].trim().parse::<u8>(),
            ) {
                return Some(format!("#{:02x}{:02x}{:02x}", r, g, b));
            }
        }
        None
    }

    fn rgb_luminance(val: &str) -> Option<f32> {
        let clean = val.trim();
        let parts: Vec<&str> = clean.split(',').collect();
        if parts.len() == 3 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                parts[0].trim().parse::<f32>(),
                parts[1].trim().parse::<f32>(),
                parts[2].trim().parse::<f32>(),
            ) {
                return Some(0.299 * r + 0.587 * g + 0.114 * b);
            }
        }
        None
    }

    fn detect_kde_theme(home: &str) -> Option<SystemThemeInfo> {
        let kdeglobals_path = std::path::Path::new(home).join(".config/kdeglobals");
        let content = std::fs::read_to_string(kdeglobals_path).ok()?;

        let mut current_section = String::new();
        let mut color_scheme = String::new();
        let mut look_and_feel = String::new();
        let mut accent_color = None;
        let mut window_bg = None;
        let mut window_fg = None;
        let mut view_bg = None;
        let mut button_bg = None;
        let mut selection_bg = None;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                current_section = trimmed[1..trimmed.len() - 1].to_string();
                continue;
            }
            if let Some((k, v)) = trimmed.split_once('=') {
                let key = k.trim();
                let val = v.trim();
                match (current_section.as_str(), key) {
                    ("General", "ColorScheme") => color_scheme = val.to_string(),
                    ("General", "AccentColor") => accent_color = parse_rgb_to_hex(val),
                    ("KDE", "LookAndFeelPackage") => look_and_feel = val.to_string(),
                    ("Colors:Window", "BackgroundNormal") => window_bg = Some(val.to_string()),
                    ("Colors:Window", "ForegroundNormal") => window_fg = Some(val.to_string()),
                    ("Colors:View", "BackgroundNormal") => view_bg = Some(val.to_string()),
                    ("Colors:Button", "BackgroundNormal") => button_bg = Some(val.to_string()),
                    ("Colors:Selection", "BackgroundNormal") => selection_bg = Some(val.to_string()),
                    _ => {}
                }
            }
        }

        // If color scheme was named, also check if there is a .colors file in color-schemes directories
        let scheme_name = if !color_scheme.is_empty() {
            color_scheme.clone()
        } else if !look_and_feel.is_empty() {
            look_and_feel.replace("org.kde.", "").replace(".desktop", "")
        } else {
            String::new()
        };

        if (window_bg.is_none() || selection_bg.is_none()) && !scheme_name.is_empty() {
            let scheme_paths = [
                format!("{home}/.local/share/color-schemes/{scheme_name}.colors"),
                format!("/usr/share/color-schemes/{scheme_name}.colors"),
                format!("/usr/share/color-schemes/{scheme_name}Dark.colors"),
                "/usr/share/color-schemes/BreezeDark.colors".to_string(),
                "/usr/share/color-schemes/BreezeLight.colors".to_string(),
            ];
            for path in &scheme_paths {
                if let Ok(scheme_content) = std::fs::read_to_string(path) {
                    let mut sec = String::new();
                    for l in scheme_content.lines() {
                        let t = l.trim();
                        if t.starts_with('[') && t.ends_with(']') {
                            sec = t[1..t.len() - 1].to_string();
                            continue;
                        }
                        if let Some((k, v)) = t.split_once('=') {
                            let key = k.trim();
                            let val = v.trim();
                            match (sec.as_str(), key) {
                                ("Colors:Window", "BackgroundNormal") if window_bg.is_none() => window_bg = Some(val.to_string()),
                                ("Colors:Window", "ForegroundNormal") if window_fg.is_none() => window_fg = Some(val.to_string()),
                                ("Colors:View", "BackgroundNormal") if view_bg.is_none() => view_bg = Some(val.to_string()),
                                ("Colors:Button", "BackgroundNormal") if button_bg.is_none() => button_bg = Some(val.to_string()),
                                ("Colors:Selection", "BackgroundNormal") if selection_bg.is_none() => selection_bg = Some(val.to_string()),
                                _ => {}
                            }
                        }
                    }
                    break;
                }
            }
        }

        // Check dark mode
        let mut is_dark = scheme_name.to_lowercase().contains("dark")
            || look_and_feel.to_lowercase().contains("dark");

        if let Some(ref w_bg) = window_bg {
            if let Some(lum) = rgb_luminance(w_bg) {
                is_dark = lum < 128.0;
            }
        }

        let final_accent = accent_color
            .or_else(|| selection_bg.as_deref().and_then(parse_rgb_to_hex))
            .unwrap_or_else(|| if is_dark { "#3daee9".to_string() } else { "#1d99f3".to_string() }); // Breeze Blue

        let final_window_bg = window_bg.as_deref().and_then(parse_rgb_to_hex)
            .unwrap_or_else(|| if is_dark { "#232629".to_string() } else { "#eff0f1".to_string() });

        let final_view_bg = view_bg.as_deref().and_then(parse_rgb_to_hex)
            .unwrap_or_else(|| if is_dark { "#1b1e20".to_string() } else { "#fcfcfc".to_string() });

        let final_card_bg = button_bg.as_deref().and_then(parse_rgb_to_hex)
            .unwrap_or_else(|| if is_dark { "#31363b".to_string() } else { "#ffffff".to_string() });

        let final_text = window_fg.as_deref().and_then(parse_rgb_to_hex)
            .unwrap_or_else(|| if is_dark { "#eff0f1".to_string() } else { "#232629".to_string() });

        let final_text_sec = if is_dark { "#bdc3c7".to_string() } else { "#6e747b".to_string() };
        let border = if is_dark { "rgba(255, 255, 255, 0.12)".to_string() } else { "rgba(0, 0, 0, 0.12)".to_string() };

        let display_name = if !color_scheme.is_empty() {
            format!("KDE Plasma ({color_scheme})")
        } else if is_dark {
            "KDE Plasma (Breeze Dark)".to_string()
        } else {
            "KDE Plasma (Breeze)".to_string()
        };

        Some(SystemThemeInfo {
            desktop_env: "KDE Plasma".to_string(),
            theme_name: display_name,
            is_dark,
            accent_color: final_accent,
            window_bg: final_window_bg,
            view_bg: final_view_bg,
            card_bg: final_card_bg,
            text_primary: final_text,
            text_secondary: final_text_sec,
            border_color: border,
            is_fallback: false,
        })
    }

    fn detect_gtk_theme(home: &str) -> Option<SystemThemeInfo> {
        let mut theme_name = String::new();
        let mut prefer_dark = false;

        for ini_path in &[
            format!("{home}/.config/gtk-3.0/settings.ini"),
            format!("{home}/.config/gtk-4.0/settings.ini"),
        ] {
            if let Ok(content) = std::fs::read_to_string(ini_path) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if let Some((k, v)) = trimmed.split_once('=') {
                        match k.trim() {
                            "gtk-theme-name" if theme_name.is_empty() => theme_name = v.trim().to_string(),
                            "gtk-application-prefer-dark-theme" => prefer_dark = v.trim() == "1" || v.trim().eq_ignore_ascii_case("true"),
                            _ => {}
                        }
                    }
                }
            }
        }

        if theme_name.is_empty() {
            if let Ok(out) = std::process::Command::new("gsettings")
                .args(["get", "org.gnome.desktop.interface", "gtk-theme"])
                .output()
            {
                let s = String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_string();
                if !s.is_empty() && !s.contains("No such") {
                    theme_name = s;
                }
            }
        }

        if let Ok(out) = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
        {
            let s = String::from_utf8_lossy(&out.stdout);
            if s.contains("prefer-dark") {
                prefer_dark = true;
            }
        }

        if theme_name.is_empty() && !prefer_dark {
            return None;
        }

        let is_dark = prefer_dark || theme_name.to_lowercase().contains("dark");

        let mut bg_color = None;
        let mut sel_color = None;
        let mut fg_color = None;

        if !theme_name.is_empty() {
            for base in &[
                format!("{home}/.themes/{theme_name}/gtk-3.0/gtk.css"),
                format!("{home}/.local/share/themes/{theme_name}/gtk-3.0/gtk.css"),
                format!("/usr/share/themes/{theme_name}/gtk-3.0/gtk.css"),
            ] {
                if let Ok(css) = std::fs::read_to_string(base) {
                    for line in css.lines() {
                        let trimmed = line.trim();
                        if trimmed.starts_with("@define-color theme_bg_color") {
                            if let Some(color) = trimmed.split_whitespace().nth(2) {
                                bg_color = Some(color.trim_end_matches(';').to_string());
                            }
                        } else if trimmed.starts_with("@define-color theme_selected_bg_color") {
                            if let Some(color) = trimmed.split_whitespace().nth(2) {
                                sel_color = Some(color.trim_end_matches(';').to_string());
                            }
                        } else if trimmed.starts_with("@define-color theme_fg_color") {
                            if let Some(color) = trimmed.split_whitespace().nth(2) {
                                fg_color = Some(color.trim_end_matches(';').to_string());
                            }
                        }
                    }
                    break;
                }
            }
        }

        let accent = sel_color.unwrap_or_else(|| if is_dark { "#3584e4".to_string() } else { "#1c71d8".to_string() });
        let win_bg = bg_color.unwrap_or_else(|| if is_dark { "#242424".to_string() } else { "#fafafa".to_string() });
        let card_bg = if is_dark { "#303030".to_string() } else { "#ffffff".to_string() };
        let view_bg = if is_dark { "#1e1e1e".to_string() } else { "#f0f0f0".to_string() };
        let text = fg_color.unwrap_or_else(|| if is_dark { "#ffffff".to_string() } else { "#1e1e1e".to_string() });
        let text_sec = if is_dark { "#a0a0a0".to_string() } else { "#5e5e5e".to_string() };
        let border = if is_dark { "rgba(255, 255, 255, 0.08)".to_string() } else { "rgba(0, 0, 0, 0.08)".to_string() };

        Some(SystemThemeInfo {
            desktop_env: "GNOME / GTK".to_string(),
            theme_name: if theme_name.is_empty() { "Adwaita".to_string() } else { theme_name },
            is_dark,
            accent_color: accent,
            window_bg: win_bg,
            view_bg,
            card_bg,
            text_primary: text,
            text_secondary: text_sec,
            border_color: border,
            is_fallback: false,
        })
    }

    /// Dynamically inspects the Linux desktop environment (KDE Plasma, GNOME, or FreeDesktop portal)
    /// to extract the exact system theme color palette.
    #[tauri::command]
    pub fn get_system_theme_info() -> SystemThemeInfo {
        let desktop_raw = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_uppercase();
        let home = std::env::var("HOME").unwrap_or_default();

        let is_kde = desktop_raw.contains("KDE")
            || desktop_raw.contains("PLASMA")
            || std::env::var("KDE_FULL_SESSION").is_ok();

        // 1. If KDE Plasma, extract colors directly from KDE configs
        if is_kde {
            if let Some(kde_info) = detect_kde_theme(&home) {
                return kde_info;
            }
        }

        // 2. Try GTK theme inspection
        if let Some(gtk_info) = detect_gtk_theme(&home) {
            return gtk_info;
        }

        // 3. Fallback: FreeDesktop Portal query via gdbus
        let mut is_dark = true;
        if let Ok(output) = std::process::Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest", "org.freedesktop.portal.Desktop",
                "--object-path", "/org/freedesktop/portal/desktop",
                "--method", "org.freedesktop.portal.Settings.Read",
                "org.freedesktop.appearance",
                "color-scheme",
            ])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("uint32 1") {
                is_dark = true;
            } else if stdout.contains("uint32 2") || stdout.contains("uint32 0") {
                is_dark = false;
            }
        }

        SystemThemeInfo {
            desktop_env: if desktop_raw.is_empty() { "Standard Desktop".to_string() } else { desktop_raw },
            theme_name: if is_dark { "Default Dark (Fallback)".to_string() } else { "Default Light (Fallback)".to_string() },
            is_dark,
            accent_color: if is_dark { "#3daee9".to_string() } else { "#1d99f3".to_string() },
            window_bg: if is_dark { "#232629".to_string() } else { "#eff0f1".to_string() },
            view_bg: if is_dark { "#1b1e20".to_string() } else { "#fcfcfc".to_string() },
            card_bg: if is_dark { "#31363b".to_string() } else { "#ffffff".to_string() },
            text_primary: if is_dark { "#eff0f1".to_string() } else { "#232629".to_string() },
            text_secondary: if is_dark { "#bdc3c7".to_string() } else { "#6e747b".to_string() },
            border_color: if is_dark { "rgba(255, 255, 255, 0.12)".to_string() } else { "rgba(0, 0, 0, 0.12)".to_string() },
            is_fallback: true,
        }
    }
}

// ----------------- System Cloud Tray Icon Resolution -----------------

fn load_or_render_icon(path: &std::path::Path, is_dark: bool) -> Option<tauri::image::Image<'static>> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("png") {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(img) = tauri::image::Image::from_bytes(&bytes) {
                return Some(img);
            }
        }
    } else if ext.eq_ignore_ascii_case("svg") {
        let path_str = path.to_string_lossy();
        if path_str.contains("cloudstatus") || path_str.contains("folder-cloud") {
            let rgba = render_breeze_cloudstatus_rgba(24, 24, is_dark);
            return Some(tauri::image::Image::new_owned(rgba, 24, 24));
        }

        // Try rsvg-convert (standard in GNOME / librsvg)
        if let Ok(out) = std::process::Command::new("rsvg-convert")
            .args(["-w", "24", "-h", "24"])
            .arg(path)
            .output()
        {
            if out.status.success() && !out.stdout.is_empty() {
                if let Ok(img) = tauri::image::Image::from_bytes(&out.stdout) {
                    return Some(img);
                }
            }
        }

        // Try ImageMagick convert
        if let Ok(out) = std::process::Command::new("convert")
            .args(["-background", "none", "-resize", "24x24"])
            .arg(path)
            .arg("png:-")
            .output()
        {
            if out.status.success() && !out.stdout.is_empty() {
                if let Ok(img) = tauri::image::Image::from_bytes(&out.stdout) {
                    return Some(img);
                }
            }
        }
    }
    None
}

/// Locates cloud and sync icons from active system icon themes (KDE Breeze, GNOME Adwaita, etc.)
pub fn find_system_cloud_icon(is_dark: bool) -> Option<tauri::image::Image<'static>> {
    let home = std::env::var("HOME").unwrap_or_default();

    // 1. Detect active desktop icon theme name
    let mut theme_names = Vec::new();

    // Check KDE Plasma icon theme
    let kdeglobals = format!("{home}/.config/kdeglobals");
    if let Ok(content) = std::fs::read_to_string(&kdeglobals) {
        let mut in_icons = false;
        for line in content.lines() {
            let t = line.trim();
            if t == "[Icons]" {
                in_icons = true;
            } else if t.starts_with('[') {
                in_icons = false;
            } else if in_icons && t.starts_with("Theme=") {
                let name = t.trim_start_matches("Theme=").trim();
                if !name.is_empty() {
                    theme_names.push(name.to_string());
                }
            }
        }
    }

    // Check GTK icon theme
    for ini in &[
        format!("{home}/.config/gtk-3.0/settings.ini"),
        format!("{home}/.config/gtk-4.0/settings.ini"),
    ] {
        if let Ok(content) = std::fs::read_to_string(ini) {
            for line in content.lines() {
                if let Some((k, v)) = line.trim().split_once('=') {
                    if k.trim() == "gtk-icon-theme-name" {
                        let name = v.trim();
                        if !name.is_empty() && !theme_names.contains(&name.to_string()) {
                            theme_names.push(name.to_string());
                        }
                    }
                }
            }
        }
    }

    // Add standard fallback themes
    if is_dark {
        theme_names.push("breeze-dark".to_string());
        theme_names.push("Adwaita".to_string());
        theme_names.push("Yaru-dark".to_string());
        theme_names.push("Papirus-Dark".to_string());
    } else {
        theme_names.push("breeze".to_string());
        theme_names.push("Adwaita".to_string());
        theme_names.push("Yaru".to_string());
        theme_names.push("Papirus".to_string());
    }
    theme_names.push("hicolor".to_string());

    let search_roots = [
        format!("{home}/.local/share/icons"),
        format!("{home}/.icons"),
        "/usr/share/icons".to_string(),
        "/usr/share/pixmaps".to_string(),
    ];

    let icon_candidates = [
        "cloudstatus",
        "cloudstatus-symbolic",
        "folder-cloud-symbolic",
        "folder-cloud",
        "cloud-upload-symbolic",
        "cloud-upload",
        "cloud-download-symbolic",
        "cloud-download",
        "state-sync",
        "emblem-synchronizing",
        "emblem-synchronizing-symbolic",
    ];

    let subdirs = [
        "22x22/status",
        "24x24/status",
        "scalable/status",
        "status/22",
        "status/24",
        "status/scalable",
        "status/symbolic",
        "22x22/places",
        "24x24/places",
        "32x32/places",
        "places/22",
        "places/32",
        "places/scalable",
        "places/symbolic",
        "scalable/places",
        "22x22/actions",
        "actions/22",
        "actions/scalable",
        "22x22/apps",
        "24x24/apps",
        "32x32/apps",
        "apps/22",
        "apps/24",
        "apps/scalable",
        "scalable/apps",
        "scalable/emblems",
        "emblems/22",
        "emblems/scalable",
    ];

    for theme in &theme_names {
        for root in &search_roots {
            let theme_dir = std::path::Path::new(root).join(theme);
            if !theme_dir.exists() {
                continue;
            }

            for icon_name in &icon_candidates {
                for sub in &subdirs {
                    let dir = theme_dir.join(sub);
                    for ext in &["png", "svg"] {
                        let path = dir.join(format!("{icon_name}.{ext}"));
                        if path.exists() {
                            if let Some(img) = load_or_render_icon(&path, is_dark) {
                                tracing::info!("Resolved system cloud tray icon: {:?}", path);
                                return Some(img);
                            }
                        }
                    }
                }
            }
        }
    }

    // Direct /usr/share/pixmaps check
    for icon_name in &icon_candidates {
        for ext in &["png", "svg"] {
            let path = std::path::Path::new("/usr/share/pixmaps").join(format!("{icon_name}.{ext}"));
            if path.exists() {
                if let Some(img) = load_or_render_icon(&path, is_dark) {
                    tracing::info!("Resolved system pixmap tray icon: {:?}", path);
                    return Some(img);
                }
            }
        }
    }

    None
}

/// Renders the exact KDE Breeze cloudstatus geometry as raw RGBA bytes with supersampling anti-aliasing
pub fn render_breeze_cloudstatus_rgba(width: u32, height: u32, is_dark: bool) -> Vec<u8> {
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let (r, g, b) = if is_dark {
        (252, 252, 252) // KDE Breeze Dark ColorScheme-Text #fcfcfc
    } else {
        (35, 38, 41)    // KDE Breeze Light ColorScheme-Text #232629
    };

    let scale = (width.min(height) as f32) / 24.0;
    let offset_x = (width as f32 - 22.0 * scale) / 2.0;
    let offset_y = (height as f32 - 22.0 * scale) / 2.0;

    for y in 0..height {
        for x in 0..width {
            let mut inside_count = 0;
            const SAMPLES: usize = 4;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = (x as f32 + (sx as f32 + 0.5) / (SAMPLES as f32) - offset_x) / scale;
                    let py = (y as f32 + (sy as f32 + 0.5) / (SAMPLES as f32) - offset_y) / scale;

                    // Breeze cloudstatus outer geometry
                    let outer_d1 = (px - 11.0).hypot(py - 10.0) <= 6.0;
                    let outer_d2 = (px - 6.0).hypot(py - 14.0) <= 4.0;
                    let outer_d3 = (px - 15.0).hypot(py - 13.0) <= 5.0;
                    let outer_mid = (6.0..=15.0).contains(&px) && (10.0..=18.0).contains(&py);
                    let is_outer = outer_d1 || outer_d2 || outer_d3 || outer_mid;

                    // Breeze cloudstatus inner cutout geometry
                    let inner_d1 = (px - 11.0).hypot(py - 10.0) < 4.9;
                    let inner_d2 = (px - 6.0).hypot(py - 14.0) < 2.9;
                    let inner_d3 = (px - 15.0).hypot(py - 13.0) < 3.9;
                    let inner_mid = (6.0..=15.0).contains(&px) && (10.0..=17.0).contains(&py);
                    let is_inner = inner_d1 || inner_d2 || inner_d3 || inner_mid;

                    if is_outer && !is_inner {
                        inside_count += 1;
                    }
                }
            }

            let alpha = ((inside_count as f32) / ((SAMPLES * SAMPLES) as f32) * 255.0).round() as u8;
            let idx = ((y * width + x) * 4) as usize;
            rgba[idx] = r;
            rgba[idx + 1] = g;
            rgba[idx + 2] = b;
            rgba[idx + 3] = alpha;
        }
    }
    rgba
}

/// Generates a clean procedural cloud-sync tray icon matching the system dark/light theme
pub fn generate_cloud_sync_icon(is_dark: bool) -> tauri::image::Image<'static> {
    let rgba = render_breeze_cloudstatus_rgba(24, 24, is_dark);
    tauri::image::Image::new_owned(rgba, 24, 24)
}

// ----------------- Application Entrypoint -----------------

pub fn run() {
    tracing_subscriber::fmt::init();
    info!("Starting MyCloud GUI with System Tray");

    let gui_state = Arc::new(GuiState::new());

    let app = tauri::Builder::default()
        .manage(Arc::clone(&gui_state))
        .setup({
            let gui_state = Arc::clone(&gui_state);
            move |app| {
                // Build Tray Context Menu
                let is_initially_visible = app
                    .get_webview_window("main")
                    .and_then(|w| w.is_visible().ok())
                    .unwrap_or(true);
                let initial_label = if is_initially_visible {
                    "Hide Dashboard"
                } else {
                    "Open Dashboard"
                };
                let settings_item =
                    MenuItem::with_id(app, "settings", initial_label, true, None::<&str>)?;
                let folder_item = MenuItem::with_id(
                    app,
                    "open_folder",
                    "Open MyCloud Folder",
                    false,
                    None::<&str>,
                )?;
                let sync_item =
                    MenuItem::with_id(app, "trigger_sync", "Sync Now", false, None::<&str>)?;
                let pause_item =
                    MenuItem::with_id(app, "toggle_pause", "Pause Sync", false, None::<&str>)?;
                let quit_item = MenuItem::with_id(app, "quit", "Quit MyCloud", true, None::<&str>)?;

                // Store menu item handles in GuiState for dynamic state synchronization
                if let Ok(mut guard) = gui_state.dashboard_item.lock() {
                    *guard = Some(settings_item.clone());
                }
                if let Ok(mut guard) = gui_state.folder_item.lock() {
                    *guard = Some(folder_item.clone());
                }
                if let Ok(mut guard) = gui_state.sync_item.lock() {
                    *guard = Some(sync_item.clone());
                }
                if let Ok(mut guard) = gui_state.pause_item.lock() {
                    *guard = Some(pause_item.clone());
                }

                let menu = Menu::with_items(
                    app,
                    &[
                        &settings_item,
                        &folder_item,
                        &sync_item,
                        &pause_item,
                        &quit_item,
                    ],
                )?;

                // Setup System Tray with system cloud/sync icon
                let sys_theme = commands::get_system_theme_info();
                let tray_icon = find_system_cloud_icon(sys_theme.is_dark)
                    .unwrap_or_else(|| generate_cloud_sync_icon(sys_theme.is_dark));

                let _tray = TrayIconBuilder::with_id("main-tray")
                    .icon(tray_icon)
                    .menu(&menu)
                    .tooltip("MyCloud Files On-Demand")
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "settings" => {
                            let state: tauri::State<'_, Arc<GuiState>> = app.state();
                            if let Some(window) = app
                                .get_webview_window("main")
                                .or_else(|| app.webview_windows().values().next().cloned())
                            {
                                let is_visible = window.is_visible().unwrap_or(false);
                                let is_minimized = window.is_minimized().unwrap_or(false);
                                if is_visible && !is_minimized {
                                    let _ = window.hide();
                                    state.update_dashboard_item_label(false);
                                } else {
                                    let _ = window.show();
                                    let _ = window.unminimize();
                                    let _ = window.set_focus();
                                    state.update_dashboard_item_label(true);
                                }
                            }
                        }
                        "open_folder" => {
                            tauri::async_runtime::spawn(async {
                                let _ = commands::open_cloud_folder().await;
                            });
                        }
                        "trigger_sync" => {
                            let state: tauri::State<'_, Arc<GuiState>> = app.state();
                            let state_clone = Arc::clone(&state);
                            tauri::async_runtime::spawn(async move {
                                let _ = state_clone.query_daemon("{\"cmd\":\"TriggerSync\"}").await;
                            });
                        }
                        "toggle_pause" => {
                            let state: tauri::State<'_, Arc<GuiState>> = app.state();
                            let current = state.is_paused.load(Ordering::SeqCst);
                            let next = !current;
                            state.is_paused.store(next, Ordering::SeqCst);
                            if let Ok(guard) = state.pause_item.lock() {
                                if let Some(ref item) = *guard {
                                    let label = if next { "Resume Sync" } else { "Pause Sync" };
                                    let _ = item.set_text(label);
                                }
                            }
                            let state_clone = Arc::clone(&state);
                            tauri::async_runtime::spawn(async move {
                                let cmd = format!("{{\"cmd\":\"SetPaused\",\"data\":{next}}}");
                                let _ = state_clone.query_daemon(&cmd).await;
                            });
                        }
                        "quit" => {
                            std::process::exit(0);
                        }
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click { .. } = event {
                            let app = tray.app_handle();
                            let state: tauri::State<'_, Arc<GuiState>> = app.state();
                            if let Some(window) = app
                                .get_webview_window("main")
                                .or_else(|| app.webview_windows().values().next().cloned())
                            {
                                let is_visible = window.is_visible().unwrap_or(false);
                                let is_minimized = window.is_minimized().unwrap_or(false);
                                if is_visible && !is_minimized {
                                    let _ = window.hide();
                                    state.update_dashboard_item_label(false);
                                } else {
                                    let _ = window.show();
                                    let _ = window.unminimize();
                                    let _ = window.set_focus();
                                    state.update_dashboard_item_label(true);
                                }
                            }
                        }
                    })
                    .build(app)?;

                Ok(())
            }
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                let state: tauri::State<'_, Arc<GuiState>> = window.state();
                state.update_dashboard_item_label(false);
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_sync_state,
            commands::check_setup_status,
            commands::trigger_drive_auth,
            commands::initialize_vault,
            commands::unlock_vault,
            commands::get_transfer_stats,
            commands::toggle_pause,
            commands::trigger_sync,
            commands::open_cloud_folder,
            commands::get_system_theme_info,
            commands::get_sync_folders,
            commands::set_sync_folders,
            commands::get_sync_hidden_files,
            commands::set_sync_hidden_files,
            commands::get_drive_api_credentials,
            commands::set_drive_api_credentials,
            commands::lock_vault,
            commands::change_passphrase,
            commands::recover_vault
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, _event| {});
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_save_cloud_icons() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("icons");

        // 32x32
        let rgba32 = render_breeze_cloudstatus_rgba(32, 32, true);
        image::save_buffer_with_format(base.join("32x32.png"), &rgba32, 32, 32, image::ColorType::Rgba8, image::ImageFormat::Png).unwrap();

        // 128x128
        let rgba128 = render_breeze_cloudstatus_rgba(128, 128, true);
        image::save_buffer_with_format(base.join("128x128.png"), &rgba128, 128, 128, image::ColorType::Rgba8, image::ImageFormat::Png).unwrap();

        // 128x128@2x (256x256)
        let rgba256 = render_breeze_cloudstatus_rgba(256, 256, true);
        image::save_buffer_with_format(base.join("128x128@2x.png"), &rgba256, 256, 256, image::ColorType::Rgba8, image::ImageFormat::Png).unwrap();

        // icon.png (512x512)
        let rgba512 = render_breeze_cloudstatus_rgba(512, 512, true);
        image::save_buffer_with_format(base.join("icon.png"), &rgba512, 512, 512, image::ColorType::Rgba8, image::ImageFormat::Png).unwrap();

        println!("Successfully generated all cloudstatus icons in {:?}", base);
    }
}
