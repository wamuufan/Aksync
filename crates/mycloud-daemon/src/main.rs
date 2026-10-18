pub mod config;
pub mod home_sync;
pub mod ipc;
pub mod process;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use clap::Parser;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{broadcast, mpsc, watch, RwLock};
use tracing::{error, info, warn};

use config::DaemonConfig;
use ipc::{DaemonSharedState, IpcServer, SyncState};
use process::{ProcessConfig, ProcessManager};

/// Command-line configuration for the MyCloud daemon
#[derive(Parser, Debug)]
#[command(
    name = "mycloud-daemon",
    version,
    about = "Files On-Demand Orchestration Daemon"
)]
struct Cli {
    /// Binary path or command name for rclone
    #[arg(long, default_value = "rclone")]
    rclone_bin: String,

    /// Remote name configured in rclone (e.g. 'mycloud_drive')
    #[arg(long)]
    rclone_remote: Option<String>,

    /// Rclone remote control HTTP server address
    #[arg(long, default_value = "127.0.0.1:5572")]
    rc_addr: String,

    /// Raw ciphertext VFS mount directory (defaults to ~/.cache/mycloud/raw_vault)
    #[arg(long)]
    raw_path: Option<String>,

    /// User-facing decrypted mount directory
    #[arg(long, default_value = "~/.local/share/mycloud/vault")]
    mount_path: String,

    /// WebDAV server address for flat Google Drive network access
    #[arg(long, default_value = "127.0.0.1:8765")]
    webdav_addr: String,

    /// WebDAV server address for decrypted MyCloud vault network access
    #[arg(long, default_value = "127.0.0.1:8766")]
    vault_webdav_addr: String,

    /// Unix Domain Socket path for IPC
    #[arg(long, default_value = "~/.cache/mycloud/mycloud.sock")]
    socket_path: String,

    /// Path to config.toml
    #[arg(long)]
    config_path: Option<PathBuf>,

    /// Path to the mycloud-fuse binary (defaults to sibling binary)
    #[arg(long)]
    fuse_bin: Option<PathBuf>,

    /// Skip spawning rclone (useful when testing with local directory)
    #[arg(long, default_value_t = false)]
    skip_rclone: bool,
}

/// Expands tilde (~) prefix to user HOME directory
fn expand_home_dir(input: &str) -> PathBuf {
    if let Some(stripped) = input.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(stripped);
        }
    } else if input == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(input)
}

/// Locates the mycloud-fuse binary, checking next to current executable first
fn resolve_fuse_binary(override_bin: Option<PathBuf>) -> PathBuf {
    if let Some(path) = override_bin {
        return path;
    }

    if let Ok(mut current_exe) = std::env::current_exe() {
        current_exe.pop();
        let sibling = current_exe.join("mycloud-fuse");
        if sibling.exists() {
            return sibling;
        }
    }

    PathBuf::from("mycloud-fuse")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }

    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let mount_path = expand_home_dir(&cli.mount_path);
    let socket_path = expand_home_dir(&cli.socket_path);
    let config_path = cli
        .config_path
        .unwrap_or_else(DaemonConfig::default_config_path);
    let fuse_bin = resolve_fuse_binary(cli.fuse_bin);

    // Load or initialize daemon config
    let mut initial_config = DaemonConfig::load_from(&config_path).unwrap_or_default();
    if let Some(remote_override) = cli.rclone_remote {
        initial_config.rclone_remote = remote_override;
    }
    let is_setup_done = initial_config.is_setup_complete();

    // Ensure rclone.conf has the exact custom_client_id and custom_client_secret from config.toml
    if initial_config.rclone_configured && !cli.skip_rclone {
        let cid = initial_config
            .custom_client_id
            .as_deref()
            .unwrap_or("")
            .trim();
        let cs = initial_config
            .custom_client_secret
            .as_deref()
            .unwrap_or("")
            .trim();
        let _ = std::process::Command::new(&cli.rclone_bin)
            .arg("config")
            .arg("update")
            .arg(&initial_config.rclone_remote)
            .arg("client_id")
            .arg(cid)
            .arg("client_secret")
            .arg(cs)
            .arg("config_refresh_token=false")
            .output();
    }

    let _provider_name = config::get_provider_name(&initial_config.rclone_remote);
    let raw_path = match cli.raw_path {
        Some(ref p) => expand_home_dir(p),
        None => {
            if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
                PathBuf::from(runtime_dir).join("mycloud/raw_vault")
            } else {
                expand_home_dir("~/.cache/mycloud/raw_vault")
            }
        }
    };

    info!("Starting MyCloud Synchronization Daemon");
    info!("RAW_MOUNT_PATH: {:?}", raw_path);
    info!("TARGET_MOUNT_PATH: {:?}", mount_path);
    info!("IPC Socket: {:?}", socket_path);
    info!("Config Path: {:?}", config_path);

    // Generate ephemeral 32-character password for rclone Remote Control (RC) Basic Auth
    let rc_user = "mycloud".to_string();
    let rc_pass: String = {
        use rand::Rng;
        rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(32)
            .map(char::from)
            .collect()
    };

    // Load or generate persistent password for rclone WebDAV Basic Auth.
    // If the user has already saved a password in KDE's KWallet (kpasswdserver), honor it
    // so Dolphin's cached credentials match rclone's htpasswd seamlessly.
    let webdav_user = "mycloud".to_string();
    let webdav_pass: String = if let Ok(env_pass) = std::env::var("MYCLOUD_WEBDAV_PASS") {
        env_pass
    } else if let Some(kw_pass) = process::read_kwallet_webdav_password(
        &webdav_user,
        &cli.webdav_addr,
        &cli.vault_webdav_addr,
    ) {
        if initial_config.webdav_password.as_deref() != Some(kw_pass.as_str()) {
            initial_config.webdav_password = Some(kw_pass.clone());
            let _ = initial_config.save_to(&config_path);
        }
        kw_pass
    } else if let Some(ref saved) = initial_config.webdav_password {
        saved.clone()
    } else {
        use rand::Rng;
        let generated: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(32)
            .map(char::from)
            .collect();
        initial_config.webdav_password = Some(generated.clone());
        let _ = initial_config.save_to(&config_path);
        generated
    };
    let htpasswd_path = process::write_htpasswd_file(&webdav_user, &webdav_pass)
        .expect("Failed to write secure WebDAV htpasswd file");

    info!(
        "WebDAV service configured with user '{}' (secure htpasswd at {:?})",
        webdav_user, htpasswd_path
    );

    process::update_kde_places(&webdav_user, &cli.webdav_addr, &cli.vault_webdav_addr);

    // Derive master key from CLI args if provided (non-interactive runs)
    let mut initial_master_key: Option<zeroize::Zeroizing<[u8; 32]>> = None;
    if let Ok(mut hex) = std::env::var("MYCLOUD_KEY_HEX") {
        if let Ok(bytes) = hex::decode(hex.trim()) {
            if bytes.len() == 32 {
                let mut arr = zeroize::Zeroizing::new([0u8; 32]);
                arr.copy_from_slice(&bytes);
                initial_master_key = Some(arr);
            }
        }
        zeroize::Zeroize::zeroize(&mut hex);
    } else if let Ok(mut pass) = std::env::var("MYCLOUD_PASSPHRASE") {
        if let Some(ref salt_b64) = initial_config.vault_salt_base64 {
            if let Ok(salt_bytes) =
                base64::engine::general_purpose::STANDARD.decode(salt_b64.trim())
            {
                if let Ok(k) = mycloud_crypto::KeyDerivation::derive_key(&pass, &salt_bytes) {
                    if let Some(ref w) = initial_config.wrapped_dmk {
                        if let Ok(dmk) = mycloud_crypto::KeyDerivation::unwrap_key(&k, w) {
                            initial_master_key = Some(dmk);
                        }
                    } else {
                        initial_master_key = Some(k);
                    }
                }
            }
        }
        zeroize::Zeroize::zeroize(&mut pass);
    }

    let initial_state = if !is_setup_done {
        SyncState::SetupRequired
    } else if initial_master_key.is_some() {
        SyncState::Connecting
    } else {
        SyncState::Locked
    };

    let (sync_state_tx, mut sync_state_rx) = watch::channel(initial_state.clone());
    let (ipc_shutdown_tx, _) = broadcast::channel::<()>(16);
    let (app_shutdown_tx, mut app_shutdown_rx) = mpsc::channel::<()>(1);

    let rclone_pid_shared = Arc::new(RwLock::new(None));
    let fuse_pid_shared = Arc::new(RwLock::new(None));
    let daemon_config_shared = Arc::new(RwLock::new(initial_config.clone()));
    let initial_stats = ipc::TransferStatsPayload {
        upload_speed: 0,
        download_speed: 0,
        storage_used: 0,
        storage_total: 0,
        files_synced: 0,
    };
    let rclone_stats_shared = Arc::new(RwLock::new(initial_stats));
    let master_key_shared = Arc::new(RwLock::new(initial_master_key));
    let raw_mount_shared = Arc::new(RwLock::new(raw_path.clone()));

    let optimizations = crate::config::OptimizationConfig::load_embedded();

    let shared_state = Arc::new(DaemonSharedState {
        sync_state: sync_state_tx.clone(),
        rclone_pid: Arc::clone(&rclone_pid_shared),
        fuse_pid: Arc::clone(&fuse_pid_shared),
        raw_mount_path: Arc::clone(&raw_mount_shared),
        target_mount_path: mount_path.clone(),
        config_path: config_path.clone(),
        daemon_config: Arc::clone(&daemon_config_shared),
        shutdown_sender: app_shutdown_tx,
        rclone_stats: Arc::clone(&rclone_stats_shared),
        rclone_bin: cli.rclone_bin.clone(),
        master_key: Arc::clone(&master_key_shared),
        optimizations: optimizations.clone(),
        rc_addr: cli.rc_addr.clone(),
        rc_user: rc_user.clone(),
        rc_pass: rc_pass.clone(),
    });

    // Start IPC Server immediately so UI can connect even during SetupRequired or Locked
    let ipc_server = IpcServer::new(socket_path, shared_state);
    let ipc_shutdown_rx = ipc_shutdown_tx.subscribe();
    let ipc_handle = tokio::spawn(async move {
        if let Err(e) = ipc_server.run(ipc_shutdown_rx).await {
            error!("IPC server exited with error: {}", e);
        }
    });

    let process_config = ProcessConfig {
        rclone_bin: cli.rclone_bin.clone(),
        rclone_remote: initial_config.rclone_remote.clone(),
        rc_addr: cli.rc_addr.clone(),
        vault_rc_addr: optimizations.process.vault_rc_bind_addr.clone(),
        rc_user: rc_user.clone(),
        rc_pass: rc_pass.clone(),
        raw_mount_path: raw_path,
        target_mount_path: mount_path.clone(),
        fuse_bin,
        config_path: config_path.clone(),
        webdav_addr: cli.webdav_addr.clone(),
        vault_webdav_addr: cli.vault_webdav_addr.clone(),
        webdav_user: webdav_user.clone(),
        webdav_pass: webdav_pass.clone(),
        htpasswd_path,
        optimizations: optimizations.clone(),
    };

    let mut process_manager = ProcessManager::new(
        process_config,
        (0, 0),
        0,
    );

    // If key is available and setup is complete, launch subprocesses directly
    if initial_state == SyncState::Connecting {
        let key_opt = master_key_shared.read().await.clone();
        if let Some(key) = key_opt {
            let mut rclone_ok = true;
            if !cli.skip_rclone {
                match process_manager.spawn_rclone().await {
                    Ok(pid) => {
                        *rclone_pid_shared.write().await = Some(pid);
                        if let Err(e) = process_manager
                            .wait_rclone_rc_ready(Duration::from_secs(optimizations.network.rc_ready_timeout_secs))
                            .await
                        {
                            warn!("Rclone RC API readiness check failed: {}", e);
                        }

                        // Wait for raw mount before freeze prevention rules and spawn_fuse (1.4)
                        let raw_mount = process_manager.raw_mount_path().to_path_buf();
                        let mount_timeout = Duration::from_secs(optimizations.network.rc_ready_timeout_secs);
                        let retry_interval = Duration::from_millis(optimizations.network.rc_ready_retry_ms);
                        if let Err(e) = process::wait_for_mount(&raw_mount, "fuse", mount_timeout, retry_interval).await {
                            error!("Raw mount readiness check failed: {}", e);
                            let _ = sync_state_tx.send(SyncState::Error(e.to_string()));
                            rclone_ok = false;
                        } else {
                            if optimizations.features.enable_freeze_prevention {
                                process::apply_folder_freeze_prevention_rules(&raw_mount);
                            }
                            process_manager.warm_vfs_cache();
                            if process_manager.webdav_pid().is_none() {
                                if let Err(e) = process_manager.spawn_webdav().await {
                                    warn!("Failed to spawn rclone WebDAV: {}", e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to spawn rclone: {}", e);
                        let _ = sync_state_tx.send(SyncState::Error(e.to_string()));
                        rclone_ok = false;
                    }
                }
            } else {
                info!("Skipping rclone spawn per --skip-rclone flag");
            }

            if rclone_ok {
                match process_manager.spawn_fuse(&key).await {
                    Ok(pid) => {
                        *fuse_pid_shared.write().await = Some(pid);
                        info!("All services initialized successfully. State: Idle");
                        let _ = sync_state_tx.send(SyncState::Idle);
                    }
                    Err(e) => {
                        error!("Failed to spawn mycloud-fuse: {}", e);
                        let _ = sync_state_tx.send(SyncState::Error(e.to_string()));
                    }
                }
            }
        }
    } else if initial_state == SyncState::Locked {
        info!("Vault is locked. Awaiting UnlockVault command via IPC...");
    } else {
        info!("Setup is incomplete. Waiting for First-Time Setup Wizard via IPC...");
    }

    // Configure graceful termination signal handlers (Ctrl+C and SIGTERM)
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut stats_interval = tokio::time::interval(Duration::from_millis(optimizations.sync.stats_interval_ms));
    let mut home_sync_interval = tokio::time::interval(Duration::from_secs(optimizations.sync.home_sync_interval_secs));
    home_sync_interval.tick().await;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Received SIGINT (Ctrl+C) signal");
                break;
            }
            _ = sigterm.recv() => {
                info!("Received SIGTERM signal");
                break;
            }
            _ = app_shutdown_rx.recv() => {
                info!("Received internal shutdown request from IPC");
                break;
            }
            _ = home_sync_interval.tick() => {
                let current_state = sync_state_rx.borrow().clone();
                if current_state == SyncState::Idle {
                    let (selected, is_fuse_mounted, sync_hidden) = {
                        let cfg = daemon_config_shared.read().await;
                        (
                            cfg.selected_sync_folders.clone(),
                            fuse_pid_shared.read().await.is_some(),
                            cfg.sync_hidden_files,
                        )
                    };
                    if !selected.is_empty() && is_fuse_mounted {
                        let rclone_bin = cli.rclone_bin.clone();
                        let target_mount = mount_path.clone();
                        let sync_state = sync_state_tx.clone();
                        let opt = optimizations.clone();
                        let rc_addr_arg = cli.rc_addr.clone();
                        let rc_user_arg = rc_user.clone();
                        let rc_pass_arg = rc_pass.clone();
                        tokio::spawn(async move {
                            let _ = sync_state.send(SyncState::Syncing);
                            let _ = crate::home_sync::execute_home_sync(
                                &rclone_bin,
                                &target_mount,
                                &selected,
                                sync_hidden,
                                opt.sync.transfers,
                                &rc_addr_arg,
                                &opt.process.vault_rc_bind_addr,
                                &rc_user_arg,
                                &rc_pass_arg,
                            ).await;
                            let _ = sync_state.send(SyncState::Idle);
                        });
                    }
                }
            }
            _ = stats_interval.tick() => {
                // Monitor health of running subprocesses with self-healing restart
                match process_manager.check_child_health() {
                    Err(process::ProcessError::ProcessExited(name)) => {
                        *rclone_pid_shared.write().await = process_manager.rclone_pid();
                        *fuse_pid_shared.write().await = process_manager.fuse_pid();
                        let current_state = sync_state_rx.borrow().clone();
                        let master_key_opt = master_key_shared.read().await.clone();

                        if let Some(key) = master_key_opt {
                            if !matches!(current_state, SyncState::Locked | SyncState::SetupRequired) {
                                if process_manager.can_attempt_restart() {
                                    let backoff = process_manager.record_restart_attempt();
                                    warn!(
                                        "Service '{}' exited unexpectedly. Attempting self-healing restart (next backoff {:?})...",
                                        name, backoff
                                    );
                                    match process_manager.respawn_crashed_services(name, &key, cli.skip_rclone).await {
                                        Ok(()) => {
                                            *rclone_pid_shared.write().await = process_manager.rclone_pid();
                                            *fuse_pid_shared.write().await = process_manager.fuse_pid();
                                            let _ = sync_state_tx.send(SyncState::Idle);
                                            info!("Self-healing restart of '{}' succeeded!", name);
                                        }
                                        Err(re_err) => {
                                            error!(
                                                "Self-healing restart of '{}' failed: {}. Will retry after {:?}",
                                                name, re_err, backoff
                                            );
                                            let _ = sync_state_tx.send(SyncState::Error(format!("Restarting {}: {}", name, re_err)));
                                        }
                                    }
                                } else {
                                    let _ = sync_state_tx.send(SyncState::Error(format!("Process '{}' exited unexpectedly", name)));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Child process monitoring error: {}", e);
                    }
                    Ok(()) => {
                        let current_state = sync_state_rx.borrow().clone();
                        let master_key_opt = master_key_shared.read().await.clone();
                        let missing_service = process_manager.fuse_pid().is_none()
                            || (!cli.skip_rclone && process_manager.rclone_pid().is_none());

                        if matches!(current_state, SyncState::Error(_)) && master_key_opt.is_some() && missing_service {
                            if process_manager.can_attempt_restart() {
                                if let Some(key) = master_key_opt {
                                    let backoff = process_manager.record_restart_attempt();
                                    let name = if !cli.skip_rclone && process_manager.rclone_pid().is_none() {
                                        "rclone"
                                    } else {
                                        "mycloud-fuse"
                                    };
                                    warn!(
                                        "Backoff delay elapsed. Attempting self-healing restart for '{}' (next backoff {:?})...",
                                        name, backoff
                                    );
                                    match process_manager.respawn_crashed_services(name, &key, cli.skip_rclone).await {
                                        Ok(()) => {
                                            *rclone_pid_shared.write().await = process_manager.rclone_pid();
                                            *fuse_pid_shared.write().await = process_manager.fuse_pid();
                                            let _ = sync_state_tx.send(SyncState::Idle);
                                            info!("Self-healing restart of '{}' succeeded!", name);
                                        }
                                        Err(re_err) => {
                                            error!(
                                                "Self-healing restart of '{}' failed: {}. Will retry after {:?}",
                                                name, re_err, backoff
                                            );
                                            let _ = sync_state_tx.send(SyncState::Error(format!("Restarting {}: {}", name, re_err)));
                                        }
                                    }
                                }
                            }
                        } else if !missing_service && matches!(current_state, SyncState::Idle | SyncState::Syncing | SyncState::Paused) {
                            process_manager.record_healthy_tick();
                        }
                    }
                }

                let current_state = sync_state_rx.borrow().clone();
                if matches!(current_state, SyncState::Idle | SyncState::Syncing | SyncState::Paused) {
                    let stats = process_manager.query_rclone_stats().await;
                    let mut w = rclone_stats_shared.write().await;
                    *w = ipc::TransferStatsPayload {
                        upload_speed: stats.upload_speed,
                        download_speed: stats.download_speed,
                        storage_used: stats.storage_used,
                        storage_total: stats.storage_total,
                        files_synced: stats.files_synced,
                    };
                }
            }
            // Monitor sync state changes: if transitioned to Idle, spawn processes if not running
            Ok(()) = sync_state_rx.changed() => {
                let current = sync_state_rx.borrow().clone();
                if current == SyncState::Locked {
                    let has_running_children = process_manager.fuse_pid().is_some()
                        || process_manager.rclone_pid().is_some()
                        || process_manager.vault_webdav_pid().is_some();
                    if has_running_children {
                        info!("Vault locked. Terminating active FUSE and cloud mounts...");
                        let _ = process_manager.terminate_all().await;
                        *rclone_pid_shared.write().await = None;
                        *fuse_pid_shared.write().await = None;
                    }
                }
                let needs_mounts = process_manager.fuse_pid().is_none()
                    || (!cli.skip_rclone && process_manager.rclone_pid().is_none());
                if (current == SyncState::Connecting || current == SyncState::Idle) && needs_mounts {
                    let key_opt = master_key_shared.read().await.clone();
                    if let Some(key) = key_opt {
                        info!("Master key present. Spawning background mounts...");
                        let remote = {
                            let cfg = daemon_config_shared.read().await;
                            cfg.rclone_remote.clone()
                        };
                        process_manager.set_remote(remote);
                        *raw_mount_shared.write().await = process_manager.raw_mount_path().to_path_buf();

                        if !cli.skip_rclone && process_manager.rclone_pid().is_none() {
                            match process_manager.spawn_rclone().await {
                                Ok(pid) => {
                                    *rclone_pid_shared.write().await = Some(pid);
                                    let _ = process_manager.wait_rclone_rc_ready(Duration::from_secs(optimizations.network.rc_ready_timeout_secs)).await;

                                    // Wait for raw mount before freeze prevention rules and spawn_fuse (1.4)
                                    let raw_mount = process_manager.raw_mount_path().to_path_buf();
                                    let mount_timeout = Duration::from_secs(optimizations.network.rc_ready_timeout_secs);
                                    let retry_interval = Duration::from_millis(optimizations.network.rc_ready_retry_ms);
                                    if let Err(e) = process::wait_for_mount(&raw_mount, "fuse", mount_timeout, retry_interval).await {
                                        error!("Raw mount readiness check failed: {}", e);
                                        let _ = sync_state_tx.send(SyncState::Error(e.to_string()));
                                    } else {
                                        if optimizations.features.enable_freeze_prevention {
                                            process::apply_folder_freeze_prevention_rules(&raw_mount);
                                        }
                                        process_manager.warm_vfs_cache();

                                        // Spawn WebDAV after raw mount is ready so startup API queries don't clash
                                        if process_manager.webdav_pid().is_none() {
                                            let _ = process_manager.spawn_webdav().await;
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to spawn rclone: {}", e);
                                    let _ = sync_state_tx.send(SyncState::Error(e.to_string()));
                                }
                            }
                        }
                        let rclone_running = cli.skip_rclone || process_manager.rclone_pid().is_some();
                        if rclone_running {
                            match process_manager.spawn_fuse(&key).await {
                                Ok(pid) => {
                                    *fuse_pid_shared.write().await = Some(pid);
                                    info!("FUSE filesystem mounted successfully with derived key.");
                                    let _ = sync_state_tx.send(SyncState::Idle);

                                    // Trigger immediate encrypted home sync for any selected folders upon unlock
                                    let (selected, sync_hidden) = {
                                        let cfg = daemon_config_shared.read().await;
                                        (cfg.selected_sync_folders.clone(), cfg.sync_hidden_files)
                                    };
                                    if !selected.is_empty() {
                                        let rclone_bin = cli.rclone_bin.clone();
                                        let target_mount = mount_path.clone();
                                        let sync_tx = sync_state_tx.clone();
                                        let opt = optimizations.clone();
                                        let rc_addr_arg = cli.rc_addr.clone();
                                        let rc_user_arg = rc_user.clone();
                                        let rc_pass_arg = rc_pass.clone();
                                        tokio::spawn(async move {
                                            let _ = sync_tx.send(SyncState::Syncing);
                                            let _ = crate::home_sync::execute_home_sync(
                                                &rclone_bin,
                                                &target_mount,
                                                &selected,
                                                sync_hidden,
                                                opt.sync.transfers,
                                                &rc_addr_arg,
                                                &opt.process.vault_rc_bind_addr,
                                                &rc_user_arg,
                                                &rc_pass_arg,
                                            ).await;
                                            let _ = sync_tx.send(SyncState::Idle);
                                        });
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to spawn mycloud-fuse: {}", e);
                                    let _ = sync_state_tx.send(SyncState::Error(e.to_string()));
                                }
                            }
                        }

                        // Query stats immediately once setup transitions to Idle
                        let stats = process_manager.query_rclone_stats().await;
                        let mut w = rclone_stats_shared.write().await;
                        *w = ipc::TransferStatsPayload {
                            upload_speed: stats.upload_speed,
                            download_speed: stats.download_speed,
                            storage_used: stats.storage_used,
                            storage_total: stats.storage_total,
                            files_synced: stats.files_synced,
                        };
                    } else {
                        warn!("State is Idle but master key is missing. Reverting to Locked state.");
                        let _ = sync_state_tx.send(SyncState::Locked);
                    }
                }
            }
        }
    }

    info!("Initiating graceful daemon shutdown sequence...");
    let _ = sync_state_tx.send(SyncState::Offline);

    // Stop IPC server
    let _ = ipc_shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), ipc_handle).await;

    // Terminate children
    if let Err(e) = process_manager.terminate_all().await {
        error!("Error during process cleanup: {}", e);
    }

    process::remove_kde_places(&webdav_user, &cli.webdav_addr, &cli.vault_webdav_addr);

    info!("MyCloud Daemon shutdown complete");
    Ok(())
}
