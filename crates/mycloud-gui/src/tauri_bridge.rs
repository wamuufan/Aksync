use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatusPayload {
    pub state: SyncState,
    pub rclone_pid: Option<u32>,
    pub fuse_pid: Option<u32>,
    pub raw_mount_path: String,
    pub target_mount_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupStatusPayload {
    pub is_complete: bool,
    pub rclone_configured: bool,
    pub vault_initialized: bool,
    pub remote_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferStats {
    pub upload_speed: u64,
    pub download_speed: u64,
    pub storage_used: u64,
    pub storage_total: u64,
    pub files_synced: u64,
}

#[derive(Serialize)]
struct TogglePauseArgs {
    paused: bool,
}

#[derive(Serialize)]
struct InitVaultArgs {
    password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangePassphraseArgs {
    current_password: String,
    new_password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoverVaultArgs {
    recovery_key: String,
    new_password: String,
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    async fn tauri_invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

fn js_error_to_string(e: JsValue) -> String {
    if let Some(s) = e.as_string() {
        s
    } else if let Some(err) = e.dyn_ref::<js_sys::Error>() {
        String::from(err.message())
    } else {
        let debug_str = format!("{e:?}");
        if let Some(inside) = debug_str.strip_prefix("JsValue(\"") {
            if let Some(clean) = inside.strip_suffix("\")") {
                return clean.to_string();
            }
        }
        debug_str
    }
}

pub async fn get_sync_state() -> DaemonStatusPayload {
    let dummy = DaemonStatusPayload {
        state: SyncState::SetupRequired,
        rclone_pid: None,
        fuse_pid: None,
        raw_mount_path: "~/.cache/mycloud/raw_mount".to_string(),
        target_mount_path: "~/MyCloud".to_string(),
    };

    let empty_args = JsValue::from_str("{}");
    match tauri_invoke("get_sync_state", empty_args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val).unwrap_or(dummy),
        Err(_) => dummy,
    }
}

pub async fn check_setup_status() -> Result<SetupStatusPayload, String> {
    let empty_args = JsValue::from_str("{}");
    match tauri_invoke("check_setup_status", empty_args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val)
            .map_err(|e| format!("Failed to parse setup status: {e}")),
        Err(e) => Err(js_error_to_string(e)),
    }
}

pub async fn trigger_drive_auth() -> Result<String, String> {
    let empty_args = JsValue::from_str("{}");
    match tauri_invoke("trigger_drive_auth", empty_args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val)
            .map_err(|e| format!("Failed to parse response: {e}")),
        Err(e) => Err(js_error_to_string(e)),
    }
}

pub async fn initialize_vault(password: String) -> Result<String, String> {
    let args = serde_wasm_bindgen::to_value(&InitVaultArgs { password }).unwrap_or(JsValue::NULL);
    match tauri_invoke("initialize_vault", args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val)
            .map_err(|e| format!("Failed to parse response: {e}")),
        Err(e) => Err(js_error_to_string(e)),
    }
}

pub async fn unlock_vault(password: String) -> Result<String, String> {
    let args = serde_wasm_bindgen::to_value(&InitVaultArgs { password }).unwrap_or(JsValue::NULL);
    match tauri_invoke("unlock_vault", args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val)
            .map_err(|e| format!("Failed to parse response: {e}")),
        Err(e) => Err(js_error_to_string(e)),
    }
}

pub async fn lock_vault() -> Result<String, String> {
    let empty_args = JsValue::from_str("{}");
    match tauri_invoke("lock_vault", empty_args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val)
            .map_err(|e| format!("Failed to parse response: {e}")),
        Err(e) => Err(js_error_to_string(e)),
    }
}

pub async fn change_passphrase(
    current_password: String,
    new_password: String,
) -> Result<String, String> {
    let args = serde_wasm_bindgen::to_value(&ChangePassphraseArgs {
        current_password,
        new_password,
    })
    .unwrap_or(JsValue::NULL);
    match tauri_invoke("change_passphrase", args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val)
            .map_err(|e| format!("Failed to parse response: {e}")),
        Err(e) => Err(js_error_to_string(e)),
    }
}

pub async fn recover_vault(
    recovery_key: String,
    new_password: String,
) -> Result<String, String> {
    let args = serde_wasm_bindgen::to_value(&RecoverVaultArgs {
        recovery_key,
        new_password,
    })
    .unwrap_or(JsValue::NULL);
    match tauri_invoke("recover_vault", args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val)
            .map_err(|e| format!("Failed to parse response: {e}")),
        Err(e) => Err(js_error_to_string(e)),
    }
}

pub async fn get_transfer_stats() -> TransferStats {
    let dummy = TransferStats {
        upload_speed: 0,
        download_speed: 0,
        storage_used: 0,
        storage_total: 0,
        files_synced: 0,
    };

    let empty_args = JsValue::from_str("{}");
    match tauri_invoke("get_transfer_stats", empty_args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val).unwrap_or(dummy),
        Err(_) => dummy,
    }
}

pub async fn toggle_pause(paused: bool) {
    let args = serde_wasm_bindgen::to_value(&TogglePauseArgs { paused }).unwrap_or(JsValue::NULL);
    let _ = tauri_invoke("toggle_pause", args).await;
}

pub async fn trigger_sync() {
    let empty_args = JsValue::from_str("{}");
    let _ = tauri_invoke("trigger_sync", empty_args).await;
}

pub async fn open_cloud_folder() {
    let empty_args = JsValue::from_str("{}");
    let _ = tauri_invoke("open_cloud_folder", empty_args).await;
}

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

pub async fn get_system_theme_info() -> SystemThemeInfo {
    let dummy = SystemThemeInfo {
        desktop_env: "Standard Desktop".to_string(),
        theme_name: "Default (Fallback)".to_string(),
        is_dark: true,
        accent_color: "#3daee9".to_string(),
        window_bg: "#232629".to_string(),
        view_bg: "#1b1e20".to_string(),
        card_bg: "#31363b".to_string(),
        text_primary: "#eff0f1".to_string(),
        text_secondary: "#bdc3c7".to_string(),
        border_color: "rgba(255, 255, 255, 0.12)".to_string(),
        is_fallback: true,
    };

    let empty_args = JsValue::from_str("{}");
    match tauri_invoke("get_system_theme_info", empty_args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val).unwrap_or(dummy),
        Err(_) => dummy,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncFolderInfo {
    pub name: String,
    pub rel_path: String,
    pub abs_path: String,
    pub selected: bool,
    pub icon: String,
}

#[derive(Serialize)]
struct SetSyncFoldersArgs {
    folders: Vec<String>,
}

pub async fn get_sync_folders() -> Vec<SyncFolderInfo> {
    let empty_args = JsValue::from_str("{}");
    match tauri_invoke("get_sync_folders", empty_args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub async fn set_sync_folders(folders: Vec<String>) -> Result<String, String> {
    let args = serde_wasm_bindgen::to_value(&SetSyncFoldersArgs { folders }).unwrap_or(JsValue::NULL);
    match tauri_invoke("set_sync_folders", args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val)
            .map_err(|e| format!("Failed to parse response: {e}")),
        Err(e) => Err(js_error_to_string(e)),
    }
}

#[derive(Serialize)]
struct SetSyncHiddenFilesArgs {
    enabled: bool,
}

#[allow(dead_code)]
pub async fn get_sync_hidden_files() -> bool {
    let empty_args = JsValue::from_str("{}");
    match tauri_invoke("get_sync_hidden_files", empty_args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val).unwrap_or(false),
        Err(_) => false,
    }
}

pub async fn set_sync_hidden_files(enabled: bool) -> Result<String, String> {
    let args = serde_wasm_bindgen::to_value(&SetSyncHiddenFilesArgs { enabled }).unwrap_or(JsValue::NULL);
    match tauri_invoke("set_sync_hidden_files", args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val)
            .map_err(|e| format!("Failed to parse response: {e}")),
        Err(e) => Err(js_error_to_string(e)),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DriveApiCredentialsPayload {
    #[serde(alias = "client_id")]
    pub client_id: String,
    #[serde(alias = "client_secret")]
    pub client_secret: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetDriveApiCredentialsArgs {
    client_id: String,
    client_secret: String,
}

pub async fn get_drive_api_credentials() -> Result<DriveApiCredentialsPayload, String> {
    let empty_args = JsValue::from_str("{}");
    match tauri_invoke("get_drive_api_credentials", empty_args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val)
            .map_err(|e| format!("Failed to parse credentials: {e}")),
        Err(e) => Err(js_error_to_string(e)),
    }
}

pub async fn set_drive_api_credentials(client_id: String, client_secret: String) -> Result<String, String> {
    let args = serde_wasm_bindgen::to_value(&SetDriveApiCredentialsArgs {
        client_id,
        client_secret,
    })
    .unwrap_or(JsValue::NULL);
    match tauri_invoke("set_drive_api_credentials", args).await {
        Ok(js_val) => serde_wasm_bindgen::from_value(js_val)
            .map_err(|e| format!("Failed to parse response: {e}")),
        Err(e) => Err(js_error_to_string(e)),
    }
}
