//! SFTP file manager operations.
//!
//! Provides [`SftpManager`] — a persistent background task that owns an SSH+SFTP
//! session and processes [`SftpCommand`] messages sent from the UI thread.
//!
//! All operations are non-blocking from the UI perspective.
//! Progress is reported via [`CoreEvent::FileTransferProgress`].

use std::time::Duration;

use anyhow::Context;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use tokio::time;
use walkdir::WalkDir;

use crate::event::{CoreEvent, TransferId, TransferStage};
use crate::ssh::client::Host;
use crate::ssh::password::Prompter;
use crate::ssh::session::{Passwords, SshSession};

/// How long the SFTP channel and subsystem may take once logged in.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// FileEntry — represents one file or directory in a panel listing
// ---------------------------------------------------------------------------

/// Metadata for a single file or directory in a file panel.
/// Max size (bytes) for the in-app editor. Larger files use external editor + temp file.
pub const MAX_IN_APP_EDIT_BYTES: u64 = 64 * 1024 * 1024;

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
}

// ---------------------------------------------------------------------------
// SftpCommand — sent from UI thread → SftpManager background task
// ---------------------------------------------------------------------------

/// Commands processed by the [`SftpManager`] background task.
pub enum SftpCommand {
    /// List the entries in a remote directory.
    ListDir(String),
    /// Download a remote file to a local path.
    Download {
        remote: String,
        local: String,
        transfer_id: TransferId,
    },
    /// Upload a local file to a remote path.
    Upload {
        local: String,
        remote: String,
        transfer_id: TransferId,
    },
    DownloadDir {
        remote: String,
        local: String,
        transfer_id: TransferId,
    },

    UploadDir {
        local: String,
        remote: String,
        transfer_id: TransferId,
    },

    /// Cancel an active upload/download.
    CancelTransfer { transfer_id: TransferId },
    /// Delete a remote file or directory.
    Delete(String),
    /// Delete a remote file or directory and report progress to the GUI.
    DeleteWithProgress {
        path: String,
        transfer_id: TransferId,
    },
    /// Create a remote directory.
    MkDir(String),
    /// Rename / move a remote path.
    Rename { from: String, to: String },
    /// Read the first 4 096 bytes of a remote file for preview.
    ReadPreview(String),
    /// Read a remote file for the in-app editor (size-capped, full UTF-8 content).
    ReadFile(String),
    /// Overwrite a remote file with UTF-8 text from the in-app editor.
    WriteFile { path: String, content: String },
    /// Shut down the task gracefully.
    Disconnect,
}

// ---------------------------------------------------------------------------
// SftpManager — handle held by App to communicate with the background task
// ---------------------------------------------------------------------------

/// Manages a persistent SSH+SFTP background task.
///
/// Use [`SftpManager::connect`] to create, [`SftpManager::send`] to enqueue
/// commands, and [`SftpManager::disconnect`] for a clean shutdown.
#[derive(Debug)]
pub struct SftpManager {
    cmd_tx: mpsc::Sender<SftpCommand>,
    cancelled: Arc<Mutex<HashSet<TransferId>>>,
}

async fn scan_remote_tree(
    sftp: &russh_sftp::client::SftpSession,
    remote_root: &str,
) -> anyhow::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    let mut scan = vec![remote_root.to_string()];

    while let Some(remote_dir) = scan.pop() {
        let dir_entries = do_list_dir(sftp, &remote_dir).await?;

        for entry in dir_entries {
            if entry.name == ".." {
                continue;
            }

            if entry.is_dir {
                scan.push(entry.path.clone());
            }

            entries.push(entry);
        }
    }

    Ok(entries)
}

async fn delete_remote_tree(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
    transfer_id: Option<TransferId>,
    event_tx: &mpsc::Sender<CoreEvent>,
    cancelled: &Arc<Mutex<HashSet<TransferId>>>,
) -> anyhow::Result<()> {
    // Preserve the fast path for a single file.
    if sftp.remove_file(path).await.is_ok() {
        let root_name = Path::new(path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        if let Some(transfer_id) = transfer_id {
            let _ = event_tx
                .send(CoreEvent::FileTransferProgress {
                    transfer_id,
                    stage: TransferStage::Transferring,
                    root_name: root_name.clone(),
                    current_file: root_name,
                    bytes_done: 1,
                    bytes_total: 1,
                    files_done: 1,
                    files_total: 1,
                })
                .await;
        }

        return Ok(());
    }

    // Reuse the same remote pre-scan used by directory download.
    let mut entries = scan_remote_tree(sftp, path).await?;
    let total_items = entries.len() as u32 + 1;
    let root_name = Path::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    // Children must be removed before their parent directories.
    entries.sort_by(|a, b| b.path.len().cmp(&a.path.len()));

    let mut deleted = 0u32;

    for entry in entries {
        if let Some(transfer_id) = transfer_id {
            if cancelled.lock().unwrap().contains(&transfer_id) {
                anyhow::bail!("Transfer cancelled");
            }
        }

        if entry.is_dir {
            sftp.remove_dir(&entry.path)
                .await
                .with_context(|| format!("remove remote directory '{}'", entry.path))?;
        } else {
            sftp.remove_file(&entry.path)
                .await
                .with_context(|| format!("remove remote file '{}'", entry.path))?;
        }

        deleted += 1;

        if let Some(transfer_id) = transfer_id {
            let _ = event_tx
                .send(CoreEvent::FileTransferProgress {
                    transfer_id,
                    stage: TransferStage::Transferring,
                    root_name: root_name.clone(),
                    current_file: entry.name,
                    bytes_done: deleted as u64,
                    bytes_total: total_items as u64,
                    files_done: deleted,
                    files_total: total_items,
                })
                .await;
        }
    }

    if let Some(transfer_id) = transfer_id {
        if cancelled.lock().unwrap().contains(&transfer_id) {
            anyhow::bail!("Transfer cancelled");
        }
    }

    sftp.remove_dir(path)
        .await
        .with_context(|| format!("remove remote directory '{path}'"))?;

    deleted += 1;

    if let Some(transfer_id) = transfer_id {
        let _ = event_tx
            .send(CoreEvent::FileTransferProgress {
                transfer_id,
                stage: TransferStage::Transferring,
                root_name: root_name.clone(),
                current_file: root_name,
                bytes_done: deleted as u64,
                bytes_total: total_items as u64,
                files_done: deleted,
                files_total: total_items,
            })
            .await;
    }

    Ok(())
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

        let (cmd_tx, cmd_rx) = mpsc::channel::<SftpCommand>(64);
        let cancelled = Arc::new(Mutex::new(HashSet::new()));
        let cancelled_task = cancelled.clone();
        let host_name = host.name.clone();

        // `session` and `sftp` are owned by this async block.  If the task
        // panics, Rust's unwind machinery calls their Drop impls before the
        // panic propagates to tokio — the TCP connection is therefore always
        // released even in the panic path.  No explicit catch_unwind needed.
        tokio::spawn(async move {
            let _ = event_tx
                .send(CoreEvent::SftpConnected {
                    host_name: host_name.clone(),
                })
                .await;
            sftp_task_loop(session, sftp, cmd_rx, event_tx.clone(), cancelled_task).await;
            tracing::info!("SFTP task for '{}' exited", host_name);
        });

        Ok(Self { cmd_tx, cancelled })
    }

    /// Enqueues a command (fire-and-forget). Silently drops if the task exited.
    pub fn send(&self, cmd: SftpCommand) {
        if let SftpCommand::CancelTransfer { transfer_id } = cmd {
            self.cancelled.lock().unwrap().insert(transfer_id);
            return;
        }

        let _ = self.cmd_tx.try_send(cmd);
    }

    /// Sends [`SftpCommand::Disconnect`] and drops the sender.
    pub fn disconnect(self) {
        let _ = self.cmd_tx.try_send(SftpCommand::Disconnect);
    }
}

// ---------------------------------------------------------------------------
// Background task loop
// ---------------------------------------------------------------------------

async fn sftp_task_loop(
    _ssh: SshSession,
    sftp: russh_sftp::client::SftpSession,
    mut cmd_rx: mpsc::Receiver<SftpCommand>,
    event_tx: mpsc::Sender<CoreEvent>,
    cancelled: Arc<Mutex<HashSet<TransferId>>>,
) {
    while let Some(cmd) = cmd_rx.recv().await {
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
                            reason: format!("ListDir failed: {e}"),
                        })
                        .await;
                }
            },

            SftpCommand::Download {
                remote,
                local,
                transfer_id,
            } => {
                let result = do_download(
                    &sftp,
                    &remote,
                    &local,
                    transfer_id,
                    &event_tx,
                    true,
                    &cancelled,
                )
                .await
                .map_err(|e| e.to_string());
                let _ = event_tx.send(CoreEvent::SftpOpDone { result }).await;
            }

            SftpCommand::Upload {
                local,
                remote,
                transfer_id,
            } => {
                let result = do_upload(
                    &local,
                    &sftp,
                    &remote,
                    transfer_id,
                    &event_tx,
                    true,
                    &cancelled,
                )
                .await
                .map_err(|e| e.to_string());
                let _ = event_tx.send(CoreEvent::SftpOpDone { result }).await;
            }

            SftpCommand::DownloadDir {
                remote,
                local,
                transfer_id,
            } => {
                let result =
                    do_download_dir(&sftp, &remote, &local, transfer_id, &event_tx, &cancelled)
                        .await
                        .map_err(|e| e.to_string());

                let _ = event_tx.send(CoreEvent::SftpOpDone { result }).await;
            }

            SftpCommand::UploadDir {
                local,
                remote,
                transfer_id,
            } => {
                let result =
                    do_upload_dir(&local, &sftp, &remote, transfer_id, &event_tx, &cancelled)
                        .await
                        .map_err(|e| e.to_string());

                let _ = event_tx.send(CoreEvent::SftpOpDone { result }).await;
            }

            SftpCommand::Delete(path) => {
                let result = delete_remote_tree(
                    &sftp,
                    &path,
                    None,
                    &event_tx,
                    &cancelled,
                )
                .await
                .map_err(|e| e.to_string());

                let _ = event_tx.send(CoreEvent::SftpOpDone { result }).await;
            }

            SftpCommand::DeleteWithProgress { path, transfer_id } => {
                let root_name = Path::new(&path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();

                let _ = event_tx
                    .send(CoreEvent::FileTransferProgress {
                        transfer_id,
                        stage: TransferStage::Preparing,
                        root_name,
                        current_file: "Scanning remote files…".into(),
                        bytes_done: 0,
                        bytes_total: 0,
                        files_done: 0,
                        files_total: 0,
                    })
                    .await;

                let result = delete_remote_tree(
                    &sftp,
                    &path,
                    Some(transfer_id),
                    &event_tx,
                    &cancelled,
                )
                .await
                .map_err(|e| e.to_string());

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

            // In-app editor: full file read (capped by MAX_IN_APP_EDIT_BYTES).
            SftpCommand::ReadFile(path) => match do_read_file(&sftp, &path).await {
                Ok(content) => {
                    let _ = event_tx
                        .send(CoreEvent::FileContentReady { path, content })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(CoreEvent::Error(format!("edit-read:{e}")))
                        .await;
                }
            },

            // In-app editor: overwrite remote file with buffer contents.
            SftpCommand::WriteFile { path, content } => {
                let result = do_write_file(&sftp, &path, &content)
                    .await
                    .map_err(|e| e.to_string());
                let _ = event_tx
                    .send(CoreEvent::FileWriteDone { path, result })
                    .await;
            }

            // Never reaches here; SftpManager::send() intercepts it.
            SftpCommand::CancelTransfer { .. } => {}
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
        entries.push(FileEntry {
            name: "..".to_string(),
            path: parent_str.to_string(),
            size: 0,
            is_dir: true,
        });
    }

    for entry in read_dir {
        let name = entry.file_name();
        let ft = entry.file_type();
        let meta = entry.metadata();

        let full_path = if path.ends_with('/') {
            format!("{path}{name}")
        } else {
            format!("{path}/{name}")
        };

        entries.push(FileEntry {
            name,
            path: full_path,
            size: meta.size.unwrap_or(0),
            is_dir: ft.is_dir(),
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

async fn do_download(
    sftp: &russh_sftp::client::SftpSession,
    remote: &str,
    local: &str,
    transfer_id: TransferId,
    event_tx: &mpsc::Sender<CoreEvent>,
    report_progress: bool,
    cancelled: &Arc<Mutex<HashSet<TransferId>>>,
) -> anyhow::Result<()> {
    // Guard against path traversal in the local destination.
    if std::path::Path::new(local)
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        anyhow::bail!("Download destination path contains '..': {local}");
    }
    if local.contains('\0') || remote.contains('\0') {
        anyhow::bail!("Path contains null bytes");
    }

    // Fetch size for progress (best-effort).
    let total = sftp
        .metadata(remote)
        .await
        .map(|m| m.size.unwrap_or(0))
        .unwrap_or(0);

    let mut remote_file = sftp
        .open(remote)
        .await
        .context("open remote file for download")?;
    let mut local_file = tokio::fs::File::create(local)
        .await
        .context("create local file")?;

    let root_name = Path::new(remote)
        .parent()
        .and_then(|p| p.file_name())
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let current_file = Path::new(remote)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let mut buf = vec![0u8; 65_536];
    let mut done: u64 = 0;

    loop {
        if cancelled.lock().unwrap().contains(&transfer_id) {
            anyhow::bail!("Transfer cancelled");
        }
        let n = remote_file
            .read(&mut buf)
            .await
            .context("read remote file")?;

        if n == 0 {
            break;
        }

        local_file
            .write_all(&buf[..n])
            .await
            .context("write local file")?;

        done += n as u64;

        if report_progress {
            let _ = event_tx
                .send(CoreEvent::FileTransferProgress {
                    transfer_id,
                    stage: TransferStage::Transferring,
                    root_name: root_name.clone(),
                    current_file: current_file.clone(),
                    bytes_done: done,
                    bytes_total: total,
                    files_done: 1,
                    files_total: 1,
                })
                .await;
        }
    }

    remote_file
        .close()
        .await
        .context("close remote file after download")?;

    Ok(())
}

async fn do_upload(
    local: &str,
    sftp: &russh_sftp::client::SftpSession,
    remote: &str,
    transfer_id: TransferId,
    event_tx: &mpsc::Sender<CoreEvent>,
    report_progress: bool,
    cancelled: &Arc<Mutex<HashSet<TransferId>>>,
) -> anyhow::Result<()> {
    // Guard against path traversal in the local source.
    if std::path::Path::new(local)
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        anyhow::bail!("Upload source path contains '..': {local}");
    }
    if local.contains('\0') || remote.contains('\0') {
        anyhow::bail!("Path contains null bytes");
    }

    let mut local_file = tokio::fs::File::open(local)
        .await
        .context("open local file for upload")?;
    let total = local_file.metadata().await.map(|m| m.len()).unwrap_or(0);

    let mut remote_file = sftp
        .create(remote)
        .await
        .context("create remote file for upload")?;

    let mut buf = vec![0u8; 65_536];
    let mut done: u64 = 0;

    let root_name = Path::new(local)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let current_file = root_name.clone();

    loop {
        if cancelled.lock().unwrap().contains(&transfer_id) {
            anyhow::bail!("Transfer cancelled");
        }
        let n = local_file.read(&mut buf).await.context("read local file")?;
        if n == 0 {
            break;
        }
        remote_file
            .write_all(&buf[..n])
            .await
            .context("write remote file")?;
        done += n as u64;
        if report_progress {
            let _ = event_tx
                .send(CoreEvent::FileTransferProgress {
                    transfer_id,
                    stage: TransferStage::Transferring,
                    root_name: root_name.clone(),
                    current_file: current_file.clone(),
                    bytes_done: done,
                    bytes_total: total,
                    files_done: 1,
                    files_total: 1,
                })
                .await;
        }
    }

    remote_file
        .close()
        .await
        .context("close remote file after upload")?;

    Ok(())
}

async fn do_upload_dir(
    local_root: &str,
    sftp: &russh_sftp::client::SftpSession,
    remote_root: &str,
    transfer_id: TransferId,
    event_tx: &mpsc::Sender<CoreEvent>,
    cancelled: &Arc<Mutex<HashSet<TransferId>>>,
) -> anyhow::Result<()> {
    let local_root = PathBuf::from(local_root);

    let root_name = local_root
        .file_name()
        .unwrap_or(local_root.as_os_str())
        .to_string_lossy()
        .into_owned();

    // Show footer immediately
    let _ = event_tx
        .send(CoreEvent::FileTransferProgress {
            transfer_id,
            stage: TransferStage::Preparing,
            root_name: root_name.clone(),
            current_file: "Scanning files…".into(),
            bytes_done: 0,
            bytes_total: 0,
            files_done: 0,
            files_total: 0,
        })
        .await;

    // ---------- Pre-scan ----------
    let mut files: Vec<(PathBuf, String, u64)> = Vec::new();
    let mut total_bytes = 0u64;

    for entry in WalkDir::new(&local_root) {
        let entry = entry?;
        let path = entry.path();

        if path == local_root {
            continue;
        }

        let rel = path.strip_prefix(&local_root)?;
        let remote_path = Path::new(remote_root)
            .join(rel)
            .to_string_lossy()
            .replace('\\', "/");

        if entry.file_type().is_file() {
            let size = entry.metadata()?.len();
            total_bytes += size;
            files.push((path.to_path_buf(), remote_path, size));
        }
    }

    // Create destination directories
    let _ = sftp.create_dir(remote_root).await;

    for entry in WalkDir::new(&local_root) {
        let entry = entry?;
        if !entry.file_type().is_dir() {
            continue;
        }

        let path = entry.path();
        if path == local_root {
            continue;
        }

        let rel = path.strip_prefix(&local_root)?;
        let remote_path = Path::new(remote_root)
            .join(rel)
            .to_string_lossy()
            .replace('\\', "/");

        let _ = sftp.create_dir(&remote_path).await;
    }

    // ---------- Upload ----------
    let total_files = files.len() as u32;
    let mut uploaded = 0u64;

    for (index, (local_path, remote_path, size)) in files.into_iter().enumerate() {
        if cancelled.lock().unwrap().contains(&transfer_id) {
            anyhow::bail!("Transfer cancelled");
        }
        do_upload(
            local_path.to_str().unwrap(),
            sftp,
            &remote_path,
            transfer_id,
            event_tx,
            false,
            cancelled,
        )
        .await?;

        uploaded += size;

        let _ = event_tx
            .send(CoreEvent::FileTransferProgress {
                transfer_id,
                stage: TransferStage::Transferring,
                root_name: root_name.clone(),
                current_file: local_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                bytes_done: uploaded,
                bytes_total: total_bytes,
                files_done: (index + 1) as u32,
                files_total: total_files,
            })
            .await;
    }

    Ok(())
}

async fn do_download_dir(
    sftp: &russh_sftp::client::SftpSession,
    remote_root: &str,
    local_root: &str,
    transfer_id: TransferId,
    event_tx: &mpsc::Sender<CoreEvent>,
    cancelled: &Arc<Mutex<HashSet<TransferId>>>,
) -> anyhow::Result<()> {
    let root_name = Path::new(remote_root)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    // Show footer immediately
    let _ = event_tx
        .send(CoreEvent::FileTransferProgress {
            transfer_id,
            stage: TransferStage::Preparing,
            root_name: root_name.clone(),
            current_file: "Scanning remote files…".into(),
            bytes_done: 0,
            bytes_total: 0,
            files_done: 0,
            files_total: 0,
        })
        .await;
    // ---------- Pre-scan ----------
    let scanned = scan_remote_tree(sftp, remote_root).await?;
    let mut work: Vec<(String, PathBuf, u64)> = Vec::new();
    let mut total_bytes = 0u64;

    for entry in scanned {
        if entry.is_dir {
            continue;
        }

        let relative = Path::new(&entry.path)
            .strip_prefix(Path::new(remote_root))
            .unwrap_or_else(|_| Path::new(&entry.name));
        let local_path = PathBuf::from(local_root).join(relative);
        total_bytes += entry.size;
        work.push((entry.path, local_path, entry.size));
    }

    // ---------- Download ----------
    let root_name = Path::new(remote_root)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let total_files = work.len() as u32;
    let mut downloaded = 0u64;

    for (index, (remote_path, local_path, size)) in work.into_iter().enumerate() {
        if cancelled.lock().unwrap().contains(&transfer_id) {
            anyhow::bail!("Transfer cancelled");
        }
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        do_download(
            sftp,
            &remote_path,
            local_path.to_str().unwrap(),
            transfer_id,
            event_tx,
            false,
            cancelled,
        )
        .await?;

        downloaded += size;

        let _ = event_tx
            .send(CoreEvent::FileTransferProgress {
                transfer_id,
                stage: TransferStage::Transferring,
                root_name: root_name.clone(),
                current_file: local_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                bytes_done: downloaded,
                bytes_total: total_bytes,
                files_done: (index + 1) as u32,
                files_total: total_files,
            })
            .await;
    }

    Ok(())
}

async fn do_read_preview(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
) -> anyhow::Result<String> {
    let mut file = sftp.open(path).await.context("open for preview")?;
    let mut buf = vec![0u8; 4_096];
    let n = file.read(&mut buf).await.context("read preview bytes")?;
    buf.truncate(n);
    let content = String::from_utf8_lossy(&buf).into_owned();
    file.close()
        .await
        .context("close remote file after preview")?;
    Ok(content)
}

/// Read a full remote text file for the in-app editor.
///
/// Rejects paths with null bytes, enforces [`MAX_IN_APP_EDIT_BYTES`], and
/// requires valid UTF-8. Oversized files error with a `FILE_TOO_LARGE:` prefix.
async fn do_read_file(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
) -> anyhow::Result<String> {
    if path.contains('\0') {
        anyhow::bail!("path contains null bytes");
    }

    let meta = sftp.metadata(path).await.context("stat remote file")?;
    let size = meta.size.unwrap_or(0);
    if size > MAX_IN_APP_EDIT_BYTES {
        anyhow::bail!("FILE_TOO_LARGE:{size}");
    }

    let mut remote = sftp.open(path).await.context("open remote for edit")?;
    let mut chunk = vec![0u8; 65_536];
    let mut data = Vec::with_capacity(size.min(MAX_IN_APP_EDIT_BYTES) as usize);

    loop {
        let n = remote.read(&mut chunk).await.context("read remote")?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
        if data.len() as u64 > MAX_IN_APP_EDIT_BYTES {
            anyhow::bail!("FILE_TOO_LARGE:{}", data.len());
        }
    }

    let content = String::from_utf8(data).context("file is not valid UTF-8")?;
    remote
        .close()
        .await
        .context("close remote file after read")?;
    Ok(content)
}

/// Truncate/create a remote file and write UTF-8 content (in-app editor save).
async fn do_write_file(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
    content: &str,
) -> anyhow::Result<()> {
    if path.contains('\0') {
        anyhow::bail!("path contains null bytes");
    }
    let mut remote = sftp.create(path).await.context("create/truncate remote")?;
    remote
        .write_all(content.as_bytes())
        .await
        .context("write remote")?;
    remote
        .close()
        .await
        .context("close remote file after write")?;
    Ok(())
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
        entries.push(FileEntry {
            name: "..".to_string(),
            path: parent_str.to_string(),
            size: 0,
            is_dir: true,
        });
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

    #[cfg(windows)]
    #[test]
    fn the_system_drive_is_a_root() {
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        assert!(local_roots().contains(&format!("{}\\", system.to_uppercase())));
    }
}
