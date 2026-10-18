use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncFolderInfo {
    pub name: String,
    pub rel_path: String,
    pub abs_path: String,
    pub selected: bool,
    pub icon: String,
}

/// Reads ~/.config/user-dirs.dirs according to the FreeDesktop XDG User Dirs specification
/// to discover the user's localized home directories (e.g. Masaüstü, Belgeler, İndirilenler).
pub fn get_user_home_folders(selected: &[String]) -> Vec<SyncFolderInfo> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let home_path = Path::new(&home);
    let user_dirs_file = home_path.join(".config/user-dirs.dirs");

    let mut folders = Vec::new();
    let mut seen = HashSet::new();

    if let Ok(content) = std::fs::read_to_string(&user_dirs_file) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((key, val)) = trimmed.split_once('=') {
                let clean_val = val.trim().trim_matches('"');
                let resolved = if let Some(sub) = clean_val.strip_prefix("$HOME/") {
                    home_path.join(sub)
                } else if let Some(sub) = clean_val.strip_prefix("~/") {
                    home_path.join(sub)
                } else {
                    PathBuf::from(clean_val)
                };

                if resolved.exists() && resolved.is_dir() {
                    if let Ok(rel) = resolved.strip_prefix(home_path) {
                        let rel_str = rel.to_string_lossy().to_string();
                        // Ignore root home itself or duplicate paths
                        if !rel_str.is_empty() && !seen.contains(&rel_str) {
                            seen.insert(rel_str.clone());
                            let icon = match key.trim() {
                                "XDG_DESKTOP_DIR" => "desktop",
                                "XDG_DOCUMENTS_DIR" => "documents",
                                "XDG_DOWNLOAD_DIR" => "downloads",
                                "XDG_MUSIC_DIR" => "music",
                                "XDG_PICTURES_DIR" => "pictures",
                                "XDG_VIDEOS_DIR" => "videos",
                                _ => "folder",
                            };
                            let name = resolved
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| rel_str.clone());
                            let is_selected = selected.contains(&rel_str);
                            folders.push(SyncFolderInfo {
                                name,
                                rel_path: rel_str,
                                abs_path: resolved.to_string_lossy().to_string(),
                                selected: is_selected,
                                icon: icon.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    // Resilient Fallback: If user-dirs.dirs did not exist or was empty, check standard directory names
    if folders.is_empty() {
        let candidates = [
            ("Masaüstü", "desktop"),
            ("Desktop", "desktop"),
            ("Belgeler", "documents"),
            ("Documents", "documents"),
            ("İndirilenler", "downloads"),
            ("Downloads", "downloads"),
            ("Müzik", "music"),
            ("Music", "music"),
            ("Resimler", "pictures"),
            ("Pictures", "pictures"),
            ("Videolar", "videos"),
            ("Videos", "videos"),
        ];

        for (name, icon) in candidates {
            let p = home_path.join(name);
            if p.is_dir() && !seen.contains(name) {
                seen.insert(name.to_string());
                folders.push(SyncFolderInfo {
                    name: name.to_string(),
                    rel_path: name.to_string(),
                    abs_path: p.to_string_lossy().to_string(),
                    selected: selected.contains(&name.to_string()),
                    icon: icon.to_string(),
                });
            }
        }
    }

    folders
}

/// Executes one-way synchronization for all selected home folders into the encrypted vault:
/// `{target_vault_path}/home/{rel_path}`
#[allow(clippy::too_many_arguments)]
pub async fn execute_home_sync(
    rclone_bin: &str,
    target_vault_path: &Path,
    folders: &[String],
    sync_hidden: bool,
    transfers: u32,
    rc_addr: &str,
    vault_rc_addr: &str,
    rc_user: &str,
    rc_pass: &str,
) -> Result<(), String> {
    if folders.is_empty() {
        info!("No home folders selected for sync. Skipping home sync execution.");
        return Ok(());
    }

    if !target_vault_path.exists() {
        warn!(
            "Target vault mount path {:?} does not exist. Encrypted vault must be mounted before syncing.",
            target_vault_path
        );
        return Err("Encrypted vault is not mounted".to_string());
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let home_path = Path::new(&home);
    let vault_home_dir = target_vault_path.join("home");

    if let Err(e) = std::fs::create_dir_all(&vault_home_dir) {
        warn!(
            "Could not ensure 'home' directory inside vault {:?}: {}",
            vault_home_dir, e
        );
    }
    crate::process::apply_folder_freeze_prevention_rules(&vault_home_dir);

    info!(
        "Starting encrypted home folders sync (sync_hidden={}, transfers={}) to '{:?}/home/' for {} folders: {:?}",
        sync_hidden,
        transfers,
        target_vault_path,
        folders.len(),
        folders
    );

    for rel_folder in folders {
        let source_dir = home_path.join(rel_folder);
        if !source_dir.exists() || !source_dir.is_dir() {
            warn!(
                "Selected home folder {:?} does not exist. Skipping.",
                source_dir
            );
            continue;
        }

        let target_dir = vault_home_dir.join(rel_folder);
        info!(
            "Syncing home folder {:?} -> {:?} (encrypted via FUSE, sync_hidden={})",
            source_dir, target_dir, sync_hidden
        );

        let mut cmd = Command::new(rclone_bin);
        cmd.arg("sync")
            .arg(&source_dir)
            .arg(&target_dir)
            .arg("--transfers")
            .arg(transfers.to_string());

        if !sync_hidden {
            cmd.arg("--exclude")
                .arg(".*")
                .arg("--exclude")
                .arg(".*/**");
        }

        cmd.arg("--log-level")
            .arg("NOTICE");

        match cmd.output().await {
            Ok(output) => {
                if output.status.success() {
                    info!("Successfully synced {:?} -> {:?} (encrypted)", source_dir, target_dir);
                    crate::process::apply_folder_freeze_prevention_rules(&target_dir);
                } else {
                    let err = String::from_utf8_lossy(&output.stderr);
                    error!("Error syncing {:?} -> {:?}: {}", source_dir, target_dir, err);
                }
            }
            Err(e) => {
                error!("Failed to launch rclone sync for {:?}: {}", source_dir, e);
            }
        }
    }

    // Refresh VFS cache on Vault WebDAV and raw mount so new files appear immediately
    let http = reqwest::Client::new();
    let vault_refresh_url = format!("http://{}/vfs/refresh", vault_rc_addr);
    let _ = http.post(&vault_refresh_url)
        .basic_auth(rc_user, Some(rc_pass))
        .json(&serde_json::json!({ "recursive": "true" }))
        .send()
        .await;

    let refresh_url = format!("http://{}/vfs/refresh", rc_addr);
    let _ = http.post(&refresh_url)
        .basic_auth(rc_user, Some(rc_pass))
        .json(&serde_json::json!({ "recursive": "true" }))
        .send()
        .await;

    info!("Home folders encrypted synchronization cycle completed.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_home_sync_empty_folders() {
        let temp_dir = std::env::temp_dir().join(format!("mycloud_test_homesync_{}", std::process::id()));
        let res = execute_home_sync(
            "rclone",
            &temp_dir,
            &[],
            false,
            4,
            "127.0.0.1:5572",
            "127.0.0.1:5573",
            "mycloud",
            "test_pass",
        )
        .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_execute_home_sync_unmounted_vault() {
        let nonexistent = Path::new("/nonexistent/vault/path");
        let folders = vec!["Belgeler".to_string()];
        let res = execute_home_sync(
            "rclone",
            nonexistent,
            &folders,
            false,
            4,
            "127.0.0.1:5572",
            "127.0.0.1:5573",
            "mycloud",
            "test_pass",
        )
        .await;
        assert!(res.is_err());
        assert_eq!(res.unwrap_err(), "Encrypted vault is not mounted");
    }
}
