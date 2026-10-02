//! Backend-managed state (tech-gui.md §3.4). Holds the host cache, the
//! metrics/status poller, the PTY manager (with the raw-byte tap, §3.6), the
//! per-session terminal channels, and the session registry that maps public ids to
//! the core's inner handles. The shared engine channel feeds the bridge.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use omnyssh_core::event::{CoreEvent, SessionId, TransferId};
use omnyssh_core::ssh::client::Host;
use omnyssh_core::ssh::pool::PollManager;
use omnyssh_core::ssh::pty::PtyManager;
use omnyssh_core::ssh::sftp::{SftpCommand, SftpManager};
use omnyssh_core::ssh::tunnel::TunnelManager;
use tauri::ipc::Channel;
use tokio::sync::mpsc;

use crate::dto::{HostDto, TerminalBytes, TunnelStatusDto};

/// Metric poll cadence. Mirrors the TUI's fixed interval; a configurable refresh
/// interval lands with settings in Stage 4.3 (tech-gui.md §4.3).
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Maps frontend-facing **public** session ids to the core's **inner** handles, and
/// back for terminals (the bridge labels `terminal-exited` by inner PTY id, §3.4).
/// A single monotonic id space keeps terminal — and, from Stage 3.2, SFTP — ids from
/// ever colliding in the frontend.
#[derive(Default)]
pub struct SessionRegistry {
    next: SessionId,
    pty_by_public: HashMap<SessionId, SessionId>,
    public_by_pty: HashMap<SessionId, SessionId>,
}

impl SessionRegistry {
    /// Allocate a fresh public id from the shared monotonic space (§3.4). SFTP
    /// sessions use this directly — they are keyed by their public id, with no inner
    /// mapping — so terminal and SFTP ids can never collide in the frontend.
    pub fn allocate(&mut self) -> SessionId {
        self.next += 1;
        self.next
    }

    /// Allocate a fresh public id for a terminal backed by PTY inner id `inner`.
    pub fn register_terminal(&mut self, inner: SessionId) -> SessionId {
        let public = self.allocate();
        self.pty_by_public.insert(public, inner);
        self.public_by_pty.insert(inner, public);
        public
    }

    /// The PTY inner id for a public id (command → core), if the session is live.
    pub fn pty_inner(&self, public: SessionId) -> Option<SessionId> {
        self.pty_by_public.get(&public).copied()
    }

    /// Drop a terminal by public id (user-initiated close), returning its inner id.
    pub fn remove_by_public(&mut self, public: SessionId) -> Option<SessionId> {
        let inner = self.pty_by_public.remove(&public)?;
        self.public_by_pty.remove(&inner);
        Some(inner)
    }

    /// Drop a terminal by inner id (remote exit), returning its public id.
    pub fn remove_by_pty(&mut self, inner: SessionId) -> Option<SessionId> {
        let public = self.public_by_pty.remove(&inner)?;
        self.pty_by_public.remove(&public);
        Some(public)
    }
}

pub struct GuiState {
    /// Cached host list, mapped to `HostDto` on demand for `list_hosts`.
    hosts: RwLock<Vec<Host>>,
    /// Metrics/status/discovery poller; replaced on reload, dropped on exit.
    poll: Mutex<Option<PollManager>>,
    /// All terminal sessions; constructed with the raw-byte tap (§3.6).
    pty: Mutex<PtyManager>,
    /// Terminal output routing: PTY inner id -> that tab's frontend channel.
    term_channels: Mutex<HashMap<SessionId, Channel<TerminalBytes>>>,
    /// One SFTP manager per tab, keyed by its public session id (§3.4).
    sftp: Mutex<HashMap<SessionId, SftpManager>>,
    /// SFTP progress routing: GUI-allocated transfer id -> owning session (§3.4).
    transfer_owner: Mutex<HashMap<TransferId, SessionId>>,
    /// Monotonic source for GUI-allocated transfer ids (§3.4).
    next_transfer_id: AtomicU64,
    /// One-shot latch so the startup update check fires once, on the frontend's first
    /// `reload_hosts` — i.e. only after its event bridge is listening (§3.4).
    update_check_started: AtomicBool,
    /// The host with a key-setup in flight, if any. One run at a time, mirroring the
    /// TUI's single-popup model — a second run would race a second `hosts.toml` write
    /// and clobber the progress panel (§4.2).
    key_setup: Mutex<Option<String>>,
    /// Public id <-> inner handle mapping for all sessions.
    sessions: Mutex<SessionRegistry>,
    /// Every host's port-forwarding tunnel. Unlike the pollers it is never rebuilt on
    /// reload — that would drop live tunnels on each save — only reconciled.
    tunnels: Mutex<TunnelManager>,
    /// One-shot latch so tunnels autostart once, on the first successful host load.
    tunnels_autostarted: AtomicBool,
    /// The last status of every tunnel that has not stopped, replayed to a
    /// frontend that reloads while its tunnels keep running.
    tunnel_statuses: Mutex<HashMap<String, TunnelStatusDto>>,
    /// Shared engine channel the bridge drains; cloned to `PollManager`/`PtyManager`.
    engine_tx: mpsc::Sender<CoreEvent>,
}

impl GuiState {
    pub fn new(engine_tx: mpsc::Sender<CoreEvent>, pty: PtyManager) -> Self {
        Self {
            hosts: RwLock::new(Vec::new()),
            poll: Mutex::new(None),
            pty: Mutex::new(pty),
            term_channels: Mutex::new(HashMap::new()),
            sftp: Mutex::new(HashMap::new()),
            transfer_owner: Mutex::new(HashMap::new()),
            next_transfer_id: AtomicU64::new(0),
            update_check_started: AtomicBool::new(false),
            key_setup: Mutex::new(None),
            sessions: Mutex::new(SessionRegistry::default()),
            tunnels: Mutex::new(TunnelManager::new(engine_tx.clone())),
            tunnels_autostarted: AtomicBool::new(false),
            tunnel_statuses: Mutex::new(HashMap::new()),
            engine_tx,
        }
    }

    /// Replace the host cache with a freshly loaded list.
    pub fn set_hosts(&self, hosts: Vec<Host>) {
        *self.hosts.write().expect("hosts lock poisoned") = hosts;
    }

    /// Clone the shared engine sender for a command that drives the core directly and
    /// reports via `CoreEvent` on the bridge — key setup (§4.2) and the startup update
    /// check (§4.3). Same channel the pollers and PTY sessions use (§3.4).
    pub fn engine_sender(&self) -> mpsc::Sender<CoreEvent> {
        self.engine_tx.clone()
    }

    /// Snapshot the cached hosts as wire DTOs (secret fields dropped by the map).
    pub fn host_dtos(&self) -> Vec<HostDto> {
        self.hosts
            .read()
            .expect("hosts lock poisoned")
            .iter()
            .map(HostDto::from)
            .collect()
    }

    /// Resolve `names` to their full `Host` records for a backend-only op (snippet
    /// execute, key setup). Secret material rides along here and never crosses the
    /// IPC boundary. Unknown names are skipped; order follows `names`.
    pub fn hosts_by_name(&self, names: &[String]) -> Vec<Host> {
        let hosts = self.hosts.read().expect("hosts lock poisoned");
        names
            .iter()
            .filter_map(|name| hosts.iter().find(|h| &h.name == name).cloned())
            .collect()
    }

    /// Clone one host record for a backend-only connect (SFTP `connect` needs the full
    /// `Host`, secrets included; they stay backend-side, §3.4).
    pub fn host_by_name(&self, name: &str) -> Option<Host> {
        self.hosts
            .read()
            .expect("hosts lock poisoned")
            .iter()
            .find(|h| h.name == name)
            .cloned()
    }

    /// Trigger an immediate metric poll of every host (tech-gui.md §4.2). A no-op if
    /// the pollers have not started yet. Non-blocking — it only nudges the poller tasks.
    pub fn refresh_metrics(&self) {
        if let Some(poll) = self.poll.lock().expect("poll lock poisoned").as_ref() {
            poll.refresh_all();
        }
    }

    /// Reserve the single key-setup slot for `host`. `Ok` starts the run; `Err` names the
    /// host already running one, so a concurrent start is rejected instead of racing a
    /// second `hosts.toml` write and clobbering the progress panel (§4.2). Paired with
    /// `end_key_setup`, called on every terminal outcome of the run.
    pub fn try_begin_key_setup(&self, host: &str) -> Result<(), String> {
        let mut slot = self.key_setup.lock().expect("key_setup lock poisoned");
        if let Some(active) = slot.as_ref() {
            return Err(format!("Key setup is already running for '{active}'"));
        }
        *slot = Some(host.to_string());
        Ok(())
    }

    /// Release the key-setup slot when a run reaches any terminal outcome.
    pub fn end_key_setup(&self) {
        *self.key_setup.lock().expect("key_setup lock poisoned") = None;
    }

    /// Claim the one-shot startup update check: `true` exactly once, on the first call.
    /// Driven from `reload_hosts` so the check runs only after the frontend's event
    /// bridge is listening, never dropping `update-available` on a listener race (§3.4).
    pub fn claim_update_check(&self) -> bool {
        self.update_check_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Start (or restart) the pollers for the cached hosts. Must run inside the
    /// Tauri async runtime — `PollManager::start` spawns tokio tasks (§3.4).
    pub fn restart_pollers(&self) {
        let hosts = self.hosts.read().expect("hosts lock poisoned").clone();
        let mut poll = self.poll.lock().expect("poll lock poisoned");
        if let Some(old) = poll.take() {
            old.shutdown();
        }
        *poll = Some(PollManager::start(
            hosts,
            self.engine_tx.clone(),
            POLL_INTERVAL,
        ));
    }

    /// Start (or restart) `name`'s tunnel. Must run inside the Tauri async runtime —
    /// the tunnel is a tokio task.
    pub fn start_tunnel(&self, name: &str) -> Result<(), String> {
        let host = self
            .host_by_name(name)
            .ok_or_else(|| format!("unknown host '{name}'"))?;
        if host.local_forwards.is_empty() {
            return Err(format!("'{name}' has no port forwards"));
        }
        self.tunnels
            .lock()
            .expect("tunnels lock poisoned")
            .start(host);
        Ok(())
    }

    /// Stop `name`'s tunnel; a no-op when none runs.
    pub fn stop_tunnel(&self, name: &str) {
        self.tunnels
            .lock()
            .expect("tunnels lock poisoned")
            .stop(name);
    }

    /// Apply a reloaded host list to the running tunnels: restart the ones whose
    /// connection or rules changed, stop the ones whose host or rules are gone.
    pub fn sync_tunnels(&self) {
        let hosts = self.hosts.read().expect("hosts lock poisoned").clone();
        self.tunnels
            .lock()
            .expect("tunnels lock poisoned")
            .sync(&hosts);
    }

    /// Start every tunnel marked to start on launch — once, on the first call that
    /// finds hosts loaded. Driven from `reload_hosts`, so the statuses reach a
    /// frontend that is already listening (§3.4).
    pub fn autostart_tunnels(&self) {
        if self
            .tunnels_autostarted
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let hosts = self.hosts.read().expect("hosts lock poisoned").clone();
        self.tunnels
            .lock()
            .expect("tunnels lock poisoned")
            .autostart(&hosts);
    }

    /// Record a tunnel's status and `emit` it under one lock, so a replay can never
    /// deliver an older status after a newer one.
    pub fn tunnel_status_changed(
        &self,
        host_name: &str,
        status: &TunnelStatusDto,
        emit: impl FnOnce(),
    ) {
        let mut statuses = self
            .tunnel_statuses
            .lock()
            .expect("tunnel_statuses lock poisoned");
        if matches!(status, TunnelStatusDto::Stopped) {
            statuses.remove(host_name);
        } else {
            statuses.insert(host_name.to_string(), status.clone());
        }
        emit();
    }

    /// `emit` the last status of every tunnel of a host still in the list — the
    /// frontend's store is empty after a reload (§3.4). A status left by a host
    /// that is gone is dropped here.
    pub fn replay_tunnel_statuses(&self, mut emit: impl FnMut(&str, &TunnelStatusDto)) {
        let names: Vec<String> = self
            .hosts
            .read()
            .expect("hosts lock poisoned")
            .iter()
            .map(|h| h.name.clone())
            .collect();
        let mut statuses = self
            .tunnel_statuses
            .lock()
            .expect("tunnel_statuses lock poisoned");
        statuses.retain(|name, _| names.contains(name));
        for (name, status) in statuses.iter() {
            emit(name, status);
        }
    }

    /// Open a terminal for `host_name`, wiring its raw-output `channel`, and return
    /// the public session id. The session task starts here but cannot emit output
    /// until it connects, so registering the channel right after `open` beats the
    /// first byte (§3.4). Locks are taken un-nested to stay deadlock-free.
    pub fn open_terminal(
        &self,
        host_name: &str,
        cols: u16,
        rows: u16,
        channel: Channel<TerminalBytes>,
    ) -> Result<SessionId, String> {
        let host = self
            .hosts
            .read()
            .expect("hosts lock poisoned")
            .iter()
            .find(|h| h.name == host_name)
            .cloned()
            .ok_or_else(|| format!("unknown host '{host_name}'"))?;
        let inner = self
            .pty
            .lock()
            .expect("pty lock poisoned")
            .open(&host, cols, rows, self.engine_tx.clone())
            .map_err(|e| e.to_string())?;
        self.term_channels
            .lock()
            .expect("term_channels lock poisoned")
            .insert(inner, channel);
        Ok(self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .register_terminal(inner))
    }

    /// Resolve a public id to its live PTY inner id and run `f` on the manager. The
    /// un-nested lock order (sessions dropped before pty is taken) lives here alone so
    /// write/resize can't drift apart. Unknown/closed ids are a no-op.
    fn on_pty_inner(&self, public: SessionId, f: impl FnOnce(&mut PtyManager, SessionId)) {
        let inner = self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .pty_inner(public);
        if let Some(inner) = inner {
            let mut pty = self.pty.lock().expect("pty lock poisoned");
            f(&mut pty, inner);
        }
    }

    /// Forward keystrokes to a terminal. Unknown/closed ids are a no-op.
    pub fn write_terminal(&self, public: SessionId, data: &[u8]) {
        self.on_pty_inner(public, |pty, inner| {
            let _ = pty.write(inner, data);
        });
    }

    /// Relay a resize (window_change) to a terminal. Unknown/closed ids are a no-op.
    pub fn resize_terminal(&self, public: SessionId, cols: u16, rows: u16) {
        self.on_pty_inner(public, |pty, inner| {
            let _ = pty.resize(inner, cols, rows);
        });
    }

    /// User-initiated close: tear down the core session and drop its routing state,
    /// so the task's later `PtyExited` finds no mapping and emits no `terminal-exited`
    /// (the frontend already tore the tab down, §3.4).
    pub fn close_terminal(&self, public: SessionId) {
        let inner = self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .remove_by_public(public);
        if let Some(inner) = inner {
            self.pty.lock().expect("pty lock poisoned").close(inner);
            self.term_channels
                .lock()
                .expect("term_channels lock poisoned")
                .remove(&inner);
        }
    }

    /// Route a raw PTY chunk (keyed by inner id) into its tab's channel. Called by
    /// the raw-output forwarder; unknown/closed ids are dropped (§3.6). `true` when a
    /// tab took it.
    pub fn send_terminal_output(&self, inner: SessionId, bytes: Vec<u8>) -> bool {
        let channel = self
            .term_channels
            .lock()
            .expect("term_channels lock poisoned")
            .get(&inner)
            .cloned();
        channel.is_some_and(|channel| channel.send(TerminalBytes(bytes)).is_ok())
    }

    /// Remote-side exit: map the inner id to its public id and drop all routing
    /// state. Returns the public id to emit `terminal-exited` for, or `None` if the
    /// user already closed it (§3.4). The ended task never prunes its own
    /// `PtyManager` slot, so — like the TUI's `PtyExited` handler — we `close` it
    /// here (a no-op control send to a dead task) to avoid leaking the session Vec
    /// entry on every remote exit or failed connect.
    pub fn terminal_exited(&self, inner: SessionId) -> Option<SessionId> {
        let public = self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .remove_by_pty(inner);
        self.pty.lock().expect("pty lock poisoned").close(inner);
        self.term_channels
            .lock()
            .expect("term_channels lock poisoned")
            .remove(&inner);
        public
    }

    /// Store a freshly connected SFTP manager under a new public session id (§3.4).
    /// The id comes from the shared registry so it never collides with a terminal id.
    pub fn register_sftp(&self, manager: SftpManager) -> SessionId {
        let id = self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .allocate();
        self.sftp
            .lock()
            .expect("sftp lock poisoned")
            .insert(id, manager);
        id
    }

    /// Enqueue a command to a live SFTP session. Unknown/closed ids are a no-op
    /// (fire-and-forget, mirroring the core's own model, §3.4).
    pub fn send_sftp(&self, session_id: SessionId, cmd: SftpCommand) {
        if let Some(manager) = self
            .sftp
            .lock()
            .expect("sftp lock poisoned")
            .get(&session_id)
        {
            manager.send(cmd);
        }
    }

    /// Cancel the transfer a live SFTP session is running; unknown ids are a no-op.
    pub fn cancel_sftp(&self, session_id: SessionId) {
        if let Some(manager) = self
            .sftp
            .lock()
            .expect("sftp lock poisoned")
            .get(&session_id)
        {
            manager.cancel();
        }
    }

    /// Allocate a transfer id owned by `session_id`, so its `FileTransferProgress`
    /// events route back to the right tab via `transfer_owner` (§3.4).
    pub fn next_transfer(&self, session_id: SessionId) -> TransferId {
        let id = self.next_transfer_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.transfer_owner
            .lock()
            .expect("transfer_owner lock poisoned")
            .insert(id, session_id);
        id
    }

    /// The session that owns a transfer id, for progress routing (§3.4/§4.1).
    pub fn transfer_session(&self, transfer_id: TransferId) -> Option<SessionId> {
        self.transfer_owner
            .lock()
            .expect("transfer_owner lock poisoned")
            .get(&transfer_id)
            .copied()
    }

    /// User-initiated SFTP close: drop the manager (its transfer cancelled, a graceful
    /// `Disconnect` to its task) and prune this session's transfer-owner entries (§3.4).
    pub fn close_sftp(&self, session_id: SessionId) {
        if let Some(manager) = self
            .sftp
            .lock()
            .expect("sftp lock poisoned")
            .remove(&session_id)
        {
            manager.disconnect();
        }
        self.transfer_owner
            .lock()
            .expect("transfer_owner lock poisoned")
            .retain(|_, owner| *owner != session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ids_are_unique_and_monotonic() {
        let mut reg = SessionRegistry::default();
        let a = reg.register_terminal(1);
        let b = reg.register_terminal(2);
        let c = reg.register_terminal(3);
        assert_eq!((a, b, c), (1, 2, 3));
    }

    #[test]
    fn distinct_inner_ids_get_distinct_public_ids() {
        // Two PTY sessions that (hypothetically) reused an inner id space still map
        // to unique public ids — the frontend never sees a collision (§3.4).
        let mut reg = SessionRegistry::default();
        let first = reg.register_terminal(1);
        let second = reg.register_terminal(1);
        assert_ne!(first, second);
    }

    #[test]
    fn maps_public_to_inner_both_ways() {
        let mut reg = SessionRegistry::default();
        let public = reg.register_terminal(7);
        assert_eq!(reg.pty_inner(public), Some(7));
    }

    #[test]
    fn remove_by_public_clears_both_directions() {
        let mut reg = SessionRegistry::default();
        let public = reg.register_terminal(7);
        assert_eq!(reg.remove_by_public(public), Some(7));
        assert_eq!(reg.pty_inner(public), None);
        // The reverse mapping is gone too: a late exit finds nothing to emit.
        assert_eq!(reg.remove_by_pty(7), None);
    }

    #[test]
    fn remove_by_pty_returns_public_for_the_exit_event() {
        let mut reg = SessionRegistry::default();
        let public = reg.register_terminal(9);
        assert_eq!(reg.remove_by_pty(9), Some(public));
        assert_eq!(reg.pty_inner(public), None);
    }

    #[test]
    fn removing_one_session_leaves_others_intact() {
        let mut reg = SessionRegistry::default();
        let a = reg.register_terminal(10);
        let b = reg.register_terminal(20);
        reg.remove_by_public(a);
        assert_eq!(reg.pty_inner(b), Some(20));
    }

    #[test]
    fn unknown_ids_resolve_to_none() {
        let reg = SessionRegistry::default();
        assert_eq!(reg.pty_inner(999), None);
    }

    #[test]
    fn sftp_and_terminal_ids_share_one_monotonic_space() {
        // An SFTP `allocate` and a terminal `register` draw from the same counter, so
        // the frontend never sees a terminal id collide with an SFTP id (§3.4).
        let mut reg = SessionRegistry::default();
        let term = reg.register_terminal(1);
        let sftp = reg.allocate();
        let term2 = reg.register_terminal(1);
        assert_eq!((term, sftp, term2), (1, 2, 3));
        // The SFTP id has no inner PTY mapping — it is keyed by its public id directly.
        assert_eq!(reg.pty_inner(sftp), None);
    }

    #[test]
    fn transfer_ids_are_unique_and_route_to_their_owning_session() {
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(8);
        let state = GuiState::new(engine_tx, PtyManager::new());

        // Two concurrent SFTP tabs (public ids 1 and 2) each issue transfers.
        let (a, b) = (1, 2);
        let t1 = state.next_transfer(a);
        let t2 = state.next_transfer(b);
        let t3 = state.next_transfer(a);
        assert!(
            t1 != t2 && t2 != t3 && t1 != t3,
            "transfer ids must be unique"
        );
        assert_eq!(state.transfer_session(t1), Some(a));
        assert_eq!(state.transfer_session(t2), Some(b));
        assert_eq!(state.transfer_session(t3), Some(a));
        assert_eq!(state.transfer_session(999), None);

        // Closing one session prunes only its transfers; the other tab is untouched.
        state.close_sftp(a);
        assert_eq!(state.transfer_session(t1), None);
        assert_eq!(state.transfer_session(t3), None);
        assert_eq!(state.transfer_session(t2), Some(b));
    }

    // A remote exit must prune the core PtyManager slot (the ended task never removes
    // its own entry). Without `pty.close` in `terminal_exited` this leaks per exit.
    #[tokio::test]
    async fn remote_exit_prunes_the_pty_session() {
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(8);
        let (raw_tx, _raw_rx) = mpsc::channel(8);
        let state = GuiState::new(engine_tx, PtyManager::with_raw_output(raw_tx));
        state.set_hosts(vec![Host {
            name: "h".to_string(),
            ..Host::default()
        }]);

        // A no-op output channel is enough — this asserts lifecycle, not bytes.
        let channel = Channel::new(|_| Ok(()));
        let public = state.open_terminal("h", 80, 24, channel).unwrap();
        let inner = state
            .sessions
            .lock()
            .unwrap()
            .pty_inner(public)
            .expect("registered");

        // Live session: `parser_for` is Some only while the manager holds the slot.
        assert!(state.pty.lock().unwrap().parser_for(inner).is_some());

        assert_eq!(state.terminal_exited(inner), Some(public));
        // Pruned — no leaked PtyManager.sessions entry.
        assert!(state.pty.lock().unwrap().parser_for(inner).is_none());
    }

    #[test]
    fn key_setup_runs_one_at_a_time() {
        // A second start while one is in flight is rejected (no racing hosts.toml write /
        // panel clobber); the slot frees on the terminal outcome so a retry succeeds (§4.2).
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(8);
        let state = GuiState::new(engine_tx, PtyManager::new());

        assert!(state.try_begin_key_setup("web-1").is_ok());
        let busy = state.try_begin_key_setup("web-2").unwrap_err();
        assert!(
            busy.contains("web-1"),
            "rejection names the busy host: {busy}"
        );
        // The same host cannot double-start either.
        assert!(state.try_begin_key_setup("web-1").is_err());

        state.end_key_setup();
        assert!(state.try_begin_key_setup("web-2").is_ok());
    }

    #[test]
    fn update_check_is_claimed_exactly_once() {
        // The startup update check must fire once (the first reload), never again — so a
        // later `reload_hosts` (after a host edit) does not re-hit GitHub (§3.4).
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(8);
        let state = GuiState::new(engine_tx, PtyManager::new());
        assert!(state.claim_update_check());
        assert!(!state.claim_update_check());
        assert!(!state.claim_update_check());
    }

    #[test]
    fn open_terminal_rejects_an_unknown_host() {
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(8);
        let state = GuiState::new(engine_tx, PtyManager::new());
        let channel = Channel::new(|_| Ok(()));
        assert!(state.open_terminal("nope", 80, 24, channel).is_err());
    }

    fn tunnel_host(name: &str, autostart: bool, forwards: &[&str]) -> Host {
        Host {
            name: name.to_string(),
            // Nothing answers on port 1, so a started tunnel just keeps retrying.
            hostname: "127.0.0.1".to_string(),
            port: 1,
            local_forwards: forwards.iter().map(|f| f.parse().expect("rule")).collect(),
            tunnel_autostart: autostart,
            ..Host::default()
        }
    }

    /// A loopback port nothing listens on right now.
    fn free_port() -> u16 {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        probe.local_addr().expect("addr").port()
    }

    #[tokio::test]
    async fn start_tunnel_needs_a_known_host_with_forwards() {
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(8);
        let state = GuiState::new(engine_tx, PtyManager::new());
        state.set_hosts(vec![tunnel_host("bare", false, &[])]);
        assert!(state
            .start_tunnel("ghost")
            .unwrap_err()
            .contains("unknown host"));
        assert!(state
            .start_tunnel("bare")
            .unwrap_err()
            .contains("no port forwards"));
    }

    #[tokio::test]
    async fn tunnels_autostart_once_and_only_where_asked() {
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(64);
        let state = GuiState::new(engine_tx, PtyManager::new());
        let rule = |port: u16| format!("{port}:localhost:80");
        state.set_hosts(vec![
            tunnel_host("auto", true, &[&rule(free_port())]),
            tunnel_host("manual", false, &[&rule(free_port())]),
            tunnel_host("empty", true, &[]),
        ]);

        state.autostart_tunnels();
        let running = |name| state.tunnels.lock().unwrap().is_running(name);
        assert!(running("auto"));
        assert!(!running("manual") && !running("empty"));

        // A later reload must not start what the user has since stopped.
        state.stop_tunnel("auto");
        state.autostart_tunnels();
        assert!(!running("auto"));
    }

    #[test]
    fn a_reloaded_frontend_gets_the_live_tunnel_statuses_back() {
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(8);
        let state = GuiState::new(engine_tx, PtyManager::new());
        state.set_hosts(vec![
            tunnel_host("up", false, &[]),
            tunnel_host("down", false, &[]),
        ]);

        let mut emitted = 0;
        let retrying = TunnelStatusDto::Retrying {
            message: "connection lost".to_string(),
        };
        state.tunnel_status_changed("up", &TunnelStatusDto::Up, || emitted += 1);
        state.tunnel_status_changed("down", &retrying, || emitted += 1);
        state.tunnel_status_changed("down", &TunnelStatusDto::Stopped, || emitted += 1);
        state.tunnel_status_changed("gone", &TunnelStatusDto::Up, || emitted += 1);
        assert_eq!(emitted, 4, "every change still goes out live");

        // Only a tunnel that has not stopped, of a host still listed, comes back.
        let mut replayed = Vec::new();
        state.replay_tunnel_statuses(|name, status| {
            replayed.push((name.to_string(), status.clone()))
        });
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].0, "up");
        assert!(matches!(replayed[0].1, TunnelStatusDto::Up));
    }
}
