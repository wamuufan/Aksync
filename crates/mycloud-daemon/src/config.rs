use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::info;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML serialization error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("TOML deserialization error: {0}")]
    TomlDe(#[from] toml::de::Error),
}

/// Persistent daemon configuration stored in ~/.config/mycloud/config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Remote name configured in rclone (default: "mycloud_drive")
    pub rclone_remote: String,

    /// Whether rclone remote has been successfully authorized and configured
    pub rclone_configured: bool,

    /// Base64 encoded Argon2id salt for vault master key derivation
    pub vault_salt_base64: Option<String>,

    /// Whether the cryptographic vault has been initialized
    pub vault_initialized: bool,

    /// Verification token (hex-encoded canary ciphertext) to check master password correctness on unlock
    pub vault_verification_tag: Option<String>,

    /// Last known cached storage used bytes (persisted for instant 0ms GUI startup)
    #[serde(default)]
    pub cached_storage_used: u64,

    /// Last known cached storage total bytes (persisted for instant 0ms GUI startup)
    #[serde(default)]
    pub cached_storage_total: u64,

    /// Last known cached count of monitored files (persisted for instant 0ms GUI startup)
    #[serde(default)]
    pub cached_files_count: u64,

    /// List of user home folders selected for cloud sync (e.g. ["Belgeler", "Masaüstü"])
    #[serde(default)]
    pub selected_sync_folders: Vec<String>,

    /// Whether to synchronize hidden files and directories (starting with dot) (default: false)
    #[serde(default)]
    pub sync_hidden_files: bool,

    /// Persistent password for local WebDAV Basic Auth (Dolphin / KIO)
    #[serde(default)]
    pub webdav_password: Option<String>,

    /// Custom Google Drive OAuth2 Client ID (overrides rclone's shared client)
    #[serde(default)]
    pub custom_client_id: Option<String>,

    /// Custom Google Drive OAuth2 Client Secret
    #[serde(default)]
    pub custom_client_secret: Option<String>,

    /// Hex-encoded wrapped Data Master Key (encrypted with KEK derived from user passphrase)
    #[serde(default)]
    pub wrapped_dmk: Option<String>,

    /// Hex-encoded wrapped Data Master Key (encrypted with KEK derived from recovery key)
    #[serde(default)]
    pub wrapped_dmk_recovery: Option<String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        let opt = OptimizationConfig::load_embedded();
        Self {
            rclone_remote: "mycloud_drive".to_string(),
            rclone_configured: false,
            vault_salt_base64: None,
            vault_initialized: false,
            vault_verification_tag: None,
            cached_storage_used: 0,
            cached_storage_total: 0,
            cached_files_count: 0,
            selected_sync_folders: Vec::new(),
            sync_hidden_files: opt.sync.default_sync_hidden_files,
            webdav_password: None,
            custom_client_id: None,
            custom_client_secret: None,
            wrapped_dmk: None,
            wrapped_dmk_recovery: None,
        }
    }
}

impl DaemonConfig {
    /// Returns the standard path to config.toml: ~/.config/mycloud/config.toml
    pub fn default_config_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".config/mycloud/config.toml")
    }

    /// Loads the configuration from disk, or returns default if not present
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    /// Saves the current configuration to disk
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        info!("Saved daemon configuration to {:?}", path);
        Ok(())
    }

    /// Checks if both cloud drive auth and vault initialization are complete
    pub fn is_setup_complete(&self) -> bool {
        self.rclone_configured && self.vault_initialized && self.vault_salt_base64.is_some()
    }
}

/// Determines the human-friendly cloud provider name (e.g. "Google Drive", "OneDrive", "Dropbox")
pub fn get_provider_name(remote: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let rclone_conf = format!("{home}/.config/rclone/rclone.conf");
    let target_section = format!("[{}]", remote.trim_end_matches(':'));

    if let Ok(content) = std::fs::read_to_string(&rclone_conf) {
        let mut in_section = false;
        for line in content.lines() {
            let t = line.trim();
            if t == target_section {
                in_section = true;
            } else if t.starts_with('[') {
                in_section = false;
            } else if in_section && t.starts_with("type") {
                if let Some((_, val)) = t.split_once('=') {
                    match val.trim().to_lowercase().as_str() {
                        "drive" => return "Google Drive".to_string(),
                        "onedrive" => return "OneDrive".to_string(),
                        "dropbox" => return "Dropbox".to_string(),
                        "s3" => return "Amazon S3".to_string(),
                        "webdav" => return "WebDAV".to_string(),
                        "pcloud" => return "pCloud".to_string(),
                        "mega" => return "MEGA".to_string(),
                        "box" => return "Box".to_string(),
                        other => {
                            if !other.is_empty() {
                                let mut chars = other.chars();
                                return match chars.next() {
                                    None => "Google Drive".to_string(),
                                    Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
                                };
                            }
                        }
                    }
                }
            }
        }
    }

    "Google Drive".to_string()
}

// ----------------- Performance & VFS Optimization Config -----------------

pub const EMBEDDED_OPTIMIZATIONS_TOML: &str = include_str!("../../../optimizations.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VfsSection {
    #[serde(default = "default_vfs_cache_mode")]
    pub cache_mode: String,
    #[serde(default = "default_vfs_cache_max_size")]
    pub cache_max_size: String,
    #[serde(default = "default_vfs_cache_max_age")]
    pub cache_max_age: String,
    #[serde(default = "default_vfs_read_chunk_size")]
    pub read_chunk_size: String,
    #[serde(default = "default_vfs_read_chunk_size_limit")]
    pub read_chunk_size_limit: String,
    #[serde(default = "default_vfs_read_ahead")]
    pub read_ahead: String,
    #[serde(default = "default_buffer_size")]
    pub buffer_size: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSection {
    #[serde(default = "default_dir_cache_time")]
    pub dir_cache_time: String,
    #[serde(default = "default_attr_timeout")]
    pub attr_timeout: String,
    #[serde(default = "default_poll_interval")]
    pub poll_interval: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSection {
    #[serde(default = "default_drive_pacer_min_sleep")]
    pub drive_pacer_min_sleep: String,
    #[serde(default = "default_drive_pacer_burst")]
    pub drive_pacer_burst: u32,
    #[serde(default = "default_true")]
    pub no_checksum: bool,
    #[serde(default = "default_http_timeout_secs")]
    pub http_timeout_secs: u64,
    #[serde(default = "default_about_timeout_secs")]
    pub about_timeout_secs: u64,
    #[serde(default = "default_about_refresh_interval_secs")]
    pub about_refresh_interval_secs: u64,
    #[serde(default = "default_rc_ready_timeout_secs")]
    pub rc_ready_timeout_secs: u64,
    #[serde(default = "default_rc_ready_retry_ms")]
    pub rc_ready_retry_ms: u64,
    #[serde(default = "default_rclone_timeout")]
    pub timeout: String,
    #[serde(default = "default_rclone_contimeout")]
    pub contimeout: String,
    #[serde(default = "default_rclone_low_level_retries")]
    pub low_level_retries: u32,
    #[serde(default = "default_rclone_retries")]
    pub retries: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSection {
    #[serde(default = "default_home_sync_interval_secs")]
    pub home_sync_interval_secs: u64,
    #[serde(default = "default_stats_interval_ms")]
    pub stats_interval_ms: u64,
    #[serde(default = "default_transfers")]
    pub transfers: u32,
    #[serde(default = "default_sync_hidden_files")]
    pub default_sync_hidden_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessSection {
    #[serde(default = "default_sigterm_timeout_secs")]
    pub sigterm_timeout_secs: u64,
    #[serde(default = "default_webdav_bind_addr")]
    pub webdav_bind_addr: String,
    #[serde(default = "default_vault_webdav_bind_addr")]
    pub vault_webdav_bind_addr: String,
    #[serde(default = "default_rc_bind_addr")]
    pub rc_bind_addr: String,
    #[serde(default = "default_vault_rc_bind_addr")]
    pub vault_rc_bind_addr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturesSection {
    #[serde(default = "default_true")]
    pub enable_vfs_prewarm: bool,
    #[serde(default = "default_true")]
    pub enable_freeze_prevention: bool,
}

fn default_vfs_cache_mode() -> String { "writes".to_string() }
fn default_vfs_cache_max_size() -> String { "1G".to_string() }
fn default_vfs_cache_max_age() -> String { "1h".to_string() }
fn default_vfs_read_chunk_size() -> String { "128k".to_string() }
fn default_vfs_read_chunk_size_limit() -> String { "64M".to_string() }
fn default_vfs_read_ahead() -> String { "128k".to_string() }
fn default_buffer_size() -> String { "128k".to_string() }
fn default_dir_cache_time() -> String { "24h".to_string() }
fn default_attr_timeout() -> String { "24h".to_string() }
fn default_poll_interval() -> String { "1m".to_string() }
fn default_drive_pacer_min_sleep() -> String { "200ms".to_string() }
fn default_drive_pacer_burst() -> u32 { 10 }
fn default_true() -> bool { true }
fn default_http_timeout_secs() -> u64 { 3 }
fn default_about_timeout_secs() -> u64 { 30 }
fn default_about_refresh_interval_secs() -> u64 { 300 }
fn default_rc_ready_timeout_secs() -> u64 { 60 }
fn default_rc_ready_retry_ms() -> u64 { 250 }
fn default_home_sync_interval_secs() -> u64 { 30 }
fn default_stats_interval_ms() -> u64 { 1000 }
fn default_transfers() -> u32 { 4 }
fn default_sync_hidden_files() -> bool { false }
fn default_sigterm_timeout_secs() -> u64 { 3 }
fn default_webdav_bind_addr() -> String { "127.0.0.1:8765".to_string() }
fn default_vault_webdav_bind_addr() -> String { "127.0.0.1:8766".to_string() }
fn default_rc_bind_addr() -> String { "127.0.0.1:5572".to_string() }
fn default_vault_rc_bind_addr() -> String { "127.0.0.1:5573".to_string() }
fn default_rclone_timeout() -> String { "30s".to_string() }
fn default_rclone_contimeout() -> String { "10s".to_string() }
fn default_rclone_low_level_retries() -> u32 { 3 }
fn default_rclone_retries() -> u32 { 3 }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OptimizationConfig {
    #[serde(default)]
    pub vfs: VfsSection,
    #[serde(default)]
    pub cache: CacheSection,
    #[serde(default)]
    pub network: NetworkSection,
    #[serde(default)]
    pub sync: SyncSection,
    #[serde(default)]
    pub process: ProcessSection,
    #[serde(default)]
    pub features: FeaturesSection,
}

impl Default for VfsSection {
    fn default() -> Self {
        Self {
            cache_mode: default_vfs_cache_mode(),
            cache_max_size: default_vfs_cache_max_size(),
            cache_max_age: default_vfs_cache_max_age(),
            read_chunk_size: default_vfs_read_chunk_size(),
            read_chunk_size_limit: default_vfs_read_chunk_size_limit(),
            read_ahead: default_vfs_read_ahead(),
            buffer_size: default_buffer_size(),
        }
    }
}

impl Default for CacheSection {
    fn default() -> Self {
        Self {
            dir_cache_time: default_dir_cache_time(),
            attr_timeout: default_attr_timeout(),
            poll_interval: default_poll_interval(),
        }
    }
}

impl Default for NetworkSection {
    fn default() -> Self {
        Self {
            drive_pacer_min_sleep: default_drive_pacer_min_sleep(),
            drive_pacer_burst: default_drive_pacer_burst(),
            no_checksum: default_true(),
            http_timeout_secs: default_http_timeout_secs(),
            about_timeout_secs: default_about_timeout_secs(),
            about_refresh_interval_secs: default_about_refresh_interval_secs(),
            rc_ready_timeout_secs: default_rc_ready_timeout_secs(),
            rc_ready_retry_ms: default_rc_ready_retry_ms(),
            timeout: default_rclone_timeout(),
            contimeout: default_rclone_contimeout(),
            low_level_retries: default_rclone_low_level_retries(),
            retries: default_rclone_retries(),
        }
    }
}

impl Default for SyncSection {
    fn default() -> Self {
        Self {
            home_sync_interval_secs: default_home_sync_interval_secs(),
            stats_interval_ms: default_stats_interval_ms(),
            transfers: default_transfers(),
            default_sync_hidden_files: default_sync_hidden_files(),
        }
    }
}

impl Default for ProcessSection {
    fn default() -> Self {
        Self {
            sigterm_timeout_secs: default_sigterm_timeout_secs(),
            webdav_bind_addr: default_webdav_bind_addr(),
            vault_webdav_bind_addr: default_vault_webdav_bind_addr(),
            rc_bind_addr: default_rc_bind_addr(),
            vault_rc_bind_addr: default_vault_rc_bind_addr(),
        }
    }
}

impl Default for FeaturesSection {
    fn default() -> Self {
        Self {
            enable_vfs_prewarm: default_true(),
            enable_freeze_prevention: default_true(),
        }
    }
}

impl OptimizationConfig {
    /// Loads the configuration baked directly into the binary at compile-time via optimizations.toml
    pub fn load_embedded() -> Self {
        match toml::from_str::<Self>(EMBEDDED_OPTIMIZATIONS_TOML) {
            Ok(cfg) => {
                info!("Using compiled-in optimization configuration from optimizations.toml");
                cfg
            }
            Err(e) => {
                tracing::error!("Failed to parse embedded optimizations.toml: {}. Using fallback defaults.", e);
                Self::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_serialization_roundtrip() {
        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_cfg_test_{}", std::process::id()));
        let config_path = temp_dir.join("config.toml");

        let mut config = DaemonConfig::default();
        assert!(!config.is_setup_complete());

        config.rclone_configured = true;
        config.vault_salt_base64 = Some("c2FsdGVkX2V4YW1wbGVfc2FsdA==".to_string());
        config.vault_initialized = true;
        config.custom_client_id = Some("test-client-id.apps.googleusercontent.com".to_string());
        config.custom_client_secret = Some("test-secret".to_string());
        assert!(config.is_setup_complete());

        config.save_to(&config_path).expect("Failed to save config");
        let loaded = DaemonConfig::load_from(&config_path).expect("Failed to load config");

        assert_eq!(loaded.rclone_remote, "mycloud_drive");
        assert!(loaded.rclone_configured);
        assert_eq!(
            loaded.vault_salt_base64.as_deref(),
            Some("c2FsdGVkX2V4YW1wbGVfc2FsdA==")
        );
        assert_eq!(
            loaded.custom_client_id.as_deref(),
            Some("test-client-id.apps.googleusercontent.com")
        );
        assert_eq!(loaded.custom_client_secret.as_deref(), Some("test-secret"));
        assert!(loaded.vault_initialized);
        assert!(loaded.is_setup_complete());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_embedded_optimization_config_validity() {
        // Validates that the optimizations.toml embedded at compile-time is 100% valid and parses cleanly
        let cfg = OptimizationConfig::load_embedded();
        assert_eq!(cfg.vfs.cache_mode, "writes");
        assert_eq!(cfg.vfs.cache_max_size, "1G");
        assert_eq!(cfg.cache.dir_cache_time, "24h");
        assert_eq!(cfg.network.drive_pacer_burst, 10);
        assert_eq!(cfg.sync.transfers, 4);
        assert_eq!(cfg.sync.home_sync_interval_secs, 30);
        assert_eq!(cfg.process.sigterm_timeout_secs, 3);
        assert_eq!(cfg.network.timeout, "30s");
        assert_eq!(cfg.network.contimeout, "10s");
        assert_eq!(cfg.network.low_level_retries, 3);
        assert_eq!(cfg.network.retries, 3);
        assert!(cfg.features.enable_vfs_prewarm);
        assert!(cfg.features.enable_freeze_prevention);
    }
}
