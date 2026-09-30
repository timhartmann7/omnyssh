//! Core-event bridge (tech-gui.md §3.4): one long-lived task drains the shared
//! engine channel and maps each `CoreEvent` to a typed IPC event. Status, metrics,
//! discovered services and PTY-exit land here; the raw PTY byte stream rides its own
//! forwarder (`forward_terminal_output`) into the per-session channels (§3.6).

use crate::dto::{FileEntryDto, TransferProgressDto, TunnelStatusDto};
use crate::events;
use crate::state::GuiState;
use omnyssh_core::event::{CoreEvent, SessionId, TransferStage};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;
use tokio::sync::mpsc;

pub async fn forward_core_events(app: AppHandle, mut rx: mpsc::Receiver<CoreEvent>) {
    while let Some(event) = rx.recv().await {
        // A match with an explicit ignore arm (§3.4), grown one variant per slice.
        match event {
            CoreEvent::HostStatusChanged(host_name, status) => {
                let _ = events::HostStatusChanged {
                    host_name,
                    status: (&status).into(),
                }
                .emit(&app);
            }
            CoreEvent::MetricsUpdate(host_name, metrics) => {
                let _ = events::MetricsUpdated {
                    host_name,
                    metrics: (&metrics).into(),
                }
                .emit(&app);
            }
            CoreEvent::DiscoveryQuickScanDone(host_name, services) => {
                let _ = events::ServicesDetected {
                    host_name,
                    services: services.iter().map(crate::dto::ServiceDto::from).collect(),
                }
                .emit(&app);
            }
            CoreEvent::DiscoveryFailed(host_name, message) => {
                let _ = events::ServicesFailed { host_name, message }.emit(&app);
            }
            CoreEvent::TunnelStatusChanged(host_name, status) => {
                let status = TunnelStatusDto::from(&status);
                app.state::<GuiState>()
                    .tunnel_status_changed(&host_name, &status, || {
                        let _ = events::TunnelStatusChanged {
                            host_name: host_name.clone(),
                            status: status.clone(),
                        }
                        .emit(&app);
                    });
            }
            CoreEvent::Error(message) => {
                let _ = events::Error { message }.emit(&app);
            }
            CoreEvent::KeyPassphraseRequired {
                host_name,
                key_path,
            } => {
                let _ = events::KeyPassphraseRequired {
                    host_name,
                    key_path,
                }
                .emit(&app);
            }
            CoreEvent::PasswordRequired {
                request_id,
                host_name,
                login,
                retry,
                new_host_key,
            } => {
                let _ = events::PasswordRequired {
                    request_id,
                    host_name,
                    login,
                    retry,
                    new_host_key,
                }
                .emit(&app);
            }
            // Remote shell exit / dropped connection. Map the inner PTY id to its
            // public id (dropping routing state); `None` means the user already
            // closed the tab, so nothing is emitted (§3.4).
            CoreEvent::PtyExited(inner_id) => {
                if let Some(session_id) = app.state::<GuiState>().terminal_exited(inner_id) {
                    let _ = events::TerminalExited { session_id }.emit(&app);
                }
            }
            // Auto key-setup (§4.2): `start_key_setup` drives the core and reports these
            // on the shared engine channel; the host name identifies the run.
            CoreEvent::KeySetupProgress(host_name, step) => {
                let _ = events::KeySetupProgress {
                    host_name,
                    step: step.into(),
                }
                .emit(&app);
            }
            CoreEvent::KeySetupComplete(host_name, key_path) => {
                let _ = events::KeySetupComplete {
                    host_name,
                    key_path: key_path.to_string_lossy().to_string(),
                }
                .emit(&app);
            }
            CoreEvent::KeySetupFailed(host_name, error) => {
                let _ = events::KeySetupFailed { host_name, error }.emit(&app);
            }
            CoreEvent::KeySetupRollback(host_name, result) => {
                let _ = events::KeySetupRollback { host_name, result }.emit(&app);
            }
            // A newer release found by the startup check (§4.3) → the update banner.
            CoreEvent::UpdateAvailable(info) => {
                let _ = events::UpdateAvailable {
                    info: (&info).into(),
                }
                .emit(&app);
            }
            // Other variants are mapped as their producers start. `HostsLoaded`
            // is emitted directly by its command, SFTP results by the per-session
            // forwarder, and `PtyOutput` is superseded by the raw tap (§3.4/§3.6).
            _ => {}
        }
    }
}

/// Drains the PTY raw-output tap (§3.6) and demuxes each chunk into its tab's
/// channel, keyed by the core's inner PTY id. Spawned once at startup.
pub async fn forward_terminal_output(app: AppHandle, mut rx: mpsc::Receiver<(SessionId, Vec<u8>)>) {
    while let Some((inner_id, bytes)) = rx.recv().await {
        app.state::<GuiState>()
            .send_terminal_output(inner_id, bytes);
    }
}

/// A typed SFTP event stamped with its owning session id, ready to emit. Built by
/// [`map_sftp_event`] so per-session routing stays pure and unit-testable (§3.4).
enum SftpOutbound {
    Connected(events::SftpConnected),
    DirListed(events::SftpDirListed),
    OpDone(events::SftpOpDone),
    Disconnected(events::SftpDisconnected),
    Preview(events::FilePreview),
    ContentReady(events::FileContentReady),
    ContentReadFailed(events::FileContentReadFailed),
    WriteDone(events::FileWriteDone),
    Progress(events::TransferProgress),
}

impl SftpOutbound {
    fn emit(self, app: &AppHandle) {
        let _ = match self {
            SftpOutbound::Connected(e) => e.emit(app),
            SftpOutbound::DirListed(e) => e.emit(app),
            SftpOutbound::OpDone(e) => e.emit(app),
            SftpOutbound::Disconnected(e) => e.emit(app),
            SftpOutbound::Preview(e) => e.emit(app),
            SftpOutbound::ContentReady(e) => e.emit(app),
            SftpOutbound::ContentReadFailed(e) => e.emit(app),
            SftpOutbound::WriteDone(e) => e.emit(app),
            SftpOutbound::Progress(e) => e.emit(app),
        };
    }
}

/// Stamp a core SFTP event with `session_id` (its resolved owner) and map it to a
/// typed IPC event (§3.4). `None` for a variant that never travels a per-session SFTP
/// channel. Pure: the session id comes from the channel's owner, never from the
/// (absent) event field — this is what makes two tabs listing the same path resolve
/// to distinct sessions.
fn map_sftp_event(session_id: SessionId, event: CoreEvent) -> Option<SftpOutbound> {
    Some(match event {
        CoreEvent::SftpConnected { host_name } => SftpOutbound::Connected(events::SftpConnected {
            session_id,
            host_name,
        }),
        CoreEvent::FileDirListed { path, entries } => {
            SftpOutbound::DirListed(events::SftpDirListed {
                session_id,
                path,
                entries: entries.iter().map(FileEntryDto::from).collect(),
            })
        }
        CoreEvent::SftpOpDone { result } => {
            let (ok, error) = match result {
                Ok(()) => (true, None),
                Err(message) => (false, Some(message)),
            };
            SftpOutbound::OpDone(events::SftpOpDone {
                session_id,
                ok,
                error,
            })
        }
        CoreEvent::SftpDisconnected { reason } => {
            SftpOutbound::Disconnected(events::SftpDisconnected { session_id, reason })
        }
        CoreEvent::FilePreviewReady { path, content } => {
            SftpOutbound::Preview(events::FilePreview {
                session_id,
                path,
                content,
            })
        }
        CoreEvent::FileContentReady { path, content } => {
            SftpOutbound::ContentReady(events::FileContentReady {
                session_id,
                path,
                content,
            })
        }
        CoreEvent::FileWriteDone { path, result } => {
            let (ok, error) = match result {
                Ok(()) => (true, None),
                Err(message) => (false, Some(message)),
            };
            SftpOutbound::WriteDone(events::FileWriteDone {
                session_id,
                path,
                ok,
                error,
            })
        }
        CoreEvent::Error(message) if message.starts_with("edit-read:") => {
            let error = message.trim_start_matches("edit-read:").to_string();
            // The core error currently carries no path. Keep this as a generic
            // session error; the requested path is retained in the frontend editor state.
            SftpOutbound::ContentReadFailed(events::FileContentReadFailed {
                session_id,
                path: String::new(),
                error,
            })
        }
        CoreEvent::FileTransferProgress {
            transfer_id,
            stage,
            root_name,
            current_file,
            bytes_done,
            bytes_total,
            files_done,
            files_total,
        } => SftpOutbound::Progress(events::TransferProgress(TransferProgressDto {
            session_id: session_id as u32,
            transfer_id: transfer_id as u32,
            stage: match stage {
                TransferStage::Preparing => "preparing".into(),
                TransferStage::Transferring => "transferring".into(),
            },
            root_name,
            current_file,
            bytes_done,
            bytes_total,
            files_done,
            files_total,
        })),
        // Not produced on a per-session SFTP channel (`SftpManagerReady` is TUI-only).
        _ => return None,
    })
}

/// Per-session SFTP forwarder (§3.4): drains a tab's dedicated core-event channel,
/// stamps each event with the tab's `session_id`, and emits the typed IPC event.
/// Transfer progress is attributed to its owner via `transfer_owner` (the
/// GUI-allocated transfer id's session). Ends when the core task drops its sender.
pub async fn forward_sftp_events(
    app: AppHandle,
    session_id: SessionId,
    mut rx: mpsc::Receiver<CoreEvent>,
) {
    while let Some(event) = rx.recv().await {
        // A transfer's owner comes from `transfer_owner`; every other event is stamped
        // with this forwarder's own session (§3.4).
        let owner = match &event {
            CoreEvent::FileTransferProgress { transfer_id, .. } => app
                .state::<GuiState>()
                .transfer_session(*transfer_id)
                .unwrap_or(session_id),
            _ => session_id,
        };
        if let Some(outbound) = map_sftp_event(owner, event) {
            outbound.emit(&app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnyssh_core::ssh::sftp::FileEntry;

    fn file(name: &str) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: format!("/srv/{name}"),
            size: 0,
            is_dir: false,
        }
    }

    // Two tabs listing the SAME path resolve to DISTINCT sessions — the id comes from
    // the channel's owner, not the (absent) event field (tech-gui.md §3.2/§3.4).
    #[test]
    fn same_path_listing_stamps_distinct_sessions() {
        let listing = || CoreEvent::FileDirListed {
            path: "/srv".to_string(),
            entries: vec![file("a")],
        };
        match (
            map_sftp_event(1, listing()).unwrap(),
            map_sftp_event(2, listing()).unwrap(),
        ) {
            (SftpOutbound::DirListed(a), SftpOutbound::DirListed(b)) => {
                assert_eq!(a.path, b.path, "same remote path");
                assert_eq!(a.session_id, 1);
                assert_eq!(b.session_id, 2, "distinct sessions");
                assert_eq!(a.entries.len(), 1);
                assert_eq!(a.entries[0].name, "a");
            }
            _ => panic!("expected DirListed for both"),
        }
    }

    #[test]
    fn connected_is_stamped_with_the_owning_session() {
        match map_sftp_event(
            5,
            CoreEvent::SftpConnected {
                host_name: "web-1".to_string(),
            },
        )
        .unwrap()
        {
            SftpOutbound::Connected(e) => {
                assert_eq!(e.session_id, 5);
                assert_eq!(e.host_name, "web-1");
            }
            _ => panic!("expected Connected"),
        }
    }

    #[test]
    fn progress_carries_the_resolved_owner_and_transfer_id() {
        match map_sftp_event(
            7,
            CoreEvent::FileTransferProgress {
                transfer_id: 42,
                stage: TransferStage::Transferring,
                root_name: "test.txt".into(),
                current_file: "test.txt".into(),
                bytes_done: 512,
                bytes_total: 2048,
                files_done: 1,
                files_total: 1,
            },
        )
        .unwrap()
        {
            SftpOutbound::Progress(events::TransferProgress(dto)) => {
                assert_eq!(dto.session_id, 7);
                assert_eq!(dto.transfer_id, 42);
                assert_eq!((dto.bytes_done, dto.bytes_total), (512, 2048));
            }
            _ => panic!("expected Progress"),
        }
    }

    #[test]
    fn op_done_flattens_ok_and_error() {
        match map_sftp_event(1, CoreEvent::SftpOpDone { result: Ok(()) }).unwrap() {
            SftpOutbound::OpDone(e) => {
                assert!(e.ok);
                assert!(e.error.is_none());
            }
            _ => panic!("expected OpDone"),
        }
        match map_sftp_event(
            1,
            CoreEvent::SftpOpDone {
                result: Err("permission denied".to_string()),
            },
        )
        .unwrap()
        {
            SftpOutbound::OpDone(e) => {
                assert!(!e.ok);
                assert_eq!(e.error.as_deref(), Some("permission denied"));
            }
            _ => panic!("expected OpDone"),
        }
    }

    #[test]
    fn unrelated_variants_never_map_to_an_sftp_event() {
        // A terminal render-nudge would never arrive here, but the catch-all keeps the
        // forwarder robust and the match exhaustive without inventing an event (§3.4).
        assert!(map_sftp_event(1, CoreEvent::PtyOutput(3)).is_none());
    }
}
