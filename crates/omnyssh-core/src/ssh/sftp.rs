//! SFTP file manager operations.
//!
//! Provides [`SftpManager`] — a persistent background task that owns an SSH+SFTP
//! session and processes [`SftpCommand`] messages sent from the UI thread.
//!
//! All operations are non-blocking from the UI perspective.
//! Progress is reported via [`CoreEvent::FileTransferProgress`].

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use russh_sftp::protocol::{FileAttributes, FileType, OpenFlags};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Notify};
use tokio::time;

use crate::event::{CoreEvent, TransferId};
use crate::ssh::client::Host;
use crate::ssh::password::Prompter;
use crate::ssh::session::{Passwords, SshSession};

/// How long the SFTP channel and subsystem may take once logged in.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// FileEntry — represents one file or directory in a panel listing
// ---------------------------------------------------------------------------

/// Metadata for a single file or directory in a file panel.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Base file name (not the full path).
    pub name: String,
    /// Absolute path string (used as the stable identifier for marked sets).
    pub path: String,
    /// File size in bytes (`0` for directories).
    pub size: u64,
    /// `true` when this entry is a directory.
    pub is_dir: bool,
    /// Last modification time, in seconds since the Unix epoch, when known.
    pub modified: Option<i64>,
}

impl FileEntry {
    /// The synthetic `..` entry pointing at `parent`.
    fn parent(parent: &str) -> Self {
        Self {
            name: "..".to_string(),
            path: parent.to_string(),
            size: 0,
            is_dir: true,
            modified: None,
        }
    }
}

/// Seconds since the Unix epoch for a file time, or `None` if it is unavailable.
fn unix_secs(time: std::io::Result<std::time::SystemTime>) -> Option<i64> {
    let time = time.ok()?;
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok(),
        Err(e) => i64::try_from(e.duration().as_secs()).ok().map(|s| -s),
    }
}

// ---------------------------------------------------------------------------
// SftpCommand — sent from UI thread → SftpManager background task
// ---------------------------------------------------------------------------

/// Commands processed by the [`SftpManager`] background task.
pub enum SftpCommand {
    /// List the entries in a remote directory.
    ListDir(String),
    /// Download a remote file, or a remote folder with everything in it, to a local
    /// path. An existing folder is merged into and an existing file replaced; new
    /// files and folders get the source's permission bits. Inside a folder, symlinks,
    /// special files and names this system cannot create are skipped; the transfer
    /// goes on past them and past failed files, and its [`CoreEvent::SftpOpDone`]
    /// error counts them. [`SftpManager::cancel`] stops it.
    Download {
        remote: String,
        local: String,
        transfer_id: TransferId,
    },
    /// Upload a local file or folder to a remote path, by the rules of `Download`.
    Upload {
        local: String,
        remote: String,
        transfer_id: TransferId,
    },
    /// Delete a remote file (falls back to removing an empty directory).
    Delete(String),
    /// Create a remote directory.
    MkDir(String),
    /// Rename / move a remote path.
    Rename { from: String, to: String },
    /// Read the first 4 096 bytes of a remote file for preview.
    ReadPreview(String),
    /// Shut down the task gracefully.
    Disconnect,
}

/// A command and the cancel count it was sent under (see [`SftpManager::cancel`]).
type Queued = (u64, SftpCommand);

// ---------------------------------------------------------------------------
// SftpManager — handle held by App to communicate with the background task
// ---------------------------------------------------------------------------

/// Manages a persistent SSH+SFTP background task.
///
/// Use [`SftpManager::connect`] to create, [`SftpManager::send`] to enqueue
/// commands, and [`SftpManager::disconnect`] for a clean shutdown.
#[derive(Debug)]
pub struct SftpManager {
    cmd_tx: mpsc::Sender<Queued>,
    /// Shared with the task rather than queued: the transfer to stop holds the queue
    /// until it ends.
    cancels: Arc<Cancels>,
}

impl SftpManager {
    /// Connects to `host` via SSH + SFTP subsystem and spawns the background task.
    /// A login the keys do not get into asks for the password through `prompter`.
    ///
    /// On success sends [`CoreEvent::SftpConnected`] through `event_tx`.
    /// On failure the task sends [`CoreEvent::SftpDisconnected`].
    ///
    /// # Errors
    /// Returns an error if the SSH connection fails before the task is spawned,
    /// including a cancelled password prompt.
    pub async fn connect(
        host: &Host,
        event_tx: mpsc::Sender<CoreEvent>,
        mut prompter: Prompter,
    ) -> anyhow::Result<Self> {
        let session = SshSession::connect_with(host, Passwords::Ask(&mut prompter))
            .await
            .context("SFTP SSH connect")?;
        // The login is bounded step by step; the channel must not hang either.
        let sftp = time::timeout(OPEN_TIMEOUT, async {
            let stream = session
                .open_sftp_channel()
                .await
                .context("open SFTP channel")?;
            russh_sftp::client::SftpSession::new(stream)
                .await
                .context("create SFTP session")
        })
        .await
        .map_err(|_| anyhow::anyhow!("SFTP did not start within {}s", OPEN_TIMEOUT.as_secs()))??;

        let (cmd_tx, cmd_rx) = mpsc::channel::<Queued>(64);
        let cancels = Arc::new(Cancels::default());
        let host_name = host.name.clone();

        // `session` and `sftp` are owned by this async block.  If the task
        // panics, Rust's unwind machinery calls their Drop impls before the
        // panic propagates to tokio — the TCP connection is therefore always
        // released even in the panic path.  No explicit catch_unwind needed.
        let task_cancels = Arc::clone(&cancels);
        tokio::spawn(async move {
            let _ = event_tx
                .send(CoreEvent::SftpConnected {
                    host_name: host_name.clone(),
                })
                .await;
            sftp_task_loop(session, sftp, cmd_rx, event_tx.clone(), task_cancels).await;
            tracing::info!("SFTP task for '{}' exited", host_name);
        });

        Ok(Self { cmd_tx, cancels })
    }

    /// Enqueues a command (fire-and-forget). Silently drops if the task exited.
    pub fn send(&self, cmd: SftpCommand) {
        let _ = self.cmd_tx.try_send((self.cancels.count(), cmd));
    }

    /// Cancels every transfer sent so far. The running one stops at its next step and
    /// removes the file it was part way through; files it finished stay. Queued ones
    /// stop before they start. Each still ends with its [`CoreEvent::SftpOpDone`],
    /// whose error says it was cancelled.
    pub fn cancel(&self) {
        self.cancels.bump();
    }

    /// Cancels the transfers, sends [`SftpCommand::Disconnect`] and drops the sender.
    pub fn disconnect(self) {
        self.send(SftpCommand::Disconnect);
    }
}

impl Drop for SftpManager {
    // Nothing can follow a transfer's progress once its manager is gone.
    fn drop(&mut self) {
        self.cancel();
    }
}

// ---------------------------------------------------------------------------
// Background task loop
// ---------------------------------------------------------------------------

async fn sftp_task_loop(
    _ssh: SshSession, // kept alive to hold the SSH connection open
    sftp: russh_sftp::client::SftpSession,
    mut cmd_rx: mpsc::Receiver<Queued>,
    event_tx: mpsc::Sender<CoreEvent>,
    cancels: Arc<Cancels>,
) {
    while let Some((sent, cmd)) = cmd_rx.recv().await {
        let cancel = Cancel {
            cancels: &cancels,
            sent,
        };
        match cmd {
            SftpCommand::ListDir(path) => match do_list_dir(&sftp, &path).await {
                Ok(entries) => {
                    let _ = event_tx
                        .send(CoreEvent::FileDirListed { path, entries })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(CoreEvent::SftpDisconnected {
                            reason: format!("ListDir failed: {e:#}"),
                        })
                        .await;
                }
            },

            SftpCommand::Download {
                remote,
                local,
                transfer_id,
            } => {
                let result = do_download(&sftp, &remote, &local, transfer_id, &event_tx, &cancel)
                    .await
                    .map_err(|e| format!("{e:#}"));
                let _ = event_tx.send(CoreEvent::SftpOpDone { result }).await;
            }

            SftpCommand::Upload {
                local,
                remote,
                transfer_id,
            } => {
                let result = do_upload(&local, &sftp, &remote, transfer_id, &event_tx, &cancel)
                    .await
                    .map_err(|e| format!("{e:#}"));
                let _ = event_tx.send(CoreEvent::SftpOpDone { result }).await;
            }

            SftpCommand::Delete(path) => {
                // Try remove_file first; on failure try remove_dir (empty dirs only).
                let result = match sftp.remove_file(&path).await {
                    Ok(()) => Ok(()),
                    Err(_) => sftp.remove_dir(&path).await.map_err(|e| e.to_string()),
                };
                let _ = event_tx.send(CoreEvent::SftpOpDone { result }).await;
            }

            SftpCommand::MkDir(path) => {
                let result = sftp.create_dir(&path).await.map_err(|e| e.to_string());
                let _ = event_tx.send(CoreEvent::SftpOpDone { result }).await;
            }

            SftpCommand::Rename { from, to } => {
                let result = sftp.rename(&from, &to).await.map_err(|e| e.to_string());
                let _ = event_tx.send(CoreEvent::SftpOpDone { result }).await;
            }

            SftpCommand::ReadPreview(path) => {
                if let Ok(content) = do_read_preview(&sftp, &path).await {
                    let _ = event_tx
                        .send(CoreEvent::FilePreviewReady { path, content })
                        .await;
                }
            }

            SftpCommand::Disconnect => break,
        }
    }
}

// ---------------------------------------------------------------------------
// SFTP helpers
// ---------------------------------------------------------------------------

async fn do_list_dir(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
) -> anyhow::Result<Vec<FileEntry>> {
    let read_dir = sftp
        .read_dir(path)
        .await
        .with_context(|| format!("read remote dir '{path}'"))?;

    let mut entries: Vec<FileEntry> = Vec::new();

    // ".." parent entry (omit at root "/")
    if let Some(parent) = std::path::Path::new(path).parent() {
        let parent_str = parent.to_string_lossy();
        let parent_str = if parent_str.is_empty() {
            "/"
        } else {
            &parent_str
        };
        entries.push(FileEntry::parent(parent_str));
    }

    for entry in read_dir {
        let name = entry.file_name();
        let ft = entry.file_type();
        let meta = entry.metadata();

        let full_path = join_remote(path, &name);

        entries.push(FileEntry {
            name,
            path: full_path,
            size: meta.size.unwrap_or(0),
            is_dir: ft.is_dir(),
            modified: meta.mtime.map(i64::from),
        });
    }

    // Sort: ".." first, then dirs, then files — all alphabetically.
    entries.sort_by(|a, b| {
        if a.name == ".." {
            return std::cmp::Ordering::Less;
        }
        if b.name == ".." {
            return std::cmp::Ordering::Greater;
        }
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    Ok(entries)
}

/// `name` joined onto the remote directory `dir`.
fn join_remote(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Whether a name the server gave can be created locally as itself: one plain path
/// component, so it neither climbs out of the destination nor, as Windows' `C:x` does,
/// replaces it.
fn is_safe_name(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    let single = matches!(
        (parts.next(), parts.next()),
        (Some(std::path::Component::Normal(part)), None) if part == name
    );
    single && !name.contains('\0') && (!cfg!(windows) || is_windows_name(name))
}

/// Windows' own rules: no `:` (drives, alternate streams) or other reserved character,
/// no trailing dot or space (Win32 drops them, so `a.` would overwrite `a`), and no
/// device name, with or without an extension (`aux.c` opens the AUX device).
fn is_windows_name(name: &str) -> bool {
    if name.chars().any(|c| c < ' ' || "<>:\"/\\|?*".contains(c)) || name.ends_with(['.', ' ']) {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    let device = match upper.get(..3) {
        Some("CON" | "PRN" | "AUX" | "NUL") => upper.len() == 3,
        Some("COM" | "LPT") => {
            let port = &upper[3..];
            port.chars().count() == 1 && "0123456789\u{b9}\u{b2}\u{b3}".contains(port)
        }
        _ => false,
    };
    !device
}

/// `name` from the server joined onto the local folder `dir`, unless it would be
/// anything but a direct child of `dir`.
fn join_local(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(name);
    (is_safe_name(name) && path.parent() == Some(dir)).then_some(path)
}

/// Rejects a download whose own name, as the server lists it or as it ends `local`,
/// could not be created here as itself: the frontends build `local` from that name, and
/// on Windows `a\b` would turn into a folder and a file.
fn check_download_name(remote: &str, local: &str) -> anyhow::Result<()> {
    let local_name = Path::new(local).file_name().and_then(|n| n.to_str());
    for name in [remote.rsplit('/').next(), local_name] {
        let name = name.unwrap_or_default();
        if !is_safe_name(name) {
            anyhow::bail!("'{name}' is not a name that can be created here");
        }
    }
    Ok(())
}

/// Rejects a transfer whose local path climbs out with `..` or either path holds a
/// null byte.
fn check_paths(local: &str, remote: &str) -> anyhow::Result<()> {
    if Path::new(local)
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        anyhow::bail!("Local path contains '..': {local}");
    }
    if local.contains('\0') || remote.contains('\0') {
        anyhow::bail!("Path contains null bytes");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Transfers
// ---------------------------------------------------------------------------

/// How deep a folder transfer goes: past any real tree, short of one a server makes
/// up to never end.
const MAX_DEPTH: usize = 64;

/// How many times a manager's transfers were cancelled, and a wake-up for a step
/// waiting on that.
#[derive(Debug, Default)]
struct Cancels {
    count: AtomicU64,
    changed: Notify,
}

impl Cancels {
    fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    fn bump(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.changed.notify_waiters();
    }
}

/// Whether the transfer at hand was cancelled: the manager's cancel count has moved
/// on from the one its command was sent under.
struct Cancel<'a> {
    cancels: &'a Cancels,
    sent: u64,
}

impl Cancel<'_> {
    fn check(&self) -> anyhow::Result<()> {
        if self.cancels.count() == self.sent {
            Ok(())
        } else {
            Err(Cancelled.into())
        }
    }

    /// Resolves once the transfer is cancelled, for a step a server can keep going.
    async fn cancelled(&self) {
        loop {
            // Made before the check, so a cancel in between still wakes it.
            let changed = self.cancels.changed.notified();
            if self.check().is_err() {
                return;
            }
            changed.await;
        }
    }
}

/// The error a cancelled transfer ends with.
#[derive(Debug, thiserror::Error)]
#[error("Transfer cancelled")]
struct Cancelled;

/// What a folder transfer left out. It goes on past every skip and failure and
/// reports them in its one result.
#[derive(Debug, Default)]
struct Skipped {
    count: usize,
    first: Option<String>,
}

impl Skipped {
    fn add(&mut self, path: &str, reason: impl std::fmt::Display) {
        self.count += 1;
        self.first
            .get_or_insert_with(|| format!("'{path}': {reason}"));
    }

    fn into_result(self) -> anyhow::Result<()> {
        let Some(first) = self.first else {
            return Ok(());
        };
        let items = if self.count == 1 { "item" } else { "items" };
        anyhow::bail!("{} {items} skipped or failed, first {first}", self.count)
    }
}

/// One folder or file of a folder transfer.
#[derive(Debug, PartialEq)]
struct Planned {
    src: String,
    dst: String,
    size: u64,
    /// The source's permission bits, when known.
    mode: Option<u32>,
}

/// Everything a folder transfer creates, worked out before the first byte moves so
/// the progress bar covers the whole folder. `dirs` lists parents before children.
#[derive(Debug, Default)]
struct TreePlan {
    dirs: Vec<Planned>,
    files: Vec<Planned>,
}

impl TreePlan {
    /// Sizes come from the server, which may claim anything.
    fn total(&self) -> u64 {
        self.files
            .iter()
            .fold(0, |total, f| total.saturating_add(f.size))
    }
}

/// Byte progress across every file of one transfer.
struct Progress<'a> {
    transfer_id: TransferId,
    done: u64,
    total: u64,
    event_tx: &'a mpsc::Sender<CoreEvent>,
}

impl<'a> Progress<'a> {
    /// Starts the count. Its first tick tells the frontends that planning is over.
    async fn start(
        transfer_id: TransferId,
        total: u64,
        event_tx: &'a mpsc::Sender<CoreEvent>,
    ) -> Self {
        let mut progress = Self {
            transfer_id,
            done: 0,
            total,
            event_tx,
        };
        progress.advance(0).await;
        progress
    }

    async fn advance(&mut self, n: usize) {
        self.done += n as u64;
        let _ = self
            .event_tx
            .send(CoreEvent::FileTransferProgress(
                self.transfer_id,
                self.done,
                self.total,
            ))
            .await;
    }
}

/// A local file's permission bits; Windows has none to give.
fn local_mode(meta: &std::fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(meta.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

/// A remote entry's permission bits, when the server sent them.
fn remote_mode(attrs: &FileAttributes) -> Option<u32> {
    attrs.permissions.map(|p| p & 0o777)
}

/// Walks the local folder `root`, whose permission bits are `mode`, mapping it onto
/// the remote folder `dst_root`. Nothing is followed: a symlink is skipped, as is
/// anything else that is not a plain file or folder.
async fn plan_local_tree(
    root: &str,
    mode: Option<u32>,
    dst_root: &str,
    cancel: &Cancel<'_>,
    skipped: &mut Skipped,
) -> anyhow::Result<TreePlan> {
    let mut plan = TreePlan::default();
    let mut stack = vec![(PathBuf::from(root), dst_root.to_string(), mode, 0)];
    while let Some((src, dst, mode, depth)) = stack.pop() {
        cancel.check()?;
        // Every name on the way is UTF-8, so the path converts losslessly.
        let src_str = src.to_string_lossy().into_owned();
        let mut read_dir = match tokio::fs::read_dir(&src).await {
            Ok(read_dir) => read_dir,
            Err(e) => {
                skipped.add(&src_str, format_args!("read local dir: {e}"));
                continue;
            }
        };
        plan.dirs.push(Planned {
            src: src_str.clone(),
            dst: dst.clone(),
            size: 0,
            mode,
        });
        loop {
            let entry = match read_dir.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(e) => {
                    skipped.add(&src_str, format_args!("read local dir: {e}"));
                    break;
                }
            };
            let path = entry.path();
            let shown = path.to_string_lossy();
            let Ok(name) = entry.file_name().into_string() else {
                skipped.add(&shown, "name is not valid UTF-8");
                continue;
            };
            // `DirEntry::metadata` does not follow symlinks.
            let meta = match entry.metadata().await {
                Ok(meta) => meta,
                Err(e) => {
                    skipped.add(&shown, e);
                    continue;
                }
            };
            let remote = join_remote(&dst, &name);
            let file_type = meta.file_type();
            if file_type.is_dir() && depth < MAX_DEPTH {
                stack.push((path.clone(), remote, local_mode(&meta), depth + 1));
            } else if file_type.is_dir() {
                skipped.add(&shown, "nested too deep");
            } else if file_type.is_file() {
                plan.files.push(Planned {
                    src: shown.into_owned(),
                    dst: remote,
                    size: meta.len(),
                    mode: local_mode(&meta),
                });
            } else if file_type.is_symlink() {
                skipped.add(&shown, "symbolic link, not followed");
            } else {
                skipped.add(&shown, "not a regular file or folder");
            }
        }
    }
    Ok(plan)
}

/// Walks the remote folder `root`, whose permission bits are `mode`, mapping it onto
/// the local folder `dst_root`; `list` reads one remote folder. A symlink is skipped
/// (servers report links, not their targets), as is anything that is not a plain
/// file or folder and any name that cannot be created here.
async fn plan_remote_tree<F, Fut>(
    mut list: F,
    root: &str,
    mode: Option<u32>,
    dst_root: &Path,
    cancel: &Cancel<'_>,
    skipped: &mut Skipped,
) -> anyhow::Result<TreePlan>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = anyhow::Result<Vec<(String, FileAttributes)>>>,
{
    let mut plan = TreePlan::default();
    let mut stack = vec![(root.to_string(), dst_root.to_path_buf(), mode, 0)];
    while let Some((src, dst, mode, depth)) = stack.pop() {
        cancel.check()?;
        // A server can answer one listing for ever.
        let listing = tokio::select! {
            listing = list(src.clone()) => listing,
            () = cancel.cancelled() => return Err(Cancelled.into()),
        };
        let entries = match listing {
            Ok(entries) => entries,
            Err(e) => {
                skipped.add(&src, format_args!("read remote dir: {e:#}"));
                continue;
            }
        };
        plan.dirs.push(Planned {
            src: src.clone(),
            dst: dst.to_string_lossy().into_owned(),
            size: 0,
            mode,
        });
        for (name, attrs) in entries {
            let path = join_remote(&src, &name);
            let Some(local) = join_local(&dst, &name) else {
                skipped.add(&path, "name not allowed here");
                continue;
            };
            match attrs.file_type() {
                FileType::Dir if depth < MAX_DEPTH => {
                    stack.push((path, local, remote_mode(&attrs), depth + 1));
                }
                FileType::Dir => skipped.add(&path, "nested too deep"),
                FileType::File => plan.files.push(Planned {
                    src: path,
                    dst: local.to_string_lossy().into_owned(),
                    size: attrs.size.unwrap_or(0),
                    mode: remote_mode(&attrs),
                }),
                FileType::Symlink => skipped.add(&path, "symbolic link, not followed"),
                FileType::Other => skipped.add(&path, "not a regular file or folder"),
            }
        }
    }
    Ok(plan)
}

/// Downloads a remote file or, recursively, a remote folder to `local`.
async fn do_download(
    sftp: &russh_sftp::client::SftpSession,
    remote: &str,
    local: &str,
    transfer_id: TransferId,
    event_tx: &mpsc::Sender<CoreEvent>,
    cancel: &Cancel<'_>,
) -> anyhow::Result<()> {
    check_paths(local, remote)?;
    check_download_name(remote, local)?;
    cancel.check()?;

    // Size, mode and type best-effort: a failed stat downloads as a file, and the open
    // below reports why. Without permissions the type is unknown too.
    let meta = sftp.metadata(remote).await.ok();
    let mode = meta.as_ref().and_then(remote_mode);
    match meta
        .as_ref()
        .map(|m| (m.permissions.is_some(), m.file_type()))
    {
        Some((_, FileType::Dir)) => {}
        Some((true, FileType::Symlink | FileType::Other)) => {
            anyhow::bail!("'{remote}' is not a regular file or folder")
        }
        _ => {
            let total = meta.and_then(|m| m.size).unwrap_or(0);
            let mut progress = Progress::start(transfer_id, total, event_tx).await;
            return download_file(sftp, remote, local, mode, &mut progress, cancel).await;
        }
    }

    let mut skipped = Skipped::default();
    let list = |dir: String| async move {
        let entries = sftp.read_dir(dir).await?;
        anyhow::Ok(entries.map(|e| (e.file_name(), e.metadata())).collect())
    };
    let plan = plan_remote_tree(list, remote, mode, Path::new(local), cancel, &mut skipped).await?;
    let mut progress = Progress::start(transfer_id, plan.total(), event_tx).await;
    for dir in &plan.dirs {
        cancel.check()?;
        if let Err(e) = create_local_dir(&dir.dst, dir.mode).await {
            skipped.add(&dir.src, format_args!("create local dir: {e}"));
        }
    }
    for file in &plan.files {
        match download_file(sftp, &file.src, &file.dst, file.mode, &mut progress, cancel).await {
            Err(e) if e.is::<Cancelled>() => return Err(e),
            Err(e) => skipped.add(&file.src, format_args!("{e:#}")),
            Ok(()) => {}
        }
    }
    skipped.into_result()
}

async fn download_file(
    sftp: &russh_sftp::client::SftpSession,
    remote: &str,
    local: &str,
    mode: Option<u32>,
    progress: &mut Progress<'_>,
    cancel: &Cancel<'_>,
) -> anyhow::Result<()> {
    cancel.check()?;
    let remote_file = sftp
        .open(remote)
        .await
        .context("open remote file for download")?;
    write_local(remote_file, local, mode, progress, cancel).await
}

/// Writes everything `source` yields to the local file `local`. A cancel or a failure
/// removes the part written so far, which would otherwise pass for the whole file.
async fn write_local(
    mut source: impl AsyncRead + Unpin,
    local: &str,
    mode: Option<u32>,
    progress: &mut Progress<'_>,
    cancel: &Cancel<'_>,
) -> anyhow::Result<()> {
    // A cancel during the remote open leaves a file already there as it was.
    cancel.check()?;
    let mut local_file = create_local_file(local, mode)
        .await
        .context("create local file")?;

    let mut buf = vec![0u8; 65_536];
    let copied = async {
        loop {
            cancel.check()?;
            // A stalled server would otherwise hold a cancel until its request times out.
            let n = tokio::select! {
                n = source.read(&mut buf) => n.context("read remote file")?,
                () = cancel.cancelled() => return Err(Cancelled.into()),
            };
            if n == 0 {
                return Ok(());
            }
            local_file
                .write_all(&buf[..n])
                .await
                .context("write local file")?;
            progress.advance(n).await;
        }
    }
    .await;

    // The last write may still be in flight; this is where it fails, if it does.
    let flushed = local_file.flush().await.context("write local file");
    let result = copied.and(flushed);
    if result.is_err() {
        drop(local_file);
        let _ = tokio::fs::remove_file(local).await;
    }
    result
}

/// Creates or truncates the local file `path`. A new file gets the permission bits
/// `mode` less the umask, as sftp gives it, owner write included so the next transfer
/// can replace it; an existing one keeps its own.
async fn create_local_file(path: &str, mode: Option<u32>) -> std::io::Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if let Some(mode) = mode {
        options.mode(mode | 0o200);
    }
    #[cfg(not(unix))]
    let _ = mode;
    options.open(path).await
}

/// Creates the local folder `path`, or merges into the one there. A new folder gets
/// the permission bits `mode` less the umask, owner access added so it can be filled.
async fn create_local_dir(path: &str, mode: Option<u32>) -> std::io::Result<()> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    if let Some(mode) = mode {
        builder.mode(mode | 0o700);
    }
    #[cfg(not(unix))]
    let _ = mode;
    builder.create(path).await
}

/// Uploads a local file or, recursively, a local folder to `remote`.
async fn do_upload(
    local: &str,
    sftp: &russh_sftp::client::SftpSession,
    remote: &str,
    transfer_id: TransferId,
    event_tx: &mpsc::Sender<CoreEvent>,
    cancel: &Cancel<'_>,
) -> anyhow::Result<()> {
    check_paths(local, remote)?;
    cancel.check()?;

    // Follows a symlink: a linked folder the user picked is uploaded as a folder.
    let meta = tokio::fs::metadata(local)
        .await
        .context("open local file for upload")?;
    let mode = local_mode(&meta);
    if meta.is_file() {
        let mut progress = Progress::start(transfer_id, meta.len(), event_tx).await;
        return upload_file(local, sftp, remote, mode, &mut progress, cancel).await;
    }
    if !meta.is_dir() {
        anyhow::bail!("'{local}' is not a regular file or folder");
    }

    let mut skipped = Skipped::default();
    let plan = plan_local_tree(local, mode, remote, cancel, &mut skipped).await?;
    let mut progress = Progress::start(transfer_id, plan.total(), event_tx).await;
    for dir in &plan.dirs {
        cancel.check()?;
        if let Err(e) = create_remote_dir(sftp, &dir.dst, dir.mode).await {
            skipped.add(&dir.src, format_args!("{e:#}"));
        }
    }
    for file in &plan.files {
        match upload_file(&file.src, sftp, &file.dst, file.mode, &mut progress, cancel).await {
            Err(e) if e.is::<Cancelled>() => return Err(e),
            Err(e) => skipped.add(&file.src, format_args!("{e:#}")),
            Ok(()) => {}
        }
    }
    skipped.into_result()
}

/// Creates the remote folder `path`, or merges into the one there. A new folder gets
/// the permission bits `mode`, owner access added so it can be filled.
async fn create_remote_dir(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
    mode: Option<u32>,
) -> anyhow::Result<()> {
    if let Err(e) = sftp.create_dir(path).await {
        if sftp
            .metadata(path)
            .await
            .is_ok_and(|m| m.file_type().is_dir())
        {
            return Ok(());
        }
        return Err(e).context("create remote dir");
    }
    // SFTP's mkdir takes a mode, but this client sends none. A server may refuse to set
    // one; the folder is there all the same.
    if let Some(mode) = mode {
        let attrs = FileAttributes {
            permissions: Some(mode | 0o700),
            ..FileAttributes::empty()
        };
        let _ = sftp.set_metadata(path, attrs).await;
    }
    Ok(())
}

async fn upload_file(
    local: &str,
    sftp: &russh_sftp::client::SftpSession,
    remote: &str,
    mode: Option<u32>,
    progress: &mut Progress<'_>,
    cancel: &Cancel<'_>,
) -> anyhow::Result<()> {
    cancel.check()?;
    let mut local_file = tokio::fs::File::open(local)
        .await
        .context("open local file for upload")?;
    // A new file gets the source's mode less the server's umask, owner write included
    // so the next upload can replace it; an existing one keeps its own.
    let attrs = FileAttributes {
        permissions: mode.map(|mode| mode | 0o200),
        ..FileAttributes::empty()
    };
    let flags = OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE;
    let mut remote_file = sftp
        .open_with_flags_and_attributes(remote, flags, attrs)
        .await
        .context("create remote file for upload")?;

    let mut buf = vec![0u8; 65_536];
    let copied = async {
        loop {
            cancel.check()?;
            let n = local_file.read(&mut buf).await.context("read local file")?;
            if n == 0 {
                return Ok(());
            }
            // A stalled server would otherwise hold a cancel until its request times out.
            tokio::select! {
                written = remote_file.write_all(&buf[..n]) => {
                    written.context("write remote file")?;
                }
                () = cancel.cancelled() => return Err(Cancelled.into()),
            }
            progress.advance(n).await;
        }
    }
    .await;

    if copied.is_err() {
        // Bounded: a server that stalled the copy may not answer these either.
        let _ = time::timeout(Duration::from_secs(5), async {
            // Closed first, or the handle would close only after the removal.
            let _ = remote_file.shutdown().await;
            let _ = sftp.remove_file(remote).await;
        })
        .await;
    }
    copied
}

async fn do_read_preview(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
) -> anyhow::Result<String> {
    let mut file = sftp.open(path).await.context("open for preview")?;
    let mut buf = vec![0u8; 4_096];
    let n = file.read(&mut buf).await.context("read preview bytes")?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

// ---------------------------------------------------------------------------
// Local filesystem helpers (called via inline tokio::spawn in App)
// ---------------------------------------------------------------------------

/// Lists the entries of a local directory, sorted dirs-first then alphabetically.
///
/// Prepends a `".."` entry for the parent directory (omitted at filesystem root).
///
/// # Errors
/// Returns an error if the directory cannot be read (e.g. permission denied).
pub async fn list_local_dir(path: &str) -> anyhow::Result<Vec<FileEntry>> {
    let mut read_dir = tokio::fs::read_dir(path)
        .await
        .with_context(|| format!("read local dir '{path}'"))?;

    let mut entries: Vec<FileEntry> = Vec::new();

    // ".." parent entry.
    if let Some(parent) = std::path::Path::new(path).parent() {
        let parent_str = parent.to_string_lossy();
        let parent_str = if parent_str.is_empty() {
            "/"
        } else {
            &parent_str
        };
        entries.push(FileEntry::parent(parent_str));
    }

    while let Some(entry) = read_dir
        .next_entry()
        .await
        .context("read local dir entry")?
    {
        let file_type = entry.file_type().await.ok();
        let is_dir = file_type.as_ref().map(|ft| ft.is_dir()).unwrap_or(false);
        let meta = entry.metadata().await.ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);

        let name = entry.file_name().to_string_lossy().into_owned();
        let path_str = entry.path().to_string_lossy().into_owned();

        entries.push(FileEntry {
            name,
            path: path_str,
            size,
            is_dir,
            modified: meta.and_then(|m| unix_secs(m.modified())),
        });
    }

    // Sort: ".." first, then dirs, then files — case-insensitive alphabetically.
    entries.sort_by(|a, b| {
        if a.name == ".." {
            return std::cmp::Ordering::Less;
        }
        if b.name == ".." {
            return std::cmp::Ordering::Greater;
        }
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    Ok(entries)
}

/// The roots the local file system can be browsed from: every drive letter on
/// Windows, where `..` stops at the drive the pane is on, and `/` elsewhere.
pub fn local_roots() -> Vec<String> {
    #[cfg(windows)]
    {
        // SAFETY: GetLogicalDrives takes no arguments and only returns a bitmask.
        let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
        drive_roots(mask)
    }
    #[cfg(not(windows))]
    {
        vec!["/".to_string()]
    }
}

/// `C:\`-style roots for the drives set in `mask` (bit 0 is `A:`).
#[cfg_attr(not(windows), allow(dead_code))]
fn drive_roots(mask: u32) -> Vec<String> {
    (b'A'..=b'Z')
        .enumerate()
        .filter(|(bit, _)| mask & (1 << bit) != 0)
        .map(|(_, letter)| format!("{}:\\", letter as char))
        .collect()
}

/// Reads up to 4 096 bytes from a local file and returns them as a UTF-8 string.
///
/// Non-UTF-8 bytes are replaced with the Unicode replacement character.
///
/// # Errors
/// Returns an error if the file cannot be opened or read.
pub async fn preview_local_file(path: &str) -> anyhow::Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .context("open local file for preview")?;
    let mut buf = vec![0u8; 4_096];
    let n = file
        .read(&mut buf)
        .await
        .context("read local preview bytes")?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_roots_follow_the_mask() {
        assert_eq!(drive_roots(0b1100), ["C:\\", "D:\\"]);
        assert_eq!(drive_roots(1 | 1 << 25), ["A:\\", "Z:\\"]);
        assert!(drive_roots(0).is_empty());
    }

    fn no_cancel(cancels: &Cancels) -> Cancel<'_> {
        Cancel { cancels, sent: 0 }
    }

    fn attrs(permissions: u32, size: u64) -> FileAttributes {
        FileAttributes {
            permissions: Some(permissions),
            size: Some(size),
            ..FileAttributes::empty()
        }
    }

    #[tokio::test]
    async fn local_listing_carries_modification_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"hello").expect("write scratch file");

        let entries = list_local_dir(&dir.path().to_string_lossy())
            .await
            .expect("list scratch dir");
        let file = entries
            .iter()
            .find(|e| e.name == "a.txt")
            .expect("file listed");
        assert_eq!(file.size, 5);
        assert!(!file.is_dir);
        assert!(file.modified.is_some_and(|t| t > 0));
        let parent = entries.first().expect("parent entry");
        assert_eq!(parent.name, "..");
        assert!(parent.modified.is_none());
    }

    #[tokio::test]
    async fn the_local_plan_maps_files_and_folders_and_reports_the_rest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("sub").join("deep")).expect("create scratch tree");
        std::fs::write(dir.join("a.txt"), b"abc").expect("write scratch file");
        std::fs::write(dir.join("sub").join("deep").join("b.bin"), b"hello")
            .expect("write scratch file");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir, dir.join("sub").join("loop")).expect("symlink");
            std::os::unix::net::UnixListener::bind(dir.join("sock")).expect("socket");
        }

        let cancels = Cancels::default();
        let mut skipped = Skipped::default();
        let root = dir.to_str().expect("utf-8 temp dir");
        let plan = plan_local_tree(
            root,
            Some(0o750),
            "/srv/up",
            &no_cancel(&cancels),
            &mut skipped,
        )
        .await
        .expect("plan tree");
        assert_eq!(plan.dirs[0].dst, "/srv/up");
        assert_eq!(plan.dirs[0].mode, Some(0o750));
        let mut dirs: Vec<_> = plan.dirs.iter().map(|d| d.dst.as_str()).collect();
        dirs.sort();
        assert_eq!(dirs, ["/srv/up", "/srv/up/sub", "/srv/up/sub/deep"]);
        // Parents come before their children, so creating in order never fails.
        let sub = plan.dirs.iter().position(|d| d.dst == "/srv/up/sub");
        let deep = plan.dirs.iter().position(|d| d.dst == "/srv/up/sub/deep");
        assert!(sub < deep);
        let mut files: Vec<_> = plan
            .files
            .iter()
            .map(|f| (f.dst.as_str(), f.size, f.mode.is_some()))
            .collect();
        files.sort();
        let moded = cfg!(unix);
        assert_eq!(
            files,
            [
                ("/srv/up/a.txt", 3, moded),
                ("/srv/up/sub/deep/b.bin", 5, moded)
            ]
        );
        assert_eq!(plan.total(), 8);
        // The link back up and the socket are left out, and said so.
        assert_eq!(skipped.count, if cfg!(unix) { 2 } else { 0 });
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_local_name_that_is_not_utf8_is_reported_not_fatal() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("ok.txt"), b"ok").expect("write");
        let bad = std::ffi::OsStr::from_bytes(b"bad\xff.txt");
        std::fs::write(tmp.path().join(bad), b"bad").expect("write");

        let cancels = Cancels::default();
        let mut skipped = Skipped::default();
        let root = tmp.path().to_str().expect("utf-8 temp dir");
        let plan = plan_local_tree(root, None, "/up", &no_cancel(&cancels), &mut skipped)
            .await
            .expect("plan tree");
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].dst, "/up/ok.txt");
        let err = skipped.into_result().expect_err("the bad name is reported");
        assert!(
            err.to_string().ends_with("': name is not valid UTF-8"),
            "{err}"
        );
    }

    /// A remote file system: folder -> listing. A folder not in it cannot be read.
    fn fake_server(
        tree: Vec<(&'static str, Vec<(&'static str, FileAttributes)>)>,
    ) -> impl FnMut(String) -> std::future::Ready<anyhow::Result<Vec<(String, FileAttributes)>>>
    {
        move |dir| {
            let listing = tree
                .iter()
                .find(|(path, _)| *path == dir)
                .map(|(_, entries)| {
                    entries
                        .iter()
                        .map(|(name, attrs)| (name.to_string(), attrs.clone()))
                        .collect()
                });
            std::future::ready(listing.ok_or_else(|| anyhow::anyhow!("Permission denied")))
        }
    }

    #[tokio::test]
    async fn the_remote_plan_maps_a_tree_and_reports_what_it_leaves_out() {
        let server = fake_server(vec![
            (
                "/srv/app",
                vec![
                    ("run.sh", attrs(0o100_755, 3)),
                    ("conf", attrs(0o40_700, 0)),
                    ("current", attrs(0o120_777, 0)),
                    ("fifo", attrs(0o10_644, 0)),
                    ("a/b", attrs(0o100_644, 1)),
                ],
            ),
            (
                "/srv/app/conf",
                vec![
                    ("app.toml", attrs(0o100_600, 5)),
                    ("locked", attrs(0o40_000, 0)),
                ],
            ),
        ]);
        let dst = Path::new("dl").join("app");
        let cancels = Cancels::default();
        let mut skipped = Skipped::default();
        let plan = plan_remote_tree(
            server,
            "/srv/app",
            Some(0o755),
            &dst,
            &no_cancel(&cancels),
            &mut skipped,
        )
        .await
        .expect("plan tree");

        let local = |rel: &[&str]| {
            rel.iter()
                .fold(dst.clone(), |p, part| p.join(part))
                .to_string_lossy()
                .into_owned()
        };
        let dirs: Vec<_> = plan.dirs.iter().map(|d| (d.dst.clone(), d.mode)).collect();
        assert_eq!(
            dirs,
            [(local(&[]), Some(0o755)), (local(&["conf"]), Some(0o700))]
        );
        assert_eq!(
            plan.files,
            [
                Planned {
                    src: "/srv/app/run.sh".into(),
                    dst: local(&["run.sh"]),
                    size: 3,
                    mode: Some(0o755),
                },
                Planned {
                    src: "/srv/app/conf/app.toml".into(),
                    dst: local(&["conf", "app.toml"]),
                    size: 5,
                    mode: Some(0o600),
                },
            ]
        );
        // The symlink, the FIFO, the name with a separator and the unreadable folder:
        // left out, counted, and the first one named.
        assert_eq!(skipped.count, 4);
        let err = skipped.into_result().expect_err("skips are reported");
        assert_eq!(
            err.to_string(),
            "4 items skipped or failed, first '/srv/app/current': symbolic link, not followed"
        );
    }

    #[test]
    fn sizes_a_server_makes_up_do_not_overflow_the_total() {
        let file = |size| Planned {
            src: String::new(),
            dst: String::new(),
            size,
            mode: None,
        };
        let plan = TreePlan {
            dirs: Vec::new(),
            files: vec![file(u64::MAX), file(2)],
        };
        assert_eq!(plan.total(), u64::MAX);
    }

    #[tokio::test]
    async fn the_remote_plan_stops_at_the_depth_cap() {
        // A server that makes up one more folder at every level.
        let server = |_dir: String| {
            std::future::ready(anyhow::Ok(vec![("d".to_string(), attrs(0o40_755, 0))]))
        };
        let cancels = Cancels::default();
        let mut skipped = Skipped::default();
        let plan = plan_remote_tree(
            server,
            "/x",
            None,
            Path::new("dl"),
            &no_cancel(&cancels),
            &mut skipped,
        )
        .await
        .expect("plan tree");
        assert_eq!(plan.dirs.len(), MAX_DEPTH + 1);
        let err = skipped.into_result().expect_err("the cut is reported");
        assert!(err
            .to_string()
            .starts_with("1 item skipped or failed, first '/x/d/d/"));
        assert!(err.to_string().ends_with("/d': nested too deep"), "{err}");
    }

    #[tokio::test]
    async fn planning_stops_at_the_next_folder_once_cancelled() {
        let cancels = Cancels::default();
        let server = |_dir: String| {
            cancels.bump();
            std::future::ready(anyhow::Ok(vec![("d".to_string(), attrs(0o40_755, 0))]))
        };
        let mut skipped = Skipped::default();
        let err = plan_remote_tree(
            server,
            "/x",
            None,
            Path::new("dl"),
            &no_cancel(&cancels),
            &mut skipped,
        )
        .await
        .expect_err("cancelled");
        assert!(err.is::<Cancelled>());
        assert_eq!(cancels.count(), 1, "one folder read, no more");
    }

    #[tokio::test]
    async fn a_cancel_stops_a_listing_that_never_ends() {
        let cancels = Cancels::default();
        // A server that keeps the first listing going.
        let server =
            |_dir: String| std::future::pending::<anyhow::Result<Vec<(String, FileAttributes)>>>();
        let cancel = no_cancel(&cancels);
        let mut skipped = Skipped::default();
        let plan = plan_remote_tree(server, "/x", None, Path::new("dl"), &cancel, &mut skipped);
        let press_cancel = async {
            cancels.bump();
            std::future::pending::<()>().await
        };
        let stopped = time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                biased;
                result = plan => result,
                () = press_cancel => unreachable!("never ends"),
            }
        })
        .await
        .expect("planning stopped");
        assert!(stopped.expect_err("cancelled").is::<Cancelled>());
    }

    #[test]
    fn skips_are_counted_and_the_first_reason_kept_whole() {
        assert!(Skipped::default().into_result().is_ok());

        let mut skipped = Skipped::default();
        let cause = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let err = anyhow::Error::from(cause).context("open remote file for download");
        skipped.add("/srv/a", format_args!("{err:#}"));
        let one = format!(
            "1 item skipped or failed, first '/srv/a': open remote file for download: {}",
            std::io::Error::from(std::io::ErrorKind::PermissionDenied)
        );
        let mut again = Skipped::default();
        again.add("/srv/a", format_args!("{err:#}"));
        assert_eq!(again.into_result().expect_err("one").to_string(), one);

        skipped.add("/srv/b", "symbolic link, not followed");
        assert_eq!(
            skipped.into_result().expect_err("two").to_string(),
            one.replacen("1 item", "2 items", 1)
        );
    }

    #[test]
    fn a_cancel_stops_what_was_sent_before_it_and_a_closed_manager_stops_all() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(8);
        let manager = SftpManager {
            cmd_tx,
            cancels: Arc::default(),
        };
        let cancels = Arc::clone(&manager.cancels);
        let cancelled = |sent| {
            Cancel {
                cancels: &cancels,
                sent,
            }
            .check()
            .is_err_and(|e| e.is::<Cancelled>())
        };

        manager.send(SftpCommand::ListDir("/before".into()));
        manager.cancel();
        manager.send(SftpCommand::ListDir("/after".into()));
        let (before, _) = cmd_rx.try_recv().expect("queued");
        let (after, _) = cmd_rx.try_recv().expect("queued");
        assert!(cancelled(before));
        assert!(!cancelled(after));

        manager.disconnect();
        assert!(cancelled(after));
        assert!(matches!(
            cmd_rx.try_recv(),
            Ok((_, SftpCommand::Disconnect))
        ));
    }

    /// Hands out `data` in one read and cancels the transfer as it does.
    struct CancellingReader<'a> {
        data: &'a [u8],
        cancels: &'a Cancels,
    }

    impl AsyncRead for CancellingReader<'_> {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let n = self.data.len().min(buf.remaining());
            buf.put_slice(&self.data[..n]);
            self.data = &self.data[n..];
            self.cancels.bump();
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_cancelled_download_removes_the_part_it_wrote() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let local = tmp.path().join("part.bin");
        let cancels = Cancels::default();
        let (tx, _rx) = mpsc::channel(8);
        let mut progress = Progress::start(1, 100, &tx).await;
        let source = CancellingReader {
            data: b"the first chunk of many",
            cancels: &cancels,
        };

        let local_str = local.to_str().expect("utf-8 temp dir");
        let err = write_local(source, local_str, None, &mut progress, &no_cancel(&cancels))
            .await
            .expect_err("cancelled");
        assert!(err.is::<Cancelled>());
        assert_eq!(progress.done, 23, "the chunk was written before the cancel");
        assert!(!local.exists(), "the partial file is gone");
    }

    #[tokio::test]
    async fn a_cancel_before_the_first_byte_leaves_the_local_file_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let local = tmp.path().join("keep.txt");
        std::fs::write(&local, "the user's own").expect("write");
        let cancels = Cancels::default();
        let cancel = no_cancel(&cancels);
        cancels.bump();
        let (tx, _rx) = mpsc::channel(8);
        let mut progress = Progress::start(1, 100, &tx).await;

        let local_str = local.to_str().expect("utf-8 temp dir");
        let err = write_local(&b"new"[..], local_str, None, &mut progress, &cancel)
            .await
            .expect_err("cancelled");
        assert!(err.is::<Cancelled>());
        assert_eq!(
            std::fs::read(&local).expect("still there"),
            b"the user's own"
        );
    }

    /// Hands out `data` once, then stalls like a server that stopped answering, or
    /// fails like a dropped connection.
    struct StoppingReader<'a> {
        data: &'a [u8],
        fail: bool,
    }

    impl AsyncRead for StoppingReader<'_> {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.data.is_empty() {
                return if self.fail {
                    std::task::Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()))
                } else {
                    std::task::Poll::Pending
                };
            }
            let n = self.data.len().min(buf.remaining());
            buf.put_slice(&self.data[..n]);
            self.data = &self.data[n..];
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_cancel_stops_a_download_the_server_stalled() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let local = tmp.path().join("stalled.bin");
        let cancels = Cancels::default();
        let cancel = no_cancel(&cancels);
        let (tx, _rx) = mpsc::channel(8);
        let mut progress = Progress::start(1, 100, &tx).await;
        let source = StoppingReader {
            data: b"a first chunk",
            fail: false,
        };

        let local_str = local.to_str().expect("utf-8 temp dir");
        let copy = write_local(source, local_str, None, &mut progress, &cancel);
        let press_cancel = async {
            time::sleep(Duration::from_millis(50)).await;
            cancels.bump();
            std::future::pending::<()>().await
        };
        let err = time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                biased;
                result = copy => result,
                () = press_cancel => unreachable!("never ends"),
            }
        })
        .await
        .expect("the cancel did not wait for the server")
        .expect_err("cancelled");
        assert!(err.is::<Cancelled>());
        assert!(!local.exists(), "the partial file is gone");
    }

    #[tokio::test]
    async fn a_download_that_breaks_off_leaves_no_partial_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let local = tmp.path().join("cut.bin");
        let cancels = Cancels::default();
        let (tx, _rx) = mpsc::channel(8);
        let mut progress = Progress::start(1, 100, &tx).await;
        let source = StoppingReader {
            data: b"a first chunk",
            fail: true,
        };

        let local_str = local.to_str().expect("utf-8 temp dir");
        let err = write_local(source, local_str, None, &mut progress, &no_cancel(&cancels))
            .await
            .expect_err("broke off");
        assert!(!err.is::<Cancelled>());
        assert!(!local.exists(), "no cut-off file passes for the whole one");
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_download_keeps_the_source_mode() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cancels = Cancels::default();
        let (tx, _rx) = mpsc::channel(8);
        let mut progress = Progress::start(1, 10, &tx).await;

        let script = tmp.path().join("run.sh");
        let script_str = script.to_str().expect("utf-8 temp dir");
        write_local(
            &b"#!/bin/sh\n"[..],
            script_str,
            Some(0o755),
            &mut progress,
            &no_cancel(&cancels),
        )
        .await
        .expect("written");
        assert_eq!(std::fs::read(&script).expect("read"), b"#!/bin/sh\n");
        // The umask may take group and other bits, never the owner's.
        assert_eq!(mode_of(&script) & 0o700, 0o700);

        // A read-only folder still gets the owner's access, to be filled.
        let dir = tmp.path().join("ro");
        create_local_dir(dir.to_str().expect("utf-8"), Some(0o500))
            .await
            .expect("created");
        assert_eq!(mode_of(&dir) & 0o700, 0o700);
        // An existing folder is merged into.
        create_local_dir(dir.to_str().expect("utf-8"), Some(0o755))
            .await
            .expect("merged");
    }

    /// An SFTP session on this machine's own files through OpenSSH's sftp-server, run
    /// with `args`, where one is installed.
    #[cfg(unix)]
    async fn local_sftp(args: &[&str]) -> Option<russh_sftp::client::SftpSession> {
        let server = [
            "/usr/lib/openssh/sftp-server",
            "/usr/libexec/openssh/sftp-server",
            "/usr/libexec/sftp-server",
        ]
        .into_iter()
        .find(|path| Path::new(path).exists())?;
        let mut child = tokio::process::Command::new(server)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        // The server exits once the session drops its end of the pipes.
        let pipes = tokio::io::join(child.stdout.take()?, child.stdin.take()?);
        russh_sftp::client::SftpSession::new(pipes).await.ok()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_folder_round_trips_through_sftp_with_its_modes_and_skips_reported() {
        use std::os::unix::fs::PermissionsExt;
        let Some(sftp) = local_sftp(&[]).await else {
            eprintln!("no sftp-server on this machine; skipped");
            return;
        };
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = |rel: &str| tmp.path().join(rel);
        let str_of = |p: &Path| p.to_str().expect("utf-8 temp dir").to_string();
        let set_mode = |p: PathBuf, mode| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).expect("chmod")
        };
        std::fs::create_dir_all(path("src/private")).expect("mkdir");
        std::fs::write(path("src/run.sh"), "#!/bin/sh\n").expect("write");
        set_mode(path("src/run.sh"), 0o755);
        std::fs::write(path("src/private/data.bin"), vec![7u8; 200_000]).expect("write");
        set_mode(path("src/private"), 0o700);
        std::os::unix::fs::symlink(path("src/run.sh"), path("src/link")).expect("symlink");

        let cancels = Cancels::default();
        let (tx, mut rx) = mpsc::channel(64);
        let err = do_upload(
            &str_of(&path("src")),
            &sftp,
            &str_of(&path("up")),
            1,
            &tx,
            &no_cancel(&cancels),
        )
        .await
        .expect_err("the link is reported");
        let link = str_of(&path("src/link"));
        assert_eq!(
            err.to_string(),
            format!("1 item skipped or failed, first '{link}': symbolic link, not followed")
        );
        assert_eq!(
            std::fs::read(path("up/private/data.bin"))
                .expect("read")
                .len(),
            200_000
        );
        assert!(!path("up/link").exists());
        assert_eq!(
            mode_of(&path("up/run.sh")) & 0o700,
            0o700,
            "still executable"
        );
        assert_eq!(mode_of(&path("up/private")), 0o700, "still private");
        // The first tick (nothing done yet) marks the end of planning.
        assert!(matches!(
            rx.try_recv(),
            Ok(CoreEvent::FileTransferProgress(1, 0, 200_010))
        ));

        // Back down, into a folder that already holds one of the files.
        std::fs::create_dir_all(path("down")).expect("mkdir");
        std::fs::write(path("down/run.sh"), "old").expect("write");
        do_download(
            &sftp,
            &str_of(&path("up")),
            &str_of(&path("down")),
            2,
            &tx,
            &no_cancel(&cancels),
        )
        .await
        .expect("downloaded");
        assert_eq!(
            std::fs::read(path("down/run.sh")).expect("read"),
            b"#!/bin/sh\n"
        );
        assert_eq!(
            std::fs::read(path("down/private/data.bin"))
                .expect("read")
                .len(),
            200_000
        );
        assert_eq!(mode_of(&path("down/private")), 0o700);

        // An unreadable folder is reported with the server's reason; the rest still
        // arrives. Root reads it anyway, so only a plain user can check this.
        set_mode(path("up/private"), 0o000);
        let readable = std::fs::read_dir(path("up/private")).is_ok();
        let result = do_download(
            &sftp,
            &str_of(&path("up")),
            &str_of(&path("again")),
            3,
            &tx,
            &no_cancel(&cancels),
        )
        .await;
        set_mode(path("up/private"), 0o700);
        if !readable {
            let err = result.expect_err("the folder is reported");
            let private = str_of(&path("up/private"));
            assert!(
                err.to_string().starts_with(&format!(
                    "1 item skipped or failed, first '{private}': read remote dir: Permission denied"
                )),
                "{err}"
            );
            assert!(path("again/run.sh").exists());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_cancelled_upload_removes_the_partial_remote_file() {
        let Some(sftp) = local_sftp(&[]).await else {
            eprintln!("no sftp-server on this machine; skipped");
            return;
        };
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("big.bin");
        std::fs::write(&src, vec![1u8; 1_000_000]).expect("write");
        let dst = tmp.path().join("copy.bin");

        // One slot: the upload cannot run more than a chunk ahead of this watcher,
        // which cancels once the first chunk is out.
        let (tx, mut rx) = mpsc::channel(1);
        let cancels = Cancels::default();
        let cancel = no_cancel(&cancels);
        let upload = do_upload(
            src.to_str().expect("utf-8"),
            &sftp,
            dst.to_str().expect("utf-8"),
            1,
            &tx,
            &cancel,
        );
        let watch = async {
            while let Some(CoreEvent::FileTransferProgress(_, done, _)) = rx.recv().await {
                if done > 0 {
                    cancels.bump();
                }
            }
        };
        let result = tokio::select! {
            result = upload => result,
            () = watch => unreachable!("the channel outlives the upload"),
        };
        assert!(result.expect_err("cancelled").is::<Cancelled>());
        assert!(!dst.exists(), "the partial file is gone");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_folder_uploads_to_a_server_that_will_not_set_modes() {
        let Some(sftp) = local_sftp(&["-P", "setstat"]).await else {
            eprintln!("no sftp-server on this machine; skipped");
            return;
        };
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = |rel: &str| tmp.path().join(rel);
        let str_of = |rel: &str| path(rel).to_str().expect("utf-8 temp dir").to_string();
        std::fs::create_dir_all(path("src/a/b")).expect("mkdir");
        std::fs::write(path("src/a/b/f.txt"), "hello").expect("write");

        let cancels = Cancels::default();
        let cancel = no_cancel(&cancels);
        let (tx, _rx) = mpsc::channel(64);
        do_upload(&str_of("src"), &sftp, &str_of("up"), 1, &tx, &cancel)
            .await
            .expect("uploaded");
        assert_eq!(std::fs::read(path("up/a/b/f.txt")).expect("read"), b"hello");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_read_only_file_is_replaced_by_the_next_transfer() {
        use std::os::unix::fs::PermissionsExt;
        let Some(sftp) = local_sftp(&[]).await else {
            eprintln!("no sftp-server on this machine; skipped");
            return;
        };
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = |rel: &str| tmp.path().join(rel);
        let str_of = |rel: &str| path(rel).to_str().expect("utf-8 temp dir").to_string();
        std::fs::create_dir(path("src")).expect("mkdir");
        for (file, mode) in [("src/obj", 0o444), ("key", 0o400)] {
            std::fs::write(path(file), file).expect("write");
            std::fs::set_permissions(path(file), std::fs::Permissions::from_mode(mode))
                .expect("chmod");
        }

        let cancels = Cancels::default();
        let cancel = no_cancel(&cancels);
        let (tx, _rx) = mpsc::channel(64);
        for round in 0..2 {
            do_download(&sftp, &str_of("src"), &str_of("tree"), round, &tx, &cancel)
                .await
                .expect("folder downloaded");
            do_download(&sftp, &str_of("key"), &str_of("dl"), round, &tx, &cancel)
                .await
                .expect("file downloaded");
            do_upload(&str_of("key"), &sftp, &str_of("up"), round, &tx, &cancel)
                .await
                .expect("file uploaded");
        }
        // As sftp's get leaves it: the owner may write, nobody else gains anything.
        assert_eq!(mode_of(&path("dl")), 0o600);
        assert_eq!(std::fs::read(path("tree/obj")).expect("read"), b"src/obj");
    }

    #[test]
    fn a_server_name_must_be_one_plain_component() {
        for name in ["report.txt", "..hidden", "with space", "caf\u{e9}.txt"] {
            assert!(is_safe_name(name), "{name:?} should be safe");
        }
        for name in ["", ".", "..", "a/b", "/abs", "dir/", "a\0b"] {
            assert!(!is_safe_name(name), "{name:?} should not be safe");
        }
        assert_eq!(join_remote("/", "x"), "/x");
        assert_eq!(join_remote("/srv", "x"), "/srv/x");
    }

    #[test]
    fn windows_names_refuse_drives_devices_and_trimmed_endings() {
        for name in [
            "C:x",
            "D:",
            "a:b",
            "..\\x",
            "a\\b",
            "CON",
            "aux.c",
            "NUL.tar.gz",
            "Com1",
            "lpt9.txt",
            "COM\u{b9}",
            "CON .txt",
            "a.",
            "a ",
            "a<b",
            "a?b",
            "a\"b",
            "tab\tname",
        ] {
            assert!(
                !is_windows_name(name),
                "{name:?} should be refused on Windows"
            );
        }
        for name in [
            "report.txt",
            "CONFIG",
            "COM10",
            "lpt",
            "aux_c",
            ".hidden",
            "a.b",
            "a b",
        ] {
            assert!(
                is_windows_name(name),
                "{name:?} should be allowed on Windows"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_a_server_name_never_leaves_the_destination() {
        let dst = Path::new(r"C:\Users\me\dl");
        for name in [
            "C:x", "D:", "a:b", r"..\x", r"\x", r"\\?\x", "CON", "aux.c", "a.",
        ] {
            assert!(
                join_local(dst, name).is_none(),
                "{name:?} should be refused"
            );
        }
        assert_eq!(join_local(dst, "x.txt"), Some(dst.join("x.txt")));
        // The item itself: the frontends join its listed name onto the destination.
        let download =
            |name: &str| check_download_name(&format!("/srv/{name}"), &format!(r"C:\dl\{name}"));
        for name in ["D:evil", "nul.txt", r"a\b", r"..\x"] {
            assert!(download(name).is_err(), "{name:?} should be refused");
        }
        assert!(download("notes.txt").is_ok());
        // As the terminal app builds it, from the path's last component.
        assert!(check_download_name(r"/srv/a\b", r"C:\dl\b").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn on_unix_backslashes_colons_and_device_names_are_plain() {
        let dst = Path::new("/home/me/dl");
        for name in ["a\\b", "..\\x", "C:x", "a:b", "CON", "a."] {
            assert_eq!(join_local(dst, name), Some(dst.join(name)), "{name:?}");
        }
        for name in ["..", "a/b", ""] {
            assert!(
                join_local(dst, name).is_none(),
                "{name:?} should be refused"
            );
        }
        assert!(check_download_name("/srv/a\\b", "/home/me/dl/a\\b").is_ok());
        assert!(check_download_name("/srv/C:x", "/home/me/dl/C:x").is_ok());
        assert!(check_download_name("/", "/home/me/dl/x").is_err());
        assert!(check_download_name("/srv/x", "/").is_err());
    }

    #[test]
    fn check_paths_rejects_parent_dirs_and_nulls() {
        assert!(check_paths("/tmp/a", "/srv/a").is_ok());
        assert!(check_paths("/tmp/../etc/a", "/srv/a").is_err());
        assert!(check_paths("/tmp/a", "/srv/a\0").is_err());
    }

    #[test]
    fn unix_secs_handles_times_before_the_epoch() {
        let before = std::time::UNIX_EPOCH - Duration::from_secs(10);
        assert_eq!(unix_secs(Ok(before)), Some(-10));
        assert_eq!(unix_secs(Ok(std::time::UNIX_EPOCH)), Some(0));
        assert_eq!(unix_secs(Err(std::io::Error::other("no"))), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_system_drive_is_a_root() {
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        assert!(local_roots().contains(&format!("{}\\", system.to_uppercase())));
    }
}
