//! SFTP session commands (tech-gui.md §4.2). `sftp_open` awaits the core connect and
//! spawns a per-session forwarder that stamps every `sftp-*` event with the tab's
//! session id (§3.4); the remote ops are thin `SftpCommand` enqueues whose results
//! arrive as events. Local filesystem listing/preview return directly — the GUI never
//! emits `LocalDirListed` (§4.3).

use omnyssh_core::event::CoreEvent;
use omnyssh_core::ssh::identity;
use omnyssh_core::ssh::password::Prompter;
use omnyssh_core::ssh::sftp::{
    list_local_dir as core_list_local_dir, local_roots,
    preview_local_file as core_preview_local_file, SftpCommand, SftpManager,
};
use std::path::PathBuf;
use tauri::Manager;
use tauri::{AppHandle, State};
use tokio::sync::mpsc;

use crate::bridge;
use crate::dto::FileEntryDto;
use crate::error::CommandError;
use crate::state::GuiState;

/// A tab's dedicated core-event channel buffer. Comfortably absorbs the connect ack
/// plus a burst of transfer-progress ticks before the forwarder drains them (§3.4).
const SFTP_EVENT_BUFFER: usize = 256;

/// Open an SFTP session for `host_name` (tech-gui.md §4.2). Awaits the core connect,
/// registers the manager under a fresh public id, and spawns the per-session
/// forwarder; the `sftp-connected` ack then arrives stamped with that id (§3.4).
#[tauri::command]
#[specta::specta]
pub async fn sftp_open(
    app: AppHandle,
    state: State<'_, GuiState>,
    host_name: String,
) -> Result<u64, CommandError> {
    let host = state.host_by_name(&host_name).ok_or_else(|| CommandError {
        message: format!("unknown host '{host_name}'"),
    })?;
    // A dedicated channel per tab: its owner is the session id, so the forwarder can
    // attribute the core's session-less `sftp-*` events to this tab (§3.4).
    let (tx, rx) = mpsc::channel::<CoreEvent>(SFTP_EVENT_BUFFER);
    // The prompt goes out on the engine channel: this tab's own channel only
    // carries `sftp-*` events.
    let prompter = Prompter::new(state.engine_sender(), &host_name);
    let manager = match SftpManager::connect(&host, tx, prompter).await {
        Ok(manager) => manager,
        Err(e) => {
            if let Some(path) = omnyssh_core::ssh::session::passphrase_required(&e) {
                identity::ask_passphrase(&state.engine_sender(), &host_name, path).await;
            }
            // The whole chain: the core wraps the cause in "SFTP SSH connect".
            return Err(CommandError {
                message: format!("{e:#}"),
            });
        }
    };
    let session_id = state.register_sftp(manager);
    tauri::async_runtime::spawn(bridge::forward_sftp_events(app, session_id, rx));
    Ok(session_id)
}

/// List a remote directory (tech-gui.md §4.2); the result arrives as `sftp-dir-listed`.
#[tauri::command]
#[specta::specta]
pub fn sftp_list(
    state: State<'_, GuiState>,
    session_id: u64,
    path: String,
) -> Result<(), CommandError> {
    state.send_sftp(session_id, SftpCommand::ListDir(path));
    Ok(())
}

/// Upload a local file to a remote path (tech-gui.md §4.2). Allocates a transfer id
/// owned by this session so `transfer-progress` routes back to the tab (§3.4).
#[tauri::command]
#[specta::specta]
pub fn sftp_upload(
    state: State<'_, GuiState>,
    session_id: u64,
    local: String,
    remote: String,
) -> Result<(), CommandError> {
    let transfer_id = state.next_transfer(session_id);
    state.send_sftp(
        session_id,
        SftpCommand::Upload {
            local,
            remote,
            transfer_id,
        },
    );
    Ok(())
}

/// Download a remote file to a local path (tech-gui.md §4.2). See `sftp_upload` for
/// the transfer-id routing; the core guards the local destination against `..` (§3.2).
#[tauri::command]
#[specta::specta]
pub fn sftp_download(
    state: State<'_, GuiState>,
    session_id: u64,
    local: String,
    remote: String,
) -> Result<(), CommandError> {
    let transfer_id = state.next_transfer(session_id);
    state.send_sftp(
        session_id,
        SftpCommand::Download {
            remote,
            local,
            transfer_id,
        },
    );
    Ok(())
}
///Cancel a file/dir transfer
#[tauri::command]
#[specta::specta]
pub fn sftp_cancel(
    state: State<'_, GuiState>,
    session_id: u32,
    transfer_id: u32,
) -> Result<(), CommandError> {
    state.send_sftp(
        session_id as u64,
        SftpCommand::CancelTransfer {
            transfer_id: transfer_id as u64,
        },
    );

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn sftp_upload_dir(
    state: State<'_, GuiState>,
    session_id: u64,
    local: String,
    remote: String,
) -> Result<(), CommandError> {
    let transfer_id = state.next_transfer(session_id);

    state.send_sftp(
        session_id,
        SftpCommand::UploadDir {
            local,
            remote,
            transfer_id,
        },
    );

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn sftp_download_dir(
    state: State<'_, GuiState>,
    session_id: u64,
    remote: String,
    local: String,
) -> Result<(), CommandError> {
    let transfer_id = state.next_transfer(session_id);

    state.send_sftp(
        session_id,
        SftpCommand::DownloadDir {
            remote,
            local,
            transfer_id,
        },
    );

    Ok(())
}

/// Create a remote directory (tech-gui.md §4.2); completion arrives as `sftp-op-done`.
#[tauri::command]
#[specta::specta]
pub fn sftp_mkdir(
    state: State<'_, GuiState>,
    session_id: u64,
    path: String,
) -> Result<(), CommandError> {
    state.send_sftp(session_id, SftpCommand::MkDir(path));
    Ok(())
}

/// Rename / move a remote path (tech-gui.md §4.2).
#[tauri::command]
#[specta::specta]
pub fn sftp_rename(
    state: State<'_, GuiState>,
    session_id: u64,
    from: String,
    to: String,
) -> Result<(), CommandError> {
    state.send_sftp(session_id, SftpCommand::Rename { from, to });
    Ok(())
}

/// Delete a remote file or directory. Allocates a transfer id so the normal
/// transfer-progress channel can also report delete progress to the owning tab.
#[tauri::command]
#[specta::specta]
pub fn sftp_delete(
    state: State<'_, GuiState>,
    session_id: u64,
    path: String,
) -> Result<(), CommandError> {
    let transfer_id = state.next_transfer(session_id);
    state.send_sftp(
        session_id,
        SftpCommand::DeleteWithProgress { path, transfer_id },
    );
    Ok(())
}

/// Read a remote file's preview bytes (tech-gui.md §4.2); arrives as `file-preview`.
#[tauri::command]
#[specta::specta]
pub fn sftp_preview(
    state: State<'_, GuiState>,
    session_id: u64,
    path: String,
) -> Result<(), CommandError> {
    state.send_sftp(session_id, SftpCommand::ReadPreview(path));
    Ok(())
}

/// Read a complete remote file for the in-app editor. The core enforces the
/// 64 MiB editor cap and UTF-8 validation; the result arrives as `file-content-ready`.
#[tauri::command]
#[specta::specta]
pub fn sftp_read_file(
    state: State<'_, GuiState>,
    session_id: u64,
    path: String,
) -> Result<(), CommandError> {
    state.send_sftp(session_id, SftpCommand::ReadFile(path));
    Ok(())
}

/// Write the in-app editor buffer back to the remote file.
#[tauri::command]
#[specta::specta]
pub fn sftp_write_file(
    state: State<'_, GuiState>,
    session_id: u64,
    path: String,
    content: String,
) -> Result<(), CommandError> {
    state.send_sftp(session_id, SftpCommand::WriteFile { path, content });
    Ok(())
}

/// Close an SFTP session and its connection (tech-gui.md §4.2).
#[tauri::command]
#[specta::specta]
pub fn sftp_close(state: State<'_, GuiState>, session_id: u64) -> Result<(), CommandError> {
    state.close_sftp(session_id);
    Ok(())
}

/// List a local directory (tech-gui.md §4.2). Returns directly off the async worker;
/// the GUI never emits `LocalDirListed` (§4.3). The core prepends a `..` entry and
/// sorts dirs-first.
#[tauri::command]
#[specta::specta]
pub async fn list_local_dir(path: String) -> Result<Vec<FileEntryDto>, CommandError> {
    // The whole chain: the OS reason ("Access is denied") is the useful part.
    let entries = core_list_local_dir(&path).await.map_err(|e| CommandError {
        message: format!("{e:#}"),
    })?;
    Ok(entries.iter().map(FileEntryDto::from).collect())
}

/// The roots the local pane can switch to: every drive letter on Windows, `/`
/// elsewhere (tech-gui.md §4.2).
#[tauri::command]
#[specta::specta]
pub fn list_local_roots() -> Vec<String> {
    local_roots()
}

/// Read up to 4 KiB of a local file as UTF-8 for preview (tech-gui.md §4.2).
#[tauri::command]
#[specta::specta]
pub async fn preview_local_file(path: String) -> Result<String, CommandError> {
    core_preview_local_file(&path)
        .await
        .map_err(|e| CommandError {
            message: e.to_string(),
        })
}

/// Create a local directory (tech-gui.md §4.2). Mirrors `sftp_mkdir` for the
/// local filesystem so both panes expose the same toolbar actions.
#[tauri::command]
#[specta::specta]
pub async fn local_mkdir(path: String) -> Result<(), CommandError> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|e| CommandError {
            message: e.to_string(),
        })
}

/// Rename or move a local file or directory (tech-gui.md §4.2). Mirrors
/// `sftp_rename` and preserves the unified local/remote workflow.
#[tauri::command]
#[specta::specta]
pub async fn local_rename(from: String, to: String) -> Result<(), CommandError> {
    tokio::fs::rename(from, to).await.map_err(|e| CommandError {
        message: e.to_string(),
    })
}

/// Delete exactly one local filesystem entry. Directories are removed only when
/// already empty; the SFTP view enumerates directory contents and deletes entries
/// one-by-one so cancellation can happen between entries.
#[tauri::command]
#[specta::specta]
pub async fn local_delete(path: String) -> Result<(), CommandError> {
    let meta = tokio::fs::metadata(&path).await.map_err(|e| CommandError {
        message: e.to_string(),
    })?;

    if meta.is_dir() {
        tokio::fs::remove_dir(path)
            .await
            .map_err(|e| CommandError {
                message: e.to_string(),
            })
    } else {
        tokio::fs::remove_file(path)
            .await
            .map_err(|e| CommandError {
                message: e.to_string(),
            })
    }
}

/// Read a complete local UTF-8 text file for the in-app editor (tech-gui.md §4.2).
/// Mirrors `sftp_read_file` so the editor can switch backends transparently.
#[tauri::command]
#[specta::specta]
pub async fn local_read_file(path: String) -> Result<String, CommandError> {
    tokio::fs::read_to_string(path)
        .await
        .map_err(|e| CommandError {
            message: e.to_string(),
        })
}

/// Overwrite a local file with the editor buffer (tech-gui.md §4.2). Mirrors
/// `sftp_write_file` and keeps the editor backend-agnostic.
#[tauri::command]
#[specta::specta]
pub async fn local_write_file(path: String, content: String) -> Result<(), CommandError> {
    tokio::fs::write(path, content)
        .await
        .map_err(|e| CommandError {
            message: e.to_string(),
        })
}

/// Prepares external editors for local files
#[tauri::command]
#[specta::specta]
pub fn local_prepare_external_edit(app: tauri::AppHandle, path: String) -> Result<String, String> {
    let src = PathBuf::from(&path);

    if !src.exists() {
        return Err(format!("File not found: {}", path));
    }

    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("external-edits");

    std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;

    let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("file");

    let temp = cache_dir.join(name);

    std::fs::copy(&src, &temp).map_err(|e| e.to_string())?;

    Ok(temp.to_string_lossy().to_string())
}

/// Create a unique local temporary path for a large-file external edit. The empty
/// file is created with `create_new` so a collision cannot overwrite an existing file.
#[tauri::command]
#[specta::specta]
pub fn sftp_prepare_external_edit(remote_path: String) -> Result<String, CommandError> {
    use std::fs::{create_dir_all, OpenOptions};
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    let temp_root = std::env::temp_dir().join("omnyssh");
    create_dir_all(&temp_root).map_err(|e| CommandError {
        message: e.to_string(),
    })?;

    let filename = Path::new(&remote_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file");

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| CommandError {
            message: e.to_string(),
        })?
        .as_nanos();

    let path = temp_root.join(format!("{}-{}-{}", std::process::id(), stamp, filename));

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| CommandError {
            message: e.to_string(),
        })?;

    Ok(path.to_string_lossy().into_owned())
}

/// Hash a local edit file for change detection without sending its contents through IPC.
#[tauri::command]
#[specta::specta]
pub async fn sftp_local_file_hash(path: String) -> Result<String, CommandError> {
    tokio::fs::read(&path)
        .await
        .map(|bytes| {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            bytes.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        })
        .map_err(|e| CommandError {
            message: e.to_string(),
        })
}

/// Run the user's configured external editor and wait for it to exit.
/// GUI editors are launched directly in blocking mode, while terminal editors
/// are opened inside the platform's terminal application.
#[tauri::command]
#[specta::specta]
pub async fn sftp_open_external_editor(path: String, editor: String) -> Result<bool, CommandError> {
    tokio::task::spawn_blocking(move || {
        use std::process::Command;

        let editor = if editor.trim().is_empty() {
            std::env::var("OMNYSSH_EDITOR")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| std::env::var("VISUAL").ok())
                .or_else(|| std::env::var("EDITOR").ok())
                .unwrap_or_else(|| "nano".to_string())
        } else {
            editor
        };

        let mut parts = editor.split_whitespace();
        let program = parts.next().ok_or_else(|| CommandError {
            message: "External editor is empty".into(),
        })?;

        let mut args: Vec<String> = parts.map(str::to_string).collect();

        // Make common GUI editors block automatically.
        match program {
            "zed" => {
                if !args.iter().any(|a| a == "--wait") {
                    args.push("--wait".into());
                }
            }
            "code" | "code-insiders" => {
                if !args.iter().any(|a| a == "--wait") {
                    args.push("--wait".into());
                }
            }
            "gedit" => {
                if !args.iter().any(|a| a == "--wait") {
                    args.push("--wait".into());
                }
            }
            "kate" => {
                if !args.iter().any(|a| a == "--block") {
                    args.push("--block".into());
                }
            }
            _ => {}
        }

        let terminal_editors = ["nano", "vim", "nvim", "hx", "helix", "kak", "fresh"];

        let is_terminal = terminal_editors.contains(&program);

        let status = if is_terminal {
            #[cfg(target_os = "linux")]
            {
                let terminals = [
                    "x-terminal-emulator",
                    "kgx",
                    "gnome-terminal",
                    "kitty",
                    "konsole",
                    "xfce4-terminal",
                    "xterm",
                ];

                let terminal = terminals
                    .iter()
                    .find(|t| {
                        Command::new("which")
                            .arg(t)
                            .output()
                            .map(|o| o.status.success())
                            .unwrap_or(false)
                    })
                    .ok_or_else(|| CommandError {
                        message: "No terminal emulator found".into(),
                    })?;

                match *terminal {
                    "gnome-terminal" | "kgx" => Command::new(terminal)
                        .arg("--")
                        .arg(program)
                        .args(&args)
                        .arg(&path)
                        .status(),

                    "konsole" => Command::new(terminal)
                        .arg("-e")
                        .arg(program)
                        .args(&args)
                        .arg(&path)
                        .status(),

                    _ => Command::new(terminal)
                        .arg("-e")
                        .arg(program)
                        .args(&args)
                        .arg(&path)
                        .status(),
                }
            }

            #[cfg(target_os = "macos")]
            {
                let shell = format!(
                    "{} {} ; exit",
                    program,
                    std::iter::once(path.clone())
                        .chain(args.clone())
                        .map(|s| format!("'{}'", s.replace('\'', "'\\''")))
                        .collect::<Vec<_>>()
                        .join(" ")
                );

                Command::new("osascript")
                    .arg("-e")
                    .arg(format!(
                        r#"tell application "Terminal" to do script "{}""#,
                        shell.replace('"', "\\\"")
                    ))
                    .arg("-e")
                    .arg(r#"tell application "Terminal" to activate"#)
                    .status()
            }

            #[cfg(target_os = "windows")]
            {
                Command::new("wt.exe")
                    .arg(program)
                    .args(&args)
                    .arg(&path)
                    .status()
            }
        } else {
            Command::new(program).args(&args).arg(&path).status()
        }
        .map_err(|e| CommandError {
            message: e.to_string(),
        })?;

        Ok(status.success())
    })
    .await
    .map_err(|e| CommandError {
        message: e.to_string(),
    })?
}

/// Remove an external-editor temporary file. Cleanup is idempotent.
#[tauri::command]
#[specta::specta]
pub async fn sftp_remove_temp_file(path: String) -> Result<(), CommandError> {
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CommandError {
            message: e.to_string(),
        }),
    }
}
