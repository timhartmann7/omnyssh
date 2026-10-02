//! Application root: terminal setup, the shared/UI state split, and the main
//! event loop.
//!
//! Feature-specific state types and the matching `App` methods live in the
//! submodules declared below; this file keeps the core types (`App`,
//! `AppState`, `ViewState`, `Screen`, `SortOrder`) and the run / main-loop
//! plumbing. Every public item of the submodules is re-exported here so the
//! rest of the crate keeps using the flat `crate::app::Type` paths.

use std::collections::{HashMap, VecDeque};
use std::io::Stdout;
use std::sync::Arc;
use std::time::Duration;

use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::{mpsc, RwLock};

use crate::event::{spawn_event_thread, AppEvent};
use crate::keybindings::ParsedKeybindings;
use crate::ui;
use crate::ui::theme::Theme;
use omnyssh_core::config;
use omnyssh_core::config::app_config::AppConfig;
use omnyssh_core::config::snippets::Snippet;
use omnyssh_core::event::{CoreEvent, Metrics, ServiceKind, TransferId};
use omnyssh_core::ssh::client::{ConnectionStatus, Host};
use omnyssh_core::ssh::pool::PollManager;
use omnyssh_core::ssh::pty::PtyManager;
use omnyssh_core::ssh::sftp::{SftpCommand, SftpManager};
use omnyssh_core::ssh::tunnel::{TunnelManager, TunnelStatus};

mod action;
mod actions;
mod file_manager;
mod host;
mod input;
mod snippets;
mod terminal;
mod update;

pub use action::*;
pub use file_manager::*;
pub use host::*;
pub use snippets::*;
pub use terminal::*;
pub use update::*;

// ---------------------------------------------------------------------------
// Screen
// ---------------------------------------------------------------------------

/// The active top-level screen shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Dashboard,
    /// Detail View — shows comprehensive server information.
    DetailView,
    FileManager,
    Snippets,
    /// PTY-backed multi-session terminal.
    Terminal,
}

// ---------------------------------------------------------------------------
// Sort order
// ---------------------------------------------------------------------------

/// How dashboard cards are sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SortOrder {
    /// Alphabetical by host name.
    #[default]
    Name,
    /// Descending CPU usage (highest first).
    Cpu,
    /// Descending RAM usage (highest first).
    Ram,
    /// By connection status: Connected → Connecting → Unknown → Failed.
    Status,
}

impl SortOrder {
    /// Cycle to the next sort order.
    pub fn next(&self) -> Self {
        match self {
            SortOrder::Name => SortOrder::Cpu,
            SortOrder::Cpu => SortOrder::Ram,
            SortOrder::Ram => SortOrder::Status,
            SortOrder::Status => SortOrder::Name,
        }
    }

    /// Human-readable label for display in the dashboard header.
    pub fn label(&self) -> &'static str {
        match self {
            SortOrder::Name => "name",
            SortOrder::Cpu => "cpu",
            SortOrder::Ram => "ram",
            SortOrder::Status => "status",
        }
    }
}

// ---------------------------------------------------------------------------
// AppState — shared between UI and background tasks
// ---------------------------------------------------------------------------

/// Application data shared between the UI thread and background SSH tasks.
/// Wrapped in `Arc<RwLock<_>>` — multiple readers, rare writers.
#[derive(Debug, Default)]
pub struct AppState {
    /// Currently visible screen.
    pub screen: Screen,
    /// Full host list (manual + SSH-config imports).
    pub hosts: Vec<Host>,
    /// Per-host runtime connection status (keyed by `host.name`).
    pub connection_statuses: HashMap<String, ConnectionStatus>,
    /// Live metrics per host, keyed by `host.name`.
    pub metrics: HashMap<String, Metrics>,
    /// Saved command snippets.
    pub snippets: Vec<Snippet>,
    /// Detected services per host.
    pub services: HashMap<String, Vec<omnyssh_core::event::DetectedService>>,
    /// Status of each host's tunnel, keyed by `host.name`. No entry means none
    /// is running.
    pub tunnel_statuses: HashMap<String, TunnelStatus>,
}

// ---------------------------------------------------------------------------
// ViewState — UI-only, not shared
// ---------------------------------------------------------------------------

/// UI-specific state that lives only on the main thread.
pub struct ViewState {
    /// Whether the help popup is currently shown.
    pub show_help: bool,
    /// Scroll offset for help popup (0 = top).
    pub help_scroll: usize,
    /// Transient status message shown in the status bar (overrides hints).
    pub status_message: Option<String>,
    /// State for the host-list / Dashboard screen.
    pub host_list: HostListView,
    /// State for the Snippets screen.
    pub snippets_view: SnippetsView,
    /// State for the File Manager screen.
    pub file_manager: FileManagerView,
    /// State for the Terminal multi-session screen.
    pub terminal_view: TerminalView,
    /// Active colour theme — loaded from config on startup.
    pub theme: Theme,
    /// Parsed keybindings — loaded from config on startup.
    pub keybindings: ParsedKeybindings,
    /// Monotonically-incrementing tick counter for animations (e.g. spinner).
    pub tick_count: u64,
    /// Startup update-notification popup, shown when a newer release exists.
    pub update_popup: Option<UpdatePopup>,
    /// Encrypted keys waiting for a passphrase, one per key; the first is shown.
    pub passphrase_prompts: Vec<PassphrasePrompt>,
    /// Logins waiting for a password, oldest first; the first is shown.
    pub password_prompts: Vec<PasswordPrompt>,
}

impl ViewState {
    /// Constructs a `ViewState` where `theme` and `keybindings` are filled
    /// with defaults so that struct-update syntax can supply them later.
    fn default_inner() -> Self {
        Self {
            show_help: false,
            help_scroll: 0,
            status_message: None,
            host_list: HostListView::default(),
            snippets_view: SnippetsView::default(),
            file_manager: FileManagerView::default(),
            terminal_view: TerminalView::default(),
            theme: Theme::default(),
            keybindings: ParsedKeybindings::default(),
            tick_count: 0,
            update_popup: None,
            passphrase_prompts: Vec::new(),
            password_prompts: Vec::new(),
        }
    }
}

impl Default for ViewState {
    fn default() -> Self {
        Self::default_inner()
    }
}

/// In-memory prompt for an encrypted SSH identity file.
pub struct PassphrasePrompt {
    pub host_name: String,
    pub key_path: String,
    pub field: FormField,
    pub error: Option<String>,
    /// Set while the key is being decrypted; input is ignored meanwhile.
    pub unlocking: bool,
}

/// In-memory prompt for the login password a connection waits on.
pub struct PasswordPrompt {
    pub request_id: u64,
    pub host_name: String,
    /// `user@host`, as the server is asked.
    pub login: String,
    /// The previous password for this login was refused.
    pub retry: bool,
    /// Fingerprint of a host key first seen on this connection.
    pub new_host_key: Option<String>,
    pub field: FormField,
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// How long quitting waits for a running transfer to stop and clean up after itself.
const QUIT_GRACE: Duration = Duration::from_secs(3);

/// Root application struct — owns the terminal, state, and event channel.
pub struct App {
    /// Shared state readable by background tokio tasks.
    pub state: Arc<RwLock<AppState>>,
    /// UI-only state, main thread only.
    pub view: ViewState,
    /// Sender half — given to the event thread and the core-event forwarder.
    event_tx: mpsc::Sender<AppEvent>,
    /// Receiver half — consumed by the main loop.
    event_rx: mpsc::Receiver<AppEvent>,
    /// Sender half of the domain-event channel — given to the SSH engine and
    /// background tasks.
    core_tx: mpsc::Sender<CoreEvent>,
    /// Receiver half of the domain-event channel; taken by `run`, which
    /// forwards each event into the main channel as `AppEvent::Core`.
    core_rx: Option<mpsc::Receiver<CoreEvent>>,
    /// Persistent SFTP session manager for the File Manager.
    sftp_manager: Option<SftpManager>,
    /// SFTP operations of a batch waiting for the one before them to finish. Sent
    /// in one burst, those past the task's 64-command queue were dropped unseen.
    sftp_queue: VecDeque<SftpCommand>,
    /// Monotone counter for assigning unique [`TransferId`] values.
    next_transfer_id: TransferId,
    /// Background metrics polling manager. Stored in `App` (not in
    /// `AppState`) to avoid a reference cycle through `Arc`. Dropping it
    /// signals all per-host tasks to stop.
    poll_manager: Option<PollManager>,
    /// PTY session manager for the Terminal multi-session screen.
    pty_manager: Option<PtyManager>,
    /// Runs the hosts' port-forwarding tunnels. Taken on quit to stop them.
    tunnel_manager: Option<TunnelManager>,
    /// Set by the first `HostsLoaded`: autostart belongs to launch, not to a
    /// reload.
    hosts_loaded: bool,
    /// One heavyweight event (Key, etc.) that was pulled from the channel
    /// during a lightweight-event drain but could not be handled inline.
    /// Consumed at the top of the next main-loop iteration before blocking
    /// on `event_rx.recv()`.
    pending_event: Option<AppEvent>,
    /// Application config — retained so update preferences can be persisted.
    config: AppConfig,
}

impl App {
    /// Creates a new `App` with the provided [`AppConfig`].
    ///
    /// The config is used to set the active theme and keybindings at startup.
    /// Call [`App::default`] to use a default config without loading a file.
    pub fn new(config: AppConfig) -> Self {
        let (tx, rx) = mpsc::channel(256);
        let (core_tx, core_rx) = mpsc::channel(256);
        let theme = Theme::from_name(&config.ui.theme);
        let keybindings = ParsedKeybindings::from_config(&config.keybindings);
        let tunnel_manager = TunnelManager::new(core_tx.clone());
        Self {
            state: Arc::new(RwLock::new(AppState::default())),
            view: ViewState {
                theme,
                keybindings,
                ..ViewState::default_inner()
            },
            event_tx: tx,
            event_rx: rx,
            core_tx,
            core_rx: Some(core_rx),
            sftp_manager: None,
            sftp_queue: VecDeque::new(),
            next_transfer_id: 0,
            poll_manager: None,
            pty_manager: None,
            tunnel_manager: Some(tunnel_manager),
            hosts_loaded: false,
            pending_event: None,
            config,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new(AppConfig::default())
    }
}

impl App {
    /// Runs the application: sets up the terminal, starts the event thread,
    /// spawns the host-loading task, then enters the main render/event loop.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        // Terminal setup.
        crossterm::terminal::enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        crossterm::execute!(
            stdout,
            crossterm::terminal::EnterAlternateScreen,
            crate::utils::mouse::EnableMinimalMouseCapture,
            crossterm::event::EnableBracketedPaste,
        )?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Background event thread (keyboard + tick).
        spawn_event_thread(self.event_tx.clone())?;

        // Forward domain events from the core channel into the main event
        // channel so the main loop consumes a single stream.
        if let Some(mut core_rx) = self.core_rx.take() {
            let tx = self.event_tx.clone();
            tokio::spawn(async move {
                while let Some(e) = core_rx.recv().await {
                    if tx.send(AppEvent::Core(e)).await.is_err() {
                        break;
                    }
                }
            });
        }

        // Load hosts in a background task.
        {
            let tx = self.core_tx.clone();
            tokio::spawn(async move {
                match config::load_all_hosts() {
                    Ok(hosts) => {
                        let _ = tx.send(CoreEvent::HostsLoaded(hosts)).await;
                    }
                    Err(e) => tracing::warn!("Failed to load hosts: {}", e),
                }
            });
        }

        // Load snippets in a background task.
        {
            let tx = self.core_tx.clone();
            tokio::spawn(async move {
                match config::snippets::load_snippets() {
                    Ok(snippets) => {
                        let _ = tx.send(CoreEvent::SnippetsLoaded(snippets)).await;
                    }
                    Err(e) => tracing::warn!("Failed to load snippets: {}", e),
                }
            });
        }

        // Check GitHub for a newer release in a background task. A failed or
        // slow check never delays startup; a skipped version is dropped here.
        if self.config.update.check_on_startup {
            let tx = self.core_tx.clone();
            let skip_version = self.config.update.skip_version.clone();
            tokio::spawn(async move {
                if let Some(info) = omnyssh_core::update::check().await {
                    if info.latest != skip_version {
                        let _ = tx.send(CoreEvent::UpdateAvailable(info)).await;
                    }
                }
            });
        }

        // Main loop.
        let result = self.main_loop(&mut terminal).await;

        // Gracefully shut down all metric polling tasks.
        if let Some(mgr) = self.poll_manager.take() {
            mgr.shutdown();
        }
        // Gracefully shut down the SFTP session.
        self.let_transfer_stop().await;
        if let Some(sftp) = self.sftp_manager.take() {
            sftp.disconnect();
        }
        // Gracefully shut down all PTY sessions.
        if let Some(mgr) = self.pty_manager.take() {
            mgr.shutdown();
        }
        // Stop every tunnel, releasing its local ports.
        if let Some(mgr) = self.tunnel_manager.take() {
            mgr.shutdown();
        }

        // Terminal restore — always runs even if main_loop returned Err.
        // Each step runs unconditionally so a failure in one does not prevent
        // the remaining steps from executing.
        let r1 = crossterm::terminal::disable_raw_mode();
        let r2 = crossterm::execute!(
            terminal.backend_mut(),
            crossterm::terminal::LeaveAlternateScreen,
            crate::utils::mouse::DisableMinimalMouseCapture,
            crossterm::event::DisableBracketedPaste,
        );
        let r3 = terminal.show_cursor();
        r1.and(r2).and(r3)?;

        result
    }

    /// Cancels a running transfer and waits, [`QUIT_GRACE`] at most, for it to report
    /// in: once the app exits, the runtime would drop it mid-step and leave the file it
    /// was part way through.
    async fn let_transfer_stop(&mut self) {
        let Some(sftp) = &self.sftp_manager else {
            return;
        };
        if self.view.file_manager.pending_ops == 0 {
            return;
        }
        sftp.cancel();
        let events = &mut self.event_rx;
        let _ = tokio::time::timeout(QUIT_GRACE, async {
            while let Some(event) = events.recv().await {
                if matches!(event, AppEvent::Core(CoreEvent::SftpOpDone { .. })) {
                    break;
                }
            }
        })
        .await;
    }

    /// Inner loop — separated from `run` so terminal restore always happens.
    async fn main_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> anyhow::Result<()> {
        loop {
            // ----------------------------------------------------------------
            // Render.
            // ----------------------------------------------------------------
            {
                let state = self.state.read().await;
                terminal.draw(|frame| ui::render(frame, &state, &self.view))?;
            }

            // ----------------------------------------------------------------
            // Sync scroll_offset to the actual scrollback depth that vt100
            // applied.  vt100's set_scrollback() clamps the requested value to
            // the number of lines actually stored in the scrollback buffer
            // (which grows up to 1000 as output arrives).  Without this sync
            // the user's scroll_offset can be 1000 while only 80 lines of
            // history exist, forcing them to scroll down ~330 times to return
            // to the live view.
            // ----------------------------------------------------------------
            for tab in &mut self.view.terminal_view.tabs {
                if tab.scroll_offset > 0 {
                    // Non-blocking: skip sync if the reader thread holds the
                    // lock; we'll pick up the correct value on the next frame.
                    if let Ok(p) = tab.parser.try_lock() {
                        // The render already called set_scrollback(tab.scroll_offset);
                        // read back what vt100 actually clamped it to.
                        let actual = p.screen().scrollback();
                        tab.scroll_offset = actual;
                    }
                }
            }

            // ----------------------------------------------------------------
            // Wait for the next event.
            // Check the pending-event slot first (populated by the drain
            // loop below when a heavyweight event is found mid-drain).
            // ----------------------------------------------------------------
            let event = if let Some(e) = self.pending_event.take() {
                e
            } else {
                match self.event_rx.recv().await {
                    Some(e) => e,
                    None => break,
                }
            };

            // ----------------------------------------------------------------
            // Handle event.
            // ----------------------------------------------------------------
            match event {
                AppEvent::Key(key) => {
                    let action = self.handle_key(key).await?;
                    if matches!(action, Some(AppAction::Quit)) {
                        break;
                    }
                    self.process_action(action).await?;
                }

                AppEvent::Paste(text) => {
                    // On the terminal screen a paste is forwarded to the PTY as
                    // one block; elsewhere it is replayed as key events so other
                    // input widgets keep handling it as before.
                    let on_terminal = {
                        let state = self.state.read().await;
                        state.screen == crate::app::Screen::Terminal
                    };
                    if on_terminal {
                        self.handle_term_paste(&text);
                    } else if let Some(prompt) = self.view.passphrase_prompts.first_mut() {
                        // Typed in, never submitted by a trailing newline.
                        if !prompt.unlocking {
                            text.chars()
                                .filter(|c| !matches!(c, '\r' | '\n'))
                                .for_each(|c| prompt.field.insert_char(c));
                        }
                    } else if let Some(prompt) = self.view.password_prompts.first_mut() {
                        text.chars()
                            .filter(|c| !matches!(c, '\r' | '\n'))
                            .for_each(|c| prompt.field.insert_char(c));
                    } else {
                        for key in crate::utils::paste::paste_to_keys(&text) {
                            let action = self.handle_key(key).await?;
                            if matches!(action, Some(AppAction::Quit)) {
                                break;
                            }
                            self.process_action(action).await?;
                        }
                    }
                }

                AppEvent::Tick => {
                    // Increment tick counter for spinner animation.
                    self.view.tick_count = self.view.tick_count.wrapping_add(1);
                }

                AppEvent::TermScroll(delta) => {
                    // Only act when the Terminal screen is focused. The wheel
                    // either scrolls the tab's local scrollback or is forwarded
                    // to the foreground application (see `handle_term_scroll`).
                    let on_terminal = {
                        let state = self.state.read().await;
                        state.screen == crate::app::Screen::Terminal
                    };
                    if on_terminal {
                        self.handle_term_scroll(delta);
                    }
                }

                AppEvent::TerminalResized(cols, rows) => {
                    // Reserve rows: status bar (1) + tab bar (1) + pane border top+bottom (2) = 4.
                    // Reserve cols: pane border left+right (2) = 2.
                    let pty_rows = rows.saturating_sub(4);
                    let pty_cols = cols.saturating_sub(2);
                    if let Some(mgr) = &mut self.pty_manager {
                        for tab in &self.view.terminal_view.tabs {
                            let _ = mgr.resize(tab.session_id, pty_cols, pty_rows);
                        }
                    }
                    // Also update the vt100 parsers so the screen dimensions are
                    // consistent with the render area.
                    for tab in &self.view.terminal_view.tabs {
                        if let Ok(mut p) = tab.parser.lock() {
                            p.set_size(pty_rows, pty_cols);
                        }
                    }
                }

                AppEvent::Core(event) => self.handle_core_event(event).await?,
                AppEvent::PassphraseUnlocked { key_path, result } => {
                    self.finish_unlock(key_path, result);
                }
            }

            // ----------------------------------------------------------------
            // Lightweight-event drain — batch before next render.
            //
            // The main loop renders once per event.  During rapid mouse-wheel
            // scrolling the event queue can hold hundreds of TermScroll events,
            // causing one expensive render (+ parser-lock attempt) per tick and
            // freezing the UI for 1-2 s.  After handling the primary event we
            // consume all remaining lightweight events synchronously so they
            // collapse into a single render on the next iteration.
            //
            // Heavyweight events (Key, etc.) need `await` and must be handled
            // by the main loop; we store the first one in `pending_event` so it
            // is picked up at the top of the next iteration without blocking on
            // `recv()`.
            // ----------------------------------------------------------------
            {
                // Cache whether Terminal screen is active once — avoids an
                // async read inside the tight drain loop.
                let on_terminal = {
                    let st = self.state.read().await;
                    st.screen == crate::app::Screen::Terminal
                };

                loop {
                    match self.event_rx.try_recv() {
                        Ok(AppEvent::Tick) => {
                            self.view.tick_count = self.view.tick_count.wrapping_add(1);
                        }
                        Ok(AppEvent::Core(CoreEvent::PtyOutput(sid))) => {
                            // Mark activity exactly like the primary handler.
                            let active_id = self.view.terminal_view.active_session_id();
                            if active_id != Some(sid) {
                                if let Some(tab) = self
                                    .view
                                    .terminal_view
                                    .tabs
                                    .iter_mut()
                                    .find(|t| t.session_id == sid)
                                {
                                    tab.has_activity = true;
                                }
                            }
                        }
                        Ok(AppEvent::TermScroll(delta)) => {
                            // Handle help popup scrolling first.
                            if self.view.show_help {
                                if delta > 0 {
                                    // Scroll down (wheel down)
                                    self.view.help_scroll =
                                        self.view.help_scroll.saturating_add(delta as usize);
                                } else {
                                    // Scroll up (wheel up)
                                    self.view.help_scroll =
                                        self.view.help_scroll.saturating_sub((-delta) as usize);
                                }
                            } else if on_terminal {
                                self.handle_term_scroll(delta);
                            }
                        }
                        Ok(heavyweight) => {
                            // Can't handle async events here; buffer for next iter.
                            self.pending_event = Some(heavyweight);
                            break;
                        }
                        Err(_) => break, // channel empty
                    }
                }
            }
        }
        Ok(())
    }

    /// Applies a domain event produced by the SSH engine or a background
    /// task. Split out of `main_loop` so the input/domain event split stays
    /// mechanical.
    async fn handle_core_event(&mut self, event: CoreEvent) -> anyhow::Result<()> {
        match event {
            CoreEvent::HostsLoaded(hosts) => {
                let n = hosts.len();
                {
                    let mut state = self.state.write().await;
                    state.hosts = hosts;
                }
                // Rebuild filter with the new host list.
                {
                    let state = self.state.read().await;
                    self.view.host_list.rebuild_filter(
                        &state.hosts,
                        &state.metrics,
                        &state.connection_statuses,
                    );
                    self.view.host_list.rebuild_tags(&state.hosts);

                    // Start (or restart) the metrics polling manager.
                    if let Some(old) = self.poll_manager.take() {
                        old.shutdown();
                    }
                    self.poll_manager = Some(PollManager::start(
                        state.hosts.clone(),
                        self.core_tx.clone(),
                        Duration::from_secs(30),
                    ));
                }
                // A reload only brings running tunnels in line with the new list.
                if std::mem::replace(&mut self.hosts_loaded, true) {
                    self.sync_tunnels().await;
                } else if let Some(tunnels) = &mut self.tunnel_manager {
                    tunnels.autostart(&self.state.read().await.hosts);
                }
                tracing::info!("Loaded {} host(s)", n);
            }

            CoreEvent::MetricsUpdate(host_name, new_metrics) => {
                let mut state = self.state.write().await;
                // Merge new metrics with existing ones to avoid overwriting fields
                let merged = if let Some(existing) = state.metrics.get(&host_name) {
                    Metrics {
                        cpu_percent: new_metrics.cpu_percent.or(existing.cpu_percent),
                        ram_percent: new_metrics.ram_percent.or(existing.ram_percent),
                        disk_percent: new_metrics.disk_percent.or(existing.disk_percent),
                        uptime: new_metrics
                            .uptime
                            .clone()
                            .or_else(|| existing.uptime.clone()),
                        load_avg: new_metrics
                            .load_avg
                            .clone()
                            .or_else(|| existing.load_avg.clone()),
                        os_info: new_metrics
                            .os_info
                            .clone()
                            .or_else(|| existing.os_info.clone()),
                        top_processes: new_metrics
                            .top_processes
                            .clone()
                            .or_else(|| existing.top_processes.clone()),
                        last_updated: new_metrics.last_updated,
                    }
                } else {
                    new_metrics
                };
                state.metrics.insert(host_name, merged);
                // Clear the "Refreshing metrics…" banner once data arrives.
                if matches!(
                    self.view.status_message.as_deref(),
                    Some("Refreshing metrics…")
                ) {
                    self.view.status_message = None;
                }
            }

            CoreEvent::HostStatusChanged(host_name, status) => {
                {
                    let mut state = self.state.write().await;
                    state
                        .connection_statuses
                        .insert(host_name.clone(), status.clone());
                }

                // Show notification for connection/metrics failures (extract essential message)
                if let ConnectionStatus::Failed(ref error) = status {
                    let short_error = error.split(':').next().unwrap_or(error).trim().to_string();

                    self.view.status_message = Some(format!(
                        "Connection failed for '{}': {}",
                        host_name, short_error
                    ));
                }

                // Re-sort if sorting by status.
                if self.view.host_list.sort_order == SortOrder::Status {
                    let state = self.state.read().await;
                    self.view.host_list.rebuild_filter(
                        &state.hosts,
                        &state.metrics,
                        &state.connection_statuses,
                    );
                }
            }

            // ----------------------------------------------------------------
            // Smart Server Context discovery events
            // ----------------------------------------------------------------
            CoreEvent::DiscoveryQuickScanDone(host_name, services) => {
                let mut state = self.state.write().await;
                state.services.insert(host_name, services);
            }

            CoreEvent::DiscoveryFailed(host_name, error) => {
                tracing::debug!(host = %host_name, error = %error, "discovery failed");

                // Show error notification in status bar (extract only the essential error message)
                // Discovery errors often contain the full command after a colon, so extract just the first part
                let short_error = error.split(':').next().unwrap_or(&error).trim().to_string();

                self.view.status_message = Some(format!(
                    "Discovery failed for '{}': {}",
                    host_name, short_error
                ));
            }

            CoreEvent::TunnelStatusChanged(host_name, status) => {
                match &status {
                    TunnelStatus::Up => {
                        self.view.status_message = Some(format!("Tunnel for '{host_name}' is up"));
                    }
                    TunnelStatus::Failed(reason) => {
                        self.view.status_message =
                            Some(format!("Tunnel for '{host_name}' failed: {reason}"));
                    }
                    _ => {}
                }
                let mut state = self.state.write().await;
                if status == TunnelStatus::Stopped {
                    state.tunnel_statuses.remove(&host_name);
                } else {
                    state.tunnel_statuses.insert(host_name, status);
                }
            }

            // ----------------------------------------------------------------
            // Auto SSH Key Setup events
            // ----------------------------------------------------------------
            CoreEvent::KeySetupProgress(host_name, step) => {
                tracing::debug!(host = %host_name, step = ?step, "key setup progress");
                // Update the progress popup's current step.
                if let Some(HostPopup::KeySetupProgress {
                    current_step,
                    host_name: popup_host,
                    ..
                }) = &mut self.view.host_list.popup
                {
                    if *popup_host == host_name {
                        *current_step = Some(step);
                    }
                }
            }

            CoreEvent::KeySetupComplete(host_name, key_path) => {
                tracing::info!(
                    host = %host_name,
                    key = %key_path.display(),
                    "key setup complete"
                );

                // Update host config: set identity_file, key_setup_date,
                // password_auth_disabled.
                {
                    let mut state = self.state.write().await;
                    if let Some(host) = state.hosts.iter_mut().find(|h| h.name == host_name) {
                        host.identity_file = Some(key_path.to_string_lossy().to_string());
                        host.key_setup_date = Some(chrono::Utc::now().to_rfc3339());
                        host.password_auth_disabled = Some(true);
                        // Clear password since key auth is now configured.
                        host.password = None;
                    }
                    // Persist updated hosts to disk.
                    if let Err(e) = config::save_hosts(&state.hosts) {
                        tracing::warn!("Failed to save hosts after key setup: {}", e);
                    }
                }
                // The server now refuses the password a running tunnel would
                // redial with.
                self.sync_tunnels().await;

                // Close popup and show success.
                self.view.host_list.popup = None;
                self.view.status_message = Some(format!(
                    "✓ SSH key setup complete for '{}'. Key: {}",
                    host_name,
                    key_path.display()
                ));
            }

            CoreEvent::KeySetupFailed(host_name, error) => {
                tracing::error!(host = %host_name, error = %error, "key setup failed");
                // Close popup and show error.
                self.view.host_list.popup = None;
                self.view.status_message =
                    Some(format!("✗ Key setup failed for '{}': {}", host_name, error));
            }

            CoreEvent::KeySetupRollback(host_name, result) => {
                tracing::warn!(host = %host_name, result = %result, "key setup rollback");
                // Close popup and show rollback result.
                self.view.host_list.popup = None;
                self.view.status_message = Some(format!(
                    "⚠ Key setup rolled back for '{}': {}",
                    host_name, result
                ));
            }

            CoreEvent::KeyPassphraseRequired {
                host_name,
                key_path,
            } => {
                // The terminal screen keeps its keys; the prompt waits until the
                // user leaves it. Said on every ask, as other messages replace it.
                if self.state.read().await.screen == Screen::Terminal {
                    self.view.status_message = Some(format!(
                        "SSH key needs a passphrase — Ctrl+Q to enter it: {key_path}"
                    ));
                }
                // One prompt per key: a single unlock serves every host using it.
                if !self
                    .view
                    .passphrase_prompts
                    .iter()
                    .any(|p| p.key_path == key_path)
                {
                    self.view.passphrase_prompts.push(PassphrasePrompt {
                        host_name,
                        key_path,
                        field: FormField::default(),
                        error: None,
                        unlocking: false,
                    });
                }
            }

            CoreEvent::PasswordRequired {
                request_id,
                host_name,
                login,
                retry,
                new_host_key,
            } => {
                // As with a passphrase, the terminal screen keeps its keys.
                if self.state.read().await.screen == Screen::Terminal {
                    self.view.status_message =
                        Some(format!("{login} needs a password — Ctrl+Q to enter it"));
                }
                self.view.password_prompts.push(PasswordPrompt {
                    request_id,
                    host_name,
                    login,
                    retry,
                    new_host_key,
                    field: FormField::default(),
                });
            }

            // ----------------------------------------------------------------
            // Update checker events
            // ----------------------------------------------------------------
            CoreEvent::UpdateAvailable(info) => {
                // Show the popup only if one is not already visible.
                if self.view.update_popup.is_none() {
                    self.view.update_popup = Some(UpdatePopup {
                        info,
                        phase: UpdatePopupPhase::Prompt { selected: 0 },
                    });
                }
            }

            CoreEvent::UpdateInstalled(result) => {
                if let Some(popup) = &mut self.view.update_popup {
                    popup.phase = match result {
                        Ok(()) => UpdatePopupPhase::Done {
                            message: "Update installed. Restart omny to use \
                                      the new version."
                                .to_string(),
                            ok: true,
                        },
                        Err(err) => UpdatePopupPhase::Done {
                            message: format!("Update failed: {}", err),
                            ok: false,
                        },
                    };
                }
            }

            // ----------------------------------------------------------------
            // PTY terminal events
            // ----------------------------------------------------------------
            CoreEvent::PtyOutput(session_id) => {
                // Data already processed into the vt100 parser by the reader
                // thread. Mark the tab as having unread activity if it is not
                // the currently focused tab.
                let active_id = self.view.terminal_view.active_session_id();
                if let Some(tab) = self
                    .view
                    .terminal_view
                    .tabs
                    .iter_mut()
                    .find(|t| t.session_id == session_id)
                {
                    tab.saw_output = true;
                    if active_id != Some(session_id) {
                        tab.has_activity = true;
                    }
                }
            }

            CoreEvent::PtyExited(session_id) => {
                // Remove the session from the manager.
                if let Some(mgr) = &mut self.pty_manager {
                    mgr.close(session_id);
                }
                let tabs = &mut self.view.terminal_view.tabs;
                // A tab that showed anything stays, ended, so the server's last words
                // can be read; a blank one (a failed connect) goes now.
                if let Some(pos) = tabs.iter().position(|t| t.session_id == session_id) {
                    if !tabs[pos].end() {
                        self.remove_ended_tab(pos).await;
                    }
                }
            }

            CoreEvent::Error(msg) => {
                self.view.status_message = Some(msg);
            }

            // ----------------------------------------------------------------
            // File Manager events
            // ----------------------------------------------------------------
            CoreEvent::FileTransferProgress(tid, done, total) => {
                if let Some(FileManagerPopup::TransferProgress {
                    transfer_id,
                    done: d,
                    total: t,
                    ..
                }) = &mut self.view.file_manager.popup
                {
                    // Accept progress from any tid >= the popup's current tid
                    // so multi-file queues display sequential file progress.
                    if tid >= *transfer_id {
                        *transfer_id = tid;
                        *d = done;
                        *t = total;
                    }
                }
            }

            CoreEvent::SftpConnected { host_name } => {
                self.view.file_manager.connected_host = Some(host_name);
                self.view.file_manager.sftp_connecting = false;
                // Close the host-picker popup now that we're connected.
                if matches!(
                    self.view.file_manager.popup,
                    Some(FileManagerPopup::HostPicker { .. })
                ) {
                    self.view.file_manager.popup = None;
                }
                // List the remote home directory.
                if let Some(mgr) = &self.sftp_manager {
                    mgr.send(SftpCommand::ListDir("/".to_string()));
                }
            }

            CoreEvent::SftpManagerReady { host_name, manager } => {
                self.sftp_manager = Some(*manager);
                self.view.file_manager.connected_host = Some(host_name.clone());
                self.view.file_manager.sftp_connecting = false;
                // Close the host-picker popup now that we're connected.
                if matches!(
                    self.view.file_manager.popup,
                    Some(FileManagerPopup::HostPicker { .. })
                ) {
                    self.view.file_manager.popup = None;
                }
                // List the remote home directory.
                if let Some(mgr) = &self.sftp_manager {
                    mgr.send(SftpCommand::ListDir("/".to_string()));
                }
                self.view.status_message = Some(format!("Connected to '{}'", host_name));
            }

            CoreEvent::SftpDisconnected { reason } => {
                self.sftp_manager = None;
                // Nothing queued for the lost session may reach the next one.
                self.sftp_queue.clear();
                self.view.file_manager.connected_host = None;
                self.view.file_manager.sftp_connecting = false;
                self.view.file_manager.remote = FilePanelView::default();
                self.view.file_manager.popup = None;
                self.view.status_message = Some(format!("SFTP: {reason}"));
            }

            CoreEvent::FileDirListed { path, entries } => {
                let rp = &mut self.view.file_manager.remote;
                rp.cwd = path;
                rp.apply_listing(entries);
                self.request_preview_for_active();
            }

            CoreEvent::LocalDirListed { path, entries } => {
                let lp = &mut self.view.file_manager.local;
                lp.cwd = path;
                lp.apply_listing(entries);
                self.request_preview_for_active();
            }

            CoreEvent::FilePreviewReady { path, content } => {
                self.view.file_manager.preview_content = Some(content);
                self.view.file_manager.preview_path = Some(path);
            }

            CoreEvent::SftpOpDone { result } => {
                let fm = &mut self.view.file_manager;
                fm.pending_ops = fm.pending_ops.saturating_sub(1);
                // The queued operations run on past a failure: the first one is kept
                // for the end, where no later result can wipe it, and the rest counted.
                // A cancel only stands where nothing really failed.
                if let Err(e) = result {
                    if e != CANCELLED {
                        fm.op_failures += 1;
                    }
                    if fm.op_error.as_deref().is_none_or(|kept| kept == CANCELLED) {
                        fm.op_error = Some(e);
                    }
                }
                self.send_next_op();
                let fm = &mut self.view.file_manager;
                if fm.pending_ops > 0 {
                    let remaining = fm.pending_ops + self.sftp_queue.len();
                    self.view.status_message = Some(format!("{remaining} item(s) remaining…"));
                } else {
                    // All queued operations finished — close the progress and refresh.
                    // A transfer hidden while stopping leaves other popups alone.
                    if matches!(fm.popup, Some(FileManagerPopup::TransferProgress { .. })) {
                        fm.popup = None;
                    }
                    fm.active_transfer = None;
                    let cancelling = std::mem::take(&mut fm.cancelling);
                    let more = std::mem::take(&mut fm.op_failures).saturating_sub(1);
                    self.view.status_message = match fm.op_error.take() {
                        Some(e) if e == CANCELLED => Some(e),
                        Some(e) if more > 0 => Some(format!("Transfer failed: {e} (+{more} more)")),
                        Some(e) => Some(format!("Transfer failed: {e}")),
                        None if cancelling => {
                            Some("The transfer finished before it could be cancelled.".to_string())
                        }
                        None => None,
                    };
                    self.refresh_active_panels().await;
                }
            }

            CoreEvent::SnippetsLoaded(snippets) => {
                let n = snippets.len();
                {
                    let mut state = self.state.write().await;
                    state.snippets = snippets;
                }
                let state = self.state.read().await;
                let q = self.view.snippets_view.search_query.clone();
                self.view.snippets_view.rebuild_filter(&state.snippets, &q);
                tracing::info!("Loaded {} snippet(s)", n);
            }

            CoreEvent::SnippetResult {
                host_name,
                snippet_name,
                output,
            } => {
                // Show error notification for failed snippet execution (extract essential message)
                if let Err(ref error) = output {
                    let short_error = error.split(':').next().unwrap_or(error).trim().to_string();

                    self.view.status_message = Some(format!(
                        "Snippet '{}' failed on '{}': {}",
                        snippet_name, host_name, short_error
                    ));
                }

                if let Some(SnippetPopup::Results { entries, .. }) =
                    &mut self.view.snippets_view.popup
                {
                    for entry in entries.iter_mut() {
                        if entry.host_name == host_name
                            && entry.snippet_name == snippet_name
                            && entry.pending
                        {
                            entry.output = output;
                            entry.pending = false;
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
