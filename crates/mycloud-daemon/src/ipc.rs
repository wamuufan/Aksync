use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Command;
use tokio::sync::{watch, RwLock};
use tracing::{error, info, warn};
use zeroize::Zeroize;

use crate::config::DaemonConfig;
use mycloud_crypto::{CryptoEngine, KeyDerivation};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Error, Debug)]
pub enum IpcError {
    #[error("IO error on UDS socket: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Represents the high-level synchronization state of the client
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

/// Information about initial setup progress
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupStatusPayload {
    pub is_complete: bool,
    pub rclone_configured: bool,
    pub vault_initialized: bool,
    pub remote_name: String,
}

/// Transfer speeds and storage statistics for IPC clients
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TransferStatsPayload {
    pub upload_speed: u64,
    pub download_speed: u64,
    pub storage_used: u64,
    pub storage_total: u64,
    pub files_synced: u64,
}

/// Custom Google Drive API credentials for rclone
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DriveApiCredentialsPayload {
    #[serde(alias = "clientId")]
    pub client_id: String,
    #[serde(alias = "clientSecret")]
    pub client_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangePassphrasePayload {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoverVaultPayload {
    pub recovery_key: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultInitPayload {
    pub message: String,
    pub recovery_key: String,
}

/// IPC commands received from CLI or UI clients
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "data")]
pub enum IpcCommand {
    GetStatus,
    CheckSetupStatus,
    GetTransferStats,
    GetSyncFolders,
    SetSyncFolders(Vec<String>),
    GetSyncHiddenFiles,
    SetSyncHiddenFiles(bool),
    GetDriveApiCredentials,
    SetDriveApiCredentials(DriveApiCredentialsPayload),
    TriggerDriveAuth,
    InitializeVault(String),
    UnlockVault(String),
    ChangePassphrase(ChangePassphrasePayload),
    RecoverVault(RecoverVaultPayload),
    LockVault,
    SetPaused(bool),
    TriggerSync,
    Shutdown,
}

/// IPC responses sent back to clients
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum IpcResponse {
    Status {
        state: SyncState,
        rclone_pid: Option<u32>,
        fuse_pid: Option<u32>,
        raw_mount_path: String,
        target_mount_path: String,
    },
    SetupStatus(SetupStatusPayload),
    TransferStats(TransferStatsPayload),
    SyncFolders(Vec<crate::home_sync::SyncFolderInfo>),
    SyncHiddenFiles(bool),
    DriveApiCredentials(DriveApiCredentialsPayload),
    Ok(String),
    Err(String),
}

/// Shared state accessible by the IPC server
#[derive(Debug)]
pub struct DaemonSharedState {
    pub sync_state: watch::Sender<SyncState>,
    pub rclone_pid: Arc<RwLock<Option<u32>>>,
    pub fuse_pid: Arc<RwLock<Option<u32>>>,
    pub raw_mount_path: Arc<RwLock<PathBuf>>,
    pub target_mount_path: PathBuf,
    pub config_path: PathBuf,
    pub daemon_config: Arc<RwLock<DaemonConfig>>,
    pub shutdown_sender: tokio::sync::mpsc::Sender<()>,
    pub rclone_stats: Arc<RwLock<TransferStatsPayload>>,
    pub rclone_bin: String,
    pub master_key: Arc<RwLock<Option<zeroize::Zeroizing<[u8; 32]>>>>,
    pub optimizations: crate::config::OptimizationConfig,
    pub rc_addr: String,
    pub rc_user: String,
    pub rc_pass: String,
}

/// Unix Domain Socket (UDS) Server for IPC communication
pub struct IpcServer {
    socket_path: PathBuf,
    shared_state: Arc<DaemonSharedState>,
}

impl IpcServer {
    pub fn new(socket_path: PathBuf, shared_state: Arc<DaemonSharedState>) -> Self {
        Self {
            socket_path,
            shared_state,
        }
    }

    /// Starts the UDS listener and processes client connections
    pub async fn run(
        &self,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), IpcError> {
        // Clean up stale socket file if it exists
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }

        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        #[cfg(unix)]
        let _ = std::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o600));
        info!("UDS IPC Server listening at {:?}", self.socket_path);

        loop {
            tokio::select! {
                Ok((stream, _)) = listener.accept() => {
                    let state = Arc::clone(&self.shared_state);
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream, state).await {
                            warn!("IPC client connection error: {}", e);
                        }
                    });
                }
                _ = shutdown_rx.recv() => {
                    info!("UDS IPC Server received shutdown signal");
                    break;
                }
            }
        }

        // Clean up socket file on graceful exit
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }

        Ok(())
    }

    /// Handles an individual client connection over UDS
    async fn handle_connection(
        mut stream: UnixStream,
        state: Arc<DaemonSharedState>,
    ) -> Result<(), IpcError> {
        let (reader, mut writer) = stream.split();
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();

        while buf_reader.read_line(&mut line).await? > 0 {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                line.clear();
                continue;
            }

            let command: Result<IpcCommand, _> = serde_json::from_str(trimmed);
            let response = match command {
                Ok(IpcCommand::GetStatus) => {
                    let current_state = state.sync_state.borrow().clone();
                    let r_pid = *state.rclone_pid.read().await;
                    let f_pid = *state.fuse_pid.read().await;
                    let raw_path = state.raw_mount_path.read().await.display().to_string();
                    IpcResponse::Status {
                        state: current_state,
                        rclone_pid: r_pid,
                        fuse_pid: f_pid,
                        raw_mount_path: raw_path,
                        target_mount_path: state.target_mount_path.display().to_string(),
                    }
                }
                Ok(IpcCommand::CheckSetupStatus) => {
                    let cfg = state.daemon_config.read().await;
                    IpcResponse::SetupStatus(SetupStatusPayload {
                        is_complete: cfg.is_setup_complete(),
                        rclone_configured: cfg.rclone_configured,
                        vault_initialized: cfg.vault_initialized,
                        remote_name: cfg.rclone_remote.clone(),
                    })
                }
                Ok(IpcCommand::GetTransferStats) => {
                    let stats = state.rclone_stats.read().await.clone();
                    IpcResponse::TransferStats(stats)
                }
                Ok(IpcCommand::GetSyncFolders) => {
                    let selected = {
                        let cfg = state.daemon_config.read().await;
                        cfg.selected_sync_folders.clone()
                    };
                    let folders = crate::home_sync::get_user_home_folders(&selected);
                    IpcResponse::SyncFolders(folders)
                }
                Ok(IpcCommand::SetSyncFolders(folders)) => {
                    let (rclone_bin, target_mount_path, is_unlocked, sync_hidden) = {
                        let mut cfg = state.daemon_config.write().await;
                        cfg.selected_sync_folders = folders.clone();
                        let _ = cfg.save_to(&state.config_path);
                        (
                            state.rclone_bin.clone(),
                            state.target_mount_path.clone(),
                            state.fuse_pid.read().await.is_some(),
                            cfg.sync_hidden_files,
                        )
                    };

                    info!("Updated selected home sync folders: {:?}", folders);

                    if is_unlocked {
                        // Trigger an asynchronous background sync for the selected folders into the encrypted vault
                        let folders_clone = folders.clone();
                        let sync_state = state.sync_state.clone();
                        let opt = state.optimizations.clone();
                        let rc_addr = state.rc_addr.clone();
                        let rc_user = state.rc_user.clone();
                        let rc_pass = state.rc_pass.clone();
                        tokio::spawn(async move {
                            let _ = sync_state.send(SyncState::Syncing);
                            let _ = crate::home_sync::execute_home_sync(
                                &rclone_bin,
                                &target_mount_path,
                                &folders_clone,
                                sync_hidden,
                                opt.sync.transfers,
                                &rc_addr,
                                &opt.process.vault_rc_bind_addr,
                                &rc_user,
                                &rc_pass,
                            )
                            .await;
                            let _ = sync_state.send(SyncState::Idle);
                        });
                    }

                    IpcResponse::Ok(format!("Configured {} sync folders", folders.len()))
                }
                Ok(IpcCommand::GetSyncHiddenFiles) => {
                    let sync_hidden = {
                        let cfg = state.daemon_config.read().await;
                        cfg.sync_hidden_files
                    };
                    IpcResponse::SyncHiddenFiles(sync_hidden)
                }
                Ok(IpcCommand::SetSyncHiddenFiles(enabled)) => {
                    let (rclone_bin, target_mount_path, is_unlocked, selected) = {
                        let mut cfg = state.daemon_config.write().await;
                        cfg.sync_hidden_files = enabled;
                        let _ = cfg.save_to(&state.config_path);
                        (
                            state.rclone_bin.clone(),
                            state.target_mount_path.clone(),
                            state.fuse_pid.read().await.is_some(),
                            cfg.selected_sync_folders.clone(),
                        )
                    };

                    info!("Updated sync_hidden_files setting to: {}", enabled);

                    if is_unlocked && !selected.is_empty() {
                        let sync_state = state.sync_state.clone();
                        let opt = state.optimizations.clone();
                        let rc_addr = state.rc_addr.clone();
                        let rc_user = state.rc_user.clone();
                        let rc_pass = state.rc_pass.clone();
                        tokio::spawn(async move {
                            let _ = sync_state.send(SyncState::Syncing);
                            let _ = crate::home_sync::execute_home_sync(
                                &rclone_bin,
                                &target_mount_path,
                                &selected,
                                enabled,
                                opt.sync.transfers,
                                &rc_addr,
                                &opt.process.vault_rc_bind_addr,
                                &rc_user,
                                &rc_pass,
                            )
                            .await;
                            let _ = sync_state.send(SyncState::Idle);
                        });
                    }

                    IpcResponse::Ok(format!("Sync hidden files set to {enabled}"))
                }
                Ok(IpcCommand::GetDriveApiCredentials) => {
                    let (cfg_client_id, cfg_client_secret, remote_name) = {
                        let cfg = state.daemon_config.read().await;
                        (
                            cfg.custom_client_id.clone(),
                            cfg.custom_client_secret.clone(),
                            cfg.rclone_remote.clone(),
                        )
                    };

                    let mut client_id = cfg_client_id.unwrap_or_default();
                    let mut client_secret = cfg_client_secret.unwrap_or_default();

                    // If not found in DaemonConfig, attempt to read from rclone config show
                    if client_id.is_empty() && client_secret.is_empty() {
                        if let Ok(output) = Command::new(&state.rclone_bin)
                            .arg("config")
                            .arg("show")
                            .arg(&remote_name)
                            .output()
                            .await
                        {
                            if output.status.success() {
                                let stdout = String::from_utf8_lossy(&output.stdout);
                                for line in stdout.lines() {
                                    let trimmed = line.trim();
                                    if let Some((k, v)) = trimmed.split_once('=') {
                                        let key = k.trim();
                                        let val = v.trim();
                                        if key == "client_id" && client_id.is_empty() {
                                            client_id = val.to_string();
                                        } else if key == "client_secret" && client_secret.is_empty() {
                                            client_secret = val.to_string();
                                        }
                                    }
                                }
                            }
                        }
                    }

                    IpcResponse::DriveApiCredentials(DriveApiCredentialsPayload {
                        client_id,
                        client_secret,
                    })
                }
                Ok(IpcCommand::SetDriveApiCredentials(payload)) => {
                    let remote_name = {
                        let mut cfg = state.daemon_config.write().await;
                        cfg.custom_client_id = if payload.client_id.trim().is_empty() {
                            None
                        } else {
                            Some(payload.client_id.trim().to_string())
                        };
                        cfg.custom_client_secret = if payload.client_secret.trim().is_empty() {
                            None
                        } else {
                            Some(payload.client_secret.trim().to_string())
                        };
                        let _ = cfg.save_to(&state.config_path);
                        cfg.rclone_remote.clone()
                    };

                    info!(
                        "Updating custom Google Drive API credentials for remote '{}'",
                        remote_name
                    );

                    let cid = payload.client_id.trim();
                    let cs = payload.client_secret.trim();

                    let mut cmd = Command::new(&state.rclone_bin);
                    cmd.arg("config")
                        .arg("update")
                        .arg(&remote_name)
                        .arg("client_id")
                        .arg(cid)
                        .arg("client_secret")
                        .arg(cs)
                        .arg("config_refresh_token=false");

                    let _ = cmd.output().await;

                    IpcResponse::Ok("Google Drive API credentials updated successfully".to_string())
                }
                Ok(IpcCommand::TriggerDriveAuth) => {
                    let (remote_name, custom_id, custom_secret) = {
                        let cfg = state.daemon_config.read().await;
                        (
                            cfg.rclone_remote.clone(),
                            cfg.custom_client_id.clone(),
                            cfg.custom_client_secret.clone(),
                        )
                    };

                    info!("Triggering rclone OAuth for remote '{}'", remote_name);
                    let mut cmd = Command::new(&state.rclone_bin);
                    cmd.arg("config")
                        .arg("create")
                        .arg(&remote_name)
                        .arg("drive")
                        .arg("config_is_local")
                        .arg("true");

                    if let Some(ref cid) = custom_id {
                        if !cid.trim().is_empty() {
                            cmd.arg("client_id").arg(cid.trim());
                        }
                    }
                    if let Some(ref cs) = custom_secret {
                        if !cs.trim().is_empty() {
                            cmd.arg("client_secret").arg(cs.trim());
                        }
                    }

                    let output_res = cmd.output().await;

                    match output_res {
                        Ok(output) if output.status.success() => {
                            let mut cfg = state.daemon_config.write().await;
                            cfg.rclone_configured = true;
                            if let Err(e) = cfg.save_to(&state.config_path) {
                                error!("Failed to save config after rclone OAuth: {}", e);
                            }
                            info!(
                                "Google Drive OAuth completed successfully for remote '{}'",
                                remote_name
                            );
                            if cfg.is_setup_complete() {
                                let is_unlocked = state.master_key.read().await.is_some();
                                if is_unlocked {
                                    let _ = state.sync_state.send(SyncState::Idle);
                                } else {
                                    let _ = state.sync_state.send(SyncState::Locked);
                                }
                            }
                            IpcResponse::Ok(format!(
                                "Drive remote '{remote_name}' configured successfully"
                            ))
                        }
                        Ok(output) => {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            warn!("rclone config create failed: {}", stderr);
                            IpcResponse::Err(format!("rclone config failed: {}", stderr.trim()))
                        }
                        Err(e) => {
                            error!("Failed to execute rclone: {}", e);
                            IpcResponse::Err(format!("Failed to execute rclone binary: {e}"))
                        }
                    }
                }
                Ok(IpcCommand::InitializeVault(password)) => {
                    Self::handle_initialize_vault(&state, password).await
                }
                Ok(IpcCommand::UnlockVault(password)) => {
                    Self::handle_unlock_vault(&state, password).await
                }
                Ok(IpcCommand::ChangePassphrase(payload)) => {
                    Self::handle_change_passphrase(&state, payload).await
                }
                Ok(IpcCommand::RecoverVault(payload)) => {
                    Self::handle_recover_vault(&state, payload).await
                }
                Ok(IpcCommand::LockVault) => {
                    let current = state.sync_state.borrow().clone();
                    if current == SyncState::SetupRequired {
                        IpcResponse::Err("Cannot lock vault before setup is complete".to_string())
                    } else {
                        *state.master_key.write().await = None;
                        let _ = state.sync_state.send(SyncState::Locked);
                        IpcResponse::Ok("Vault locked successfully".to_string())
                    }
                }
                Ok(IpcCommand::SetPaused(paused)) => {
                    let current = state.sync_state.borrow().clone();
                    match current {
                        SyncState::Locked | SyncState::SetupRequired => IpcResponse::Err(
                            "Cannot pause or resume while vault is locked or setup is required"
                                .to_string(),
                        ),
                        SyncState::Error(err) => IpcResponse::Err(format!(
                            "Cannot change pause state while daemon is in error state: {err}"
                        )),
                        _ => {
                            let next = if paused {
                                SyncState::Paused
                            } else {
                                SyncState::Idle
                            };
                            let _ = state.sync_state.send(next);
                            IpcResponse::Ok(format!("Sync paused set to {paused}"))
                        }
                    }
                }
                Ok(IpcCommand::TriggerSync) => {
                    let current = state.sync_state.borrow().clone();
                    match current {
                        SyncState::Locked | SyncState::SetupRequired => IpcResponse::Err(
                            "Cannot trigger sync while vault is locked or setup is required"
                                .to_string(),
                        ),
                        SyncState::Error(err) => IpcResponse::Err(format!(
                            "Cannot trigger sync while daemon is in error state: {err}"
                        )),
                        _ => {
                            let (rclone_bin, target_mount_path, selected, sync_hidden) = {
                                let cfg = state.daemon_config.read().await;
                                (
                                    state.rclone_bin.clone(),
                                    state.target_mount_path.clone(),
                                    cfg.selected_sync_folders.clone(),
                                    cfg.sync_hidden_files,
                                )
                            };
                            let sync_state = state.sync_state.clone();
                            let opt = state.optimizations.clone();
                            let rc_addr = state.rc_addr.clone();
                            let rc_user = state.rc_user.clone();
                            let rc_pass = state.rc_pass.clone();
                            tokio::spawn(async move {
                                let _ = sync_state.send(SyncState::Syncing);
                                let _ = crate::home_sync::execute_home_sync(
                                    &rclone_bin,
                                    &target_mount_path,
                                    &selected,
                                    sync_hidden,
                                    opt.sync.transfers,
                                    &rc_addr,
                                    &opt.process.vault_rc_bind_addr,
                                    &rc_user,
                                    &rc_pass,
                                )
                                .await;
                                let _ = sync_state.send(SyncState::Idle);
                            });
                            IpcResponse::Ok("Sync triggered".to_string())
                        }
                    }
                }
                Ok(IpcCommand::Shutdown) => {
                    let _ = state.shutdown_sender.send(()).await;
                    IpcResponse::Ok("Shutdown scheduled".to_string())
                }
                Err(err) => IpcResponse::Err(format!("Invalid command JSON: {err}")),
            };

            let mut resp_json = serde_json::to_string(&response)?;
            resp_json.push('\n');
            writer.write_all(resp_json.as_bytes()).await?;
            writer.flush().await?;

            line.clear();
        }

        Ok(())
    }

    async fn handle_initialize_vault(
        state: &Arc<DaemonSharedState>,
        mut password: String,
    ) -> IpcResponse {
        let is_already_init = {
            let cfg = state.daemon_config.read().await;
            cfg.vault_initialized
        };
        if is_already_init {
            password.zeroize();
            return IpcResponse::Err("Vault is already initialized".to_string());
        }

        if password.trim().len() < 8 {
            password.zeroize();
            return IpcResponse::Err(
                "Master password must be at least 8 characters long".to_string(),
            );
        }

        let salt_bytes = KeyDerivation::generate_salt();
        let salt_clone = salt_bytes;
        let pass_clone = password.clone();
        let kek_res = tokio::task::spawn_blocking(move || {
            let mut p = pass_clone;
            let res = KeyDerivation::derive_key(&p, &salt_clone);
            p.zeroize();
            res
        })
        .await;

        let effective_webdav_pass = if state.rclone_bin != "true" {
            crate::process::read_kwallet_webdav_password(
                "mycloud",
                &state.optimizations.process.webdav_bind_addr,
                &state.optimizations.process.vault_webdav_bind_addr,
            )
            .unwrap_or_else(|| password.clone())
        } else {
            password.clone()
        };
        if state.rclone_bin != "true" {
            let _ = crate::process::write_htpasswd_file("mycloud", &effective_webdav_pass);
        }

        password.zeroize();

        let kek = match kek_res {
            Ok(Ok(k)) => k,
            Ok(Err(e)) => return IpcResponse::Err(format!("Key derivation failed: {e}")),
            Err(e) => return IpcResponse::Err(format!("Task execution failed: {e}")),
        };

        let dmk = KeyDerivation::generate_master_key();

        let wrapped_dmk = match KeyDerivation::wrap_key(&kek, &dmk) {
            Ok(w) => w,
            Err(e) => return IpcResponse::Err(format!("Failed to wrap DMK: {e}")),
        };

        let recovery_key = KeyDerivation::generate_recovery_key();
        let rec_kek = match KeyDerivation::derive_kek_from_recovery(&recovery_key) {
            Ok(k) => k,
            Err(e) => {
                return IpcResponse::Err(format!("Failed to derive KEK from recovery key: {e}"))
            }
        };

        let wrapped_dmk_recovery = match KeyDerivation::wrap_key(&rec_kek, &dmk) {
            Ok(w) => w,
            Err(e) => {
                return IpcResponse::Err(format!("Failed to wrap DMK with recovery key: {e}"))
            }
        };

        let engine = CryptoEngine::from_key(*dmk);
        let v_token = match engine.generate_vault_verification_token() {
            Ok(t) => t,
            Err(e) => return IpcResponse::Err(format!("Failed to generate vault token: {e}")),
        };

        let salt_b64 = base64::engine::general_purpose::STANDARD.encode(salt_bytes);
        let is_complete = {
            let mut cfg = state.daemon_config.write().await;
            cfg.vault_initialized = true;
            cfg.vault_salt_base64 = Some(salt_b64);
            cfg.vault_verification_tag = Some(v_token);
            cfg.wrapped_dmk = Some(wrapped_dmk);
            cfg.wrapped_dmk_recovery = Some(wrapped_dmk_recovery);
            cfg.webdav_password = Some(effective_webdav_pass);
            cfg.sync_hidden_files = state.optimizations.sync.default_sync_hidden_files;
            if let Err(e) = cfg.save_to(&state.config_path) {
                return IpcResponse::Err(format!("Failed to save config: {e}"));
            }
            cfg.is_setup_complete()
        };

        *state.master_key.write().await = Some(dmk);

        if is_complete {
            let _ = state.sync_state.send(SyncState::Connecting);
        }

        IpcResponse::Ok(format!(
            "Vault initialized successfully. Recovery Key: {}",
            *recovery_key
        ))
    }

    async fn handle_unlock_vault(
        state: &Arc<DaemonSharedState>,
        mut password: String,
    ) -> IpcResponse {
        let (is_init, salt_opt, wrapped_opt, token_opt) = {
            let cfg = state.daemon_config.read().await;
            (
                cfg.vault_initialized,
                cfg.vault_salt_base64.clone(),
                cfg.wrapped_dmk.clone(),
                cfg.vault_verification_tag.clone(),
            )
        };

        if !is_init || salt_opt.is_none() {
            password.zeroize();
            return IpcResponse::Err("Vault is not initialized".to_string());
        }

        let salt_b64 = salt_opt.unwrap();
        let salt_bytes = match base64::engine::general_purpose::STANDARD.decode(salt_b64.trim()) {
            Ok(b) => b,
            Err(e) => {
                password.zeroize();
                return IpcResponse::Err(format!("Corrupt salt in configuration: {e}"));
            }
        };

        let mut webdav_candidate = password.clone();
        let pass_clone = password.clone();
        let kek_res = tokio::task::spawn_blocking(move || {
            let mut p = pass_clone;
            let res = KeyDerivation::derive_key(&p, &salt_bytes);
            p.zeroize();
            res
        })
        .await;
        password.zeroize();

        let kek = match kek_res {
            Ok(Ok(k)) => k,
            Ok(Err(e)) => {
                webdav_candidate.zeroize();
                return IpcResponse::Err(format!("Key derivation failed: {e}"));
            }
            Err(e) => {
                webdav_candidate.zeroize();
                return IpcResponse::Err(format!("Task execution failed: {e}"));
            }
        };

        let dmk = if let Some(ref w_dmk) = wrapped_opt {
            let dmk = match KeyDerivation::unwrap_key(&kek, w_dmk) {
                Ok(k) => k,
                Err(_) => {
                    webdav_candidate.zeroize();
                    return IpcResponse::Err("Invalid master password".to_string());
                }
            };
            match token_opt {
                Some(ref token) => {
                    let engine = CryptoEngine::from_key(*dmk);
                    if !engine.verify_vault_token(token) {
                        webdav_candidate.zeroize();
                        return IpcResponse::Err("Invalid master password".to_string());
                    }
                }
                None => {
                    webdav_candidate.zeroize();
                    return IpcResponse::Err(
                        "Vault has no verification token (legacy vault); password cannot be verified safely.".to_string(),
                    );
                }
            }
            dmk
        } else {
            // Stage 5 migration: no wrapped DMK present
            match token_opt {
                Some(ref token) => {
                    let engine = CryptoEngine::from_key(*kek);
                    if !engine.verify_vault_token(token) {
                        webdav_candidate.zeroize();
                        return IpcResponse::Err("Invalid master password".to_string());
                    }
                }
                None => {
                    webdav_candidate.zeroize();
                    return IpcResponse::Err(
                        "Vault has no verification token (legacy vault); password cannot be verified safely.".to_string(),
                    );
                }
            }
            let dmk = kek.clone();
            if let Ok(wrapped) = KeyDerivation::wrap_key(&kek, &dmk) {
                let mut cfg = state.daemon_config.write().await;
                cfg.wrapped_dmk = Some(wrapped);
                let _ = cfg.save_to(&state.config_path);
            }
            dmk
        };

        let effective_webdav_pass = if state.rclone_bin != "true" {
            crate::process::read_kwallet_webdav_password(
                "mycloud",
                &state.optimizations.process.webdav_bind_addr,
                &state.optimizations.process.vault_webdav_bind_addr,
            )
            .unwrap_or_else(|| webdav_candidate.clone())
        } else {
            webdav_candidate.clone()
        };
        webdav_candidate.zeroize();

        if state.rclone_bin != "true" {
            let _ = crate::process::write_htpasswd_file("mycloud", &effective_webdav_pass);
        }

        {
            let mut cfg = state.daemon_config.write().await;
            cfg.webdav_password = Some(effective_webdav_pass);
            let _ = cfg.save_to(&state.config_path);
        }

        *state.master_key.write().await = Some(dmk);

        let current_sync = state.sync_state.borrow().clone();
        if current_sync == SyncState::Locked {
            let _ = state.sync_state.send(SyncState::Connecting);
        }

        IpcResponse::Ok("Vault unlocked successfully".to_string())
    }

    async fn handle_change_passphrase(
        state: &Arc<DaemonSharedState>,
        mut payload: ChangePassphrasePayload,
    ) -> IpcResponse {
        if payload.new_password.trim().len() < 8 {
            payload.current_password.zeroize();
            payload.new_password.zeroize();
            return IpcResponse::Err(
                "New master password must be at least 8 characters long".to_string(),
            );
        }

        let (is_init, salt_opt, wrapped_opt, token_opt) = {
            let cfg = state.daemon_config.read().await;
            (
                cfg.vault_initialized,
                cfg.vault_salt_base64.clone(),
                cfg.wrapped_dmk.clone(),
                cfg.vault_verification_tag.clone(),
            )
        };

        if !is_init || salt_opt.is_none() {
            payload.current_password.zeroize();
            payload.new_password.zeroize();
            return IpcResponse::Err("Vault is not initialized".to_string());
        }

        let old_salt_bytes =
            match base64::engine::general_purpose::STANDARD.decode(salt_opt.unwrap().trim()) {
                Ok(b) => b,
                Err(e) => {
                    payload.current_password.zeroize();
                    payload.new_password.zeroize();
                    return IpcResponse::Err(format!("Corrupt salt in configuration: {e}"));
                }
            };

        let cur_pass = payload.current_password.clone();
        let old_kek_res = tokio::task::spawn_blocking(move || {
            let mut p = cur_pass;
            let res = KeyDerivation::derive_key(&p, &old_salt_bytes);
            p.zeroize();
            res
        })
        .await;
        payload.current_password.zeroize();

        let old_kek = match old_kek_res {
            Ok(Ok(k)) => k,
            _ => {
                payload.new_password.zeroize();
                return IpcResponse::Err("Invalid current master password".to_string());
            }
        };

        let dmk = if let Some(ref w_dmk) = wrapped_opt {
            let dmk = match KeyDerivation::unwrap_key(&old_kek, w_dmk) {
                Ok(k) => k,
                Err(_) => {
                    payload.new_password.zeroize();
                    return IpcResponse::Err("Invalid current master password".to_string());
                }
            };
            match token_opt {
                Some(ref token) => {
                    let engine = CryptoEngine::from_key(*dmk);
                    if !engine.verify_vault_token(token) {
                        payload.new_password.zeroize();
                        return IpcResponse::Err("Invalid current master password".to_string());
                    }
                }
                None => {
                    payload.new_password.zeroize();
                    return IpcResponse::Err(
                        "Vault has no verification token (legacy vault); password cannot be verified safely.".to_string(),
                    );
                }
            }
            dmk
        } else {
            match token_opt {
                Some(ref token) => {
                    let engine = CryptoEngine::from_key(*old_kek);
                    if !engine.verify_vault_token(token) {
                        payload.new_password.zeroize();
                        return IpcResponse::Err("Invalid current master password".to_string());
                    }
                }
                None => {
                    payload.new_password.zeroize();
                    return IpcResponse::Err(
                        "Vault has no verification token (legacy vault); password cannot be verified safely.".to_string(),
                    );
                }
            }
            old_kek.clone()
        };

        let new_salt_bytes = KeyDerivation::generate_salt();
        let new_salt_clone = new_salt_bytes;
        let new_pass = payload.new_password.clone();
        let new_kek_res = tokio::task::spawn_blocking(move || {
            let mut p = new_pass;
            let res = KeyDerivation::derive_key(&p, &new_salt_clone);
            p.zeroize();
            res
        })
        .await;
        payload.new_password.zeroize();

        let new_kek = match new_kek_res {
            Ok(Ok(k)) => k,
            Ok(Err(e)) => return IpcResponse::Err(format!("Key derivation failed: {e}")),
            Err(e) => return IpcResponse::Err(format!("Task execution failed: {e}")),
        };

        let new_wrapped = match KeyDerivation::wrap_key(&new_kek, &dmk) {
            Ok(w) => w,
            Err(e) => return IpcResponse::Err(format!("Failed to wrap DMK: {e}")),
        };

        let new_salt_b64 = base64::engine::general_purpose::STANDARD.encode(new_salt_bytes);
        {
            let mut cfg = state.daemon_config.write().await;
            cfg.vault_salt_base64 = Some(new_salt_b64);
            cfg.wrapped_dmk = Some(new_wrapped);
            if let Err(e) = cfg.save_to(&state.config_path) {
                return IpcResponse::Err(format!("Failed to save config: {e}"));
            }
        }

        *state.master_key.write().await = Some(dmk);

        IpcResponse::Ok("Master passphrase changed successfully".to_string())
    }

    async fn handle_recover_vault(
        state: &Arc<DaemonSharedState>,
        mut payload: RecoverVaultPayload,
    ) -> IpcResponse {
        if payload.new_password.trim().len() < 8 {
            payload.recovery_key.zeroize();
            payload.new_password.zeroize();
            return IpcResponse::Err(
                "New master password must be at least 8 characters long".to_string(),
            );
        }

        let (is_init, wrapped_recovery_opt, token_opt) = {
            let cfg = state.daemon_config.read().await;
            (
                cfg.vault_initialized,
                cfg.wrapped_dmk_recovery.clone(),
                cfg.vault_verification_tag.clone(),
            )
        };

        if !is_init {
            payload.recovery_key.zeroize();
            payload.new_password.zeroize();
            return IpcResponse::Err("Vault is not initialized".to_string());
        }

        let wrapped_recovery = match wrapped_recovery_opt {
            Some(w) => w,
            None => {
                payload.recovery_key.zeroize();
                payload.new_password.zeroize();
                return IpcResponse::Err("No recovery key configured for this vault".to_string());
            }
        };

        let rec_kek = match KeyDerivation::derive_kek_from_recovery(&payload.recovery_key) {
            Ok(k) => k,
            Err(_) => {
                payload.recovery_key.zeroize();
                payload.new_password.zeroize();
                return IpcResponse::Err("Invalid recovery key".to_string());
            }
        };
        payload.recovery_key.zeroize();

        let dmk = match KeyDerivation::unwrap_key(&rec_kek, &wrapped_recovery) {
            Ok(k) => k,
            Err(_) => {
                payload.new_password.zeroize();
                return IpcResponse::Err("Invalid recovery key".to_string());
            }
        };

        match token_opt {
            Some(ref token) => {
                let engine = CryptoEngine::from_key(*dmk);
                if !engine.verify_vault_token(token) {
                    payload.new_password.zeroize();
                    return IpcResponse::Err("Invalid recovery key".to_string());
                }
            }
            None => {
                payload.new_password.zeroize();
                return IpcResponse::Err(
                    "Vault has no verification token (legacy vault); password cannot be verified safely.".to_string(),
                );
            }
        }

        let new_salt_bytes = KeyDerivation::generate_salt();
        let new_salt_clone = new_salt_bytes;
        let new_pass = payload.new_password.clone();
        let new_kek_res = tokio::task::spawn_blocking(move || {
            let mut p = new_pass;
            let res = KeyDerivation::derive_key(&p, &new_salt_clone);
            p.zeroize();
            res
        })
        .await;
        payload.new_password.zeroize();

        let new_kek = match new_kek_res {
            Ok(Ok(k)) => k,
            Ok(Err(e)) => return IpcResponse::Err(format!("Key derivation failed: {e}")),
            Err(e) => return IpcResponse::Err(format!("Task execution failed: {e}")),
        };

        let new_wrapped = match KeyDerivation::wrap_key(&new_kek, &dmk) {
            Ok(w) => w,
            Err(e) => return IpcResponse::Err(format!("Failed to wrap DMK: {e}")),
        };

        let new_salt_b64 = base64::engine::general_purpose::STANDARD.encode(new_salt_bytes);
        {
            let mut cfg = state.daemon_config.write().await;
            cfg.vault_salt_base64 = Some(new_salt_b64);
            cfg.wrapped_dmk = Some(new_wrapped);
            if let Err(e) = cfg.save_to(&state.config_path) {
                return IpcResponse::Err(format!("Failed to save config: {e}"));
            }
        }

        *state.master_key.write().await = Some(dmk);

        let current_sync = state.sync_state.borrow().clone();
        if current_sync == SyncState::Locked {
            let _ = state.sync_state.send(SyncState::Connecting);
        }

        IpcResponse::Ok("Vault recovered and passphrase updated successfully".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_sync_state_serialization() {
        let state = SyncState::SetupRequired;
        let serialized = serde_json::to_string(&state).expect("serialize failed");
        assert_eq!(serialized, "\"setup_required\"");

        let deserialized: SyncState =
            serde_json::from_str(&serialized).expect("deserialize failed");
        assert_eq!(deserialized, SyncState::SetupRequired);

        let err_state = SyncState::Error("network connection lost".to_string());
        let err_serialized =
            serde_json::to_string(&err_state).expect("serialize error state failed");
        assert!(err_serialized.contains("network connection lost"));
    }

    #[tokio::test]
    async fn test_ipc_server_roundtrip() {
        let temp_sock =
            std::env::temp_dir().join(format!("test_mycloud_{}.sock", std::process::id()));
        let temp_cfg =
            std::env::temp_dir().join(format!("test_mycloud_cfg_{}.toml", std::process::id()));
        let (sync_state_tx, _sync_state_rx) = watch::channel(SyncState::Idle);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
        let (app_shutdown_tx, _app_shutdown_rx) = tokio::sync::mpsc::channel(1);

        let test_daemon_cfg = DaemonConfig {
            rclone_remote: "test_mock_remote".to_string(),
            ..DaemonConfig::default()
        };

        let shared_state = Arc::new(DaemonSharedState {
            sync_state: sync_state_tx,
            rclone_pid: Arc::new(RwLock::new(Some(1234))),
            fuse_pid: Arc::new(RwLock::new(Some(5678))),
            raw_mount_path: Arc::new(RwLock::new(PathBuf::from("/tmp/raw"))),
            target_mount_path: PathBuf::from("/tmp/target"),
            config_path: temp_cfg.clone(),
            daemon_config: Arc::new(RwLock::new(test_daemon_cfg)),
            shutdown_sender: app_shutdown_tx,
            rclone_stats: Arc::new(RwLock::new(TransferStatsPayload::default())),
            rclone_bin: "true".to_string(),
            master_key: Arc::new(RwLock::new(None)),
            optimizations: crate::config::OptimizationConfig::default(),
            rc_addr: "127.0.0.1:5572".to_string(),
            rc_user: "mycloud".to_string(),
            rc_pass: "testpass".to_string(),
        });

        let server = IpcServer::new(temp_sock.clone(), Arc::clone(&shared_state));
        let server_shutdown_rx = shutdown_tx.subscribe();
        let server_handle = tokio::spawn(async move {
            server.run(server_shutdown_rx).await.unwrap();
        });

        // Give the listener a moment to bind
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = UnixStream::connect(&temp_sock)
            .await
            .expect("failed to connect to socket");
        let (reader, mut writer) = client.split();
        let mut buf_reader = BufReader::new(reader);

        // Send CheckSetupStatus command
        let req = serde_json::to_string(&IpcCommand::CheckSetupStatus).unwrap() + "\n";
        writer.write_all(req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        let mut response_line = String::new();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();

        match resp {
            IpcResponse::SetupStatus(payload) => {
                assert!(!payload.is_complete);
                assert!(!payload.rclone_configured);
                assert!(!payload.vault_initialized);
            }
            _ => panic!("Unexpected response: {:?}", resp),
        }

        // Send InitializeVault command
        let init_req =
            serde_json::to_string(&IpcCommand::InitializeVault("supersecret123".to_string()))
                .unwrap()
                + "\n";
        writer.write_all(init_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let init_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match init_resp {
            IpcResponse::Ok(msg) => assert!(msg.contains("Vault initialized")),
            _ => panic!("Expected Ok response, got {:?}", init_resp),
        }

        // Verify master key was derived in memory
        assert!(shared_state.master_key.read().await.is_some());

        // Check socket permissions (0o600)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&temp_sock).expect("socket metadata");
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "Socket permissions should be 0600");
        }

        // Test 1: Re-initialization attempt should be rejected
        let reinit_req =
            serde_json::to_string(&IpcCommand::InitializeVault("anothertoken123".to_string()))
                .unwrap()
                + "\n";
        writer.write_all(reinit_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let reinit_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match reinit_resp {
            IpcResponse::Err(msg) => assert!(msg.contains("already initialized")),
            _ => panic!("Expected Err for re-initialization, got {:?}", reinit_resp),
        }

        // Test 2: Unlock with WRONG password should be rejected
        let bad_unlock_req =
            serde_json::to_string(&IpcCommand::UnlockVault("wrong_pass_here".to_string())).unwrap()
                + "\n";
        writer.write_all(bad_unlock_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let bad_unlock_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match bad_unlock_resp {
            IpcResponse::Err(msg) => assert!(msg.contains("Invalid master password")),
            _ => panic!("Expected Err for wrong password, got {:?}", bad_unlock_resp),
        }

        // Test 3: Unlock with CORRECT password succeeds
        let unlock_req =
            serde_json::to_string(&IpcCommand::UnlockVault("supersecret123".to_string())).unwrap()
                + "\n";
        writer.write_all(unlock_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let unlock_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match unlock_resp {
            IpcResponse::Ok(msg) => assert!(msg.contains("Vault unlocked")),
            _ => panic!("Expected Ok response, got {:?}", unlock_resp),
        }

        // Test 4: State guards under Locked/Error states
        let _ = shared_state.sync_state.send(SyncState::Locked);
        let trigger_req = serde_json::to_string(&IpcCommand::TriggerSync).unwrap() + "\n";
        writer.write_all(trigger_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let trigger_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match trigger_resp {
            IpcResponse::Err(msg) => assert!(msg.contains("Cannot trigger sync")),
            _ => panic!(
                "Expected Err for TriggerSync while locked, got {:?}",
                trigger_resp
            ),
        }

        // Test 5: Custom Google Drive API Credentials Roundtrip
        let set_creds_cmd = IpcCommand::SetDriveApiCredentials(DriveApiCredentialsPayload {
            client_id: "custom-id-123.apps.googleusercontent.com".to_string(),
            client_secret: "custom-secret-abc".to_string(),
        });
        let set_creds_req = serde_json::to_string(&set_creds_cmd).unwrap() + "\n";
        writer.write_all(set_creds_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let set_creds_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match set_creds_resp {
            IpcResponse::Ok(msg) => assert!(msg.contains("credentials updated")),
            _ => panic!("Expected Ok response for SetDriveApiCredentials, got {:?}", set_creds_resp),
        }

        let get_creds_req = serde_json::to_string(&IpcCommand::GetDriveApiCredentials).unwrap() + "\n";
        writer.write_all(get_creds_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let get_creds_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match get_creds_resp {
            IpcResponse::DriveApiCredentials(creds) => {
                assert_eq!(creds.client_id, "custom-id-123.apps.googleusercontent.com");
                assert_eq!(creds.client_secret, "custom-secret-abc");
            }
            _ => panic!("Expected DriveApiCredentials response, got {:?}", get_creds_resp),
        }

        // Clean up
        let _ = shutdown_tx.send(());
        let _ = server_handle.await;
        let _ = std::fs::remove_file(&temp_sock);
        let _ = std::fs::remove_file(&temp_cfg);
    }

    #[tokio::test]
    async fn test_ipc_passphrase_change_and_recovery() {
        use std::time::Duration;

        let temp_sock = std::env::temp_dir().join(format!("mycloud_ipc_test2_{}.sock", std::process::id()));
        let temp_cfg = std::env::temp_dir().join(format!("mycloud_cfg_test2_{}.toml", std::process::id()));
        let (sync_state_tx, _sync_state_rx) = watch::channel(SyncState::SetupRequired);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
        let (app_shutdown_tx, _app_shutdown_rx) = tokio::sync::mpsc::channel(1);

        let test_daemon_cfg = DaemonConfig {
            rclone_configured: true,
            ..DaemonConfig::default()
        };

        let shared_state = Arc::new(DaemonSharedState {
            sync_state: sync_state_tx,
            rclone_pid: Arc::new(RwLock::new(Some(1234))),
            fuse_pid: Arc::new(RwLock::new(Some(5678))),
            raw_mount_path: Arc::new(RwLock::new(PathBuf::from("/tmp/raw"))),
            target_mount_path: PathBuf::from("/tmp/target"),
            config_path: temp_cfg.clone(),
            daemon_config: Arc::new(RwLock::new(test_daemon_cfg)),
            shutdown_sender: app_shutdown_tx,
            rclone_stats: Arc::new(RwLock::new(TransferStatsPayload::default())),
            rclone_bin: "true".to_string(),
            master_key: Arc::new(RwLock::new(None)),
            optimizations: crate::config::OptimizationConfig::default(),
            rc_addr: "127.0.0.1:5572".to_string(),
            rc_user: "mycloud".to_string(),
            rc_pass: "testpass".to_string(),
        });

        let server = IpcServer::new(temp_sock.clone(), Arc::clone(&shared_state));
        let server_shutdown_rx = shutdown_tx.subscribe();
        let server_handle = tokio::spawn(async move {
            server.run(server_shutdown_rx).await.unwrap();
        });

        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = UnixStream::connect(&temp_sock)
            .await
            .expect("failed to connect to socket");
        let (reader, mut writer) = client.split();
        let mut buf_reader = BufReader::new(reader);
        let mut response_line = String::new();

        // 1. Initialize vault and extract Recovery Key
        let init_req = serde_json::to_string(&IpcCommand::InitializeVault("supersecret123".to_string())).unwrap() + "\n";
        writer.write_all(init_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let init_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        let recovery_key = match init_resp {
            IpcResponse::Ok(msg) => {
                assert!(msg.contains("Vault initialized successfully. Recovery Key: "));
                let parts: Vec<&str> = msg.split("Recovery Key: ").collect();
                assert_eq!(parts.len(), 2);
                parts[1].trim().to_string()
            }
            _ => panic!("Expected Ok response from InitializeVault, got {:?}", init_resp),
        };
        assert_eq!(recovery_key.len(), 52);

        // Record DMK before changes
        let dmk_before = shared_state.master_key.read().await.as_ref().unwrap().clone();

        // 2. Change passphrase: supersecret123 -> new_ultra_secret_456
        let change_cmd = IpcCommand::ChangePassphrase(ChangePassphrasePayload {
            current_password: "supersecret123".to_string(),
            new_password: "new_ultra_secret_456".to_string(),
        });
        let change_req = serde_json::to_string(&change_cmd).unwrap() + "\n";
        writer.write_all(change_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let change_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match change_resp {
            IpcResponse::Ok(msg) => assert!(msg.contains("passphrase changed successfully")),
            _ => panic!("Expected Ok response from ChangePassphrase, got {:?}", change_resp),
        }

        // 3. Unlock with old password must fail
        let old_unlock_req = serde_json::to_string(&IpcCommand::UnlockVault("supersecret123".to_string())).unwrap() + "\n";
        writer.write_all(old_unlock_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let old_unlock_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match old_unlock_resp {
            IpcResponse::Err(msg) => assert!(msg.contains("Invalid master password")),
            _ => panic!("Expected Err for old password unlock, got {:?}", old_unlock_resp),
        }

        // 4. Unlock with new password must succeed
        let new_unlock_req = serde_json::to_string(&IpcCommand::UnlockVault("new_ultra_secret_456".to_string())).unwrap() + "\n";
        writer.write_all(new_unlock_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let new_unlock_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match new_unlock_resp {
            IpcResponse::Ok(msg) => assert!(msg.contains("Vault unlocked")),
            _ => panic!("Expected Ok response for new password unlock, got {:?}", new_unlock_resp),
        }

        let dmk_after_change = shared_state.master_key.read().await.as_ref().unwrap().clone();
        assert_eq!(dmk_after_change, dmk_before);

        // 5. Recover vault with recovery key -> recovered_pass_789
        let recover_cmd = IpcCommand::RecoverVault(RecoverVaultPayload {
            recovery_key: recovery_key.clone(),
            new_password: "recovered_pass_789".to_string(),
        });
        let recover_req = serde_json::to_string(&recover_cmd).unwrap() + "\n";
        writer.write_all(recover_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let recover_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match recover_resp {
            IpcResponse::Ok(msg) => assert!(msg.contains("Vault recovered")),
            _ => panic!("Expected Ok response from RecoverVault, got {:?}", recover_resp),
        }

        // 6. Unlock with recovered password succeeds and DMK remains identical
        let rec_unlock_req = serde_json::to_string(&IpcCommand::UnlockVault("recovered_pass_789".to_string())).unwrap() + "\n";
        writer.write_all(rec_unlock_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let rec_unlock_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match rec_unlock_resp {
            IpcResponse::Ok(msg) => assert!(msg.contains("Vault unlocked")),
            _ => panic!("Expected Ok response for recovered password unlock, got {:?}", rec_unlock_resp),
        }

        let dmk_after_recovery = shared_state.master_key.read().await.as_ref().unwrap().clone();
        assert_eq!(dmk_after_recovery, dmk_before);

        // 7. Lock vault command must zeroize/drop DMK and set state to Locked
        let lock_req = serde_json::to_string(&IpcCommand::LockVault).unwrap() + "\n";
        writer.write_all(lock_req.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        response_line.clear();
        buf_reader.read_line(&mut response_line).await.unwrap();
        let lock_resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match lock_resp {
            IpcResponse::Ok(msg) => assert!(msg.contains("Vault locked")),
            _ => panic!("Expected Ok response for LockVault, got {:?}", lock_resp),
        }

        assert!(shared_state.master_key.read().await.is_none());
        assert_eq!(*shared_state.sync_state.borrow(), SyncState::Locked);

        // Clean up
        let _ = shutdown_tx.send(());
        let _ = server_handle.await;
        let _ = std::fs::remove_file(&temp_sock);
        let _ = std::fs::remove_file(&temp_cfg);
    }
}
