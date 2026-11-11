use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{error, info, warn};

pub const LARGE_CONFLICT_THRESHOLD_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB

static HOME_SYNC_RUNNING: AtomicBool = AtomicBool::new(false);

struct SyncRunGuard;

impl Drop for SyncRunGuard {
    fn drop(&mut self) {
        HOME_SYNC_RUNNING.store(false, Ordering::SeqCst);
    }
}

pub fn is_valid_sync_folder_rel_path(rel: &str) -> bool {
    let trimmed = rel.trim();
    if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return false;
    }
    for comp in Path::new(trimmed).components() {
        match comp {
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return false,
            _ => {}
        }
    }
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncFolderInfo {
    pub name: String,
    pub rel_path: String,
    pub abs_path: String,
    pub selected: bool,
    pub icon: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FileEntryState {
    pub size: u64,
    pub mtime_secs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FolderSyncManifest {
    pub version: u32,
    pub files: BTreeMap<String, FileEntryState>,
}

impl Default for FolderSyncManifest {
    fn default() -> Self {
        Self {
            version: 1,
            files: BTreeMap::new(),
        }
    }
}

/// Derives the state directory for storing sync manifests.
/// In production, target_vault_path is ~/.local/share/mycloud/vault,
/// so target_vault_path.parent().join("sync_state") resolves to ~/.local/share/mycloud/sync_state.
/// In unit tests with temporary vault directories, manifests remain completely isolated.
pub fn get_sync_state_dir(target_vault_path: &Path) -> PathBuf {
    target_vault_path
        .parent()
        .unwrap_or_else(|| Path::new("/tmp"))
        .join("sync_state")
}

/// Computes the persistent manifest file path for a given sync folder.
pub fn get_manifest_path(target_vault_path: &Path, rel_folder: &str) -> PathBuf {
    let state_dir = get_sync_state_dir(target_vault_path);
    let sanitized: String = rel_folder
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    state_dir.join(format!("{}.json", sanitized))
}

/// Loads the persistent sync manifest from disk, defaulting to an empty manifest if missing or corrupted.
pub fn load_manifest(path: &Path) -> FolderSyncManifest {
    if let Ok(content) = std::fs::read_to_string(path) {
        if let Ok(manifest) = serde_json::from_str::<FolderSyncManifest>(&content) {
            return manifest;
        }
    }
    FolderSyncManifest::default()
}

/// Atomically saves the sync manifest by writing to a temporary file and renaming.
pub fn save_manifest(path: &Path, manifest: &FolderSyncManifest) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(manifest).map_err(std::io::Error::other)?;
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, data)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Checks whether a file or directory name should be skipped during directory scanning.
pub fn should_skip_name(name: &str, sync_hidden: bool) -> bool {
    if !sync_hidden && name.starts_with('.') {
        return true;
    }
    if name == ".aksync_diriv"
        || name == ".aksync_vault.json"
        || name == ".nomedia"
        || name == ".directory"
        || name.starts_with(".aksync_")
        || name.ends_with(".tmp")
        || name.ends_with(".part")
        || name.contains(".tmp.")
    {
        return true;
    }
    false
}

fn scan_tree_recursive(
    base: &Path,
    current: &Path,
    sync_hidden: bool,
    out: &mut BTreeMap<String, FileEntryState>,
) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(err) => {
            warn!("Could not read directory {:?}: {}", current, err);
            return Ok(());
        }
    };

    for entry_res in entries {
        let entry = match entry_res {
            Ok(e) => e,
            Err(_) => continue,
        };

        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        if should_skip_name(&name_str, sync_hidden) {
            continue;
        }

        let path = entry.path();
        let symlink_meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        if symlink_meta.is_symlink() {
            // Do not follow symlinks
            continue;
        }

        if symlink_meta.is_dir() {
            scan_tree_recursive(base, &path, sync_hidden, out)?;
        } else if symlink_meta.is_file() {
            let rel_path = match path.strip_prefix(base) {
                Ok(p) => {
                    let parts: Vec<&str> = p.iter().filter_map(|os| os.to_str()).collect();
                    parts.join("/")
                }
                Err(_) => continue,
            };

            let mtime_secs = match symlink_meta.modified() {
                Ok(mtime) => match mtime.duration_since(std::time::UNIX_EPOCH) {
                    Ok(d) => d.as_secs() as i64,
                    Err(e) => -(e.duration().as_secs() as i64),
                },
                Err(_) => 0,
            };

            out.insert(
                rel_path,
                FileEntryState {
                    size: symlink_meta.len(),
                    mtime_secs,
                },
            );
        }
    }

    Ok(())
}

/// Recursively scans a directory tree and returns a map of relative file paths to their state.
pub fn scan_tree(
    root: &Path,
    sync_hidden: bool,
) -> std::io::Result<BTreeMap<String, FileEntryState>> {
    if !root.exists() || !root.is_dir() {
        return Ok(BTreeMap::new());
    }

    let mut out = BTreeMap::new();
    scan_tree_recursive(root, root, sync_hidden, &mut out)?;
    Ok(out)
}

/// Compares two FileEntryState objects with a 2-second timestamp tolerance for cloud/FUSE granularity.
pub fn same_version(a: &FileEntryState, b: &FileEntryState) -> bool {
    a.size == b.size && a.mtime_secs.abs_diff(b.mtime_secs) <= 2
}

/// Copies a file via a temporary sibling file, preserving modified timestamp from source.
pub fn copy_file_preserving_mtime(src: &Path, dst: &Path) -> std::io::Result<FileEntryState> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let filename = dst
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let tmp_path = dst.with_file_name(format!(".{}.aksync.tmp", filename));

    // Copy via temporary sibling file
    std::fs::copy(src, &tmp_path)?;

    // Preserve modification time from source file
    let src_meta = std::fs::symlink_metadata(src)?;
    if let Ok(mtime) = src_meta.modified() {
        let file_res = std::fs::OpenOptions::new()
            .write(true)
            .open(&tmp_path)
            .or_else(|_| File::open(&tmp_path));
        if let Ok(file) = file_res {
            let _ = file.set_modified(mtime);
        }
    }

    // Atomic rename to final destination
    if let Err(e) = std::fs::rename(&tmp_path, dst) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    let dst_meta = std::fs::symlink_metadata(dst)?;
    let mtime_secs = match dst_meta.modified() {
        Ok(t) => match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs() as i64,
            Err(e) => -(e.duration().as_secs() as i64),
        },
        Err(_) => 0,
    };

    Ok(FileEntryState {
        size: dst_meta.len(),
        mtime_secs,
    })
}

/// Moves a deleted vault file into the vault Trash directory (<target_vault_path>/Trash/home/<rel_folder>/<rel_file>).
/// Falls back to fs::remove_file if rename fails across mounts (mycloud-fuse intercepts unlink to move into Trash).
pub fn move_vault_file_to_trash(
    target_vault_path: &Path,
    rel_folder: &str,
    rel_file: &str,
) -> std::io::Result<()> {
    let vault_file_path = target_vault_path
        .join("home")
        .join(rel_folder)
        .join(rel_file);
    if !vault_file_path.exists() {
        return Ok(());
    }

    let mut trash_dest_path = target_vault_path
        .join("Trash")
        .join("home")
        .join(rel_folder)
        .join(rel_file);

    if trash_dest_path.exists() {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let orig_name = trash_dest_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "deleted_file".to_string());
        let new_name = format!("{}.deleted-{}", orig_name, now_secs);
        trash_dest_path = trash_dest_path.with_file_name(new_name);
    }

    if let Some(parent) = trash_dest_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    match std::fs::rename(&vault_file_path, &trash_dest_path) {
        Ok(()) => Ok(()),
        Err(e) => {
            warn!(
                "fs::rename to trash failed ({:?} -> {:?}): {}. Falling back to fs::remove_file",
                vault_file_path, trash_dest_path, e
            );
            std::fs::remove_file(&vault_file_path)
        }
    }
}

/// Returns true if the relative file path represents a conflict file.
pub fn is_conflict_file(rel_path: &str) -> bool {
    let p = Path::new(rel_path);
    if let Some(file_name) = p.file_name() {
        file_name.to_string_lossy().contains(".sync-conflict-")
    } else {
        rel_path.contains(".sync-conflict-")
    }
}

/// Generates a conflict filename inserting `.sync-conflict-<unix_secs>` before the extension.
pub fn make_conflict_rel_path(rel_path: &str, unix_secs: u64) -> String {
    let p = Path::new(rel_path);
    let parent = p.parent();
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let ext = p.extension().map(|e| e.to_string_lossy());
    let new_filename = match ext {
        Some(extension) => format!("{}.sync-conflict-{}.{}", stem, unix_secs, extension),
        None => format!("{}.sync-conflict-{}", stem, unix_secs),
    };
    match parent {
        Some(par) if !par.as_os_str().is_empty() => {
            let par_str = par
                .iter()
                .filter_map(|s| s.to_str())
                .collect::<Vec<_>>()
                .join("/");
            format!("{}/{}", par_str, new_filename)
        }
        _ => new_filename,
    }
}

/// Reconciles differences between a local folder and the corresponding encrypted vault folder.
pub fn reconcile_folder(
    local_root: &Path,
    vault_root: &Path,
    target_vault_path: &Path,
    rel_folder: &str,
    sync_hidden: bool,
    manifest: &FolderSyncManifest,
) -> Result<FolderSyncManifest, String> {
    std::fs::create_dir_all(local_root)
        .map_err(|e| format!("Failed to create local directory {:?}: {}", local_root, e))?;
    std::fs::create_dir_all(vault_root)
        .map_err(|e| format!("Failed to create vault directory {:?}: {}", vault_root, e))?;

    let local_files = scan_tree(local_root, sync_hidden)
        .map_err(|e| format!("Failed to scan local folder {:?}: {}", local_root, e))?;
    let vault_files = scan_tree(vault_root, sync_hidden)
        .map_err(|e| format!("Failed to scan vault folder {:?}: {}", vault_root, e))?;

    let mut all_paths = std::collections::BTreeSet::new();
    for k in local_files.keys() {
        all_paths.insert(k.clone());
    }
    for k in vault_files.keys() {
        all_paths.insert(k.clone());
    }
    for k in manifest.files.keys() {
        all_paths.insert(k.clone());
    }

    let mut new_manifest = FolderSyncManifest {
        version: 1,
        files: BTreeMap::new(),
    };

    for rel_path in all_paths {
        let local_opt = local_files.get(&rel_path);
        let vault_opt = vault_files.get(&rel_path);
        let manifest_opt = manifest.files.get(&rel_path);

        match (local_opt, vault_opt) {
            // Case 1: Present in BOTH local and vault
            (Some(l_state), Some(v_state)) => {
                if same_version(l_state, v_state) {
                    new_manifest.files.insert(rel_path.clone(), l_state.clone());
                } else {
                    let is_conflict = match manifest_opt {
                        Some(m_state) => {
                            let local_matches = same_version(l_state, m_state);
                            let vault_matches = same_version(v_state, m_state);

                            if local_matches && !vault_matches {
                                // Vault was modified while local was untouched: copy vault -> local
                                let local_file_path = local_root.join(&rel_path);
                                let vault_file_path = vault_root.join(&rel_path);
                                match copy_file_preserving_mtime(&vault_file_path, &local_file_path)
                                {
                                    Ok(new_state) => {
                                        new_manifest.files.insert(rel_path.clone(), new_state);
                                    }
                                    Err(e) => {
                                        warn!(
                                            "Failed to copy vault file to local {:?}: {}",
                                            rel_path, e
                                        );
                                    }
                                }
                                false
                            } else if !local_matches && vault_matches {
                                // Local was modified while vault was untouched: copy local -> vault
                                let local_file_path = local_root.join(&rel_path);
                                let vault_file_path = vault_root.join(&rel_path);
                                match copy_file_preserving_mtime(&local_file_path, &vault_file_path)
                                {
                                    Ok(new_state) => {
                                        new_manifest.files.insert(rel_path.clone(), new_state);
                                    }
                                    Err(e) => {
                                        warn!(
                                            "Failed to copy local file to vault {:?}: {}",
                                            rel_path, e
                                        );
                                    }
                                }
                                false
                            } else {
                                // Both changed differently since last sync -> Conflict
                                true
                            }
                        }
                        None => {
                            // Manifest has no entry for rel_path (e.g. PC reset or pre-existing files on both sides)
                            true
                        }
                    };

                    if is_conflict {
                        let now_secs = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let conflict_rel = make_conflict_rel_path(&rel_path, now_secs);

                        let local_file_path = local_root.join(&rel_path);
                        let vault_file_path = vault_root.join(&rel_path);

                        if v_state.size < LARGE_CONFLICT_THRESHOLD_BYTES {
                            // Small conflict (< 50 MiB):
                            // 1. Rename in vault_root to conflict_rel
                            let vault_conflict_path = vault_root.join(&conflict_rel);
                            if let Some(parent) = vault_conflict_path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            if let Err(e) = std::fs::rename(&vault_file_path, &vault_conflict_path)
                            {
                                warn!(
                                    "fs::rename failed ({:?} -> {:?}): {}. Attempting copy fallback.",
                                    vault_file_path, vault_conflict_path, e
                                );
                                if copy_file_preserving_mtime(
                                    &vault_file_path,
                                    &vault_conflict_path,
                                )
                                .is_ok()
                                {
                                    let _ = std::fs::remove_file(&vault_file_path);
                                }
                            }

                            // 2. Upload the local file to vault_root.join(rel_path)
                            match copy_file_preserving_mtime(&local_file_path, &vault_file_path) {
                                Ok(uploaded_state) => {
                                    new_manifest.files.insert(rel_path.clone(), uploaded_state);
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to upload local file during conflict {:?}: {}",
                                        rel_path, e
                                    );
                                }
                            }

                            // 3. Copy conflict_rel from vault to local_root
                            let local_conflict_path = local_root.join(&conflict_rel);
                            match copy_file_preserving_mtime(
                                &vault_conflict_path,
                                &local_conflict_path,
                            ) {
                                Ok(conflict_state) => {
                                    new_manifest.files.insert(conflict_rel, conflict_state);
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to copy conflict file to local {:?}: {}",
                                        conflict_rel, e
                                    );
                                }
                            }
                        } else {
                            // Large conflict (>= 50 MiB):
                            // Zero-copy rename the vault version directly into Trash/home/<rel_folder>/<conflict_rel>
                            let trash_conflict_path = target_vault_path
                                .join("Trash")
                                .join("home")
                                .join(rel_folder)
                                .join(&conflict_rel);
                            if let Some(parent) = trash_conflict_path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            if let Err(e) = std::fs::rename(&vault_file_path, &trash_conflict_path)
                            {
                                warn!(
                                    "Failed to rename large vault file to trash ({:?} -> {:?}): {}. Falling back to remove_file.",
                                    vault_file_path, trash_conflict_path, e
                                );
                                let _ = std::fs::remove_file(&vault_file_path);
                            }

                            // Upload the small local file to vault_root.join(rel_path)
                            match copy_file_preserving_mtime(&local_file_path, &vault_file_path) {
                                Ok(uploaded_state) => {
                                    new_manifest.files.insert(rel_path.clone(), uploaded_state);
                                }
                                Err(e) => {
                                    warn!("Failed to upload local file during large conflict {:?}: {}", rel_path, e);
                                }
                            }
                        }
                    }
                }
            }

            // Case 2: Present ONLY in local, absent in vault
            (Some(_l_state), None) => {
                let local_file_path = local_root.join(&rel_path);
                if manifest_opt.is_some() {
                    // File existed at last sync and was deleted from the vault: delete local file
                    if let Err(e) = std::fs::remove_file(&local_file_path) {
                        warn!("Failed to remove local file {:?}: {}", local_file_path, e);
                    }
                } else {
                    // New local file: copy local -> vault and record in new_manifest
                    let vault_file_path = vault_root.join(&rel_path);
                    match copy_file_preserving_mtime(&local_file_path, &vault_file_path) {
                        Ok(new_state) => {
                            new_manifest.files.insert(rel_path.clone(), new_state);
                        }
                        Err(e) => {
                            warn!(
                                "Failed to copy new local file to vault {:?}: {}",
                                rel_path, e
                            );
                        }
                    }
                }
            }

            // Case 3: Present ONLY in vault, absent in local
            (None, Some(v_state)) => {
                if manifest_opt.is_some() {
                    // File existed at last sync and was deleted locally: move vault file to Trash
                    if let Err(e) =
                        move_vault_file_to_trash(target_vault_path, rel_folder, &rel_path)
                    {
                        warn!("Failed to move vault file to trash {:?}: {}", rel_path, e);
                    }
                } else {
                    // New file in vault OR user reset PC and ticked sync on existing vault
                    if is_conflict_file(&rel_path) && v_state.size >= LARGE_CONFLICT_THRESHOLD_BYTES
                    {
                        info!(
                            "Skipping download of large conflict file from vault: {:?}",
                            rel_path
                        );
                    } else {
                        let vault_file_path = vault_root.join(&rel_path);
                        let local_file_path = local_root.join(&rel_path);
                        match copy_file_preserving_mtime(&vault_file_path, &local_file_path) {
                            Ok(new_state) => {
                                new_manifest.files.insert(rel_path.clone(), new_state);
                            }
                            Err(e) => {
                                warn!("Failed to copy vault file to local {:?}: {}", rel_path, e);
                            }
                        }
                    }
                }
            }

            // Case 4: Absent in both local and vault (deleted on both sides since last sync)
            (None, None) => {}
        }
    }

    Ok(new_manifest)
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

/// Executes resilient bidirectional synchronization for all selected home folders with the encrypted vault:
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
    let _ = transfers;

    if HOME_SYNC_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        info!("Home sync is already in progress");
        return Ok(());
    }
    let _guard = SyncRunGuard;

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

    if rclone_bin != "true" {
        let is_mounted = std::fs::read_to_string("/proc/self/mountinfo")
            .map(|info| crate::process::is_mountpoint_ready(&info, target_vault_path, "fuse"))
            .unwrap_or(false);
        if !is_mounted {
            warn!(
                "Target vault path {:?} is not an active FUSE mount in /proc/self/mountinfo. Aborting home sync to prevent writing plaintext to disk.",
                target_vault_path
            );
            return Err("Encrypted vault FUSE filesystem is not actively mounted".to_string());
        }
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
        "Starting bidirectional encrypted home folders sync (sync_hidden={}) with '{:?}/home/' for {} folders: {:?}",
        sync_hidden,
        target_vault_path,
        folders.len(),
        folders
    );

    for rel_folder in folders {
        if !is_valid_sync_folder_rel_path(rel_folder) {
            warn!(
                "Invalid relative sync folder path {:?}. Skipping.",
                rel_folder
            );
            continue;
        }

        let source_dir = home_path.join(rel_folder);
        if !source_dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&source_dir) {
                warn!("Could not create local home folder {:?}: {}", source_dir, e);
                continue;
            }
        }

        let target_dir = vault_home_dir.join(rel_folder);
        if let Err(e) = std::fs::create_dir_all(&target_dir) {
            warn!(
                "Could not create vault target folder {:?}: {}",
                target_dir, e
            );
            continue;
        }

        info!(
            "Reconciling home folder {:?} <-> {:?} (sync_hidden={})",
            source_dir, target_dir, sync_hidden
        );

        let manifest_file = get_manifest_path(target_vault_path, rel_folder);
        let manifest = load_manifest(&manifest_file);

        match reconcile_folder(
            &source_dir,
            &target_dir,
            target_vault_path,
            rel_folder,
            sync_hidden,
            &manifest,
        ) {
            Ok(new_manifest) => {
                if let Err(e) = save_manifest(&manifest_file, &new_manifest) {
                    error!("Failed to save sync manifest to {:?}: {}", manifest_file, e);
                }
                crate::process::apply_folder_freeze_prevention_rules(&target_dir);
                info!(
                    "Successfully reconciled {:?} <-> {:?}",
                    source_dir, target_dir
                );
            }
            Err(e) => {
                error!(
                    "Reconciliation error for {:?} <-> {:?}: {}",
                    source_dir, target_dir, e
                );
            }
        }
    }

    // Refresh VFS cache on Vault WebDAV and raw mount so new files appear immediately
    let http = reqwest::Client::new();
    let vault_refresh_url = format!("http://{}/vfs/refresh", vault_rc_addr);
    let _ = http
        .post(&vault_refresh_url)
        .basic_auth(rc_user, Some(rc_pass))
        .json(&serde_json::json!({ "recursive": "true" }))
        .send()
        .await;

    let refresh_url = format!("http://{}/vfs/refresh", rc_addr);
    let _ = http
        .post(&refresh_url)
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

    static TEST_SYNC_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn setup_test_env(test_name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "mycloud_test_{}_{}_{}",
            test_name,
            std::process::id(),
            now
        ));
        let local_root = base.join("local");
        let target_vault_path = base.join("vault");
        let vault_root = target_vault_path.join("home").join("Documents");
        std::fs::create_dir_all(&local_root).unwrap();
        std::fs::create_dir_all(&vault_root).unwrap();
        (local_root, vault_root, target_vault_path)
    }

    #[test]
    fn test_sync_folder_path_validation() {
        assert!(is_valid_sync_folder_rel_path("Belgeler"));
        assert!(!is_valid_sync_folder_rel_path("../etc"));
        assert!(!is_valid_sync_folder_rel_path("/etc/passwd"));
        assert!(!is_valid_sync_folder_rel_path("a/../../b"));
    }

    #[tokio::test]
    async fn test_execute_home_sync_empty_folders() {
        let _lock = TEST_SYNC_MUTEX.lock().await;
        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_test_homesync_{}", std::process::id()));
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
        let _lock = TEST_SYNC_MUTEX.lock().await;
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

    #[test]
    fn test_conflict_rel_path_generation() {
        let conflict = make_conflict_rel_path("docs/notes.txt", 1731280000);
        assert_eq!(conflict, "docs/notes.sync-conflict-1731280000.txt");
        assert!(is_conflict_file(&conflict));
        assert!(!is_conflict_file("docs/notes.txt"));

        let no_ext = make_conflict_rel_path("notes", 1731280000);
        assert_eq!(no_ext, "notes.sync-conflict-1731280000");
        assert!(is_conflict_file(&no_ext));
    }

    #[test]
    fn test_same_version_tolerance() {
        let a = FileEntryState {
            size: 100,
            mtime_secs: 1000,
        };
        let b = FileEntryState {
            size: 100,
            mtime_secs: 1002,
        };
        let c = FileEntryState {
            size: 100,
            mtime_secs: 1003,
        };
        let d = FileEntryState {
            size: 101,
            mtime_secs: 1000,
        };

        assert!(same_version(&a, &b));
        assert!(!same_version(&a, &c));
        assert!(!same_version(&a, &d));
    }

    #[test]
    fn test_pc_reset_downloads_vault_files_without_deleting() {
        let (local_root, vault_root, target_vault_path) = setup_test_env("pc_reset");

        // Existing files in vault (e.g. from previous device/WebDAV)
        std::fs::write(vault_root.join("existing1.txt"), "vault data 1").unwrap();
        std::fs::write(vault_root.join("existing2.txt"), "vault data 2").unwrap();

        // 1 new local file created on this machine
        std::fs::write(local_root.join("new_local.txt"), "local data").unwrap();

        let initial_manifest = FolderSyncManifest::default();
        let new_manifest = reconcile_folder(
            &local_root,
            &vault_root,
            &target_vault_path,
            "Documents",
            false,
            &initial_manifest,
        )
        .unwrap();

        // Check that existing vault files were downloaded to local
        assert!(local_root.join("existing1.txt").exists());
        assert_eq!(
            std::fs::read_to_string(local_root.join("existing1.txt")).unwrap(),
            "vault data 1"
        );
        assert!(local_root.join("existing2.txt").exists());
        assert_eq!(
            std::fs::read_to_string(local_root.join("existing2.txt")).unwrap(),
            "vault data 2"
        );

        // Check that new local file was uploaded to vault
        assert!(vault_root.join("new_local.txt").exists());
        assert_eq!(
            std::fs::read_to_string(vault_root.join("new_local.txt")).unwrap(),
            "local data"
        );

        // Check that nothing was deleted
        assert!(vault_root.join("existing1.txt").exists());
        assert!(vault_root.join("existing2.txt").exists());
        assert!(local_root.join("new_local.txt").exists());

        assert_eq!(new_manifest.files.len(), 3);
    }

    #[test]
    fn test_local_deletion_moves_vault_file_to_trash() {
        let (local_root, vault_root, target_vault_path) = setup_test_env("local_del");

        // Initial state: 1 file created locally
        std::fs::write(local_root.join("file1.txt"), "hello world").unwrap();

        let initial_manifest = FolderSyncManifest::default();
        let manifest = reconcile_folder(
            &local_root,
            &vault_root,
            &target_vault_path,
            "Documents",
            false,
            &initial_manifest,
        )
        .unwrap();

        assert!(vault_root.join("file1.txt").exists());
        assert!(manifest.files.contains_key("file1.txt"));

        // User deletes file locally
        std::fs::remove_file(local_root.join("file1.txt")).unwrap();

        // Next sync cycle
        let manifest_after_del = reconcile_folder(
            &local_root,
            &vault_root,
            &target_vault_path,
            "Documents",
            false,
            &manifest,
        )
        .unwrap();

        // File must no longer exist in vault_root
        assert!(!vault_root.join("file1.txt").exists());
        assert!(!manifest_after_del.files.contains_key("file1.txt"));

        // File must be moved to Trash/home/Documents/file1.txt
        let trash_file = target_vault_path
            .join("Trash")
            .join("home")
            .join("Documents")
            .join("file1.txt");
        assert!(trash_file.exists());
        assert_eq!(std::fs::read_to_string(trash_file).unwrap(), "hello world");
    }

    #[test]
    fn test_vault_deletion_removes_local_file() {
        let (local_root, vault_root, target_vault_path) = setup_test_env("vault_del");

        std::fs::write(local_root.join("file1.txt"), "keep this sync").unwrap();

        let initial_manifest = FolderSyncManifest::default();
        let manifest = reconcile_folder(
            &local_root,
            &vault_root,
            &target_vault_path,
            "Documents",
            false,
            &initial_manifest,
        )
        .unwrap();

        assert!(vault_root.join("file1.txt").exists());

        // File is deleted from vault (e.g. via WebDAV or another device)
        std::fs::remove_file(vault_root.join("file1.txt")).unwrap();

        let manifest_after_del = reconcile_folder(
            &local_root,
            &vault_root,
            &target_vault_path,
            "Documents",
            false,
            &manifest,
        )
        .unwrap();

        // File must be removed locally
        assert!(!local_root.join("file1.txt").exists());
        assert!(!manifest_after_del.files.contains_key("file1.txt"));
    }

    #[test]
    fn test_small_conflict_creates_syncthing_conflict_copy_on_both_sides() {
        let (local_root, vault_root, target_vault_path) = setup_test_env("small_conflict");

        std::fs::write(local_root.join("notes.txt"), "original").unwrap();

        let initial_manifest = FolderSyncManifest::default();
        let manifest = reconcile_folder(
            &local_root,
            &vault_root,
            &target_vault_path,
            "Documents",
            false,
            &initial_manifest,
        )
        .unwrap();

        // Both sides modify notes.txt differently
        std::fs::write(
            local_root.join("notes.txt"),
            "local edited notes with unique size",
        )
        .unwrap();
        std::fs::write(
            vault_root.join("notes.txt"),
            "vault edited notes with another distinct length",
        )
        .unwrap();

        let manifest_after_conflict = reconcile_folder(
            &local_root,
            &vault_root,
            &target_vault_path,
            "Documents",
            false,
            &manifest,
        )
        .unwrap();

        // Original notes.txt has local content on both sides
        assert_eq!(
            std::fs::read_to_string(local_root.join("notes.txt")).unwrap(),
            "local edited notes with unique size"
        );
        assert_eq!(
            std::fs::read_to_string(vault_root.join("notes.txt")).unwrap(),
            "local edited notes with unique size"
        );

        // A conflict file exists in local_root
        let local_files = scan_tree(&local_root, false).unwrap();
        let conflict_rel = local_files
            .keys()
            .find(|k| is_conflict_file(k))
            .expect("Expected conflict file to be created in local_root");

        // Conflict file has vault content
        assert_eq!(
            std::fs::read_to_string(local_root.join(conflict_rel)).unwrap(),
            "vault edited notes with another distinct length"
        );

        // Conflict file also exists in vault_root
        assert!(vault_root.join(conflict_rel).exists());
        assert_eq!(
            std::fs::read_to_string(vault_root.join(conflict_rel)).unwrap(),
            "vault edited notes with another distinct length"
        );

        // Manifest records both notes.txt and conflict file
        assert!(manifest_after_conflict.files.contains_key("notes.txt"));
        assert!(manifest_after_conflict.files.contains_key(conflict_rel));
    }

    #[test]
    fn test_large_conflict_moves_vault_version_to_trash_without_local_download() {
        let (local_root, vault_root, target_vault_path) = setup_test_env("large_conflict");

        std::fs::write(local_root.join("data.bin"), "initial small data").unwrap();

        let initial_manifest = FolderSyncManifest::default();
        let manifest = reconcile_folder(
            &local_root,
            &vault_root,
            &target_vault_path,
            "Documents",
            false,
            &initial_manifest,
        )
        .unwrap();

        // Modify local file with small new content
        std::fs::write(local_root.join("data.bin"), "updated small local content").unwrap();

        // Modify vault file with a large sparse file (50 MiB + 16 bytes)
        let large_size = LARGE_CONFLICT_THRESHOLD_BYTES + 16;
        let vfile = std::fs::OpenOptions::new()
            .write(true)
            .open(vault_root.join("data.bin"))
            .unwrap();
        vfile.set_len(large_size).unwrap();

        let manifest_after_conflict = reconcile_folder(
            &local_root,
            &vault_root,
            &target_vault_path,
            "Documents",
            false,
            &manifest,
        )
        .unwrap();

        // Vault now contains the updated small local content
        assert_eq!(
            std::fs::read_to_string(vault_root.join("data.bin")).unwrap(),
            "updated small local content"
        );
        assert_eq!(
            std::fs::read_to_string(local_root.join("data.bin")).unwrap(),
            "updated small local content"
        );

        // Ensure NO conflict file was downloaded to local_root
        let local_files = scan_tree(&local_root, false).unwrap();
        let local_has_conflict = local_files.keys().any(|k| is_conflict_file(k));
        assert!(
            !local_has_conflict,
            "Large conflict file must NOT be downloaded to local_root"
        );

        // Ensure the large conflict file was moved into Trash
        let trash_dir = target_vault_path
            .join("Trash")
            .join("home")
            .join("Documents");
        assert!(trash_dir.exists());

        let trash_entries = std::fs::read_dir(&trash_dir).unwrap();
        let mut found_trash_conflict = false;
        for entry in trash_entries {
            let e = entry.unwrap();
            let name = e.file_name().to_string_lossy().to_string();
            if is_conflict_file(&name) {
                let meta = e.metadata().unwrap();
                assert_eq!(meta.len(), large_size);
                found_trash_conflict = true;
            }
        }
        assert!(
            found_trash_conflict,
            "Expected large conflict file in vault Trash directory"
        );

        // Manifest only tracks the active data.bin file
        assert!(manifest_after_conflict.files.contains_key("data.bin"));
    }
}
