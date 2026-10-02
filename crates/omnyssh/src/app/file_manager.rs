//! File Manager screen state and the `App` methods that drive local/remote
//! directory navigation and SFTP transfers.

use std::collections::HashSet;

use super::*;
use omnyssh_core::ssh::identity;
use omnyssh_core::ssh::password::Prompter;
use omnyssh_core::ssh::sftp::{self, FileEntry, SftpCommand, SftpManager};

// ---------------------------------------------------------------------------
// File Manager state (ViewState-only)
// ---------------------------------------------------------------------------

/// Which of the two file panels is currently focused.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FmPanel {
    /// Left panel — local filesystem.
    #[default]
    Local,
    /// Right panel — remote filesystem (SFTP).
    Remote,
}

/// Items held in the copy clipboard.
#[derive(Debug, Clone)]
pub struct FmClipboard {
    /// Absolute paths of the copied items.
    pub paths: Vec<String>,
    /// Which panel the items were copied from.
    pub source_panel: FmPanel,
}

/// UI state for a single file panel (local or remote).
#[derive(Debug, Default)]
pub struct FilePanelView {
    /// Current working directory being displayed.
    pub cwd: String,
    /// Unfiltered entries as returned by the last listing. Used to re-derive
    /// `entries` when the user toggles hidden-file visibility, without
    /// re-fetching from the filesystem or SFTP server.
    pub raw_entries: Vec<FileEntry>,
    /// Visible entries after applying the hidden-files filter. The render
    /// code, cursor, and scroll all operate on this list.
    pub entries: Vec<FileEntry>,
    /// Absolute cursor index into `entries`.
    pub cursor: usize,
    /// Index of the first visible row (for scrolling).
    /// Uses [`std::cell::Cell`] so the render function can persist the computed
    /// scroll position through a shared `&FilePanelView` reference.
    pub scroll: std::cell::Cell<usize>,
    /// Set of `entry.path` values that are Space-marked.
    pub marked: HashSet<String>,
    /// When set, the next listing for this panel will position the cursor on
    /// the entry whose `path` matches this value, then clear it. Used to
    /// remember the directory the user just left so navigating back places
    /// the cursor on the child entry (matching the behaviour of `ranger`,
    /// `lf`, `nnn`, `vifm`).
    pub pending_focus_path: Option<String>,
    /// When `false` (the default), entries whose name starts with `.` are
    /// hidden. The synthetic `..` parent entry is always shown. Toggled with
    /// the `.` key on the file panel.
    pub show_hidden: bool,
}

impl FilePanelView {
    /// Returns a reference to the entry under the cursor, if any.
    pub fn cursor_entry(&self) -> Option<&FileEntry> {
        self.entries.get(self.cursor)
    }

    /// Returns the paths to operate on: all marked entries, or the cursor entry
    /// if nothing is marked.
    pub fn marked_or_cursor_paths(&self) -> Vec<String> {
        if !self.marked.is_empty() {
            self.marked.iter().cloned().collect()
        } else if let Some(e) = self.cursor_entry() {
            if e.name != ".." {
                vec![e.path.clone()]
            } else {
                vec![]
            }
        } else {
            vec![]
        }
    }

    /// Move cursor down by one row, staying in bounds.
    pub fn select_next(&mut self) {
        if !self.entries.is_empty() {
            self.cursor = (self.cursor + 1).min(self.entries.len() - 1);
        }
    }

    /// Move cursor up by one row, staying in bounds.
    pub fn select_prev(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Rebuilds `entries` from `raw_entries` according to `show_hidden`.
    /// The cursor is preserved by path — if the entry under the cursor is
    /// still visible it stays put, otherwise the cursor falls back to 0.
    /// `..` is always considered visible (it is the parent entry, not a
    /// user-visible hidden file).
    pub fn apply_hidden_filter(&mut self) {
        let prev_path = self.entries.get(self.cursor).map(|e| e.path.clone());
        self.entries = self
            .raw_entries
            .iter()
            .filter(|e| self.show_hidden || e.name == ".." || !e.name.starts_with('.'))
            .cloned()
            .collect();
        self.cursor = prev_path
            .and_then(|p| self.entries.iter().position(|e| e.path == p))
            .unwrap_or(0);
        // Drop marks on entries that are no longer visible so a hidden file
        // can't be silently included in a later delete/copy.
        let visible = &self.entries;
        self.marked.retain(|p| visible.iter().any(|e| &e.path == p));
    }

    /// Applies a fresh directory listing: stores `raw`, derives the visible
    /// `entries` via the hidden-files filter, then positions the cursor on
    /// `pending_focus_path` (the entry just left) if it is still visible.
    /// Scroll and marks are reset. The cursor is set *after* filtering so the
    /// filter cannot reset it. Both the core-event handlers and the tests go
    /// through this method, so they exercise the same code path.
    pub fn apply_listing(&mut self, raw: Vec<FileEntry>) {
        self.raw_entries = raw;
        self.apply_hidden_filter();
        self.cursor = self
            .pending_focus_path
            .take()
            .and_then(|target| self.entries.iter().position(|e| e.path == target))
            .unwrap_or(0);
        self.scroll.set(0);
        self.marked.clear();
    }
}

/// Active popup on the File Manager screen.
#[derive(Debug)]
pub enum FileManagerPopup {
    /// Pick which host to connect the remote panel to.
    HostPicker { cursor: usize },
    /// Confirm deletion of one or more items.
    DeleteConfirm { paths: Vec<String> },
    /// Creating a new remote or local directory.
    MkDir(FormField),
    /// Renaming the item under the cursor.
    Rename {
        original_name: String,
        field: FormField,
    },
    /// Live file-transfer progress.
    TransferProgress {
        transfer_id: TransferId,
        filename: String,
        done: u64,
        total: u64,
    },
}

/// All UI state for the File Manager screen.
#[derive(Debug, Default)]
pub struct FileManagerView {
    /// Which panel has keyboard focus.
    pub active_panel: FmPanel,
    /// State of the local (left) panel.
    pub local: FilePanelView,
    /// State of the remote (right) panel.
    pub remote: FilePanelView,
    /// Name of the currently connected remote host, if any.
    pub connected_host: Option<String>,
    /// SFTP connection in progress (shows "Connecting..." indicator).
    pub sftp_connecting: bool,
    /// Copy clipboard.
    pub clipboard: Option<FmClipboard>,
    /// Active popup, if any.
    pub popup: Option<FileManagerPopup>,
    /// Text content shown in the preview zone.
    pub preview_content: Option<String>,
    /// Path whose preview is currently shown (avoids redundant re-fetches).
    pub preview_path: Option<String>,
    /// Transfer id of an in-progress transfer (for the progress popup).
    pub active_transfer: Option<TransferId>,
    /// Queued operations sent and not yet reported in: at most one.
    pub pending_ops: usize,
    /// The first failure among them, reported once they are all done.
    pub op_error: Option<String>,
    /// How many of them failed.
    pub op_failures: usize,
    /// Esc asked the running transfer to stop, and it has not reported in yet.
    pub cancelling: bool,
}

/// The core's error for a transfer that was cancelled.
pub(crate) const CANCELLED: &str = "Transfer cancelled";

impl App {
    // -----------------------------------------------------------------------
    // File Manager private helper methods
    // -----------------------------------------------------------------------

    /// Returns a mutable reference to the active file panel view.
    pub(crate) fn active_fm_panel_mut(&mut self) -> &mut FilePanelView {
        match self.view.file_manager.active_panel {
            FmPanel::Local => &mut self.view.file_manager.local,
            FmPanel::Remote => &mut self.view.file_manager.remote,
        }
    }

    /// Returns a shared reference to the active file panel view.
    pub(crate) fn active_fm_panel_ref(&self) -> &FilePanelView {
        match self.view.file_manager.active_panel {
            FmPanel::Local => &self.view.file_manager.local,
            FmPanel::Remote => &self.view.file_manager.remote,
        }
    }

    /// Initialises the file manager when the user first switches to it.
    ///
    /// - Loads the local panel from `home_dir` (or `/`) if it is empty.
    /// - Opens the host-picker popup if no remote session is active.
    pub(crate) async fn bootstrap_file_manager(&mut self) {
        // Load local panel if not yet populated.
        if self.view.file_manager.local.cwd.is_empty() {
            let start = dirs::home_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".to_string());
            let tx = self.core_tx.clone();
            let path = start.clone();
            tokio::spawn(async move {
                match sftp::list_local_dir(&path).await {
                    Ok(entries) => {
                        let _ = tx.send(CoreEvent::LocalDirListed { path, entries }).await;
                    }
                    Err(e) => tracing::warn!("Local bootstrap failed: {e}"),
                }
            });
        }

        // Open the host-picker if no remote connection exists.
        if self.sftp_manager.is_none() && self.view.file_manager.connected_host.is_none() {
            self.view.file_manager.popup = Some(FileManagerPopup::HostPicker { cursor: 0 });
        }
    }

    /// Requests a preview for the entry under the cursor in the active panel.
    ///
    /// Skips the request when the same path is already previewed.
    pub(crate) fn request_preview_for_active(&mut self) {
        let is_remote = self.view.file_manager.active_panel == FmPanel::Remote;

        let (path, already_shown) = {
            let panel = self.active_fm_panel_ref();
            let Some(entry) = panel.cursor_entry() else {
                return;
            };
            if entry.is_dir {
                return;
            }
            let path = entry.path.clone();
            let shown = self.view.file_manager.preview_path.as_deref() == Some(&path);
            (path, shown)
        };

        if already_shown {
            return;
        }

        if is_remote {
            if let Some(mgr) = &self.sftp_manager {
                mgr.send(SftpCommand::ReadPreview(path));
            }
        } else {
            let tx = self.core_tx.clone();
            tokio::spawn(async move {
                if let Ok(content) = sftp::preview_local_file(&path).await {
                    let _ = tx.send(CoreEvent::FilePreviewReady { path, content }).await;
                }
            });
        }
    }

    /// Re-lists both panels after a mutating operation completes.
    pub(crate) async fn refresh_active_panels(&mut self) {
        let local_path = self.view.file_manager.local.cwd.clone();
        if !local_path.is_empty() {
            let tx = self.core_tx.clone();
            tokio::spawn(async move {
                match sftp::list_local_dir(&local_path).await {
                    Ok(entries) => {
                        let _ = tx
                            .send(CoreEvent::LocalDirListed {
                                path: local_path,
                                entries,
                            })
                            .await;
                    }
                    Err(e) => tracing::warn!("Local refresh failed: {e}"),
                }
            });
        }

        let remote_path = self.view.file_manager.remote.cwd.clone();
        if !remote_path.is_empty() {
            if let Some(mgr) = &self.sftp_manager {
                mgr.send(SftpCommand::ListDir(remote_path));
            }
        }
    }

    /// Initiates an SFTP connection to the host at `idx` in `AppState.hosts`.
    pub(crate) async fn fm_connect_host(&mut self, idx: usize) {
        let host = {
            let state = self.state.read().await;
            state.hosts.get(idx).cloned()
        };
        let Some(host) = host else {
            self.view.status_message = Some("Host not found.".to_string());
            return;
        };

        // Disconnect any existing session.
        if let Some(old) = self.sftp_manager.take() {
            old.disconnect();
        }
        // What the old session still had to send or report is moot now.
        self.sftp_queue.clear();
        self.view.file_manager.pending_ops = 0;
        self.view.file_manager.connected_host = None;
        self.view.file_manager.remote = FilePanelView::default();

        self.view.status_message = Some(format!("Connecting to '{}'…", host.name));
        self.view.file_manager.sftp_connecting = true;

        // In the background: the login may wait on a password prompt. Every
        // step of the connect has a bound of its own.
        let tx = self.core_tx.clone();
        let host_clone = host.clone();
        tokio::spawn(async move {
            let prompter = Prompter::new(tx.clone(), &host_clone.name);
            match SftpManager::connect(&host_clone, tx.clone(), prompter).await {
                Ok(mgr) => {
                    // Send the manager through a new event type
                    let _ = tx
                        .send(CoreEvent::SftpManagerReady {
                            host_name: host_clone.name.clone(),
                            manager: Box::new(mgr),
                        })
                        .await;
                }
                Err(e) => {
                    if let Some(path) = omnyssh_core::ssh::session::passphrase_required(&e) {
                        identity::ask_passphrase(&tx, &host_clone.name, path).await;
                    }
                    let _ = tx
                        .send(CoreEvent::SftpDisconnected {
                            reason: format!("{e:#}"),
                        })
                        .await;
                }
            }
        });
    }

    /// Enters the directory under the cursor in the active panel.
    pub(crate) async fn fm_enter_dir(&mut self) {
        let is_remote = self.view.file_manager.active_panel == FmPanel::Remote;
        let entry = self.active_fm_panel_ref().cursor_entry().cloned();
        let Some(entry) = entry else { return };
        if !entry.is_dir {
            return;
        }

        // Stash the directory we are leaving so the next listing positions
        // the cursor on the matching child entry. When the cursor is on a
        // regular child this is a no-op (the new listing won't contain the
        // old cwd); when the cursor is on `..` it lands us back on the
        // directory we just left.
        let prev_cwd = self.active_fm_panel_ref().cwd.clone();
        self.active_fm_panel_mut().pending_focus_path = Some(prev_cwd);

        if is_remote {
            if let Some(mgr) = &self.sftp_manager {
                mgr.send(SftpCommand::ListDir(entry.path.clone()));
            }
        } else {
            let path = entry.path.clone();
            let tx = self.core_tx.clone();
            tokio::spawn(async move {
                match sftp::list_local_dir(&path).await {
                    Ok(entries) => {
                        let _ = tx.send(CoreEvent::LocalDirListed { path, entries }).await;
                    }
                    Err(e) => {
                        let _ = tx.send(CoreEvent::Error(e.to_string())).await;
                    }
                }
            });
        }
    }

    /// Navigates to the parent of the current working directory.
    pub(crate) async fn fm_parent_dir(&mut self) {
        let is_remote = self.view.file_manager.active_panel == FmPanel::Remote;
        let cwd = self.active_fm_panel_ref().cwd.clone();

        let parent = std::path::Path::new(&cwd).parent().map(|p| {
            let s = p.to_string_lossy().into_owned();
            if s.is_empty() {
                "/".to_string()
            } else {
                s
            }
        });

        let Some(parent) = parent else { return };

        // Remember the directory we are leaving so the next listing lands the
        // cursor on its entry in the parent's listing.
        self.active_fm_panel_mut().pending_focus_path = Some(cwd);

        if is_remote {
            if let Some(mgr) = &self.sftp_manager {
                mgr.send(SftpCommand::ListDir(parent));
            }
        } else {
            let tx = self.core_tx.clone();
            tokio::spawn(async move {
                match sftp::list_local_dir(&parent).await {
                    Ok(entries) => {
                        let _ = tx
                            .send(CoreEvent::LocalDirListed {
                                path: parent,
                                entries,
                            })
                            .await;
                    }
                    Err(e) => tracing::warn!("Parent dir failed: {e}"),
                }
            });
        }
    }

    /// Moves the local panel to the next drive: on Windows `..` stops at the root
    /// of the drive it is on. A drive that does not list (an empty card reader)
    /// is passed over, so it cannot hide the ones after it.
    pub(crate) async fn fm_next_drive(&mut self) {
        let next = roots_after(&sftp::local_roots(), &self.view.file_manager.local.cwd);
        if next.is_empty() {
            self.view.status_message = Some("No other drives".to_string());
            return;
        }
        self.view.file_manager.active_panel = FmPanel::Local;
        let tx = self.core_tx.clone();
        tokio::spawn(async move {
            let mut failure = None;
            for path in next {
                match sftp::list_local_dir(&path).await {
                    Ok(entries) => {
                        let _ = tx.send(CoreEvent::LocalDirListed { path, entries }).await;
                        return;
                    }
                    Err(e) => failure = Some(format!("{e:#}")),
                }
            }
            if let Some(failure) = failure {
                let _ = tx.send(CoreEvent::Error(failure)).await;
            }
        });
    }

    /// Pastes all clipboard contents into the active panel (upload / download).
    ///
    /// Each item goes to the background task once the one before it is done.
    pub(crate) async fn fm_paste(&mut self) {
        let Some(clipboard) = self.view.file_manager.clipboard.clone() else {
            self.view.status_message = Some("Nothing in clipboard.".to_string());
            return;
        };

        let dst_panel = self.view.file_manager.active_panel.clone();

        if clipboard.source_panel == dst_panel {
            self.view.status_message = Some("Cannot paste to the same panel.".to_string());
            return;
        }

        if clipboard.paths.is_empty() {
            self.view.status_message = Some("Clipboard is empty.".to_string());
            return;
        }

        if self.sftp_busy() {
            return;
        }

        // Nothing would be sent, and the progress would wait for it for good.
        if self.sftp_manager.is_none() {
            self.view.status_message = Some("Not connected to a host.".to_string());
            return;
        }

        let dst_cwd = match &dst_panel {
            FmPanel::Local => self.view.file_manager.local.cwd.clone(),
            FmPanel::Remote => self.view.file_manager.remote.cwd.clone(),
        };

        let count = clipboard.paths.len();
        let first_tid = self.next_transfer_id;
        self.next_transfer_id += count as u64;
        self.view.file_manager.op_error = None;
        self.view.file_manager.op_failures = 0;
        self.view.file_manager.cancelling = false;

        // Show progress popup for the first file; subsequent files update it
        // via FileTransferProgress events.
        let first_name = filename_of(&clipboard.paths[0]);
        let popup_name = if count > 1 {
            format!("{first_name}  (+{} more)", count - 1)
        } else {
            first_name
        };
        self.view.file_manager.active_transfer = Some(first_tid);
        self.view.file_manager.popup = Some(FileManagerPopup::TransferProgress {
            transfer_id: first_tid,
            filename: popup_name,
            done: 0,
            total: 0,
        });

        for (i, src) in clipboard.paths.iter().enumerate() {
            let transfer_id = first_tid + i as u64;
            let dst = format!("{}/{}", dst_cwd.trim_end_matches('/'), filename_of(src));
            self.sftp_queue.push_back(match clipboard.source_panel {
                FmPanel::Local => SftpCommand::Upload {
                    local: src.clone(),
                    remote: dst,
                    transfer_id,
                },
                FmPanel::Remote => SftpCommand::Download {
                    remote: src.clone(),
                    local: dst,
                    transfer_id,
                },
            });
        }
        self.send_next_op();

        if count > 1 {
            self.view.status_message = Some(format!("Queued {count} items for transfer…"));
        }
    }

    /// Sends the next queued operation, once the one before it is done.
    pub(crate) fn send_next_op(&mut self) {
        if self.view.file_manager.pending_ops > 0 {
            return;
        }
        let Some(cmd) = self.sftp_queue.pop_front() else {
            return;
        };
        self.view.file_manager.pending_ops = 1;
        if let Some(mgr) = &self.sftp_manager {
            mgr.send(cmd);
        }
    }

    /// Whether a batch is still running, saying so. A new one waits for it, or its
    /// results would be taken for the new batch's.
    fn sftp_busy(&mut self) -> bool {
        let busy = self.view.file_manager.pending_ops > 0 || !self.sftp_queue.is_empty();
        if busy {
            self.view.status_message =
                Some("Wait for the running operation to finish.".to_string());
        }
        busy
    }

    /// Deletes items listed in the `DeleteConfirm` popup.
    pub(crate) async fn fm_delete(&mut self) {
        let popup = self.view.file_manager.popup.take();
        let Some(FileManagerPopup::DeleteConfirm { paths }) = popup else {
            return;
        };

        let is_remote = self.view.file_manager.active_panel == FmPanel::Remote;

        if is_remote {
            if self.sftp_busy() {
                return;
            }
            self.view.file_manager.op_error = None;
            self.view.file_manager.op_failures = 0;
            self.view.file_manager.cancelling = false;
            self.sftp_queue
                .extend(paths.into_iter().map(SftpCommand::Delete));
            self.send_next_op();
        } else {
            let tx = self.core_tx.clone();
            tokio::spawn(async move {
                let mut errors: Vec<String> = Vec::new();
                for path in paths {
                    let result = match tokio::fs::remove_file(&path).await {
                        Ok(()) => Ok(()),
                        Err(_) => {
                            // Might be a directory — try remove_dir (empty only).
                            tokio::fs::remove_dir(&path).await
                        }
                    };
                    if let Err(e) = result {
                        errors.push(format!("{path}: {e}"));
                    }
                }
                let result = if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors.join("; "))
                };
                let _ = tx.send(CoreEvent::SftpOpDone { result }).await;
            });
        }
    }

    /// Creates a new directory in the active panel.
    pub(crate) async fn fm_mkdir(&mut self, name: String) {
        self.view.file_manager.popup = None;
        let is_remote = self.view.file_manager.active_panel == FmPanel::Remote;

        if is_remote {
            let cwd = self.view.file_manager.remote.cwd.clone();
            let new_path = format!("{}/{}", cwd.trim_end_matches('/'), name);
            if let Some(mgr) = &self.sftp_manager {
                mgr.send(SftpCommand::MkDir(new_path));
            }
        } else {
            let cwd = self.view.file_manager.local.cwd.clone();
            let new_path = format!("{}/{}", cwd.trim_end_matches('/'), name);
            let tx = self.core_tx.clone();
            tokio::spawn(async move {
                let result = tokio::fs::create_dir(&new_path)
                    .await
                    .map_err(|e| e.to_string());
                let _ = tx.send(CoreEvent::SftpOpDone { result }).await;
            });
        }
    }

    /// Renames the file or directory under the cursor.
    pub(crate) async fn fm_rename(&mut self, new_name: String) {
        let popup = self.view.file_manager.popup.take();
        let is_remote = self.view.file_manager.active_panel == FmPanel::Remote;

        let cwd = match is_remote {
            true => self.view.file_manager.remote.cwd.clone(),
            false => self.view.file_manager.local.cwd.clone(),
        };

        let old_name = match &popup {
            Some(FileManagerPopup::Rename { original_name, .. }) => original_name.clone(),
            _ => return,
        };

        let old_path = format!("{}/{}", cwd.trim_end_matches('/'), old_name);
        let new_path = format!("{}/{}", cwd.trim_end_matches('/'), new_name);

        if is_remote {
            if let Some(mgr) = &self.sftp_manager {
                mgr.send(SftpCommand::Rename {
                    from: old_path,
                    to: new_path,
                });
            }
        } else {
            let tx = self.core_tx.clone();
            tokio::spawn(async move {
                let result = tokio::fs::rename(&old_path, &new_path)
                    .await
                    .map_err(|e| e.to_string());
                let _ = tx.send(CoreEvent::SftpOpDone { result }).await;
            });
        }
    }
}

/// Extracts the file name from an absolute path string.
fn filename_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string()
}

/// The other roots in the order `d` visits them: those after the one `cwd` is
/// on, then round to those before it; all of them when it is on none.
fn roots_after(roots: &[String], cwd: &str) -> Vec<String> {
    let cwd = cwd.to_lowercase();
    match roots
        .iter()
        .position(|root| cwd.starts_with(&root.to_lowercase()))
    {
        Some(at) => roots[at + 1..]
            .iter()
            .chain(&roots[..at])
            .cloned()
            .collect(),
        None if roots.len() > 1 => roots.to_vec(),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn the_next_drive_wraps_and_ignores_case() {
        let roots: Vec<String> = ["C:\\", "D:\\", "E:\\"].map(String::from).to_vec();
        assert_eq!(roots_after(&roots, "d:\\Media"), ["E:\\", "C:\\"]);
        assert_eq!(roots_after(&roots, "E:\\"), ["C:\\", "D:\\"]);
        // A network share is on no drive: every drive is next.
        assert_eq!(roots_after(&roots, "\\\\nas\\share"), roots);
        assert!(roots_after(&["/".to_string()], "/home").is_empty());
    }

    fn entry(name: &str, path: &str) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: path.to_string(),
            size: 0,
            is_dir: false,
            modified: None,
        }
    }

    fn panel(entries: Vec<FileEntry>) -> FilePanelView {
        FilePanelView {
            entries,
            ..FilePanelView::default()
        }
    }

    // --- FilePanelView marked/cursor selection (P1.6) ---------------------

    #[test]
    fn marked_or_cursor_uses_marked_set() {
        let mut p = panel(vec![entry("a", "/a"), entry("b", "/b")]);
        p.marked.insert("/b".to_string());
        assert_eq!(p.marked_or_cursor_paths(), vec!["/b".to_string()]);
    }

    #[test]
    fn marked_or_cursor_uses_cursor_when_unmarked() {
        let p = panel(vec![entry("f", "/f")]);
        assert_eq!(p.marked_or_cursor_paths(), vec!["/f".to_string()]);
    }

    #[test]
    fn marked_or_cursor_excludes_dotdot_under_cursor() {
        let p = panel(vec![entry("..", "/parent")]);
        assert!(p.marked_or_cursor_paths().is_empty());
    }

    #[test]
    fn marked_or_cursor_empty_entries_returns_empty() {
        let p = panel(vec![]);
        assert!(p.marked_or_cursor_paths().is_empty());
    }

    #[test]
    fn marked_or_cursor_marked_dotdot_still_returned() {
        // The ".." guard applies only to the cursor path, not the marked set.
        let mut p = panel(vec![entry("..", "/parent")]);
        p.marked.insert("/parent".to_string());
        assert_eq!(p.marked_or_cursor_paths(), vec!["/parent".to_string()]);
    }

    #[test]
    fn cursor_entry_in_bounds() {
        let mut p = panel(vec![entry("a", "/a"), entry("b", "/b")]);
        p.cursor = 1;
        assert_eq!(p.cursor_entry().map(|e| e.name.as_str()), Some("b"));
    }

    #[test]
    fn cursor_entry_out_of_bounds_returns_none() {
        let mut p = panel(vec![entry("a", "/a")]);
        p.cursor = 5;
        assert!(p.cursor_entry().is_none());
    }

    #[test]
    fn fm_select_next_clamps_at_last() {
        let mut p = panel(vec![entry("a", "/a"), entry("b", "/b")]);
        p.select_next();
        p.select_next();
        p.select_next();
        assert_eq!(p.cursor, 1);
    }

    #[test]
    fn fm_select_next_noop_when_empty() {
        let mut p = panel(vec![]);
        p.select_next();
        assert_eq!(p.cursor, 0);
    }

    #[test]
    fn fm_select_prev_saturates_at_zero() {
        let mut p = panel(vec![entry("a", "/a")]);
        p.select_prev();
        assert_eq!(p.cursor, 0);
    }

    // --- pending_focus_path: cursor auto-positioning on parent navigation -

    #[test]
    fn pending_focus_positions_cursor_on_matching_entry() {
        let entries = vec![
            entry("..", "/"),
            entry("alpha", "/alpha"),
            entry("projects", "/projects"),
            entry("zeta", "/zeta"),
        ];
        let mut p = panel(entries.clone());
        // Simulate "we were in /projects and just went up".
        p.pending_focus_path = Some("/projects".to_string());
        p.apply_listing(entries);
        assert_eq!(p.cursor, 2, "cursor should land on the 'projects' entry");
        assert!(p.pending_focus_path.is_none(), "field must be consumed");
    }

    #[test]
    fn pending_focus_falls_back_to_zero_when_no_match() {
        // The remembered path no longer exists in the new listing (e.g. it
        // was renamed or deleted). Cursor must default to 0 and the field
        // must still be cleared.
        let entries = vec![entry("..", "/"), entry("alpha", "/alpha")];
        let mut p = panel(entries.clone());
        p.pending_focus_path = Some("/vanished".to_string());
        p.apply_listing(entries);
        assert_eq!(p.cursor, 0);
        assert!(p.pending_focus_path.is_none());
    }

    #[test]
    fn pending_focus_survives_hidden_filter() {
        // Regression: the listing must place the cursor on the focus target
        // *after* the hidden-files filter runs. With show_hidden = false the
        // dotfile is filtered out, but the visible target must still be found.
        let mut p = FilePanelView {
            pending_focus_path: Some("/home/projects".to_string()),
            ..Default::default()
        };
        p.apply_listing(vec![
            entry("..", "/home"),
            entry(".cache", "/home/.cache"),
            entry("projects", "/home/projects"),
        ]);
        assert_eq!(p.entries[p.cursor].name, "projects");
    }

    #[test]
    fn no_pending_focus_starts_cursor_at_zero() {
        // A first-time listing (app start, host connect) has no focus path.
        let mut p = panel(vec![]);
        p.apply_listing(vec![entry("..", "/"), entry("alpha", "/alpha")]);
        assert_eq!(p.cursor, 0);
    }

    // --- apply_hidden_filter: dotfile visibility toggle ------------------

    /// Convenience: build a panel with both raw and visible entries the
    /// same way the real listing handlers do.
    fn panel_with_raw(raw: Vec<FileEntry>) -> FilePanelView {
        let mut p = FilePanelView {
            raw_entries: raw,
            ..Default::default()
        };
        // The real listing handlers assign `raw_entries` then call
        // `apply_hidden_filter()` to derive the visible list. Tests should
        // see the same post-filter state, so we run the filter here too.
        p.apply_hidden_filter();
        p
    }

    fn visible_names(p: &FilePanelView) -> Vec<&str> {
        p.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn default_hides_dotfiles_but_keeps_dotdot() {
        let p = panel_with_raw(vec![
            entry("..", "/"),
            entry(".bashrc", "/.bashrc"),
            entry("alpha", "/alpha"),
            entry(".config", "/.config"),
            entry("zeta", "/zeta"),
        ]);
        // show_hidden defaults to false, so dotfiles must be filtered out
        // and `..` must always remain visible.
        assert_eq!(
            visible_names(&p),
            vec!["..", "alpha", "zeta"],
            "dotfiles hidden by default, .. always visible"
        );
        assert!(!p.show_hidden);
    }

    #[test]
    fn toggling_show_hidden_reveals_dotfiles() {
        let mut p = panel_with_raw(vec![
            entry("..", "/"),
            entry(".bashrc", "/.bashrc"),
            entry("alpha", "/alpha"),
            entry(".config", "/.config"),
        ]);
        // First toggle: reveal everything.
        p.show_hidden = true;
        p.apply_hidden_filter();
        assert_eq!(
            visible_names(&p),
            vec!["..", ".bashrc", "alpha", ".config"],
            "all entries visible when show_hidden = true"
        );
        // Second toggle: hide them again, restoring the original view.
        p.show_hidden = false;
        p.apply_hidden_filter();
        assert_eq!(visible_names(&p), vec!["..", "alpha"]);
    }

    #[test]
    fn cursor_preserved_by_path_when_toggling() {
        // Start in the "show all" state: set up the panel so both raw
        // and visible entries include the dotfile, with the cursor on
        // `zeta`.
        let raw = vec![
            entry("..", "/"),
            entry("alpha", "/alpha"),
            entry(".hidden", "/.hidden"),
            entry("zeta", "/zeta"),
        ];
        let mut p = FilePanelView {
            raw_entries: raw.clone(),
            entries: raw,
            cursor: 3,
            show_hidden: true,
            ..FilePanelView::default()
        };
        // Toggle to "hide dotfiles". `..` and `zeta` must remain visible;
        // the cursor must follow `zeta` (now at its new index, since
        // `.hidden` and `alpha` shifted positions).
        p.show_hidden = false;
        p.apply_hidden_filter();
        let zeta_idx = p
            .entries
            .iter()
            .position(|e| e.name == "zeta")
            .expect("zeta must still be in entries after toggle");
        assert_eq!(p.cursor, zeta_idx, "cursor stays on zeta after hide");
        // Toggle back to "show all" — cursor must still be on `zeta`.
        p.show_hidden = true;
        p.apply_hidden_filter();
        let zeta_idx = p.entries.iter().position(|e| e.name == "zeta").unwrap();
        assert_eq!(p.cursor, zeta_idx);
    }

    #[test]
    fn cursor_falls_back_to_zero_when_cursor_path_is_filtered_out() {
        // Cursor is on the dotfile entry; toggling to "hide" must move
        // the cursor to a still-visible entry (here, `..` at index 0).
        let raw = vec![
            entry("..", "/"),
            entry("alpha", "/alpha"),
            entry(".hidden", "/.hidden"),
            entry("zeta", "/zeta"),
        ];
        let mut p = FilePanelView {
            raw_entries: raw.clone(),
            entries: raw,
            cursor: 2, // on `.hidden`
            show_hidden: true,
            ..FilePanelView::default()
        };
        p.show_hidden = false;
        p.apply_hidden_filter();
        // `.hidden` is filtered out, so cursor must fall back to 0.
        assert_eq!(p.cursor, 0);
        assert_eq!(p.entries[0].name, "..");
    }

    #[test]
    fn dotdot_always_visible_regardless_of_toggle() {
        let mut p = panel_with_raw(vec![entry("..", "/parent"), entry(".bashrc", "/.bashrc")]);
        // show_hidden = false: .. must still be present (it's not a user
        // file, it's the parent marker).
        p.show_hidden = false;
        p.apply_hidden_filter();
        assert!(
            p.entries.iter().any(|e| e.name == ".."),
            ".. must remain visible when show_hidden = false"
        );
        // show_hidden = true: .. is still there.
        p.show_hidden = true;
        p.apply_hidden_filter();
        assert!(p.entries.iter().any(|e| e.name == ".."));
    }

    #[test]
    fn hiding_drops_marks_on_now_hidden_entries() {
        let mut p = FilePanelView {
            show_hidden: true,
            ..FilePanelView::default()
        };
        p.apply_listing(vec![
            entry("..", "/"),
            entry(".secret", "/.secret"),
            entry("visible", "/visible"),
        ]);
        p.marked.insert("/.secret".to_string());
        p.marked.insert("/visible".to_string());
        // Hiding dotfiles must drop the mark on the now-invisible .secret so
        // it can't be deleted/copied blind; the visible mark stays.
        p.show_hidden = false;
        p.apply_hidden_filter();
        assert!(!p.marked.contains("/.secret"));
        assert!(p.marked.contains("/visible"));
    }

    // --- Transfers ----------------------------------------------------------

    /// An app whose file manager is part way into a paste of three items.
    async fn transferring_three() -> App {
        let mut app = App::default();
        app.state.write().await.screen = Screen::FileManager;
        app.view.file_manager.pending_ops = 1;
        app.sftp_queue = ["b", "c"]
            .into_iter()
            .zip(1..)
            .map(|(name, transfer_id)| SftpCommand::Upload {
                local: format!("/l/{name}"),
                remote: format!("/srv/{name}"),
                transfer_id,
            })
            .collect();
        let fm = &mut app.view.file_manager;
        fm.popup = Some(FileManagerPopup::TransferProgress {
            transfer_id: 0,
            filename: "a  (+2 more)".to_string(),
            done: 0,
            total: 0,
        });
        app
    }

    async fn done(app: &mut App, result: Result<(), &str>) {
        let result = result.map_err(String::from);
        let event = CoreEvent::SftpOpDone { result };
        app.handle_core_event(event).await.unwrap();
    }

    fn progress_shown(app: &App) -> bool {
        matches!(
            app.view.file_manager.popup,
            Some(FileManagerPopup::TransferProgress { .. })
        )
    }

    #[tokio::test]
    async fn a_transfer_reports_its_first_failure_once_the_rest_is_done() {
        let mut app = transferring_three().await;
        let skipped = "1 item skipped or failed, first '/l/a/fifo': not a regular file or folder";
        done(&mut app, Err(skipped)).await;
        assert!(progress_shown(&app));
        assert_eq!(
            app.view.status_message.as_deref(),
            Some("2 item(s) remaining…")
        );
        done(&mut app, Ok(())).await;
        done(
            &mut app,
            Err("open local file for upload: Permission denied"),
        )
        .await;
        assert!(!progress_shown(&app));
        assert_eq!(
            app.view.status_message,
            Some(format!("Transfer failed: {skipped} (+1 more)"))
        );
    }

    #[tokio::test]
    async fn the_next_item_goes_once_the_one_before_is_done() {
        let mut app = transferring_three().await;
        done(&mut app, Ok(())).await;
        assert!(matches!(
            app.sftp_queue.front(),
            Some(SftpCommand::Upload { local, .. }) if local == "/l/c"
        ));
        assert_eq!(app.view.file_manager.pending_ops, 1);
        done(&mut app, Ok(())).await;
        assert!(app.sftp_queue.is_empty());
        assert!(progress_shown(&app));
        done(&mut app, Ok(())).await;
        assert!(!progress_shown(&app));
        assert_eq!(app.view.status_message, None);
    }

    #[tokio::test]
    async fn a_new_batch_waits_for_the_running_one() {
        let mut app = transferring_three().await;
        let fm = &mut app.view.file_manager;
        fm.active_panel = FmPanel::Remote;
        fm.clipboard = Some(FmClipboard {
            paths: vec!["/l/d".to_string()],
            source_panel: FmPanel::Local,
        });
        app.fm_paste().await;
        let busy = Some("Wait for the running operation to finish.");
        assert_eq!(app.view.status_message.as_deref(), busy);
        app.view.status_message = None;
        app.view.file_manager.popup = Some(FileManagerPopup::DeleteConfirm {
            paths: vec!["/srv/x".to_string()],
        });
        app.fm_delete().await;
        assert_eq!(app.view.status_message.as_deref(), busy);
        assert_eq!(app.sftp_queue.len(), 2);
    }

    async fn press_esc(app: &mut App) {
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let action = app.handle_key(key).await.unwrap();
        assert!(matches!(action, Some(AppAction::FmCancelTransfer)));
        app.process_action(action).await.unwrap();
    }

    #[tokio::test]
    async fn esc_drops_what_is_queued_and_waits_for_the_running_item() {
        let mut app = transferring_three().await;
        press_esc(&mut app).await;
        assert!(app.sftp_queue.is_empty());
        assert!(progress_shown(&app));
        assert_eq!(app.view.status_message.as_deref(), Some("Cancelling…"));
        done(&mut app, Err(CANCELLED)).await;
        assert!(!progress_shown(&app));
        assert_eq!(app.view.status_message.as_deref(), Some(CANCELLED));
    }

    #[tokio::test]
    async fn items_left_unsent_count_as_cancelled() {
        let mut app = transferring_three().await;
        press_esc(&mut app).await;
        // The running item finished before the cancel reached it.
        done(&mut app, Ok(())).await;
        assert_eq!(app.view.status_message.as_deref(), Some(CANCELLED));
    }

    #[tokio::test]
    async fn a_cancel_too_late_for_the_last_item_says_so() {
        let mut app = transferring_three().await;
        done(&mut app, Ok(())).await;
        done(&mut app, Ok(())).await;
        press_esc(&mut app).await;
        done(&mut app, Ok(())).await;
        assert!(!progress_shown(&app));
        assert_eq!(
            app.view.status_message.as_deref(),
            Some("The transfer finished before it could be cancelled.")
        );
    }

    #[tokio::test]
    async fn a_real_failure_outranks_the_cancel() {
        let failure = "open local file for upload: Permission denied (os error 13)";
        // Before the cancel.
        let mut app = transferring_three().await;
        done(&mut app, Err(failure)).await;
        press_esc(&mut app).await;
        done(&mut app, Err(CANCELLED)).await;
        assert_eq!(
            app.view.status_message,
            Some(format!("Transfer failed: {failure}"))
        );
        // While stopping.
        let mut app = transferring_three().await;
        press_esc(&mut app).await;
        done(&mut app, Err(failure)).await;
        assert_eq!(
            app.view.status_message,
            Some(format!("Transfer failed: {failure}"))
        );
    }

    #[tokio::test]
    async fn a_second_esc_hides_a_transfer_slow_to_stop() {
        let mut app = transferring_three().await;
        press_esc(&mut app).await;
        press_esc(&mut app).await;
        assert!(!progress_shown(&app));
        // The panels are free again, but a new batch waits.
        app.view.file_manager.popup = Some(FileManagerPopup::MkDir(FormField::default()));
        done(&mut app, Err(CANCELLED)).await;
        assert!(matches!(
            app.view.file_manager.popup,
            Some(FileManagerPopup::MkDir(_))
        ));
        assert_eq!(app.view.status_message.as_deref(), Some(CANCELLED));
        assert!(!app.view.file_manager.cancelling);
    }

    #[tokio::test]
    async fn a_paste_needs_a_connected_host() {
        let mut app = App::default();
        let fm = &mut app.view.file_manager;
        fm.active_panel = FmPanel::Remote;
        fm.clipboard = Some(FmClipboard {
            paths: vec!["/l/a".to_string()],
            source_panel: FmPanel::Local,
        });
        app.fm_paste().await;
        assert_eq!(
            app.view.status_message.as_deref(),
            Some("Not connected to a host.")
        );
        assert!(!progress_shown(&app));
        assert_eq!(app.view.file_manager.pending_ops, 0);
    }
}
