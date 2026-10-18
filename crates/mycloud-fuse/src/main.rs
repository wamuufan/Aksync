use std::collections::{BinaryHeap, HashMap};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use clap::Parser;
use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow, FUSE_ROOT_ID,
};
use parking_lot::RwLock;
use tracing::{error, info, warn};

use mycloud_crypto::{
    decrypt_filename, encrypt_filename, hash_long_cipher_name, is_longname_file,
    is_longname_sidecar, longname_sidecar_for, CryptoEngine, CHUNK_CIPHER_SIZE_V2,
    CHUNK_DISK_SIZE_V3, CHUNK_OVERHEAD_V3, CHUNK_PLAIN_SIZE, DIRIV_FILENAME, HEADER_SIZE_V2,
    HEADER_SIZE_V3, MAC_SIZE, MAGIC_V3, MAX_RAW_FILENAME_LEN,
};

const TTL: Duration = Duration::from_secs(1);
const FUSE_SUPER_MAGIC: u64 = 0x65735546;

/// CLI arguments for mycloud-fuse
#[derive(Parser, Debug)]
#[command(
    name = "mycloud-fuse",
    version,
    about = "Files On-Demand E2EE FUSE Layer"
)]
pub struct Args {
    /// Hidden directory managed by rclone transport / VFS cache
    #[arg(long, default_value = "~/.cache/mycloud/raw_mount")]
    pub raw_path: String,

    /// User-facing decrypted FUSE mount point
    #[arg(long, default_value = "~/MyCloud")]
    pub mount_path: String,

    /// Read 32-byte cryptographic key directly from stdin (anonymous pipe)
    #[arg(long)]
    pub key_stdin: bool,

    /// Hex-encoded 32-byte cryptographic key
    #[arg(long)]
    pub key_hex: Option<String>,

    /// Passphrase for Argon2id key derivation (must be paired with --salt-base64)
    #[arg(long)]
    pub passphrase: Option<String>,

    /// Base64 encoded Argon2id salt for key derivation
    #[arg(long)]
    pub salt_base64: Option<String>,

    /// Check that raw storage has FUSE filesystem magic (disable for tmpfs/ext4 tests)
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub raw_fstype_check: bool,

    /// Timeout in seconds for any single FUSE filesystem operation before returning EIO
    #[arg(long, default_value_t = 15)]
    pub op_timeout_secs: u64,

    /// Migrate all v2 encrypted files to Format v3 under raw_path and exit
    #[arg(long)]
    pub migrate_v3: bool,
}

/// Expand tilde (~) prefix to user HOME directory
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

/// Inode tracking table maintaining two-way mapping between inodes and relative paths
pub struct InodeTable {
    pub ino_to_path: HashMap<u64, PathBuf>,
    pub path_to_ino: HashMap<PathBuf, u64>,
    pub next_ino: u64,
}

impl Default for InodeTable {
    fn default() -> Self {
        Self::new()
    }
}

impl InodeTable {
    pub fn new() -> Self {
        let mut ino_to_path = HashMap::new();
        let mut path_to_ino = HashMap::new();
        let root_path = PathBuf::new();
        ino_to_path.insert(FUSE_ROOT_ID, root_path.clone());
        path_to_ino.insert(root_path, FUSE_ROOT_ID);

        Self {
            ino_to_path,
            path_to_ino,
            next_ino: FUSE_ROOT_ID + 1,
        }
    }

    pub fn get_path(&self, ino: u64) -> Option<PathBuf> {
        self.ino_to_path.get(&ino).cloned()
    }

    pub fn get_or_insert_ino(&mut self, path: PathBuf) -> u64 {
        if let Some(&ino) = self.path_to_ino.get(&path) {
            ino
        } else {
            let ino = self.next_ino;
            self.next_ino += 1;
            self.ino_to_path.insert(ino, path.clone());
            self.path_to_ino.insert(path, ino);
            ino
        }
    }

    pub fn remove(&mut self, path: &Path) {
        let matching: Vec<(PathBuf, u64)> = self
            .path_to_ino
            .iter()
            .filter(|(p, _)| p.as_path() == path || p.starts_with(path))
            .map(|(p, &ino)| (p.clone(), ino))
            .collect();

        for (p, ino) in matching {
            self.path_to_ino.remove(&p);
            self.ino_to_path.remove(&ino);
        }
    }

    pub fn rename(&mut self, old_path: &Path, new_path: PathBuf) {
        let matching: Vec<(PathBuf, u64)> = self
            .path_to_ino
            .iter()
            .filter(|(p, _)| p.as_path() == old_path || p.starts_with(old_path))
            .map(|(p, &ino)| (p.clone(), ino))
            .collect();

        for (old_p, ino) in matching {
            let rel = old_p.strip_prefix(old_path).unwrap();
            let updated_p = if rel.as_os_str().is_empty() {
                new_path.clone()
            } else {
                new_path.join(rel)
            };
            self.path_to_ino.remove(&old_p);
            self.ino_to_path.insert(ino, updated_p.clone());
            self.path_to_ino.insert(updated_p, ino);
        }
    }
}

/// Translate I/O error to appropriate errno, preserving ENOENT only for genuine NotFound errors
fn io_err_to_errno(err: &std::io::Error) -> i32 {
    match err.kind() {
        std::io::ErrorKind::NotFound => libc::ENOENT,
        std::io::ErrorKind::PermissionDenied => libc::EACCES,
        std::io::ErrorKind::AlreadyExists => libc::EEXIST,
        _ => {
            if let Some(code) = err.raw_os_error() {
                if code == libc::ENOENT {
                    return libc::ENOENT;
                }
                if code == libc::ENOTCONN || code == libc::ECONNRESET || code == libc::EIO {
                    return libc::EIO;
                }
                return code;
            }
            libc::EIO
        }
    }
}

/// Common trait for fuser reply structures allowing unified error completion
pub trait ReplyError: Send + 'static {
    fn error(self, err: i32);
}

impl ReplyError for ReplyEntry { fn error(self, err: i32) { self.error(err); } }
impl ReplyError for ReplyAttr { fn error(self, err: i32) { self.error(err); } }
impl ReplyError for ReplyData { fn error(self, err: i32) { self.error(err); } }
impl ReplyError for ReplyWrite { fn error(self, err: i32) { self.error(err); } }
impl ReplyError for ReplyOpen { fn error(self, err: i32) { self.error(err); } }
impl ReplyError for ReplyCreate { fn error(self, err: i32) { self.error(err); } }
impl ReplyError for ReplyEmpty { fn error(self, err: i32) { self.error(err); } }
impl ReplyError for ReplyDirectory { fn error(self, err: i32) { self.error(err); } }
impl ReplyError for ReplyStatfs { fn error(self, err: i32) { self.error(err); } }

/// Converts an optional TimeOrNow value into a libc::timespec for utimensat
fn time_or_now_to_timespec(t: Option<TimeOrNow>) -> libc::timespec {
    match t {
        Some(TimeOrNow::SpecificTime(st)) => match st.duration_since(UNIX_EPOCH) {
            Ok(d) => libc::timespec {
                tv_sec: d.as_secs() as libc::time_t,
                tv_nsec: d.subsec_nanos() as libc::c_long,
            },
            Err(e) => {
                let d = e.duration();
                libc::timespec {
                    tv_sec: -(d.as_secs() as libc::time_t)
                        - if d.subsec_nanos() > 0 { 1 } else { 0 },
                    tv_nsec: if d.subsec_nanos() > 0 {
                        1_000_000_000 - d.subsec_nanos() as libc::c_long
                    } else {
                        0
                    },
                }
            }
        },
        Some(TimeOrNow::Now) => libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_NOW,
        },
        None => libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
    }
}

/// Reply guard holding a single Reply instance across worker threads and deadline timers
pub struct ReplyGuard<R: ReplyError> {
    cell: Arc<parking_lot::Mutex<Option<R>>>,
}

impl<R: ReplyError> ReplyGuard<R> {
    pub fn new(reply: R) -> (Self, Arc<parking_lot::Mutex<Option<R>>>) {
        let cell = Arc::new(parking_lot::Mutex::new(Some(reply)));
        (Self { cell: cell.clone() }, cell)
    }

    pub fn take(&self) -> Option<R> {
        self.cell.lock().take()
    }
}

impl<R: ReplyError> Drop for ReplyGuard<R> {
    fn drop(&mut self) {
        if let Some(r) = self.cell.lock().take() {
            r.error(libc::EIO);
        }
    }
}

type TimeoutCallback = Box<dyn FnOnce() + Send + 'static>;

struct ScheduledTimeout {
    deadline: Instant,
    callback: TimeoutCallback,
}

impl PartialEq for ScheduledTimeout {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline
    }
}

impl Eq for ScheduledTimeout {}

impl PartialOrd for ScheduledTimeout {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScheduledTimeout {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.deadline.cmp(&self.deadline)
    }
}

pub struct TimeoutScheduler {
    sender: crossbeam_channel::Sender<ScheduledTimeout>,
    stop: Arc<AtomicBool>,
}

impl Default for TimeoutScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeoutScheduler {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<ScheduledTimeout>();
        let stop = Arc::new(AtomicBool::new(false));
        let s_clone = stop.clone();

        std::thread::Builder::new()
            .name("fuse-timeout-scheduler".to_string())
            .spawn(move || {
                let mut heap: BinaryHeap<ScheduledTimeout> = BinaryHeap::new();

                while !s_clone.load(Ordering::Relaxed) {
                    let now = Instant::now();

                    while let Some(item) = heap.peek() {
                        if item.deadline <= now {
                            let item = heap.pop().unwrap();
                            (item.callback)();
                        } else {
                            break;
                        }
                    }

                    let timeout = if let Some(item) = heap.peek() {
                        item.deadline.saturating_duration_since(now)
                    } else {
                        Duration::from_millis(100)
                    };

                    match rx.recv_timeout(timeout) {
                        Ok(new_item) => {
                            heap.push(new_item);
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("failed to spawn timeout scheduler thread");

        Self {
            sender: tx,
            stop,
        }
    }

    pub fn schedule<F>(&self, deadline: Instant, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let _ = self.sender.send(ScheduledTimeout {
            deadline,
            callback: Box::new(callback),
        });
    }
}

impl Drop for TimeoutScheduler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Circuit breaker continuously checking health of raw underlying storage
#[derive(Clone)]
pub struct RawMountHealth {
    healthy: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    cooldown_until: Arc<parking_lot::Mutex<Option<Instant>>>,
}

impl RawMountHealth {
    pub fn new(raw_root: PathBuf, check_fstype: bool) -> Self {
        let healthy = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let cooldown_until = Arc::new(parking_lot::Mutex::new(None));

        let h_clone = healthy.clone();
        let s_clone = stop.clone();
        let path = raw_root;

        std::thread::Builder::new()
            .name("raw-health-monitor".to_string())
            .spawn(move || {
                while !s_clone.load(Ordering::Relaxed) {
                    let is_healthy = Self::probe(&path, check_fstype);
                    h_clone.store(is_healthy, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("failed to spawn raw mount health monitor");

        Self {
            healthy,
            stop,
            cooldown_until,
        }
    }

    pub fn is_healthy(&self) -> bool {
        if let Some(until) = *self.cooldown_until.lock() {
            if Instant::now() < until {
                return false;
            }
        }
        self.healthy.load(Ordering::SeqCst)
    }

    pub fn mark_unhealthy(&self) {
        self.healthy.store(false, Ordering::SeqCst);
        *self.cooldown_until.lock() = Some(Instant::now() + Duration::from_secs(15));
    }

    pub fn clear_cooldown(&self) {
        *self.cooldown_until.lock() = None;
    }

    fn probe(path: &Path, check_fstype: bool) -> bool {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let c_path = match std::ffi::CString::new(path.as_os_str().as_bytes()) {
            Ok(c) => c,
            Err(_) => return false,
        };

        std::thread::spawn(move || {
            let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
            let ret = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
            let ok = if ret == 0 {
                if check_fstype {
                    (stat.f_type as u64) == FUSE_SUPER_MAGIC
                } else {
                    true
                }
            } else {
                false
            };
            let _ = tx.send(ok);
        });

        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(healthy) => healthy,
            Err(_) => {
                warn!("statfs probe timed out after 3s on {:?}", path);
                false
            }
        }
    }
}

impl Drop for RawMountHealth {
    fn drop(&mut self) {
        if Arc::strong_count(&self.stop) <= 2 {
            self.stop.store(true, Ordering::Relaxed);
        }
    }
}

/// Open file handle table mapping fh -> open File
pub struct HandleTable {
    next_fh: AtomicU64,
    handles: RwLock<HashMap<u64, Arc<parking_lot::Mutex<File>>>>,
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new()
    }
}

impl HandleTable {
    pub fn new() -> Self {
        Self {
            next_fh: AtomicU64::new(1),
            handles: RwLock::new(HashMap::new()),
        }
    }

    pub fn insert(&self, file: File) -> u64 {
        let fh = self.next_fh.fetch_add(1, Ordering::SeqCst);
        self.handles
            .write()
            .insert(fh, Arc::new(parking_lot::Mutex::new(file)));
        fh
    }

    pub fn get(&self, fh: u64) -> Option<Arc<parking_lot::Mutex<File>>> {
        self.handles.read().get(&fh).cloned()
    }

    pub fn remove(&self, fh: u64) -> Option<Arc<parking_lot::Mutex<File>>> {
        self.handles.write().remove(&fh)
    }
}

/// Inode-level mutex table serializing write/truncate/namespace mutations and preventing deadlocks
pub struct InodeLocks {
    locks: parking_lot::Mutex<HashMap<u64, Arc<parking_lot::Mutex<()>>>>,
}

impl Default for InodeLocks {
    fn default() -> Self {
        Self::new()
    }
}

impl InodeLocks {
    pub fn new() -> Self {
        Self {
            locks: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    pub fn with_locks<R>(&self, inos: &[u64], f: impl FnOnce() -> R) -> R {
        let mut sorted = inos.to_vec();
        sorted.sort_unstable();
        sorted.dedup();

        let arcs: Vec<Arc<parking_lot::Mutex<()>>> = {
            let mut map = self.locks.lock();
            sorted
                .into_iter()
                .map(|ino| {
                    map.entry(ino)
                        .or_insert_with(|| Arc::new(parking_lot::Mutex::new(())))
                        .clone()
                })
                .collect()
        };

        let _guards: Vec<_> = arcs.iter().map(|a| a.lock()).collect();
        f()
    }
}

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Errors returned when dispatching a task to the worker pool
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchError {
    QueueFull,
    Disconnected,
}

/// Fixed-size worker pool executing FUSE operations
pub struct WorkerPool {
    sender: crossbeam_channel::Sender<Job>,
    _threads: Vec<std::thread::JoinHandle<()>>,
}

impl WorkerPool {
    pub fn new(num_threads: usize, queue_size: usize) -> Self {
        let (tx, rx) = crossbeam_channel::bounded::<Job>(queue_size);
        let mut threads = Vec::with_capacity(num_threads);

        for i in 0..num_threads {
            let rx = rx.clone();
            let handle = std::thread::Builder::new()
                .name(format!("fuse-worker-{}", i))
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        job();
                    }
                })
                .expect("failed to spawn worker thread");
            threads.push(handle);
        }

        Self {
            sender: tx,
            _threads: threads,
        }
    }

    pub fn try_dispatch<F>(&self, job: F) -> Result<(), DispatchError>
    where
        F: FnOnce() + Send + 'static,
    {
        self.sender.try_send(Box::new(job)).map_err(|e| match e {
            crossbeam_channel::TrySendError::Full(_) => {
                warn!("FUSE worker pool queue is full! Rejecting operation with EIO");
                DispatchError::QueueFull
            }
            crossbeam_channel::TrySendError::Disconnected(_) => {
                error!("FUSE worker pool is disconnected!");
                DispatchError::Disconnected
            }
        })
    }
}

pub struct FsInner {
    pub engine: CryptoEngine,
    pub raw_root: PathBuf,
    pub inodes: Arc<RwLock<InodeTable>>,
    pub health: RawMountHealth,
    pub handles: HandleTable,
    pub inode_locks: InodeLocks,
    pub worker_pool: WorkerPool,
    pub timeout_scheduler: TimeoutScheduler,
    pub op_timeout_secs: u64,
    pub dir_ids: RwLock<HashMap<PathBuf, String>>,
}

impl FsInner {
    pub fn dispatch<R, F>(self: &Arc<Self>, reply: R, op: F)
    where
        R: ReplyError,
        F: FnOnce(ReplyGuard<R>, Arc<FsInner>) + Send + 'static,
    {
        if !self.health.is_healthy() {
            reply.error(libc::EIO);
            return;
        }

        let (guard, cell) = ReplyGuard::new(reply);
        let deadline = Instant::now() + Duration::from_secs(self.op_timeout_secs);

        let cell_clone = cell.clone();
        let health_clone = self.health.clone();
        let timeout_secs = self.op_timeout_secs;
        self.timeout_scheduler.schedule(deadline, move || {
            if let Some(r) = cell_clone.lock().take() {
                warn!(
                    "FUSE operation timed out after {}s, returning EIO and marking raw mount unhealthy",
                    timeout_secs
                );
                health_clone.mark_unhealthy();
                r.error(libc::EIO);
            }
        });

        let inner_clone = self.clone();
        if self
            .worker_pool
            .try_dispatch(move || {
                op(guard, inner_clone);
            })
            .is_err()
        {
            if let Some(r) = cell.lock().take() {
                r.error(libc::EIO);
            }
        }
    }

    /// Retrieves or generates a 24-byte DirIV identity (Format v3 64-byte authenticated header) for a directory.
    /// Memory cache avoids all disk I/O on repeated lookups.
    pub fn get_or_create_dir_id(
        &self,
        raw_dir_path: &Path,
        rel_dir_path: &Path,
        create_if_missing: bool,
    ) -> Result<String, i32> {
        // 1. Check memory cache first
        if let Some(id) = self.dir_ids.read().get(rel_dir_path) {
            return Ok(id.clone());
        }

        // 2. Ensure raw_dir_path exists if it is raw_root
        if rel_dir_path.as_os_str().is_empty() && !raw_dir_path.exists() {
            let _ = fs::create_dir_all(raw_dir_path);
        }

        let diriv_path = raw_dir_path.join(DIRIV_FILENAME);
        if diriv_path.exists() {
            let mut f = File::open(&diriv_path).map_err(|e| io_err_to_errno(&e))?;
            let mut hdr = [0u8; HEADER_SIZE_V3];
            f.read_exact(&mut hdr).map_err(|_| libc::EIO)?;

            let dir_id = self.engine.verify_v3_header(&hdr).map_err(|_| libc::EIO)?;
            let hex_id = hex::encode(dir_id);
            self.dir_ids
                .write()
                .insert(rel_dir_path.to_path_buf(), hex_id.clone());
            Ok(hex_id)
        } else if create_if_missing || (rel_dir_path.as_os_str().is_empty() && raw_dir_path.exists()) {
            if !raw_dir_path.exists() {
                return Err(libc::ENOENT);
            }
            let (hdr, dir_id) = self.engine.create_v3_header();
            let rand_val = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let tmp_path = raw_dir_path.join(format!(
                ".aksync_diriv.tmp.{}.{}",
                std::process::id(),
                rand_val
            ));
            let write_res = (|| -> Result<(), i32> {
                let mut tmp = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(&tmp_path)
                    .map_err(|e| io_err_to_errno(&e))?;
                tmp.write_all(&hdr).map_err(|e| io_err_to_errno(&e))?;
                tmp.sync_all().map_err(|e| io_err_to_errno(&e))?;
                fs::rename(&tmp_path, &diriv_path).map_err(|e| io_err_to_errno(&e))?;
                Ok(())
            })();

            if write_res.is_err() {
                let _ = fs::remove_file(&tmp_path);
                return write_res.map(|_| String::new());
            }

            let hex_id = hex::encode(dir_id);
            self.dir_ids
                .write()
                .insert(rel_dir_path.to_path_buf(), hex_id.clone());
            Ok(hex_id)
        } else {
            Err(libc::ENOENT)
        }
    }

    /// Writes a sidecar (.name) file if the encrypted filename exceeds MAX_RAW_FILENAME_LEN (255 bytes)
    pub fn write_longname_sidecar_if_needed(
        &self,
        parent_raw_dir: &Path,
        parent_rel_dir: &Path,
        plain_name: &str,
    ) -> Result<(), i32> {
        if plain_name.len() > 255 {
            return Err(libc::ENAMETOOLONG);
        }
        let parent_dir_id = self.get_or_create_dir_id(parent_raw_dir, parent_rel_dir, true)?;
        let enc_name = encrypt_filename(&self.engine, plain_name, Some(&parent_dir_id))
            .map_err(|_| libc::EIO)?;
        if enc_name.len() > MAX_RAW_FILENAME_LEN {
            let short_name = hash_long_cipher_name(&enc_name);
            let sidecar_path = parent_raw_dir.join(longname_sidecar_for(&short_name));
            if !sidecar_path.exists() {
                fs::write(&sidecar_path, enc_name.as_bytes()).map_err(|e| io_err_to_errno(&e))?;
            }
        }
        Ok(())
    }

    /// Removes the companion .name sidecar file if the given raw path is a longname hashed file
    pub fn remove_longname_sidecar_if_any(&self, raw_target_path: &Path) {
        if let Some(file_name) = raw_target_path.file_name().and_then(|f| f.to_str()) {
            if is_longname_file(file_name) {
                if let Some(parent) = raw_target_path.parent() {
                    let sidecar_path = parent.join(longname_sidecar_for(file_name));
                    let _ = fs::remove_file(sidecar_path);
                }
            }
        }
    }

    /// Creates an encrypted symbolic link pointing to target under parent_rel_dir
    pub fn create_symlink(
        &self,
        parent_rel_dir: &Path,
        link_name: &str,
        target: &str,
    ) -> Result<PathBuf, i32> {
        if link_name.len() > 255 {
            return Err(libc::ENAMETOOLONG);
        }
        let raw_parent = self.to_raw_path(parent_rel_dir)?;
        let parent_dir_id = self.get_or_create_dir_id(&raw_parent, parent_rel_dir, true)?;
        let enc_target = encrypt_filename(&self.engine, target, Some(&parent_dir_id))
            .map_err(|_| libc::EIO)?;
        let child_rel = parent_rel_dir.join(link_name);
        let raw_child = self.to_raw_path(&child_rel)?;

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&enc_target, &raw_child)
                .map_err(|e| io_err_to_errno(&e))?;
        }
        #[cfg(not(unix))]
        {
            return Err(libc::ENOSYS);
        }

        self.write_longname_sidecar_if_needed(&raw_parent, parent_rel_dir, link_name)?;
        Ok(raw_child)
    }

    /// Reads an encrypted symbolic link and decrypts the target path
    pub fn read_symlink(&self, rel_path: &Path) -> Result<String, i32> {
        let raw_path = self.to_raw_path(rel_path)?;
        let parent_rel = rel_path.parent().unwrap_or_else(|| Path::new(""));
        let raw_parent = match raw_path.parent() {
            Some(p) => p,
            None => &self.raw_root,
        };
        let enc_target_path = fs::read_link(&raw_path).map_err(|e| io_err_to_errno(&e))?;
        let enc_target_str = enc_target_path.to_str().ok_or(libc::EIO)?;
        let parent_dir_id = self.get_or_create_dir_id(raw_parent, parent_rel, false)?;
        let plain_target = decrypt_filename(&self.engine, enc_target_str, Some(&parent_dir_id))
            .map_err(|_| libc::EIO)?;
        Ok(plain_target)
    }

    /// Converts a user-facing relative plaintext path to the underlying encrypted physical path
    pub fn to_raw_path(&self, rel_plain: &Path) -> Result<PathBuf, i32> {
        let mut current_raw = self.raw_root.clone();
        let mut current_rel = PathBuf::new();
        for component in rel_plain.components() {
            match component {
                std::path::Component::Normal(os_name) => {
                    let plain_str = os_name.to_str().ok_or(libc::EINVAL)?;
                    if plain_str.len() > 255 {
                        return Err(libc::ENAMETOOLONG);
                    }
                    if plain_str == DIRIV_FILENAME {
                        return Err(libc::EACCES);
                    }
                    let parent_dir_id =
                        self.get_or_create_dir_id(&current_raw, &current_rel, true)?;
                    let enc_name = encrypt_filename(&self.engine, plain_str, Some(&parent_dir_id))
                        .map_err(|_| libc::EIO)?;
                    if enc_name.len() <= MAX_RAW_FILENAME_LEN {
                        current_raw.push(enc_name);
                    } else {
                        let short_name = hash_long_cipher_name(&enc_name);
                        current_raw.push(short_name);
                    }
                    current_rel.push(os_name);
                }
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    current_raw.pop();
                    current_rel.pop();
                }
                std::path::Component::RootDir | std::path::Component::Prefix(_) => {}
            }
        }
        Ok(current_raw)
    }

    /// Builds a FileAttr structure for a given inode and physical file metadata
    pub fn build_file_attr(&self, ino: u64, raw_path: &Path, meta: &fs::Metadata) -> FileAttr {
        let is_dir = meta.is_dir();
        let is_symlink = meta.file_type().is_symlink();
        let kind = if is_dir {
            FileType::Directory
        } else if is_symlink {
            FileType::Symlink
        } else {
            FileType::RegularFile
        };

        let raw_size = meta.len();
        let plain_size = if is_dir {
            4096
        } else if is_symlink {
            raw_size
        } else if raw_size >= (HEADER_SIZE_V3 as u64) {
            let is_v3 = if let Ok(mut f) = File::open(raw_path) {
                let mut magic = [0u8; 4];
                f.read_exact(&mut magic).is_ok() && &magic == MAGIC_V3
            } else {
                false
            };

            if is_v3 {
                CryptoEngine::calculate_plain_size_v3_checked(raw_size).unwrap_or(0)
            } else {
                CryptoEngine::calculate_plain_size(raw_size)
            }
        } else {
            CryptoEngine::calculate_plain_size(raw_size)
        };

        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
        let atime = meta.accessed().unwrap_or(mtime);

        #[cfg(unix)]
        let mode_perm = {
            use std::os::unix::fs::PermissionsExt;
            let p = meta.permissions().mode() as u16 & 0o7777;
            if p == 0 {
                if is_dir {
                    0o755
                } else {
                    0o644
                }
            } else {
                p
            }
        };
        #[cfg(not(unix))]
        let mode_perm = if is_dir { 0o755 } else { 0o644 };

        FileAttr {
            ino,
            size: plain_size,
            blocks: plain_size.div_ceil(512),
            atime,
            mtime,
            ctime: mtime,
            crtime: mtime,
            kind,
            perm: mode_perm,
            nlink: if is_dir { 2 } else { 1 },
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: CHUNK_PLAIN_SIZE as u32,
            flags: 0,
        }
    }

    /// Decrypts a chunked file range from an open File handle (Format v3 with v2 fallback)
    fn read_payload_from_file(
        &self,
        file: &mut File,
        offset: u64,
        size: usize,
    ) -> Result<Vec<u8>, i32> {
        let raw_len = file.metadata().map_err(|e| io_err_to_errno(&e))?.len();
        if raw_len == 0 {
            return Ok(Vec::new());
        }

        // Check for Format v3 magic bytes
        let is_v3 = if raw_len >= 4 {
            let mut magic = [0u8; 4];
            file.seek(SeekFrom::Start(0))
                .map_err(|e| io_err_to_errno(&e))?;
            file.read_exact(&mut magic)
                .map_err(|e| io_err_to_errno(&e))?;
            &magic == MAGIC_V3
        } else {
            false
        };

        if !is_v3 {
            // ==========================================
            // Legacy Format v2 Reading
            // ==========================================
            if raw_len <= HEADER_SIZE_V2 as u64 {
                return Ok(Vec::new());
            }

            let payload_len = raw_len - HEADER_SIZE_V2 as u64;
            let rem = payload_len % (CHUNK_CIPHER_SIZE_V2 as u64);
            if rem >= 1 && rem <= MAC_SIZE as u64 {
                return Err(libc::EIO);
            }

            let plain_total = CryptoEngine::calculate_plain_size(raw_len);
            if offset >= plain_total {
                return Ok(Vec::new());
            }

            let mut base_nonce = [0u8; HEADER_SIZE_V2];
            file.seek(SeekFrom::Start(0))
                .map_err(|e| io_err_to_errno(&e))?;
            file.read_exact(&mut base_nonce)
                .map_err(|e| io_err_to_errno(&e))?;

            let bytes_to_read = std::cmp::min(size as u64, plain_total - offset) as usize;
            let plain_end = offset + bytes_to_read as u64;

            let start_chunk = CryptoEngine::plain_offset_to_chunk_index(offset);
            let end_chunk = CryptoEngine::plain_offset_to_chunk_index(plain_end - 1);

            let mut output = Vec::with_capacity(bytes_to_read);

            for chunk_idx in start_chunk..=end_chunk {
                let cipher_offset = CryptoEngine::chunk_offset_to_cipher_offset(chunk_idx);
                if cipher_offset >= raw_len {
                    break;
                }

                let available_cipher = (raw_len - cipher_offset) as usize;
                let cipher_len = std::cmp::min(available_cipher, CHUNK_CIPHER_SIZE_V2);
                if cipher_len < MAC_SIZE {
                    return Err(libc::EIO);
                }

                let mut cipher_buf = vec![0u8; cipher_len];
                file.seek(SeekFrom::Start(cipher_offset))
                    .map_err(|e| io_err_to_errno(&e))?;
                file.read_exact(&mut cipher_buf)
                    .map_err(|e| io_err_to_errno(&e))?;

                let chunk_nonce = CryptoEngine::derive_chunk_nonce(&base_nonce, chunk_idx);
                let plain_chunk = self
                    .engine
                    .decrypt_chunk(&chunk_nonce, &cipher_buf)
                    .map_err(|_| libc::EIO)?;

                let chunk_plain_start = chunk_idx * (CHUNK_PLAIN_SIZE as u64);
                let slice_start = std::cmp::max(offset, chunk_plain_start) - chunk_plain_start;
                let slice_end = std::cmp::min(plain_end, chunk_plain_start + plain_chunk.len() as u64)
                    - chunk_plain_start;

                if slice_start < slice_end && (slice_end as usize) <= plain_chunk.len() {
                    output.extend_from_slice(&plain_chunk[slice_start as usize..slice_end as usize]);
                }
            }

            return Ok(output);
        }

        // ==========================================
        // Format v3 Reading
        // ==========================================
        if raw_len < HEADER_SIZE_V3 as u64 {
            return Err(libc::EIO);
        }

        let mut hdr = [0u8; HEADER_SIZE_V3];
        file.seek(SeekFrom::Start(0))
            .map_err(|e| io_err_to_errno(&e))?;
        file.read_exact(&mut hdr)
            .map_err(|e| io_err_to_errno(&e))?;
        let file_id = self.engine.verify_v3_header(&hdr).map_err(|_| libc::EIO)?;

        let plain_total =
            CryptoEngine::calculate_plain_size_v3_checked(raw_len).map_err(|_| libc::EIO)?;
        let total_chunks =
            CryptoEngine::total_chunks_v3_checked(raw_len).map_err(|_| libc::EIO)?;

        if plain_total == 0 || offset >= plain_total {
            return Ok(Vec::new());
        }

        let bytes_to_read = std::cmp::min(size as u64, plain_total - offset) as usize;
        let plain_end = offset + bytes_to_read as u64;

        let start_chunk = CryptoEngine::plain_offset_to_chunk_index_v3(offset);
        let end_chunk = CryptoEngine::plain_offset_to_chunk_index_v3(plain_end - 1);

        let mut output = Vec::with_capacity(bytes_to_read);

        for chunk_idx in start_chunk..=end_chunk {
            let expected_is_last = (chunk_idx + 1) == total_chunks;
            let cipher_offset = CryptoEngine::chunk_offset_to_cipher_offset_v3(chunk_idx);
            if cipher_offset >= raw_len {
                break;
            }

            let available_cipher = (raw_len - cipher_offset) as usize;
            let cipher_len = std::cmp::min(available_cipher, CHUNK_DISK_SIZE_V3);
            if cipher_len < CHUNK_OVERHEAD_V3 {
                return Err(libc::EIO);
            }

            let mut cipher_buf = vec![0u8; cipher_len];
            file.seek(SeekFrom::Start(cipher_offset))
                .map_err(|e| io_err_to_errno(&e))?;
            file.read_exact(&mut cipher_buf)
                .map_err(|e| io_err_to_errno(&e))?;

            let plain_chunk = self
                .engine
                .decrypt_chunk_v3(&file_id, chunk_idx, expected_is_last, &cipher_buf)
                .map_err(|_| libc::EIO)?;

            let chunk_plain_start = chunk_idx * (CHUNK_PLAIN_SIZE as u64);
            let slice_start = std::cmp::max(offset, chunk_plain_start) - chunk_plain_start;
            let slice_end = std::cmp::min(plain_end, chunk_plain_start + plain_chunk.len() as u64)
                - chunk_plain_start;

            if slice_start < slice_end && (slice_end as usize) <= plain_chunk.len() {
                output.extend_from_slice(&plain_chunk[slice_start as usize..slice_end as usize]);
            }
        }

        Ok(output)
    }

    /// Decrypts a chunked file range from RAW_MOUNT_PATH
    pub fn read_payload(&self, raw_path: &Path, offset: u64, size: usize) -> Result<Vec<u8>, i32> {
        let mut file = File::open(raw_path).map_err(|e| io_err_to_errno(&e))?;
        self.read_payload_from_file(&mut file, offset, size)
    }

    /// Ensures the file is in Format v3, upgrading from v2 if necessary.
    fn ensure_v3_file(&self, file: &mut File, raw_path: &Path) -> Result<(), i32> {
        ensure_v3_file_direct(&self.engine, file, raw_path)
    }

    /// Encrypts and writes chunked data to an open File handle in Format v3
    fn write_payload_to_file(
        &self,
        file: &mut File,
        raw_path: &Path,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, i32> {
        if data.is_empty() {
            return Ok(0);
        }

        // 1. Upgrade from v2 or initialize v3 header if needed
        self.ensure_v3_file(file, raw_path)?;

        let mut raw_len = file.metadata().map_err(|e| io_err_to_errno(&e))?.len();
        if raw_len < HEADER_SIZE_V3 as u64 {
            return Err(libc::EIO);
        }

        // 2. Read and verify 64-byte v3 header
        let mut hdr = [0u8; HEADER_SIZE_V3];
        file.seek(SeekFrom::Start(0))
            .map_err(|e| io_err_to_errno(&e))?;
        file.read_exact(&mut hdr)
            .map_err(|e| io_err_to_errno(&e))?;
        let file_id = self.engine.verify_v3_header(&hdr).map_err(|_| libc::EIO)?;

        // 3. Current state
        let old_plain_len =
            CryptoEngine::calculate_plain_size_v3_checked(raw_len).map_err(|_| libc::EIO)?;
        let old_total_chunks =
            CryptoEngine::total_chunks_v3_checked(raw_len).map_err(|_| libc::EIO)?;

        // 4. Write range
        let plain_start = offset;
        let plain_end = offset + data.len() as u64;
        let new_plain_len = std::cmp::max(old_plain_len, plain_end);
        let new_total_chunks = if new_plain_len == 0 {
            0
        } else {
            (new_plain_len - 1) / (CHUNK_PLAIN_SIZE as u64) + 1
        };
        let write_start_chunk = plain_start / (CHUNK_PLAIN_SIZE as u64);
        let write_end_chunk = (plain_end - 1) / (CHUNK_PLAIN_SIZE as u64);

        // 5. Precondition verification (Atomic error protection - C2):
        // Decrypt and verify existing chunks that will be read or updated before writing any byte.
        let mut pre_decrypted: HashMap<u64, Vec<u8>> = HashMap::new();

        let check_old_last = old_total_chunks > 0
            && new_total_chunks > old_total_chunks
            && (old_total_chunks - 1) < write_start_chunk;

        if check_old_last {
            let old_last = old_total_chunks - 1;
            let cipher_offset = CryptoEngine::chunk_offset_to_cipher_offset_v3(old_last);
            let avail = (raw_len - cipher_offset) as usize;
            let cipher_len = std::cmp::min(avail, CHUNK_DISK_SIZE_V3);
            if cipher_len < CHUNK_OVERHEAD_V3 {
                return Err(libc::EIO);
            }
            let mut cipher_buf = vec![0u8; cipher_len];
            file.seek(SeekFrom::Start(cipher_offset))
                .map_err(|e| io_err_to_errno(&e))?;
            file.read_exact(&mut cipher_buf)
                .map_err(|e| io_err_to_errno(&e))?;

            let plain = self
                .engine
                .decrypt_chunk_v3(&file_id, old_last, true, &cipher_buf)
                .map_err(|_| libc::EIO)?;
            pre_decrypted.insert(old_last, plain);
        }

        for chunk_idx in write_start_chunk..=write_end_chunk {
            if chunk_idx < old_total_chunks && !pre_decrypted.contains_key(&chunk_idx) {
                let cipher_offset = CryptoEngine::chunk_offset_to_cipher_offset_v3(chunk_idx);
                let avail = (raw_len - cipher_offset) as usize;
                let cipher_len = std::cmp::min(avail, CHUNK_DISK_SIZE_V3);
                if cipher_len < CHUNK_OVERHEAD_V3 {
                    return Err(libc::EIO);
                }
                let mut cipher_buf = vec![0u8; cipher_len];
                file.seek(SeekFrom::Start(cipher_offset))
                    .map_err(|e| io_err_to_errno(&e))?;
                file.read_exact(&mut cipher_buf)
                    .map_err(|e| io_err_to_errno(&e))?;

                let expected_is_last = (chunk_idx + 1) == old_total_chunks;
                let plain = self
                    .engine
                    .decrypt_chunk_v3(&file_id, chunk_idx, expected_is_last, &cipher_buf)
                    .map_err(|_| libc::EIO)?;
                pre_decrypted.insert(chunk_idx, plain);
            }
        }

        // 6. Critical Step A: Update old last chunk if needed (pad to 64K, is_last = false, fresh nonce)
        if check_old_last {
            let old_last = old_total_chunks - 1;
            let mut plain = pre_decrypted.remove(&old_last).unwrap();
            if plain.len() < CHUNK_PLAIN_SIZE {
                plain.resize(CHUNK_PLAIN_SIZE, 0u8);
            }
            let cipher_chunk = self
                .engine
                .encrypt_chunk_v3(&file_id, old_last, false, &plain)
                .map_err(|_| libc::EIO)?;

            let cipher_offset = CryptoEngine::chunk_offset_to_cipher_offset_v3(old_last);
            file.seek(SeekFrom::Start(cipher_offset))
                .map_err(|e| io_err_to_errno(&e))?;
            file.write_all(&cipher_chunk)
                .map_err(|e| io_err_to_errno(&e))?;
        }

        // 7. Critical Step B: Fill sparse gap chunks with zeros (is_last = false, fresh nonce)
        if write_start_chunk > old_total_chunks {
            let zeros = vec![0u8; CHUNK_PLAIN_SIZE];
            for gap_idx in old_total_chunks..write_start_chunk {
                let cipher_chunk = self
                    .engine
                    .encrypt_chunk_v3(&file_id, gap_idx, false, &zeros)
                    .map_err(|_| libc::EIO)?;

                let cipher_offset = CryptoEngine::chunk_offset_to_cipher_offset_v3(gap_idx);
                file.seek(SeekFrom::Start(cipher_offset))
                    .map_err(|e| io_err_to_errno(&e))?;
                file.write_all(&cipher_chunk)
                    .map_err(|e| io_err_to_errno(&e))?;
            }
        }

        // 8. Critical Step C: Write target chunks
        for chunk_idx in write_start_chunk..=write_end_chunk {
            let chunk_plain_start = chunk_idx * (CHUNK_PLAIN_SIZE as u64);
            let chunk_plain_end = chunk_plain_start + (CHUNK_PLAIN_SIZE as u64);

            let in_chunk_start = std::cmp::max(plain_start, chunk_plain_start) - chunk_plain_start;
            let in_chunk_end = std::cmp::min(plain_end, chunk_plain_end) - chunk_plain_start;

            let data_src_start = (chunk_plain_start + in_chunk_start - plain_start) as usize;
            let data_src_end = (chunk_plain_start + in_chunk_end - plain_start) as usize;

            let mut plain_chunk = pre_decrypted.remove(&chunk_idx).unwrap_or_default();

            let is_this_last = (chunk_idx + 1) == new_total_chunks;
            if !is_this_last {
                if plain_chunk.len() < CHUNK_PLAIN_SIZE {
                    plain_chunk.resize(CHUNK_PLAIN_SIZE, 0u8);
                }
            } else {
                let target_len = std::cmp::max(plain_chunk.len(), in_chunk_end as usize);
                if plain_chunk.len() < target_len {
                    plain_chunk.resize(target_len, 0u8);
                }
            }

            plain_chunk[in_chunk_start as usize..in_chunk_end as usize]
                .copy_from_slice(&data[data_src_start..data_src_end]);

            let cipher_chunk = self
                .engine
                .encrypt_chunk_v3(&file_id, chunk_idx, is_this_last, &plain_chunk)
                .map_err(|_| libc::EIO)?;

            let cipher_offset = CryptoEngine::chunk_offset_to_cipher_offset_v3(chunk_idx);
            file.seek(SeekFrom::Start(cipher_offset))
                .map_err(|e| io_err_to_errno(&e))?;
            file.write_all(&cipher_chunk)
                .map_err(|e| io_err_to_errno(&e))?;

            let new_end = cipher_offset + cipher_chunk.len() as u64;
            if new_end > raw_len {
                raw_len = new_end;
            }
        }

        Ok(data.len())
    }

    /// Encrypts and writes chunked data to RAW_MOUNT_PATH in Format v3
    pub fn write_payload(&self, raw_path: &Path, offset: u64, data: &[u8]) -> Result<usize, i32> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(raw_path)
            .map_err(|e| io_err_to_errno(&e))?;
        self.write_payload_to_file(&mut file, raw_path, offset, data)
    }

    /// Truncates an open file to the desired plaintext length in Format v3
    fn truncate_file_inner(
        &self,
        file: &mut File,
        raw_path: &Path,
        new_plain_size: u64,
    ) -> Result<(), i32> {
        self.ensure_v3_file(file, raw_path)?;

        if new_plain_size == 0 {
            let (fresh_header, _) = self.engine.create_v3_header();
            file.seek(SeekFrom::Start(0))
                .map_err(|e| io_err_to_errno(&e))?;
            file.write_all(&fresh_header)
                .map_err(|e| io_err_to_errno(&e))?;
            file.set_len(HEADER_SIZE_V3 as u64)
                .map_err(|e| io_err_to_errno(&e))?;
            return Ok(());
        }

        let raw_len = file.metadata().map_err(|e| io_err_to_errno(&e))?.len();
        if raw_len < HEADER_SIZE_V3 as u64 {
            return Err(libc::EIO);
        }

        let mut hdr = [0u8; HEADER_SIZE_V3];
        file.seek(SeekFrom::Start(0))
            .map_err(|e| io_err_to_errno(&e))?;
        file.read_exact(&mut hdr)
            .map_err(|e| io_err_to_errno(&e))?;
        let file_id = self.engine.verify_v3_header(&hdr).map_err(|_| libc::EIO)?;

        let old_plain_len =
            CryptoEngine::calculate_plain_size_v3_checked(raw_len).map_err(|_| libc::EIO)?;
        let old_total_chunks =
            CryptoEngine::total_chunks_v3_checked(raw_len).map_err(|_| libc::EIO)?;

        if new_plain_size == old_plain_len {
            return Ok(());
        }

        let new_total_chunks = (new_plain_size - 1) / (CHUNK_PLAIN_SIZE as u64) + 1;
        let new_last_idx = new_total_chunks - 1;
        let len_in_last = ((new_plain_size - 1) % (CHUNK_PLAIN_SIZE as u64)) as usize + 1;

        if new_plain_size < old_plain_len {
            // Truncate down (shrinking)
            let cipher_offset = CryptoEngine::chunk_offset_to_cipher_offset_v3(new_last_idx);
            let avail = (raw_len - cipher_offset) as usize;
            let cipher_len = std::cmp::min(avail, CHUNK_DISK_SIZE_V3);
            if cipher_len < CHUNK_OVERHEAD_V3 {
                return Err(libc::EIO);
            }
            let mut cipher_buf = vec![0u8; cipher_len];
            file.seek(SeekFrom::Start(cipher_offset))
                .map_err(|e| io_err_to_errno(&e))?;
            file.read_exact(&mut cipher_buf)
                .map_err(|e| io_err_to_errno(&e))?;

            let old_is_last = (new_last_idx + 1) == old_total_chunks;
            let mut plain_chunk = self
                .engine
                .decrypt_chunk_v3(&file_id, new_last_idx, old_is_last, &cipher_buf)
                .map_err(|_| libc::EIO)?;

            plain_chunk.truncate(len_in_last);

            let cipher_chunk = self
                .engine
                .encrypt_chunk_v3(&file_id, new_last_idx, true, &plain_chunk)
                .map_err(|_| libc::EIO)?;

            file.seek(SeekFrom::Start(cipher_offset))
                .map_err(|e| io_err_to_errno(&e))?;
            file.write_all(&cipher_chunk)
                .map_err(|e| io_err_to_errno(&e))?;
            file.set_len(cipher_offset + cipher_chunk.len() as u64)
                .map_err(|e| io_err_to_errno(&e))?;
        } else {
            // Truncate up (extending)
            if old_total_chunks == 0 {
                // Expanding from empty file (0 plaintext bytes)
                let zeros = vec![0u8; CHUNK_PLAIN_SIZE];
                for gap_idx in 0..new_last_idx {
                    let cipher_chunk = self
                        .engine
                        .encrypt_chunk_v3(&file_id, gap_idx, false, &zeros)
                        .map_err(|_| libc::EIO)?;
                    let cipher_offset = CryptoEngine::chunk_offset_to_cipher_offset_v3(gap_idx);
                    file.seek(SeekFrom::Start(cipher_offset))
                        .map_err(|e| io_err_to_errno(&e))?;
                    file.write_all(&cipher_chunk)
                        .map_err(|e| io_err_to_errno(&e))?;
                }

                let last_zeros = vec![0u8; len_in_last];
                let cipher_chunk = self
                    .engine
                    .encrypt_chunk_v3(&file_id, new_last_idx, true, &last_zeros)
                    .map_err(|_| libc::EIO)?;
                let cipher_offset = CryptoEngine::chunk_offset_to_cipher_offset_v3(new_last_idx);
                file.seek(SeekFrom::Start(cipher_offset))
                    .map_err(|e| io_err_to_errno(&e))?;
                file.write_all(&cipher_chunk)
                    .map_err(|e| io_err_to_errno(&e))?;
                file.set_len(cipher_offset + cipher_chunk.len() as u64)
                    .map_err(|e| io_err_to_errno(&e))?;
            } else {
                let old_last_idx = old_total_chunks - 1;
                // Pre-verify old_last_idx chunk first before changing anything!
                let old_cipher_offset =
                    CryptoEngine::chunk_offset_to_cipher_offset_v3(old_last_idx);
                let avail = (raw_len - old_cipher_offset) as usize;
                let cipher_len = std::cmp::min(avail, CHUNK_DISK_SIZE_V3);
                if cipher_len < CHUNK_OVERHEAD_V3 {
                    return Err(libc::EIO);
                }
                let mut cipher_buf = vec![0u8; cipher_len];
                file.seek(SeekFrom::Start(old_cipher_offset))
                    .map_err(|e| io_err_to_errno(&e))?;
                file.read_exact(&mut cipher_buf)
                    .map_err(|e| io_err_to_errno(&e))?;

                let mut old_last_plain = self
                    .engine
                    .decrypt_chunk_v3(&file_id, old_last_idx, true, &cipher_buf)
                    .map_err(|_| libc::EIO)?;

                if new_last_idx == old_last_idx {
                    old_last_plain.resize(len_in_last, 0u8);
                    let cipher_chunk = self
                        .engine
                        .encrypt_chunk_v3(&file_id, old_last_idx, true, &old_last_plain)
                        .map_err(|_| libc::EIO)?;

                    file.seek(SeekFrom::Start(old_cipher_offset))
                        .map_err(|e| io_err_to_errno(&e))?;
                    file.write_all(&cipher_chunk)
                        .map_err(|e| io_err_to_errno(&e))?;
                    file.set_len(old_cipher_offset + cipher_chunk.len() as u64)
                        .map_err(|e| io_err_to_errno(&e))?;
                } else {
                    // Update old last chunk to is_last = false and 65536 bytes
                    old_last_plain.resize(CHUNK_PLAIN_SIZE, 0u8);
                    let cipher_chunk = self
                        .engine
                        .encrypt_chunk_v3(&file_id, old_last_idx, false, &old_last_plain)
                        .map_err(|_| libc::EIO)?;

                    file.seek(SeekFrom::Start(old_cipher_offset))
                        .map_err(|e| io_err_to_errno(&e))?;
                    file.write_all(&cipher_chunk)
                        .map_err(|e| io_err_to_errno(&e))?;

                    // Fill middle gap chunks
                    let zeros = vec![0u8; CHUNK_PLAIN_SIZE];
                    for gap_idx in (old_last_idx + 1)..new_last_idx {
                        let cipher_chunk = self
                            .engine
                            .encrypt_chunk_v3(&file_id, gap_idx, false, &zeros)
                            .map_err(|_| libc::EIO)?;
                        let cipher_offset =
                            CryptoEngine::chunk_offset_to_cipher_offset_v3(gap_idx);
                        file.seek(SeekFrom::Start(cipher_offset))
                            .map_err(|e| io_err_to_errno(&e))?;
                        file.write_all(&cipher_chunk)
                            .map_err(|e| io_err_to_errno(&e))?;
                    }

                    // Write new last chunk
                    let last_zeros = vec![0u8; len_in_last];
                    let cipher_chunk = self
                        .engine
                        .encrypt_chunk_v3(&file_id, new_last_idx, true, &last_zeros)
                        .map_err(|_| libc::EIO)?;
                    let cipher_offset =
                        CryptoEngine::chunk_offset_to_cipher_offset_v3(new_last_idx);
                    file.seek(SeekFrom::Start(cipher_offset))
                        .map_err(|e| io_err_to_errno(&e))?;
                    file.write_all(&cipher_chunk)
                        .map_err(|e| io_err_to_errno(&e))?;
                    file.set_len(cipher_offset + cipher_chunk.len() as u64)
                        .map_err(|e| io_err_to_errno(&e))?;
                }
            }
        }

        Ok(())
    }

    /// Truncates a file to the desired plaintext length in Format v3
    pub fn truncate_file(&self, raw_path: &Path, new_plain_size: u64) -> Result<(), i32> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(raw_path)
            .map_err(|e| io_err_to_errno(&e))?;
        self.truncate_file_inner(&mut file, raw_path, new_plain_size)
    }

    fn do_lookup(&self, parent: u64, name: &str, guard: ReplyGuard<ReplyEntry>) {
        let parent_path = match self.inodes.read().get_path(parent) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let child_path = parent_path.join(name);
        let raw_child = match self.to_raw_path(&child_path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        match fs::symlink_metadata(&raw_child) {
            Ok(meta) => {
                let ino = self.inodes.write().get_or_insert_ino(child_path);
                let attr = self.build_file_attr(ino, &raw_child, &meta);
                if let Some(r) = guard.take() {
                    r.entry(&TTL, &attr, 0);
                }
            }
            Err(err) => {
                let errno = io_err_to_errno(&err);
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
            }
        }
    }

    fn do_getattr(&self, ino: u64, guard: ReplyGuard<ReplyAttr>) {
        let path = match self.inodes.read().get_path(ino) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let raw_path = match self.to_raw_path(&path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        match fs::symlink_metadata(&raw_path) {
            Ok(meta) => {
                let attr = self.build_file_attr(ino, &raw_path, &meta);
                if let Some(r) = guard.take() {
                    r.attr(&TTL, &attr);
                }
            }
            Err(err) => {
                let errno = io_err_to_errno(&err);
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn do_setattr(
        &self,
        ino: u64,
        fh: Option<u64>,
        mode: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        guard: ReplyGuard<ReplyAttr>,
    ) {
        let path = match self.inodes.read().get_path(ino) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let raw_path = match self.to_raw_path(&path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        self.inode_locks.with_locks(&[ino], || {
            if let Some(m) = mode {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&raw_path, fs::Permissions::from_mode(m));
                }
            }

            if atime.is_some() || mtime.is_some() {
                let times = [
                    time_or_now_to_timespec(atime),
                    time_or_now_to_timespec(mtime),
                ];
                if let Ok(c_raw_path) = std::ffi::CString::new(raw_path.as_os_str().as_bytes()) {
                    unsafe {
                        libc::utimensat(
                            libc::AT_FDCWD,
                            c_raw_path.as_ptr(),
                            times.as_ptr(),
                            libc::AT_SYMLINK_NOFOLLOW,
                        );
                    }
                }
            }

            if let Some(target_size) = size {
                let res = if let Some(fh_val) = fh.filter(|&v| v != 0) {
                    if let Some(handle) = self.handles.get(fh_val) {
                        let mut file = handle.lock();
                        self.truncate_file_inner(&mut file, &raw_path, target_size)
                    } else {
                        self.truncate_file(&raw_path, target_size)
                    }
                } else {
                    self.truncate_file(&raw_path, target_size)
                };

                if let Err(e) = res {
                    if let Some(r) = guard.take() {
                        r.error(e);
                    }
                    return;
                }
            }

            match fs::symlink_metadata(&raw_path) {
                Ok(meta) => {
                    let attr = self.build_file_attr(ino, &raw_path, &meta);
                    if let Some(r) = guard.take() {
                        r.attr(&TTL, &attr);
                    }
                }
                Err(err) => {
                    let errno = io_err_to_errno(&err);
                    if let Some(r) = guard.take() {
                        r.error(errno);
                    }
                }
            }
        });
    }

    fn do_readdir(&self, ino: u64, offset: i64, guard: ReplyGuard<ReplyDirectory>) {
        let dir_path = match self.inodes.read().get_path(ino) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let raw_dir = match self.to_raw_path(&dir_path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        let dir_id = match self.get_or_create_dir_id(&raw_dir, &dir_path, true) {
            Ok(id) => id,
            Err(errno) => {
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }
        };

        let entries = match fs::read_dir(&raw_dir) {
            Ok(e) => e,
            Err(err) => {
                let errno = io_err_to_errno(&err);
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }
        };

        let mut collected: Vec<(u64, FileType, String)> = Vec::new();
        for entry_res in entries {
            let entry = match entry_res {
                Ok(e) => e,
                Err(err) => {
                    let errno = io_err_to_errno(&err);
                    if let Some(r) = guard.take() {
                        r.error(errno);
                    }
                    return;
                }
            };

            let raw_file_name = match entry.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue,
            };

            if raw_file_name == DIRIV_FILENAME || is_longname_sidecar(&raw_file_name) {
                continue;
            }

            let plain_file_name = if is_longname_file(&raw_file_name) {
                let sidecar_path = raw_dir.join(longname_sidecar_for(&raw_file_name));
                let sidecar_content = match fs::read_to_string(&sidecar_path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let enc_name = sidecar_content.trim();
                if hash_long_cipher_name(enc_name) != raw_file_name {
                    continue;
                }
                match decrypt_filename(&self.engine, enc_name, Some(&dir_id)) {
                    Ok(name) => name,
                    Err(_) => continue,
                }
            } else {
                match decrypt_filename(&self.engine, &raw_file_name, Some(&dir_id)) {
                    Ok(name) => name,
                    Err(_) => continue,
                }
            };

            if plain_file_name == DIRIV_FILENAME {
                continue;
            }

            let child_rel_path = dir_path.join(&plain_file_name);
            let child_ino = self.inodes.write().get_or_insert_ino(child_rel_path);

            let ft = match entry.file_type() {
                Ok(t) if t.is_dir() => FileType::Directory,
                Ok(t) if t.is_symlink() => FileType::Symlink,
                _ => FileType::RegularFile,
            };

            collected.push((child_ino, ft, plain_file_name));
        }

        let mut reply = match guard.take() {
            Some(r) => r,
            None => return,
        };

        let mut index: i64 = 1;

        if offset < index && reply.add(ino, index, FileType::Directory, ".") {
            reply.ok();
            return;
        }
        index += 1;

        if offset < index {
            let parent_ino = FUSE_ROOT_ID;
            if reply.add(parent_ino, index, FileType::Directory, "..") {
                reply.ok();
                return;
            }
        }
        index += 1;

        for (child_ino, ft, plain_file_name) in collected {
            if offset < index && reply.add(child_ino, index, ft, &plain_file_name) {
                break;
            }
            index += 1;
        }

        reply.ok();
    }

    fn do_open(&self, ino: u64, _flags: i32, guard: ReplyGuard<ReplyOpen>) {
        let path = match self.inodes.read().get_path(ino) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let raw_path = match self.to_raw_path(&path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        let file = match OpenOptions::new().read(true).write(true).open(&raw_path) {
            Ok(f) => f,
            Err(_) => match OpenOptions::new().read(true).open(&raw_path) {
                Ok(f) => f,
                Err(err) => {
                    let errno = io_err_to_errno(&err);
                    if let Some(r) = guard.take() {
                        r.error(errno);
                    }
                    return;
                }
            },
        };

        let fh = self.handles.insert(file);
        if let Some(r) = guard.take() {
            r.opened(fh, 0);
        }
    }

    fn do_read(&self, ino: u64, fh: u64, offset: u64, size: usize, guard: ReplyGuard<ReplyData>) {
        let path = match self.inodes.read().get_path(ino) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let raw_path = match self.to_raw_path(&path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        self.inode_locks.with_locks(&[ino], || {
            let result = if fh != 0 {
                if let Some(handle) = self.handles.get(fh) {
                    let mut file = handle.lock();
                    self.read_payload_from_file(&mut file, offset, size)
                } else {
                    self.read_payload(&raw_path, offset, size)
                }
            } else {
                self.read_payload(&raw_path, offset, size)
            };

            match result {
                Ok(data) => {
                    if let Some(r) = guard.take() {
                        r.data(&data);
                    }
                }
                Err(e) => {
                    if let Some(r) = guard.take() {
                        r.error(e);
                    }
                }
            }
        });
    }

    fn do_write(
        &self,
        ino: u64,
        fh: u64,
        offset: u64,
        data: Vec<u8>,
        guard: ReplyGuard<ReplyWrite>,
    ) {
        let path = match self.inodes.read().get_path(ino) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let raw_path = match self.to_raw_path(&path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        self.inode_locks.with_locks(&[ino], || {
            let result = if fh != 0 {
                if let Some(handle) = self.handles.get(fh) {
                    let mut file = handle.lock();
                    self.write_payload_to_file(&mut file, &raw_path, offset, &data)
                } else {
                    self.write_payload(&raw_path, offset, &data)
                }
            } else {
                self.write_payload(&raw_path, offset, &data)
            };

            match result {
                Ok(written) => {
                    if let Some(r) = guard.take() {
                        r.written(written as u32);
                    }
                }
                Err(e) => {
                    if let Some(r) = guard.take() {
                        r.error(e);
                    }
                }
            }
        });
    }

    fn do_create(&self, parent: u64, name: &str, guard: ReplyGuard<ReplyCreate>) {
        let parent_path = match self.inodes.read().get_path(parent) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let child_path = parent_path.join(name);
        let raw_child = match self.to_raw_path(&child_path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        self.inode_locks.with_locks(&[parent], || {
            let (v3_header, _) = self.engine.create_v3_header();
            let mut file = match OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&raw_child)
            {
                Ok(f) => f,
                Err(err) => {
                    let errno = io_err_to_errno(&err);
                    if let Some(r) = guard.take() {
                        r.error(errno);
                    }
                    return;
                }
            };

            if let Err(err) = file.write_all(&v3_header) {
                let errno = io_err_to_errno(&err);
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }

            let raw_parent = match raw_child.parent() {
                Some(p) => p,
                None => &self.raw_root,
            };
            if let Err(errno) = self.write_longname_sidecar_if_needed(raw_parent, &parent_path, name) {
                let _ = fs::remove_file(&raw_child);
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }

            let meta = match fs::symlink_metadata(&raw_child) {
                Ok(m) => m,
                Err(err) => {
                    let errno = io_err_to_errno(&err);
                    if let Some(r) = guard.take() {
                        r.error(errno);
                    }
                    return;
                }
            };

            let ino = self.inodes.write().get_or_insert_ino(child_path);
            let attr = self.build_file_attr(ino, &raw_child, &meta);
            let fh = self.handles.insert(file);

            if let Some(r) = guard.take() {
                r.created(&TTL, &attr, 0, fh, 0);
            }
        });
    }

    fn do_mkdir(&self, parent: u64, name: &str, guard: ReplyGuard<ReplyEntry>) {
        let parent_path = match self.inodes.read().get_path(parent) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let child_path = parent_path.join(name);
        let raw_child = match self.to_raw_path(&child_path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        self.inode_locks.with_locks(&[parent], || {
            if let Err(err) = fs::create_dir_all(&raw_child) {
                let errno = io_err_to_errno(&err);
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }

            if let Err(errno) = self.get_or_create_dir_id(&raw_child, &child_path, true) {
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }

            let raw_parent = match raw_child.parent() {
                Some(p) => p,
                None => &self.raw_root,
            };
            if let Err(errno) = self.write_longname_sidecar_if_needed(raw_parent, &parent_path, name) {
                let _ = fs::remove_dir(&raw_child);
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }

            let meta = match fs::symlink_metadata(&raw_child) {
                Ok(m) => m,
                Err(err) => {
                    let errno = io_err_to_errno(&err);
                    if let Some(r) = guard.take() {
                        r.error(errno);
                    }
                    return;
                }
            };

            let ino = self.inodes.write().get_or_insert_ino(child_path);
            let attr = self.build_file_attr(ino, &raw_child, &meta);
            if let Some(r) = guard.take() {
                r.entry(&TTL, &attr, 0);
            }
        });
    }

    fn do_unlink(&self, parent: u64, name: &str, guard: ReplyGuard<ReplyEmpty>) {
        let parent_path = match self.inodes.read().get_path(parent) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let child_path = parent_path.join(name);
        let raw_child = match self.to_raw_path(&child_path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        self.inode_locks.with_locks(&[parent], || {
            if let Err(err) = fs::remove_file(&raw_child) {
                let errno = io_err_to_errno(&err);
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }

            self.remove_longname_sidecar_if_any(&raw_child);
            self.inodes.write().remove(&child_path);
            if let Some(r) = guard.take() {
                r.ok();
            }
        });
    }

    fn do_rmdir(&self, parent: u64, name: &str, guard: ReplyGuard<ReplyEmpty>) {
        let parent_path = match self.inodes.read().get_path(parent) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let child_path = parent_path.join(name);
        let raw_child = match self.to_raw_path(&child_path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        self.inode_locks.with_locks(&[parent], || {
            let read_entries = match fs::read_dir(&raw_child) {
                Ok(e) => e,
                Err(err) => {
                    let errno = io_err_to_errno(&err);
                    if let Some(r) = guard.take() {
                        r.error(errno);
                    }
                    return;
                }
            };

            let mut meta_files = Vec::new();
            let mut has_user_files = false;

            for entry_res in read_entries {
                let entry = match entry_res {
                    Ok(e) => e,
                    Err(err) => {
                        let errno = io_err_to_errno(&err);
                        if let Some(r) = guard.take() {
                            r.error(errno);
                        }
                        return;
                    }
                };

                let file_name = entry.file_name();
                let file_name_str = file_name.to_string_lossy();
                if file_name_str == DIRIV_FILENAME
                    || file_name_str == ".nomedia"
                    || file_name_str == ".directory"
                    || is_longname_sidecar(&file_name_str)
                {
                    meta_files.push(entry.path());
                } else {
                    has_user_files = true;
                    break;
                }
            }

            if has_user_files {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOTEMPTY);
                }
                return;
            }

            for meta in meta_files {
                let _ = fs::remove_file(meta);
            }

            if let Err(err) = fs::remove_dir(&raw_child) {
                let errno = io_err_to_errno(&err);
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }

            self.remove_longname_sidecar_if_any(&raw_child);
            self.dir_ids.write().retain(|k, _| !k.starts_with(&child_path));
            self.inodes.write().remove(&child_path);
            if let Some(r) = guard.take() {
                r.ok();
            }
        });
    }

    fn do_rename(
        &self,
        parent: u64,
        name: &str,
        newparent: u64,
        newname: &str,
        guard: ReplyGuard<ReplyEmpty>,
    ) {
        let parent_path = match self.inodes.read().get_path(parent) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };
        let newparent_path = match self.inodes.read().get_path(newparent) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let old_child = parent_path.join(name);
        let new_child = newparent_path.join(newname);

        if old_child == new_child {
            if let Some(r) = guard.take() {
                r.ok();
            }
            return;
        }

        let raw_old = match self.to_raw_path(&old_child) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };
        let raw_new = match self.to_raw_path(&new_child) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        self.inode_locks.with_locks(&[parent, newparent], || {
            if let Err(err) = fs::rename(&raw_old, &raw_new) {
                let errno = io_err_to_errno(&err);
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }

            self.remove_longname_sidecar_if_any(&raw_old);
            let raw_new_parent = match raw_new.parent() {
                Some(p) => p,
                None => &self.raw_root,
            };
            let _ = self.write_longname_sidecar_if_needed(raw_new_parent, &newparent_path, newname);

            self.inodes.write().rename(&old_child, new_child.clone());
            {
                let mut dir_ids = self.dir_ids.write();
                let keys_to_update: Vec<(PathBuf, String)> = dir_ids
                    .iter()
                    .filter(|(k, _)| k.starts_with(&old_child))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();

                for (old_k, val) in keys_to_update {
                    dir_ids.remove(&old_k);
                    if let Ok(suffix) = old_k.strip_prefix(&old_child) {
                        let new_k = new_child.join(suffix);
                        dir_ids.insert(new_k, val);
                    }
                }
            }

            if let Some(r) = guard.take() {
                r.ok();
            }
        });
    }

    fn do_statfs(&self, guard: ReplyGuard<ReplyStatfs>) {
        let c_path = match std::ffi::CString::new(self.raw_root.as_os_str().as_bytes()) {
            Ok(c) => c,
            Err(_) => {
                if let Some(r) = guard.take() {
                    r.error(libc::EIO);
                }
                return;
            }
        };
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
        if ret == 0 {
            if let Some(r) = guard.take() {
                r.statfs(
                    stat.f_blocks,
                    stat.f_bfree,
                    stat.f_bavail,
                    stat.f_files,
                    stat.f_ffree,
                    stat.f_bsize as u32,
                    255,
                    stat.f_frsize as u32,
                );
            }
        } else {
            if let Some(r) = guard.take() {
                r.error(libc::EIO);
            }
        }
    }

    fn do_symlink(
        &self,
        parent: u64,
        link_name: &str,
        target: &Path,
        guard: ReplyGuard<ReplyEntry>,
    ) {
        let parent_path = match self.inodes.read().get_path(parent) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let target_str = match target.to_str() {
            Some(s) => s,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::EINVAL);
                }
                return;
            }
        };

        let child_path = parent_path.join(link_name);
        let raw_child = match self.to_raw_path(&child_path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        let raw_parent = match raw_child.parent() {
            Some(p) => p,
            None => &self.raw_root,
        };

        self.inode_locks.with_locks(&[parent], || {
            let parent_dir_id = match self.get_or_create_dir_id(raw_parent, &parent_path, true) {
                Ok(id) => id,
                Err(e) => {
                    if let Some(r) = guard.take() {
                        r.error(e);
                    }
                    return;
                }
            };

            let enc_target = match encrypt_filename(&self.engine, target_str, Some(&parent_dir_id)) {
                Ok(t) => t,
                Err(_) => {
                    if let Some(r) = guard.take() {
                        r.error(libc::EIO);
                    }
                    return;
                }
            };

            #[cfg(unix)]
            {
                if let Err(err) = std::os::unix::fs::symlink(&enc_target, &raw_child) {
                    let errno = io_err_to_errno(&err);
                    if let Some(r) = guard.take() {
                        r.error(errno);
                    }
                    return;
                }
            }
            #[cfg(not(unix))]
            {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOSYS);
                }
                return;
            }

            if let Err(errno) = self.write_longname_sidecar_if_needed(raw_parent, &parent_path, link_name) {
                let _ = fs::remove_file(&raw_child);
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }

            let meta = match fs::symlink_metadata(&raw_child) {
                Ok(m) => m,
                Err(err) => {
                    let errno = io_err_to_errno(&err);
                    if let Some(r) = guard.take() {
                        r.error(errno);
                    }
                    return;
                }
            };

            let ino = self.inodes.write().get_or_insert_ino(child_path);
            let attr = self.build_file_attr(ino, &raw_child, &meta);
            if let Some(r) = guard.take() {
                r.entry(&TTL, &attr, 0);
            }
        });
    }

    fn do_readlink(&self, ino: u64, guard: ReplyGuard<ReplyData>) {
        let path = match self.inodes.read().get_path(ino) {
            Some(p) => p,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::ENOENT);
                }
                return;
            }
        };

        let parent_path = path.parent().unwrap_or_else(|| Path::new(""));

        let raw_path = match self.to_raw_path(&path) {
            Ok(p) => p,
            Err(e) => {
                if let Some(r) = guard.take() {
                    r.error(e);
                }
                return;
            }
        };

        let raw_parent = match raw_path.parent() {
            Some(p) => p,
            None => &self.raw_root,
        };

        let enc_target_path = match fs::read_link(&raw_path) {
            Ok(t) => t,
            Err(err) => {
                let errno = io_err_to_errno(&err);
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }
        };

        let enc_target_str = match enc_target_path.to_str() {
            Some(s) => s,
            None => {
                if let Some(r) = guard.take() {
                    r.error(libc::EIO);
                }
                return;
            }
        };

        let parent_dir_id = match self.get_or_create_dir_id(raw_parent, parent_path, false) {
            Ok(id) => id,
            Err(errno) => {
                if let Some(r) = guard.take() {
                    r.error(errno);
                }
                return;
            }
        };

        let plain_target = match decrypt_filename(&self.engine, enc_target_str, Some(&parent_dir_id)) {
            Ok(s) => s,
            Err(_) => {
                if let Some(r) = guard.take() {
                    r.error(libc::EIO);
                }
                return;
            }
        };

        if let Some(r) = guard.take() {
            r.data(plain_target.as_bytes());
        }
    }
}

/// Upgrades a Format v2 file to Format v3 atomically, verifying all existing v2 chunks before modifying anything.
pub fn ensure_v3_file_direct(
    engine: &CryptoEngine,
    file: &mut File,
    raw_path: &Path,
) -> Result<(), i32> {
    let raw_len = file.metadata().map_err(|e| io_err_to_errno(&e))?.len();
    if raw_len == 0 {
        let (hdr, _) = engine.create_v3_header();
        file.seek(SeekFrom::Start(0))
            .map_err(|e| io_err_to_errno(&e))?;
        file.write_all(&hdr).map_err(|e| io_err_to_errno(&e))?;
        file.set_len(HEADER_SIZE_V3 as u64)
            .map_err(|e| io_err_to_errno(&e))?;
        file.sync_data().map_err(|e| io_err_to_errno(&e))?;
        return Ok(());
    }

    if raw_len >= 4 {
        let mut magic = [0u8; 4];
        file.seek(SeekFrom::Start(0))
            .map_err(|e| io_err_to_errno(&e))?;
        if file.read_exact(&mut magic).is_ok() && &magic == MAGIC_V3 {
            return Ok(());
        }
    }

    if raw_len < HEADER_SIZE_V2 as u64 {
        return Err(libc::EIO);
    }

    let payload_len = raw_len - HEADER_SIZE_V2 as u64;
    let rem = payload_len % (CHUNK_CIPHER_SIZE_V2 as u64);
    if rem >= 1 && rem <= MAC_SIZE as u64 {
        return Err(libc::EIO);
    }

    let mut base_nonce = [0u8; HEADER_SIZE_V2];
    file.seek(SeekFrom::Start(0))
        .map_err(|e| io_err_to_errno(&e))?;
    file.read_exact(&mut base_nonce)
        .map_err(|e| io_err_to_errno(&e))?;

    let plain_total = CryptoEngine::calculate_plain_size(raw_len);
    let total_chunks = if plain_total == 0 {
        0
    } else {
        (plain_total - 1) / (CHUNK_PLAIN_SIZE as u64) + 1
    };

    // Pass 1 — Verification (Doğrulama): verify every v2 chunk MAC and drop plaintext immediately.
    for chunk_idx in 0..total_chunks {
        let cipher_offset = CryptoEngine::chunk_offset_to_cipher_offset(chunk_idx);
        let avail = (raw_len - cipher_offset) as usize;
        let cipher_len = std::cmp::min(avail, CHUNK_CIPHER_SIZE_V2);
        if cipher_len < MAC_SIZE {
            return Err(libc::EIO);
        }
        let mut cipher_buf = vec![0u8; cipher_len];
        file.seek(SeekFrom::Start(cipher_offset))
            .map_err(|e| io_err_to_errno(&e))?;
        file.read_exact(&mut cipher_buf)
            .map_err(|e| io_err_to_errno(&e))?;

        let chunk_nonce = CryptoEngine::derive_chunk_nonce(&base_nonce, chunk_idx);
        let plain_chunk = engine
            .decrypt_chunk(&chunk_nonce, &cipher_buf)
            .map_err(|_| libc::EIO)?;
        drop(plain_chunk);
    }

    // Pass 2 — Streaming Write (Akışkan Yazma): write 64-byte v3 header and stream each chunk directly to disk.
    let (v3_header, file_id) = engine.create_v3_header();

    let rand_val = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = raw_path.with_extension(format!("aks3.tmp.{}.{}", std::process::id(), rand_val));
    let write_res = (|| -> Result<(), i32> {
        let mut tmp = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .map_err(|e| io_err_to_errno(&e))?;

        tmp.write_all(&v3_header).map_err(|e| io_err_to_errno(&e))?;

        for chunk_idx in 0..total_chunks {
            let cipher_offset = CryptoEngine::chunk_offset_to_cipher_offset(chunk_idx);
            let avail = (raw_len - cipher_offset) as usize;
            let cipher_len = std::cmp::min(avail, CHUNK_CIPHER_SIZE_V2);
            let mut cipher_buf = vec![0u8; cipher_len];
            file.seek(SeekFrom::Start(cipher_offset))
                .map_err(|e| io_err_to_errno(&e))?;
            file.read_exact(&mut cipher_buf)
                .map_err(|e| io_err_to_errno(&e))?;

            let chunk_nonce = CryptoEngine::derive_chunk_nonce(&base_nonce, chunk_idx);
            let plain_chunk = engine
                .decrypt_chunk(&chunk_nonce, &cipher_buf)
                .map_err(|_| libc::EIO)?;

            let is_last = (chunk_idx + 1) == total_chunks;
            let cipher_chunk = engine
                .encrypt_chunk_v3(&file_id, chunk_idx, is_last, &plain_chunk)
                .map_err(|_| libc::EIO)?;

            tmp.write_all(&cipher_chunk).map_err(|e| io_err_to_errno(&e))?;
        }

        tmp.sync_all().map_err(|e| io_err_to_errno(&e))?;
        fs::rename(&tmp_path, raw_path).map_err(|e| io_err_to_errno(&e))?;
        let reopened = OpenOptions::new()
            .read(true)
            .write(true)
            .open(raw_path)
            .map_err(|e| io_err_to_errno(&e))?;
        *file = reopened;
        Ok(())
    })();

    if write_res.is_err() {
        let _ = fs::remove_file(&tmp_path);
        return write_res;
    }

    Ok(())
}

/// Recursively traverses directory and migrates all Format v2 files to Format v3
pub fn migrate_directory_v2_to_v3(
    engine: &CryptoEngine,
    dir: &Path,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut count = 0;
    if !dir.is_dir() {
        return Ok(0);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            count += migrate_directory_v2_to_v3(engine, &path)?;
        } else if path.is_file() {
            let mut file = match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let raw_len = file.metadata()?.len();
            if raw_len >= 4 {
                let mut magic = [0u8; 4];
                if file.read_exact(&mut magic).is_ok() && &magic == MAGIC_V3 {
                    continue; // Already v3
                }
            }
            if ensure_v3_file_direct(engine, &mut file, &path).is_ok() {
                count += 1;
                info!("Migrated {:?} to Format v3", path);
            }
        }
    }
    Ok(count)
}

/// Filesystem implementation translating decrypted FUSE requests to RAW_MOUNT_PATH
pub struct EncryptedFs {
    pub inner: Arc<FsInner>,
}

impl EncryptedFs {
    pub fn new(
        engine: CryptoEngine,
        raw_root: PathBuf,
        raw_fstype_check: bool,
        op_timeout_secs: u64,
    ) -> Self {
        let health = RawMountHealth::new(raw_root.clone(), raw_fstype_check);
        let timeout_scheduler = TimeoutScheduler::new();
        let worker_pool = WorkerPool::new(8, 64);
        let inner = Arc::new(FsInner {
            engine,
            raw_root,
            inodes: Arc::new(RwLock::new(InodeTable::new())),
            health,
            handles: HandleTable::new(),
            inode_locks: InodeLocks::new(),
            worker_pool,
            timeout_scheduler,
            op_timeout_secs,
            dir_ids: RwLock::new(HashMap::new()),
        });

        Self { inner }
    }

    pub fn new_for_test(engine: CryptoEngine, raw_root: PathBuf) -> Self {
        Self::new(engine, raw_root, false, 15)
    }

    pub fn read_payload(&self, raw_path: &Path, offset: u64, size: usize) -> Result<Vec<u8>, i32> {
        self.inner.read_payload(raw_path, offset, size)
    }

    pub fn write_payload(&self, raw_path: &Path, offset: u64, data: &[u8]) -> Result<usize, i32> {
        self.inner.write_payload(raw_path, offset, data)
    }

    pub fn truncate_file(&self, raw_path: &Path, new_plain_size: u64) -> Result<(), i32> {
        self.inner.truncate_file(raw_path, new_plain_size)
    }

    pub fn write_longname_sidecar_if_needed(
        &self,
        parent_raw_dir: &Path,
        parent_rel_dir: &Path,
        plain_name: &str,
    ) -> Result<(), i32> {
        self.inner
            .write_longname_sidecar_if_needed(parent_raw_dir, parent_rel_dir, plain_name)
    }

    pub fn create_symlink(
        &self,
        parent_rel_dir: &Path,
        link_name: &str,
        target: &str,
    ) -> Result<PathBuf, i32> {
        self.inner.create_symlink(parent_rel_dir, link_name, target)
    }

    pub fn read_symlink(&self, rel_path: &Path) -> Result<String, i32> {
        self.inner.read_symlink(rel_path)
    }
}

impl Filesystem for EncryptedFs {
    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_statfs(guard);
        });
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_lookup(parent, &name_str, guard);
        });
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_getattr(ino, guard);
        });
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_setattr(ino, fh, mode, size, atime, mtime, guard);
        });
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        reply: ReplyDirectory,
    ) {
        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_readdir(ino, offset, guard);
        });
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_open(ino, flags, guard);
        });
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }

        let offset_u64 = offset as u64;
        let size_usize = size as usize;
        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_read(ino, fh, offset_u64, size_usize, guard);
        });
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }

        let offset_u64 = offset as u64;
        let data_vec = data.to_vec();
        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_write(ino, fh, offset_u64, data_vec, guard);
        });
    }

    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_create(parent, &name_str, guard);
        });
    }

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_mkdir(parent, &name_str, guard);
        });
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_unlink(parent, &name_str, guard);
        });
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_rmdir(parent, &name_str, guard);
        });
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let newname_str = match newname.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_rename(parent, &name_str, newparent, &newname_str, guard);
        });
    }

    fn symlink(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        link_name: &OsStr,
        target: &Path,
        reply: ReplyEntry,
    ) {
        let link_name_str = match link_name.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let target_path = target.to_path_buf();
        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_symlink(parent, &link_name_str, &target_path, guard);
        });
    }

    fn readlink(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyData) {
        self.inner.dispatch(reply, move |guard, inner| {
            inner.do_readlink(ino, guard);
        });
    }

    fn flush(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn fsync(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        self.inner.dispatch(reply, move |guard, inner| {
            if let Some(file_arc) = inner.handles.get(fh) {
                let file = file_arc.lock();
                if file.sync_data().is_err() {
                    if let Some(r) = guard.take() {
                        r.error(libc::EIO);
                    }
                    return;
                }
            }
            if let Some(r) = guard.take() {
                r.ok();
            }
        });
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.inner.handles.remove(fh);
        reply.ok();
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }

    tracing_subscriber::fmt::init();

    let args = Args::parse();

    if !cfg!(debug_assertions) && (args.key_hex.is_some() || args.passphrase.is_some()) {
        return Err("In release builds, only --key-stdin is permitted for security".into());
    }

    let raw_path = expand_home_dir(&args.raw_path);
    let mount_path = expand_home_dir(&args.mount_path);

    info!("Initializing mycloud-fuse");
    info!("RAW_MOUNT_PATH: {:?}", raw_path);
    info!("TARGET_MOUNT_PATH: {:?}", mount_path);

    fs::create_dir_all(&raw_path)?;
    fs::create_dir_all(&mount_path)?;

    let engine = if args.key_stdin {
        info!("Reading 32-byte encryption key from stdin pipe");
        let mut key_buf = zeroize::Zeroizing::new([0u8; 32]);
        std::io::stdin()
            .read_exact(&mut *key_buf)
            .map_err(|e| format!("Failed to read 32-byte master key from stdin: {e}"))?;
        CryptoEngine::from_key(*key_buf)
    } else if let Some(hex_key) = args.key_hex {
        let key_bytes = hex::decode(hex_key.trim())?;
        if key_bytes.len() != 32 {
            return Err("Key hex must be exactly 32 bytes (64 hex characters)".into());
        }
        let mut key = zeroize::Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&key_bytes);
        CryptoEngine::from_key(*key)
    } else if let Some(passphrase) = args.passphrase {
        if let Some(salt_b64) = args.salt_base64 {
            let salt_bytes = base64::engine::general_purpose::STANDARD.decode(salt_b64.trim())?;
            CryptoEngine::from_passphrase(&passphrase, &salt_bytes)?
        } else {
            return Err("When providing passphrase, --salt-base64 is required. Static salts are prohibited.".into());
        }
    } else {
        return Err("No encryption key provided. mycloud-fuse requires a key via --key-stdin (or --key-hex in debug mode).".into());
    };

    if args.migrate_v3 {
        info!("Running offline Format v2 -> v3 migration on {:?}", raw_path);
        let count = migrate_directory_v2_to_v3(&engine, &raw_path)?;
        info!(
            "Migration completed successfully. {} files upgraded to Format v3.",
            count
        );
        return Ok(());
    }

    let fs = EncryptedFs::new(
        engine,
        raw_path,
        args.raw_fstype_check,
        args.op_timeout_secs,
    );

    #[cfg(unix)]
    unsafe {
        let res = libc::mlock(
            &fs.inner.engine as *const CryptoEngine as *const libc::c_void,
            std::mem::size_of::<CryptoEngine>(),
        );
        if res != 0 {
            tracing::warn!("mlock on CryptoEngine failed: {}", std::io::Error::last_os_error());
        } else {
            tracing::debug!("mlock on CryptoEngine succeeded");
        }
    }

    let options = vec![
        MountOption::FSName("mycloud".to_string()),
        MountOption::AutoUnmount,
        MountOption::DefaultPermissions,
    ];

    info!("Mounting FUSE filesystem at {:?}", mount_path);
    fuser::mount2(fs, &mount_path, &options)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycloud_crypto::CHUNK_NONCE_SIZE_V3;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn test_encrypted_fs_chunked_io() {
        let temp_dir = std::env::temp_dir().join(format!("mycloud_test_fs_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp dir failed");

        let engine = CryptoEngine::from_key([0x77; 32]);
        let fs = EncryptedFs::new_for_test(engine, temp_dir.clone());

        let raw_file = temp_dir.join("test_chunked.bin");

        // Create 70,000 bytes payload: spans chunk 0 (65536) and chunk 1 (4464)
        let mut original_data = vec![0u8; 70000];
        for (i, byte) in original_data.iter_mut().enumerate() {
            *byte = (i % 251) as u8;
        }

        // Write payload in Format v3
        let written = fs
            .write_payload(&raw_file, 0, &original_data)
            .expect("write failed");
        assert_eq!(written, 70000);

        // Verify physical ciphertext file size on disk:
        // HEADER_SIZE_V3 (64) + chunk 0 (65536 + 40 = 65576) + chunk 1 (4464 + 40 = 4504) = 70144 bytes
        let raw_meta = fs::metadata(&raw_file).expect("stat failed");
        let expected_raw_len = (HEADER_SIZE_V3 + CHUNK_DISK_SIZE_V3 + (4464 + CHUNK_OVERHEAD_V3)) as u64;
        assert_eq!(raw_meta.len(), expected_raw_len);

        // Read entire payload back
        let read_back = fs.read_payload(&raw_file, 0, 70000).expect("read failed");
        assert_eq!(read_back, original_data);

        // Read cross-chunk unaligned range: from byte 60,000 to byte 68,000 (crosses 65,536 boundary)
        let cross_chunk = fs
            .read_payload(&raw_file, 60000, 8000)
            .expect("read slice failed");
        assert_eq!(cross_chunk, original_data[60000..68000]);

        // Truncate to 30,000 bytes
        fs.truncate_file(&raw_file, 30000).expect("truncate failed");
        let truncated_raw_meta = fs::metadata(&raw_file).expect("stat failed");
        let expected_truncated_raw = (HEADER_SIZE_V3 + 30000 + CHUNK_OVERHEAD_V3) as u64;
        assert_eq!(truncated_raw_meta.len(), expected_truncated_raw);

        let truncated_data = fs
            .read_payload(&raw_file, 0, 30000)
            .expect("read truncated failed");
        assert_eq!(truncated_data, original_data[0..30000]);

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_mac_error_partial_write_preserves_file() {
        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_test_mac_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp dir failed");

        let engine = CryptoEngine::from_key([0x42; 32]);
        let fs = EncryptedFs::new_for_test(engine, temp_dir.clone());
        let raw_file = temp_dir.join("corrupted.bin");

        // Write initial 2000 bytes
        let initial_data = vec![0xAB; 2000];
        let written = fs
            .write_payload(&raw_file, 0, &initial_data)
            .expect("initial write failed");
        assert_eq!(written, 2000);

        // Read physical ciphertext file from disk
        let raw_bytes_before = fs::read(&raw_file).expect("read raw failed");
        assert!(raw_bytes_before.len() >= HEADER_SIZE_V3 + CHUNK_OVERHEAD_V3);

        // Flip 1 byte inside chunk 0 ciphertext (after the 64-byte header and 24-byte nonce)
        let corrupt_idx = HEADER_SIZE_V3 + 30;
        let mut corrupted_raw = raw_bytes_before.clone();
        corrupted_raw[corrupt_idx] ^= 0xFF;
        fs::write(&raw_file, &corrupted_raw).expect("write corrupted raw failed");

        let raw_after_corruption = fs::read(&raw_file).expect("read raw after corruption failed");
        assert_eq!(raw_after_corruption, corrupted_raw);

        // Attempt partial write to corrupted chunk
        let partial_data = vec![0x12; 50];
        let write_res = fs.write_payload(&raw_file, 100, &partial_data);

        // Must return Err(libc::EIO)
        assert_eq!(write_res, Err(libc::EIO));

        // Disk bytes must be completely untouched and identical to raw_after_corruption
        let raw_after_failed_write =
            fs::read(&raw_file).expect("read raw after failed write failed");
        assert_eq!(raw_after_failed_write, raw_after_corruption);

        // Also test truncate: must return Err(libc::EIO) and leave disk untouched
        let trunc_res = fs.truncate_file(&raw_file, 1000);
        assert_eq!(trunc_res, Err(libc::EIO));

        let raw_after_failed_trunc =
            fs::read(&raw_file).expect("read raw after failed trunc failed");
        assert_eq!(raw_after_failed_trunc, raw_after_corruption);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_reply_guard_no_double_reply() {
        struct MockReply {
            call_count: Arc<AtomicUsize>,
            last_err: Arc<parking_lot::Mutex<Option<i32>>>,
        }

        impl ReplyError for MockReply {
            fn error(self, err: i32) {
                self.call_count.fetch_add(1, Ordering::SeqCst);
                *self.last_err.lock() = Some(err);
            }
        }

        // Case 1: Normal take, only 1 reply sent
        {
            let count = Arc::new(AtomicUsize::new(0));
            let err = Arc::new(parking_lot::Mutex::new(None));
            let mock = MockReply {
                call_count: count.clone(),
                last_err: err.clone(),
            };
            let (guard, cell) = ReplyGuard::new(mock);

            let first_take = guard.take();
            assert!(first_take.is_some());
            first_take.unwrap().error(libc::EIO);

            // Second take is None
            let second_take = guard.take();
            assert!(second_take.is_none());

            // Timeout take via cell is also None
            let timeout_take = cell.lock().take();
            assert!(timeout_take.is_none());

            drop(guard);

            assert_eq!(count.load(Ordering::SeqCst), 1);
            assert_eq!(*err.lock(), Some(libc::EIO));
        }

        // Case 2: Timeout triggers first, worker finishes later
        {
            let count = Arc::new(AtomicUsize::new(0));
            let err = Arc::new(parking_lot::Mutex::new(None));
            let mock = MockReply {
                call_count: count.clone(),
                last_err: err.clone(),
            };
            let (guard, cell) = ReplyGuard::new(mock);

            // Timeout fires
            if let Some(r) = cell.lock().take() {
                r.error(libc::ETIMEDOUT);
            }

            // Worker tries to take
            let worker_take = guard.take();
            assert!(worker_take.is_none());

            drop(guard);

            assert_eq!(count.load(Ordering::SeqCst), 1);
            assert_eq!(*err.lock(), Some(libc::ETIMEDOUT));
        }

        // Case 3: Guard dropped without explicit take (e.g. panic or early return)
        {
            let count = Arc::new(AtomicUsize::new(0));
            let err = Arc::new(parking_lot::Mutex::new(None));
            let mock = MockReply {
                call_count: count.clone(),
                last_err: err.clone(),
            };
            let (guard, _cell) = ReplyGuard::new(mock);

            drop(guard);

            assert_eq!(count.load(Ordering::SeqCst), 1);
            assert_eq!(*err.lock(), Some(libc::EIO));
        }
    }

    // ==========================================
    // Stage 2 Mandatory Tests (ADIM 3)
    // ==========================================

    #[test]
    fn test_v3_nonce_rotation_on_overwrite() {
        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_test_v3_rot_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x11; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());
        let raw_file = temp_dir.join("rot.bin");

        let data1 = vec![0xAA; 65536];
        fs_inst.write_payload(&raw_file, 0, &data1).unwrap();

        let raw1 = fs::read(&raw_file).unwrap();
        assert!(raw1.len() >= HEADER_SIZE_V3 + CHUNK_NONCE_SIZE_V3);
        let nonce1 = raw1[HEADER_SIZE_V3..HEADER_SIZE_V3 + CHUNK_NONCE_SIZE_V3].to_vec();

        let data2 = vec![0xBB; 65536];
        fs_inst.write_payload(&raw_file, 0, &data2).unwrap();

        let raw2 = fs::read(&raw_file).unwrap();
        let nonce2 = raw2[HEADER_SIZE_V3..HEADER_SIZE_V3 + CHUNK_NONCE_SIZE_V3].to_vec();

        assert_ne!(nonce1, nonce2, "Nonce must rotate on overwrite of chunk 0");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_v3_sparse_write_integrity() {
        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_test_v3_sparse_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x22; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());
        let raw_file = temp_dir.join("sparse.bin");

        let initial_100 = vec![0x42; 100];
        fs_inst.write_payload(&raw_file, 0, &initial_100).unwrap();
        fs_inst.write_payload(&raw_file, 65536, b"hello").unwrap();

        let r1 = fs_inst.read_payload(&raw_file, 0, 100).unwrap();
        assert_eq!(r1, initial_100);

        let r2 = fs_inst.read_payload(&raw_file, 100, 65436).unwrap();
        assert_eq!(r2, vec![0u8; 65436]);

        let r3 = fs_inst.read_payload(&raw_file, 65536, 5).unwrap();
        assert_eq!(r3, b"hello");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_v3_exact_chunk_boundary_append() {
        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_test_v3_bnd_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x33; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());
        let raw_file = temp_dir.join("bnd.bin");

        let chunk0_data = vec![0x77; 65536];
        fs_inst.write_payload(&raw_file, 0, &chunk0_data).unwrap();

        let chunk1_data = vec![0x88; 500];
        fs_inst.write_payload(&raw_file, 65536, &chunk1_data).unwrap();

        let read_all = fs_inst.read_payload(&raw_file, 0, 66036).unwrap();
        assert_eq!(read_all.len(), 66036);
        assert_eq!(&read_all[0..65536], &chunk0_data[..]);
        assert_eq!(&read_all[65536..66036], &chunk1_data[..]);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_v3_extending_truncate() {
        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_test_v3_trunc_ext_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x44; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());
        let raw_file = temp_dir.join("trunc_ext.bin");

        let initial_10 = vec![0x55; 10];
        fs_inst.write_payload(&raw_file, 0, &initial_10).unwrap();

        fs_inst.truncate_file(&raw_file, 200_000).unwrap();
        let r1 = fs_inst.read_payload(&raw_file, 0, 200_000).unwrap();
        assert_eq!(r1.len(), 200_000);
        assert_eq!(&r1[0..10], &initial_10[..]);
        assert_eq!(&r1[10..], &vec![0u8; 199_990][..]);

        fs_inst.truncate_file(&raw_file, 65536).unwrap();
        let r2 = fs_inst.read_payload(&raw_file, 0, 65536).unwrap();
        assert_eq!(r2.len(), 65536);
        assert_eq!(&r2[0..10], &initial_10[..]);
        assert_eq!(&r2[10..], &vec![0u8; 65526][..]);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_v3_tail_truncation_detected() {
        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_test_v3_tail_trunc_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x55; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());
        let raw_file = temp_dir.join("tail.bin");

        // 150_000 bytes spans chunks 0, 1, 2
        let payload = vec![0x33; 150_000];
        fs_inst.write_payload(&raw_file, 0, &payload).unwrap();

        // Truncate last chunk from raw file (leaving header + 2 full chunks)
        let f = OpenOptions::new().write(true).open(&raw_file).unwrap();
        let new_len = (HEADER_SIZE_V3 + 2 * CHUNK_DISK_SIZE_V3) as u64;
        f.set_len(new_len).unwrap();
        drop(f);

        // Attempting to read should return EIO because chunk 1 had is_last = 0
        let res = fs_inst.read_payload(&raw_file, 0, 131072);
        assert_eq!(res, Err(libc::EIO));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_v3_chunk_reorder_detected() {
        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_test_v3_reorder_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x66; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());
        let raw_file = temp_dir.join("reorder.bin");

        // Exactly 2 full chunks (131072 bytes)
        let payload = vec![0x44; 131072];
        fs_inst.write_payload(&raw_file, 0, &payload).unwrap();

        let raw = fs::read(&raw_file).unwrap();
        assert_eq!(raw.len(), HEADER_SIZE_V3 + 2 * CHUNK_DISK_SIZE_V3);

        // Swap Chunk 0 (64..64+65576) and Chunk 1 (64+65576..64+2*65576)
        let c0_start = HEADER_SIZE_V3;
        let c1_start = HEADER_SIZE_V3 + CHUNK_DISK_SIZE_V3;
        let c1_end = HEADER_SIZE_V3 + 2 * CHUNK_DISK_SIZE_V3;

        let mut swapped = raw[..c0_start].to_vec();
        swapped.extend_from_slice(&raw[c1_start..c1_end]);
        swapped.extend_from_slice(&raw[c0_start..c1_start]);

        fs::write(&raw_file, &swapped).unwrap();

        let res = fs_inst.read_payload(&raw_file, 0, 131072);
        assert_eq!(res, Err(libc::EIO));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_v3_cross_file_chunk_swap_detected() {
        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_test_v3_cross_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x77; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());
        let raw_file_a = temp_dir.join("file_a.bin");
        let raw_file_b = temp_dir.join("file_b.bin");

        fs_inst.write_payload(&raw_file_a, 0, &vec![0xAA; 65536]).unwrap();
        fs_inst.write_payload(&raw_file_b, 0, &vec![0xBB; 65536]).unwrap();

        let raw_a = fs::read(&raw_file_a).unwrap();
        let raw_b = fs::read(&raw_file_b).unwrap();

        // Copy Chunk 0 from B to A
        let mut tampered_a = raw_a;
        tampered_a[HEADER_SIZE_V3..HEADER_SIZE_V3 + CHUNK_DISK_SIZE_V3]
            .copy_from_slice(&raw_b[HEADER_SIZE_V3..HEADER_SIZE_V3 + CHUNK_DISK_SIZE_V3]);

        fs::write(&raw_file_a, &tampered_a).unwrap();

        let res = fs_inst.read_payload(&raw_file_a, 0, 65536);
        assert_eq!(res, Err(libc::EIO));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_v3_header_tamper_detected() {
        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_test_v3_hdr_tamper_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x88; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());
        let raw_file = temp_dir.join("hdr_tamper.bin");

        fs_inst.write_payload(&raw_file, 0, &vec![0xCC; 1000]).unwrap();

        let mut raw = fs::read(&raw_file).unwrap();
        raw[10] ^= 0x01; // Tamper with file_id in header
        fs::write(&raw_file, &raw).unwrap();

        let read_res = fs_inst.read_payload(&raw_file, 0, 1000);
        assert_eq!(read_res, Err(libc::EIO));

        let write_res = fs_inst.write_payload(&raw_file, 50, &[0x99; 10]);
        assert_eq!(write_res, Err(libc::EIO));

        let raw_after = fs::read(&raw_file).unwrap();
        assert_eq!(raw, raw_after, "Failed write must not modify the file on disk");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_v2_to_v3_auto_migration_and_cli() {
        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_test_v2_migration_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x99; 32]);
        let fs_inst = EncryptedFs::new_for_test(CryptoEngine::from_key([0x99; 32]), temp_dir.clone());
        let raw_file = temp_dir.join("v2_file.bin");

        // Construct 70,000 bytes v2 file manually
        let base_nonce = CryptoEngine::generate_base_nonce();
        let chunk0_nonce = CryptoEngine::derive_chunk_nonce(&base_nonce, 0);
        let chunk1_nonce = CryptoEngine::derive_chunk_nonce(&base_nonce, 1);

        let mut orig_data = vec![0u8; 70000];
        for (i, b) in orig_data.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }

        let c0 = engine.encrypt_chunk(&chunk0_nonce, &orig_data[..65536]).unwrap();
        let c1 = engine.encrypt_chunk(&chunk1_nonce, &orig_data[65536..]).unwrap();

        let mut v2_raw = Vec::new();
        v2_raw.extend_from_slice(&base_nonce);
        v2_raw.extend_from_slice(&c0);
        v2_raw.extend_from_slice(&c1);
        fs::write(&raw_file, &v2_raw).unwrap();

        // 1. Verify v2 backward compatible read
        let read_v2 = fs_inst.read_payload(&raw_file, 0, 70000).unwrap();
        assert_eq!(read_v2, orig_data);

        // 2. Partial write automatically upgrades to Format v3
        let patch = b"v3-migration-patch";
        fs_inst.write_payload(&raw_file, 100, patch).unwrap();

        let raw_v3 = fs::read(&raw_file).unwrap();
        assert_eq!(&raw_v3[0..4], MAGIC_V3, "File must be upgraded to Format v3");

        let read_v3 = fs_inst.read_payload(&raw_file, 0, 70000).unwrap();
        let mut expected = orig_data.clone();
        expected[100..100 + patch.len()].copy_from_slice(patch);
        assert_eq!(read_v3, expected);

        // 3. Test migrate_directory_v2_to_v3 on another v2 file
        let sub_dir = temp_dir.join("subdir");
        fs::create_dir_all(&sub_dir).unwrap();
        let sub_file = sub_dir.join("another_v2.bin");
        fs::write(&sub_file, &v2_raw).unwrap();

        let migrated_count = migrate_directory_v2_to_v3(&engine, &temp_dir).unwrap();
        assert_eq!(migrated_count, 1, "Should have migrated 1 v2 file");

        let sub_raw = fs::read(&sub_file).unwrap();
        assert_eq!(&sub_raw[0..4], MAGIC_V3);
        let sub_read = fs_inst.read_payload(&sub_file, 0, 70000).unwrap();
        assert_eq!(sub_read, orig_data);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_v3_randomized_reference_model() {
        struct XorShift64(u64);
        impl XorShift64 {
            fn next(&mut self) -> u64 {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                self.0
            }
            fn gen_range(&mut self, min: usize, max: usize) -> usize {
                if min >= max {
                    return min;
                }
                min + (self.next() as usize % (max - min))
            }
        }

        let temp_dir =
            std::env::temp_dir().join(format!("mycloud_test_v3_ref_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0xEE; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());
        let raw_file = temp_dir.join("fuzz.bin");

        let mut rng = XorShift64(0x123456789ABCDEF0);
        let mut model: Vec<u8> = Vec::new();

        for step in 0..500 {
            let action = rng.gen_range(0, 10);
            if action < 6 {
                // Write
                let max_off = model.len() + 20_000;
                let offset = rng.gen_range(0, std::cmp::max(1, max_off)) as u64;
                let len = rng.gen_range(1, 20_000);
                let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();

                let end = (offset as usize) + len;
                if end > model.len() {
                    model.resize(end, 0u8);
                }
                model[(offset as usize)..end].copy_from_slice(&data);

                fs_inst
                    .write_payload(&raw_file, offset, &data)
                    .unwrap_or_else(|e| {
                        panic!("Step {step} write failed at offset {offset} len {len}: err {e}");
                    });
            } else if action < 8 {
                // Truncate
                let max_trunc = std::cmp::max(1, model.len() + 10_000);
                let new_size = rng.gen_range(0, max_trunc) as u64;
                model.resize(new_size as usize, 0u8);
                fs_inst
                    .truncate_file(&raw_file, new_size)
                    .unwrap_or_else(|e| {
                        panic!("Step {step} truncate failed at size {new_size}: err {e}");
                    });
            } else {
                // Read
                if !model.is_empty() {
                    let offset = rng.gen_range(0, model.len()) as u64;
                    let size = rng.gen_range(1, 20_000);
                    let actual = fs_inst
                        .read_payload(&raw_file, offset, size)
                        .unwrap_or_else(|e| {
                            panic!("Step {step} read failed at offset {offset} size {size}: err {e}");
                        });
                    let expected_end = std::cmp::min(model.len(), (offset as usize) + size);
                    let expected = &model[(offset as usize)..expected_end];
                    assert_eq!(
                        actual.as_slice(),
                        expected,
                        "Step {step}: read mismatch at offset {offset} size {size}"
                    );
                }
            }
        }

        // Final verification: entire file content
        let final_read = fs_inst.read_payload(&raw_file, 0, model.len()).unwrap();
        assert_eq!(final_read, model, "Final full read must match reference model");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_directory_rename_preserves_nested_files_and_inodes() {
        let temp_dir = std::env::temp_dir()
            .join(format!("mycloud_test_rename_nested_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x5A; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());

        let dir_a = PathBuf::from("dir_a");
        let dir_a_sub = dir_a.join("sub");
        let file_path = dir_a_sub.join("hello.txt");

        let raw_dir_a = fs_inst.inner.to_raw_path(&dir_a).unwrap();
        fs::create_dir_all(&raw_dir_a).unwrap();
        fs_inst.inner.get_or_create_dir_id(&raw_dir_a, &dir_a, true).unwrap();

        let raw_dir_a_sub = fs_inst.inner.to_raw_path(&dir_a_sub).unwrap();
        fs::create_dir_all(&raw_dir_a_sub).unwrap();
        fs_inst.inner.get_or_create_dir_id(&raw_dir_a_sub, &dir_a_sub, true).unwrap();

        let raw_file = fs_inst.inner.to_raw_path(&file_path).unwrap();
        let orig_physical_file_name = raw_file.file_name().unwrap().to_os_string();
        fs_inst.write_payload(&raw_file, 0, b"world").unwrap();

        let dir_a_ino = fs_inst.inner.inodes.write().get_or_insert_ino(dir_a.clone());
        let sub_ino = fs_inst.inner.inodes.write().get_or_insert_ino(dir_a_sub.clone());
        let file_ino = fs_inst.inner.inodes.write().get_or_insert_ino(file_path.clone());

        assert_eq!(
            fs_inst.inner.inodes.read().get_path(file_ino).unwrap(),
            file_path
        );

        // Rename dir_a to dir_b
        let dir_b = PathBuf::from("dir_b");
        let raw_dir_b = fs_inst.inner.to_raw_path(&dir_b).unwrap();

        // 1. Rename physical folder
        fs::rename(&raw_dir_a, &raw_dir_b).unwrap();

        // 2. Rename inode mappings & dir_ids cache (O(1) - no reencrypt_directory_children!)
        fs_inst.inner.inodes.write().rename(&dir_a, dir_b.clone());
        {
            let mut dir_ids = fs_inst.inner.dir_ids.write();
            let keys_to_update: Vec<(PathBuf, String)> = dir_ids
                .iter()
                .filter(|(k, _)| k.starts_with(&dir_a))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();

            for (old_k, val) in keys_to_update {
                dir_ids.remove(&old_k);
                if let Ok(suffix) = old_k.strip_prefix(&dir_a) {
                    let new_k = dir_b.join(suffix);
                    dir_ids.insert(new_k, val);
                }
            }
        }

        // Verify inode tracking table for parent, subfolder, and nested file
        let new_file_path = dir_b.join("sub").join("hello.txt");
        assert_eq!(
            fs_inst.inner.inodes.read().get_path(file_ino).unwrap(),
            new_file_path
        );
        assert_eq!(
            fs_inst.inner.inodes.read().get_path(dir_a_ino).unwrap(),
            dir_b
        );
        assert_eq!(
            fs_inst.inner.inodes.read().get_path(sub_ino).unwrap(),
            dir_b.join("sub")
        );

        // Critical Verification: to_raw_path on new path returns exact same physical file name!
        let new_raw_file = fs_inst.inner.to_raw_path(&new_file_path).unwrap();
        assert_eq!(
            new_raw_file.file_name().unwrap(),
            &orig_physical_file_name,
            "Nested physical file name must remain identical in O(1) DirIV architecture"
        );

        // Content read verification
        let data = fs_inst.read_payload(&new_raw_file, 0, 5).unwrap();
        assert_eq!(data, b"world");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_circuit_breaker_cooldown() {
        let temp_dir = std::env::temp_dir()
            .join(format!("mycloud_test_circuit_breaker_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let health = RawMountHealth::new(temp_dir.clone(), false);
        assert!(health.is_healthy());

        health.mark_unhealthy();
        // Immediately after mark_unhealthy, must be false
        assert!(!health.is_healthy());

        // Even after 100ms, cooldown keeps it false
        std::thread::sleep(Duration::from_millis(100));
        assert!(!health.is_healthy());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    // ==========================================
    // Stage 5 Mandatory Tests (ADIM 3)
    // ==========================================

    #[test]
    fn test_long_filename_200_chars_roundtrip() {
        let temp_dir = std::env::temp_dir()
            .join(format!("mycloud_test_longname_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x45; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());

        // 1. 200 karakterlik (119 bayt sınırını aşan, Türkçe/UTF-8 karakterler de içeren) dosya adı
        let long_name = format!("{}_ğüşiöç.pdf", "a".repeat(180));
        let rel_path = PathBuf::from(&long_name);

        // 2. to_raw_path çıktısının fiziksel dosya adının "AKS_LN_" ile başladığını ve 255 bayttan kısa olduğunu doğrula
        let raw_path = fs_inst.inner.to_raw_path(&rel_path).unwrap();
        let raw_file_name = raw_path.file_name().unwrap().to_str().unwrap();
        assert!(raw_file_name.starts_with("AKS_LN_"));
        assert!(raw_file_name.len() < 255);
        assert_eq!(raw_file_name.len(), 59);

        // 3. write_payload ve write_longname_sidecar_if_needed ile dosyayı ve .name sidecar dosyasını oluştur
        let payload = b"Hello, LongName storage test payload!";
        fs_inst.write_payload(&raw_path, 0, payload).unwrap();
        fs_inst
            .write_longname_sidecar_if_needed(&temp_dir, Path::new(""), &long_name)
            .unwrap();

        // 4. Yanında .name dosyasının oluştuğunu ve içeriğinin hash kontrolünü geçtiğini doğrula
        let sidecar_name = format!("{}.name", raw_file_name);
        let sidecar_path = temp_dir.join(&sidecar_name);
        assert!(sidecar_path.exists(), "Sidecar .name file must exist");
        let sidecar_content = fs::read_to_string(&sidecar_path).unwrap();
        assert_eq!(
            hash_long_cipher_name(sidecar_content.trim()),
            raw_file_name,
            "Sidecar cipher hash must match the short physical filename"
        );

        // 5. read_payload ile içeriğin okunduğunu doğrula
        let read_back = fs_inst.read_payload(&raw_path, 0, payload.len()).unwrap();
        assert_eq!(read_back, payload);

        // 6. Dosya yeniden adlandırıldığında (rename): eski .name silinip yeni .name oluşmalı
        let new_long_name = format!("{}_renamed_ğüşiöç.pdf", "b".repeat(170));
        let new_rel_path = PathBuf::from(&new_long_name);
        let new_raw_path = fs_inst.inner.to_raw_path(&new_rel_path).unwrap();
        let new_raw_file_name = new_raw_path.file_name().unwrap().to_str().unwrap().to_string();
        assert!(new_raw_file_name.starts_with("AKS_LN_"));

        fs::rename(&raw_path, &new_raw_path).unwrap();
        fs_inst.inner.remove_longname_sidecar_if_any(&raw_path);
        fs_inst
            .write_longname_sidecar_if_needed(&temp_dir, Path::new(""), &new_long_name)
            .unwrap();

        assert!(!sidecar_path.exists(), "Old sidecar file must be removed after rename");
        let new_sidecar_path = temp_dir.join(format!("{}.name", new_raw_file_name));
        assert!(new_sidecar_path.exists(), "New sidecar file must exist after rename");
        let new_read = fs_inst.read_payload(&new_raw_path, 0, payload.len()).unwrap();
        assert_eq!(new_read, payload);

        // 7. Unlink sonrasında .name dosyasının da temizlendiğini doğrula
        fs::remove_file(&new_raw_path).unwrap();
        fs_inst.inner.remove_longname_sidecar_if_any(&new_raw_path);
        assert!(!new_raw_path.exists(), "Target file must be removed");
        assert!(!new_sidecar_path.exists(), "Sidecar file must be removed after unlink");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_symlink_and_readlink_encrypted_target() {
        let temp_dir = std::env::temp_dir()
            .join(format!("mycloud_test_symlink_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x67; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());

        let target = "../secret_folder/target_file.txt";
        let link_name = "encrypted_link.lnk";

        // Create encrypted symlink
        let raw_link = fs_inst
            .create_symlink(Path::new(""), link_name, target)
            .unwrap();

        // Diskteki ham sembolik bağ hedefinin düz metin "secret_folder" içermediğini doğrula
        let raw_target_path = fs::read_link(&raw_link).unwrap();
        let raw_target_str = raw_target_path.to_str().unwrap();
        assert!(
            !raw_target_str.contains("secret_folder"),
            "On-disk raw symlink target must be encrypted and not leak plaintext path"
        );
        assert!(
            !raw_target_str.contains("target_file"),
            "On-disk raw symlink target must not contain target filename"
        );

        // decrypt_filename ile çözüldüğünde orijinal hedefi verdiğini doğrula
        let parent_dir_id = fs_inst
            .inner
            .get_or_create_dir_id(&temp_dir, Path::new(""), false)
            .unwrap();
        let decrypted_target =
            decrypt_filename(&fs_inst.inner.engine, raw_target_str, Some(&parent_dir_id)).unwrap();
        assert_eq!(decrypted_target, target);

        // read_symlink helper ile de çözüldüğünü doğrula
        let read_target = fs_inst.read_symlink(Path::new(link_name)).unwrap();
        assert_eq!(read_target, target);

        // Symlink metadata check
        let meta = fs::symlink_metadata(&raw_link).unwrap();
        let attr = fs_inst.inner.build_file_attr(200, &raw_link, &meta);
        assert_eq!(attr.kind, FileType::Symlink);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_chmod_and_utimens_metadata() {
        let temp_dir = std::env::temp_dir()
            .join(format!("mycloud_test_chmod_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x78; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());

        let file_path = PathBuf::from("test_chmod.txt");
        let raw_file = fs_inst.inner.to_raw_path(&file_path).unwrap();

        // Create and write payload
        fs_inst
            .write_payload(&raw_file, 0, b"permissions and timestamp metadata test")
            .unwrap();

        // Set permissions to 0o750
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&raw_file, fs::Permissions::from_mode(0o750)).unwrap();
        }

        // Set timestamps via utimensat
        let target_time = 1_700_000_000; // Fixed timestamp
        let times = [
            libc::timespec {
                tv_sec: target_time,
                tv_nsec: 0,
            },
            libc::timespec {
                tv_sec: target_time,
                tv_nsec: 0,
            },
        ];
        let c_raw = std::ffi::CString::new(raw_file.as_os_str().as_bytes()).unwrap();
        unsafe {
            let res = libc::utimensat(
                libc::AT_FDCWD,
                c_raw.as_ptr(),
                times.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            );
            assert_eq!(res, 0);
        }

        // Verify build_file_attr reflects 0o750 and timestamp
        let meta = fs::symlink_metadata(&raw_file).unwrap();
        let attr = fs_inst.inner.build_file_attr(300, &raw_file, &meta);

        #[cfg(unix)]
        assert_eq!(attr.perm, 0o750, "File permissions must match 0o750");

        let mtime_sec = attr.mtime.duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(mtime_sec, target_time as u64, "mtime must match utimensat value");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_plain_filename_over_255_returns_enametoolong() {
        let temp_dir = std::env::temp_dir()
            .join(format!("mycloud_test_toolong_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let engine = CryptoEngine::from_key([0x89; 32]);
        let fs_inst = EncryptedFs::new_for_test(engine, temp_dir.clone());

        let too_long_name = "a".repeat(256);
        let rel_path = PathBuf::from(&too_long_name);
        let err = fs_inst.inner.to_raw_path(&rel_path);
        assert_eq!(err, Err(libc::ENAMETOOLONG));

        let sidecar_err =
            fs_inst.write_longname_sidecar_if_needed(&temp_dir, Path::new(""), &too_long_name);
        assert_eq!(sidecar_err, Err(libc::ENAMETOOLONG));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}

