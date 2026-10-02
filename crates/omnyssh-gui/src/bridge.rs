//! Core-event bridge (tech-gui.md §3.4): one long-lived task drains the shared
//! engine channel and maps each `CoreEvent` to a typed IPC event. Status, metrics
//! and discovered services land here; the raw PTY byte stream rides its own
//! forwarder (`forward_terminal_output`) into the per-session channels (§3.6), and
//! PTY-exit is handed on to it so a session's last output is routed first.

use std::collections::HashSet;

use omnyssh_core::event::{CoreEvent, SessionId};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;
use tokio::sync::mpsc;

use crate::dto::{FileEntryDto, TransferProgressDto, TunnelStatusDto};
use crate::events;
use crate::state::GuiState;

pub async fn forward_core_events(
    app: AppHandle,
    mut rx: mpsc::Receiver<CoreEvent>,
    exits: mpsc::Sender<SessionId>,
) {
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
            // Remote shell exit / dropped connection. The core queued the session's
            // last bytes on the raw tap before this, so the tap's forwarder ends the
            // tab behind them (§3.4).
            CoreEvent::PtyExited(inner_id) => {
                let _ = exits.send(inner_id).await;
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

/// Written under a session's last output when it ends: dim, then the cursor hidden
/// and any mouse reporting the dead app left on turned off, so a drag selects text.
const END_LINE: &[u8] = b"\r\n\x1b[2m[Connection closed. Press Enter to close this tab.]\x1b[0m\
    \x1b[?25l\x1b[?9;1000;1002;1003l";

/// Drains the PTY raw-output tap (§3.6) and demuxes each chunk into its tab's
/// channel, keyed by the core's inner PTY id; ends a tab once its session exits.
/// Spawned once at startup.
pub async fn forward_terminal_output(
    app: AppHandle,
    raw_rx: mpsc::Receiver<(SessionId, Vec<u8>)>,
    exit_rx: mpsc::Receiver<SessionId>,
) {
    TerminalRouter::new(AppTerminals(app))
        .run(raw_rx, exit_rx)
        .await;
}

/// Where [`TerminalRouter`] delivers: the tabs' channels and the exit event.
trait TerminalSink {
    /// Routes a chunk to its tab; `false` when no tab is open for `inner`.
    fn output(&mut self, inner: SessionId, bytes: Vec<u8>) -> bool;
    /// Drops an ended session's routing: its public id, or `None` when the user
    /// already closed the tab.
    fn exited(&mut self, inner: SessionId) -> Option<SessionId>;
    /// Tells the frontend the tab's session ended.
    fn announce(&mut self, session_id: SessionId, had_output: bool);
}

struct AppTerminals(AppHandle);

impl TerminalSink for AppTerminals {
    fn output(&mut self, inner: SessionId, bytes: Vec<u8>) -> bool {
        self.0
            .state::<GuiState>()
            .send_terminal_output(inner, bytes)
    }

    fn exited(&mut self, inner: SessionId) -> Option<SessionId> {
        self.0.state::<GuiState>().terminal_exited(inner)
    }

    fn announce(&mut self, session_id: SessionId, had_output: bool) {
        let _ = events::TerminalExited {
            session_id,
            had_output,
        }
        .emit(&self.0);
    }
}

/// Keeps each session's output ahead of its exit, and remembers which sessions
/// showed anything: only those keep their tab (§3.4).
struct TerminalRouter<S> {
    sink: S,
    /// Inner ids whose output reached an open tab.
    spoke: HashSet<SessionId>,
}

impl<S: TerminalSink> TerminalRouter<S> {
    fn new(sink: S) -> Self {
        Self {
            sink,
            spoke: HashSet::new(),
        }
    }

    /// The core queues a session's bytes on the tap before it reports the exit, so
    /// whatever the tap holds when an exit comes in is routed first. Only that much:
    /// a tab streaming nonstop must not hold other tabs' exits back.
    async fn run(
        &mut self,
        mut raw_rx: mpsc::Receiver<(SessionId, Vec<u8>)>,
        mut exit_rx: mpsc::Receiver<SessionId>,
    ) {
        loop {
            tokio::select! {
                Some((inner, bytes)) = raw_rx.recv() => self.output(inner, bytes),
                Some(inner) = exit_rx.recv() => self.exit_after_output(&mut raw_rx, inner).await,
                else => break,
            }
        }
    }

    /// Routes what the tap holds now, then ends `inner`.
    async fn exit_after_output(
        &mut self,
        raw_rx: &mut mpsc::Receiver<(SessionId, Vec<u8>)>,
        inner: SessionId,
    ) {
        // `len` counts a send still mid-write too, which `recv` awaits without
        // blocking the runtime thread (`try_recv` would park it).
        for _ in 0..raw_rx.len() {
            let Some((inner, bytes)) = raw_rx.recv().await else {
                break;
            };
            self.output(inner, bytes);
        }
        self.exit(inner);
    }

    fn output(&mut self, inner: SessionId, bytes: Vec<u8>) {
        if self.sink.output(inner, bytes) {
            self.spoke.insert(inner);
        }
    }

    fn exit(&mut self, inner: SessionId) {
        let had_output = self.spoke.remove(&inner);
        if had_output {
            self.sink.output(inner, END_LINE.to_vec());
        }
        if let Some(public) = self.sink.exited(inner) {
            self.sink.announce(public, had_output);
        }
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
        CoreEvent::FileTransferProgress(transfer_id, done, total) => {
            SftpOutbound::Progress(events::TransferProgress(TransferProgressDto {
                session_id,
                transfer_id,
                done,
                total,
            }))
        }
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
            CoreEvent::FileTransferProgress(transfer_id, _, _) => app
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
    use std::collections::HashMap;

    #[derive(Debug, PartialEq)]
    enum Delivered {
        Output(SessionId, Vec<u8>),
        Exited(SessionId, bool),
    }

    /// Open tabs by inner id (-> public id) and everything delivered to them.
    struct Tabs {
        open: HashMap<SessionId, SessionId>,
        log: Vec<Delivered>,
    }

    impl TerminalSink for Tabs {
        fn output(&mut self, inner: SessionId, bytes: Vec<u8>) -> bool {
            let open = self.open.contains_key(&inner);
            if open {
                self.log.push(Delivered::Output(inner, bytes));
            }
            open
        }

        fn exited(&mut self, inner: SessionId) -> Option<SessionId> {
            self.open.remove(&inner)
        }

        fn announce(&mut self, session_id: SessionId, had_output: bool) {
            self.log.push(Delivered::Exited(session_id, had_output));
        }
    }

    fn router(open: &[(SessionId, SessionId)]) -> TerminalRouter<Tabs> {
        TerminalRouter::new(Tabs {
            open: open.iter().copied().collect(),
            log: Vec::new(),
        })
    }

    fn output(inner: SessionId, text: &str) -> Delivered {
        Delivered::Output(inner, text.as_bytes().to_vec())
    }

    // A server that refuses the shell says why in the session's last chunk, which must reach
    // the tab before the exit drops the tab's routing.
    #[tokio::test]
    async fn a_sessions_last_output_is_routed_before_its_exit() {
        let (raw_tx, mut raw_rx) = mpsc::channel(8);
        for chunk in ["Permission denied", ", please try again."] {
            raw_tx.try_send((1, chunk.as_bytes().to_vec())).unwrap();
        }

        let mut router = router(&[(1, 11)]);
        router.exit_after_output(&mut raw_rx, 1).await;

        let end_line = Delivered::Output(1, END_LINE.to_vec());
        assert_eq!(
            router.sink.log,
            [
                output(1, "Permission denied"),
                output(1, ", please try again."),
                end_line,
                Delivered::Exited(11, true),
            ]
        );
        assert!(router.sink.open.is_empty(), "routing dropped");
        assert!(router.spoke.is_empty(), "nothing kept per session");
    }

    #[test]
    fn a_session_that_never_spoke_ends_without_the_end_line() {
        let mut router = router(&[(2, 12)]);
        router.exit(2);
        assert_eq!(router.sink.log, [Delivered::Exited(12, false)]);
    }

    // `close_terminal` drops the routing first, so the exit that follows finds no tab.
    #[test]
    fn a_tab_the_user_closed_gets_no_end_line_and_no_event() {
        let mut router = router(&[(3, 13)]);
        router.output(3, b"$ ".to_vec());
        router.sink.open.remove(&3);
        router.exit(3);
        assert_eq!(router.sink.log, [output(3, "$ ")]);
        assert!(router.spoke.is_empty());
    }

    #[test]
    fn output_nobody_took_does_not_count() {
        let mut router = router(&[(5, 15)]);
        router.output(4, b"stray".to_vec());
        assert!(router.spoke.is_empty());
        // Another session's exit leaves this one's output and state alone.
        router.output(5, b"up".to_vec());
        router.exit(4);
        assert_eq!(router.sink.log, [output(5, "up")]);
        assert!(router.spoke.contains(&5));
    }

    fn file(name: &str) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: format!("/srv/{name}"),
            size: 0,
            is_dir: false,
            modified: None,
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
        match map_sftp_event(7, CoreEvent::FileTransferProgress(42, 512, 2048)).unwrap() {
            SftpOutbound::Progress(events::TransferProgress(dto)) => {
                assert_eq!(dto.session_id, 7);
                assert_eq!(dto.transfer_id, 42);
                assert_eq!((dto.done, dto.total), (512, 2048));
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
