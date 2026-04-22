use std::collections::{HashMap, HashSet};
use std::io::Stdout;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::{mpsc, RwLock};

use crate::config;
use crate::config::app_config::{AppConfig, ParsedKeybindings};
use crate::config::snippets::{Snippet, SnippetScope};
use crate::event::{spawn_event_thread, AppEvent, Metrics, ServiceKind, SessionId, TransferId};
use crate::ssh::client::{ConnectionStatus, Host, HostSource};
use crate::ssh::pool::PollManager;
use crate::ssh::pty::{self as ssh_pty, PtyManager};
use crate::ssh::session::SshSession;
use crate::ssh::sftp::{self, FileEntry, SftpCommand, SftpManager, SftpOpKind};
use crate::ui;
use crate::ui::theme::Theme;

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

impl Screen {
    /// Human-readable title used in the status bar.
    pub fn title(&self) -> &'static str {
        match self {
            Screen::Dashboard => "Dashboard",
            Screen::DetailView => "Server Details",
            Screen::FileManager => "File Manager",
            Screen::Snippets => "Snippets",
            Screen::Terminal => "Terminal",
        }
    }
}

// ---------------------------------------------------------------------------
// Actions — returned by UI input handlers, processed in main_loop.
// ---------------------------------------------------------------------------

/// Actions that a UI input handler can request from the main event loop.
/// Separating them from events allows the main loop to remain the sole
/// owner of state mutations.
#[derive(Debug)]
pub enum AppAction {
    /// Application should exit.
    Quit,
    /// Connect to `AppState.hosts[idx]` using the system SSH binary.
    ConnectAt(usize),
    /// Open the edit popup pre-filled with the currently selected host.
    OpenEditPopup,
    /// The user confirmed deletion (from inside the DeleteConfirm popup).
    ConfirmDelete,
    /// The user confirmed the add/edit form (pressed Enter).
    ConfirmForm,
    /// Reload hosts from disk + `~/.ssh/config` in a background task.
    ReloadHosts,
    /// The search query was modified — rebuild `filtered_indices`.
    SearchQueryChanged,
    /// Trigger an immediate metrics refresh on all polled hosts.
    RefreshMetrics,
    /// Cycle through sort orders on the dashboard grid.
    CycleSortOrder,
    /// Open or close the tag-filter popup.
    OpenTagFilter,
    /// Apply (or clear) the given tag filter. `None` clears the filter.
    TagFilterSelected(Option<String>),
    /// Navigate the dashboard grid.
    DashboardNav(NavDir),
    /// Start SSH key setup for the selected host (Dashboard 'k' key).
    StartKeySetup,
    /// User confirmed the key setup prompt.
    ConfirmKeySetup(usize),
    /// User cancelled the key setup prompt.
    CancelKeySetup,

    // -----------------------------------------------------------------------
    // Detail View actions
    // -----------------------------------------------------------------------
    /// Open the Detail View for the currently selected host on Dashboard.
    OpenDetailView,
    /// Close Detail View and return to Dashboard.
    CloseDetailView,
    /// Connect to host from Detail View (same as Dashboard Enter).
    ConnectFromDetailView,
    /// Show Quick View popup for a specific service (keys 4-9 in Detail View).
    ShowQuickView(ServiceKind),
    /// Close the Quick View popup.
    CloseQuickView,

    // -----------------------------------------------------------------------
    // Snippets actions
    // -----------------------------------------------------------------------
    /// Reload snippets from disk in a background task.
    ReloadSnippets,
    /// Open the snippet add form on the Snippets screen.
    OpenSnippetAdd,
    /// Open the snippet edit form for the selected snippet.
    OpenSnippetEdit,
    /// Open the delete-confirm popup for the selected snippet.
    OpenSnippetDeleteConfirm,
    /// User confirmed the snippet add/edit form.
    ConfirmSnippetForm,
    /// User confirmed snippet deletion.
    ConfirmSnippetDelete,
    /// The snippet search query changed — rebuild filtered list.
    SnippetSearchChanged,
    /// Execute snippet at `snippet_idx` on the given hosts.
    /// If `host_names` is empty, resolve the target host automatically.
    ExecuteSnippet {
        snippet_idx: usize,
        host_names: Vec<String>,
    },
    /// Open the broadcast host-picker popup for the selected snippet.
    OpenBroadcastPicker,
    /// Toggle selection of host at `host_idx` (into `AppState.hosts`) in
    /// the broadcast picker.
    ToggleBroadcastHost(usize),
    /// Confirm and start a broadcast execution with the currently-checked hosts.
    ConfirmBroadcast,
    /// Open the quick-execute command-input popup for the dashboard's selected host.
    OpenQuickExecute,
    /// Execute an ad-hoc quick-execute command on the given host.
    QuickExecute { host_name: String, command: String },
    /// Confirm parameterized snippet inputs and execute.
    ConfirmParamInput,
    /// Dismiss the snippet results popup.
    DismissSnippetResult,

    // -----------------------------------------------------------------------
    // File Manager actions
    // -----------------------------------------------------------------------
    /// Navigate the cursor up (k / Up arrow) in the active panel.
    FmNavUp,
    /// Navigate the cursor down (j / Down arrow) in the active panel.
    FmNavDown,
    /// Switch focus between left (Local) and right (Remote) panel.
    FmSwitchPanel,
    /// Enter the directory under the cursor (l / Enter).
    FmEnterDir,
    /// Navigate to the parent directory (Backspace).
    FmParentDir,
    /// Toggle the marked state of the entry under the cursor (Space).
    FmMarkFile,
    /// Open the host-picker popup to connect the remote panel (H).
    FmOpenHostPicker,
    /// User selected host at index `usize` in the host-picker popup.
    FmHostPickerSelect(usize),
    /// Copy the marked (or cursor) items to the clipboard (c).
    FmCopy,
    /// Paste clipboard contents into the active panel (p).
    FmPaste,
    /// Open the delete-confirmation popup for marked / cursor items (D).
    FmOpenDeleteConfirm,
    /// User confirmed deletion.
    FmConfirmDelete,
    /// Open the new-directory popup (n).
    FmOpenMkDir,
    /// User confirmed the new directory name.
    FmConfirmMkDir(String),
    /// Open the rename popup for the cursor item (R).
    FmOpenRename,
    /// User confirmed the new name.
    FmConfirmRename(String),
    /// Close the active file-manager popup (Esc).
    FmClosePopup,
    /// Navigate the cursor inside the host-picker popup (j/k).
    FmHostPickerNav(i8), // +1 = down, -1 = up

    // -----------------------------------------------------------------------
    // Terminal multi-session actions
    // -----------------------------------------------------------------------
    /// Open a new PTY tab for `AppState.hosts[host_idx]` and switch to Terminal screen.
    TermOpenTab(usize),
    /// Open the host-picker popup for creating a new terminal tab (Ctrl+T).
    TermOpenHostPicker,
    /// Navigate the host-picker cursor. `+1` = down, `-1` = up.
    TermHostPickerNav(i8),
    /// Confirm host selection at `cursor` index in the host-picker popup.
    TermHostPickerSelect(usize),
    /// Close the host-picker popup without connecting (Esc).
    TermCloseHostPicker,
    /// Close the active terminal tab (Ctrl+W).
    TermCloseTab,
    /// Switch to the tab at the given 0-based index (Ctrl+1..9).
    TermSwitchTab(usize),
    /// Toggle vertical split-view between primary and the next tab (Ctrl+\).
    TermSplitVertical,
    /// Toggle horizontal split-view between primary and the next tab (Ctrl+-).
    TermSplitHorizontal,
    /// Switch keyboard focus between the primary and secondary pane (Tab in split mode).
    TermFocusNextPane,
    /// Forward raw bytes to the active PTY session's stdin.
    TermInput(Vec<u8>),
    /// Switch to the named screen from within the Terminal screen (F1/F2/F3).
    SwitchScreen(Screen),
    /// Switch the host for the currently focused pane (replaces its tab with a new connection).
    /// Only available in split view mode. Opens the host picker.
    TermSwitchPaneHost,
}

/// Direction for dashboard grid navigation.
#[derive(Debug)]
pub enum NavDir {
    Up,
    Down,
    Left,
    Right,
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
// Host form (used in Add / Edit popups)
// ---------------------------------------------------------------------------

/// Labels for every field in the host add/edit form.
pub const FORM_FIELD_LABELS: &[&str] = &[
    "Name",
    "Hostname / IP",
    "User",
    "Port",
    "Identity File",
    "Password (optional)",
    "Tags (comma-sep)",
    "Notes",
];

/// A single editable text field in the host form.
#[derive(Debug, Clone, Default)]
pub struct FormField {
    pub value: String,
    /// Cursor position (byte offset) within `value`.
    pub cursor: usize,
}

impl FormField {
    pub fn with_value(s: impl Into<String>) -> Self {
        let value = s.into();
        let cursor = value.len();
        Self { value, cursor }
    }

    /// Insert a character at the current cursor position.
    pub fn insert_char(&mut self, c: char) {
        self.value.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Delete the character immediately before the cursor (backspace).
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        // Find the previous char boundary.
        let prev = self.value[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.value.drain(prev..self.cursor);
        self.cursor = prev;
    }
}

/// The host add/edit form, containing all editable fields.
#[derive(Debug, Clone)]
pub struct HostForm {
    /// Parallel to `FORM_FIELD_LABELS`.
    pub fields: Vec<FormField>,
    /// Index of the currently focused field.
    pub focused_field: usize,
}

impl HostForm {
    /// Creates an empty form for adding a new host.
    pub fn empty() -> Self {
        Self {
            fields: FORM_FIELD_LABELS
                .iter()
                .map(|_| FormField::default())
                .collect(),
            focused_field: 0,
        }
    }

    /// Creates a form pre-filled from an existing host (for editing).
    pub fn from_host(host: &Host) -> Self {
        let mut form = Self::empty();
        form.fields[0] = FormField::with_value(&host.name);
        form.fields[1] = FormField::with_value(&host.hostname);
        form.fields[2] = FormField::with_value(&host.user);
        form.fields[3] = FormField::with_value(host.port.to_string());
        form.fields[4] = FormField::with_value(host.identity_file.as_deref().unwrap_or(""));
        form.fields[5] = FormField::with_value(host.password.as_deref().unwrap_or(""));
        form.fields[6] = FormField::with_value(host.tags.join(", "));
        form.fields[7] = FormField::with_value(host.notes.as_deref().unwrap_or(""));
        form
    }

    /// Validates the form and converts it into a [`Host`].
    ///
    /// # Errors
    /// Returns a human-readable error string if validation fails.
    pub fn to_host(&self, source: HostSource) -> Result<Host, String> {
        let name = self.fields[0].value.trim().to_string();
        if name.is_empty() {
            return Err("Name cannot be empty".to_string());
        }

        let hostname = self.fields[1].value.trim().to_string();
        if hostname.is_empty() {
            return Err("Hostname / IP cannot be empty".to_string());
        }

        let user = {
            let v = self.fields[2].value.trim();
            if v.is_empty() {
                "root".to_string()
            } else {
                v.to_string()
            }
        };

        let port = {
            let v = self.fields[3].value.trim();
            if v.is_empty() {
                22u16
            } else {
                v.parse::<u16>()
                    .map_err(|_| format!("Port must be a number between 1 and 65535, got '{v}'"))?
            }
        };

        let identity_file = {
            let v = self.fields[4].value.trim();
            if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            }
        };

        let password = {
            let v = self.fields[5].value.trim();
            if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            }
        };

        let tags: Vec<String> = self.fields[6]
            .value
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();

        let notes = {
            let v = self.fields[7].value.trim();
            if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            }
        };

        Ok(Host {
            name,
            hostname,
            user,
            port,
            identity_file,
            password,
            proxy_jump: None,
            tags,
            notes,
            source,
            original_ssh_host: None,
            key_setup_date: None,
            password_auth_disabled: None,
        })
    }

    /// Move focus to the next field (wraps around).
    pub fn focus_next(&mut self) {
        self.focused_field = (self.focused_field + 1) % self.fields.len();
    }

    /// Move focus to the previous field (wraps around).
    pub fn focus_prev(&mut self) {
        if self.focused_field == 0 {
            self.focused_field = self.fields.len() - 1;
        } else {
            self.focused_field -= 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Host list popup variants
// ---------------------------------------------------------------------------

/// Which popup is currently visible over the host list.
#[derive(Debug, Clone)]
pub enum HostPopup {
    /// Adding a new host.
    Add(HostForm),
    /// Editing an existing host (`host_idx` is the index in `AppState.hosts`).
    Edit { host_idx: usize, form: HostForm },
    /// Asking for confirmation before deleting (`host_idx`).
    DeleteConfirm(usize),
    /// Asking for confirmation to set up SSH key authentication.
    KeySetupConfirm(usize),
    /// Showing key setup progress.
    KeySetupProgress {
        host_idx: usize,
        host_name: String,
        current_step: Option<crate::ssh::key_setup::KeySetupStep>,
    },
}

// ---------------------------------------------------------------------------
// Host-list view state (UI-only, not shared with SSH tasks)
// ---------------------------------------------------------------------------

/// UI state specific to the host list / Dashboard screen.
#[derive(Debug, Default)]
pub struct HostListView {
    /// Selected row index within `filtered_indices`.
    pub selected: usize,
    /// True when the user is actively typing a search query.
    pub search_mode: bool,
    /// Current fuzzy-search query.
    pub search_query: String,
    /// Indices into `AppState.hosts` that match the current query, sorted and
    /// tag-filtered according to `sort_order` / `tag_filter`.
    pub filtered_indices: Vec<usize>,
    /// Currently visible popup (if any).
    pub popup: Option<HostPopup>,
    /// Host waiting to be connected (handled before the next render).
    pub pending_connect: Option<Host>,
    // Dashboard additions -----------
    /// Active sort order for the dashboard grid.
    pub sort_order: SortOrder,
    /// Active tag filter. `None` = show all hosts.
    pub tag_filter: Option<String>,
    /// Whether the tag-filter popup is open.
    pub tag_popup_open: bool,
    /// Selected index within the tag picker popup.
    pub tag_popup_selected: usize,
    /// All unique tags across all hosts (used by the tag picker popup).
    pub available_tags: Vec<String>,
}

impl HostListView {
    /// Returns the index into `AppState.hosts` for the selected filtered row.
    pub fn selected_host_idx(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected).copied()
    }

    /// Rebuilds `filtered_indices` applying text search, tag filter, and
    /// sort order.
    ///
    /// Accepts `metrics` so CPU/RAM sorts can compare live values. Pass an
    /// empty map if metrics are not yet available.
    pub fn rebuild_filter(
        &mut self,
        hosts: &[Host],
        metrics: &HashMap<String, Metrics>,
        statuses: &HashMap<String, ConnectionStatus>,
    ) {
        use std::cmp::Ordering;

        // 1. Text filter.
        let mut indices = filter_hosts(hosts, &self.search_query);

        // Guard: drop any stale indices that fell out of bounds due to a host
        // removal that happened before rebuild_filter was called.
        indices.retain(|&i| i < hosts.len());

        // 2. Tag filter.
        if let Some(tag) = &self.tag_filter {
            let tag = tag.clone();
            indices.retain(|&i| hosts[i].tags.contains(&tag));
        }

        // 3. Sort.
        match self.sort_order {
            SortOrder::Name => {
                indices.sort_by(|&a, &b| hosts[a].name.cmp(&hosts[b].name));
            }
            SortOrder::Cpu => {
                indices.sort_by(|&a, &b| {
                    let ca = metrics
                        .get(&hosts[a].name)
                        .and_then(|m| m.cpu_percent)
                        .unwrap_or(-1.0);
                    let cb = metrics
                        .get(&hosts[b].name)
                        .and_then(|m| m.cpu_percent)
                        .unwrap_or(-1.0);
                    cb.partial_cmp(&ca).unwrap_or(Ordering::Equal)
                });
            }
            SortOrder::Ram => {
                indices.sort_by(|&a, &b| {
                    let ra = metrics
                        .get(&hosts[a].name)
                        .and_then(|m| m.ram_percent)
                        .unwrap_or(-1.0);
                    let rb = metrics
                        .get(&hosts[b].name)
                        .and_then(|m| m.ram_percent)
                        .unwrap_or(-1.0);
                    rb.partial_cmp(&ra).unwrap_or(Ordering::Equal)
                });
            }
            SortOrder::Status => {
                indices.sort_by(|&a, &b| {
                    let sa = status_priority(statuses.get(&hosts[a].name));
                    let sb = status_priority(statuses.get(&hosts[b].name));
                    sa.cmp(&sb)
                });
            }
        }

        self.filtered_indices = indices;
        // Clamp selection to the new range.
        if self.filtered_indices.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len() - 1;
        }
    }

    /// Scroll down by one row.
    pub fn select_next(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        self.selected = (self.selected + 1).min(self.filtered_indices.len() - 1);
    }

    /// Scroll up by one row.
    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Rebuild the `available_tags` list from the full host list.
    pub fn rebuild_tags(&mut self, hosts: &[Host]) {
        let mut tags: Vec<String> = hosts.iter().flat_map(|h| h.tags.iter().cloned()).collect();
        tags.sort();
        tags.dedup();
        self.available_tags = tags;
    }
}

/// Lower number = higher priority for status sort.
fn status_priority(status: Option<&ConnectionStatus>) -> u8 {
    match status {
        Some(ConnectionStatus::Connected) => 0,
        Some(ConnectionStatus::Connecting) => 1,
        Some(ConnectionStatus::Unknown) | None => 2,
        Some(ConnectionStatus::Failed(_)) => 3,
    }
}

/// Simple case-insensitive substring filter over name / hostname / tags / notes.
///
/// Returns the matching indices into `hosts`. When `query` is empty every
/// host matches.
pub fn filter_hosts(hosts: &[Host], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..hosts.len()).collect();
    }
    let q = query.to_lowercase();
    hosts
        .iter()
        .enumerate()
        .filter(|(_, h)| {
            h.name.to_lowercase().contains(&q)
                || h.hostname.to_lowercase().contains(&q)
                || h.tags.iter().any(|t| t.to_lowercase().contains(&q))
                || h.notes
                    .as_deref()
                    .map(|n| n.to_lowercase().contains(&q))
                    .unwrap_or(false)
        })
        .map(|(i, _)| i)
        .collect()
}

// ---------------------------------------------------------------------------
// Snippets structs
// ---------------------------------------------------------------------------

/// One host's result entry for the snippet execution results popup.
#[derive(Debug, Clone)]
pub struct SnippetResultEntry {
    pub host_name: String,
    pub snippet_name: String,
    /// `Ok(stdout)` when done, `Err(message)` on error, empty `Ok("")` while pending.
    pub output: Result<String, String>,
    /// True while we're still waiting for the SSH task to complete.
    pub pending: bool,
}

/// Which popup is currently shown over the Snippets screen (or as a full-screen
/// overlay from any screen).
#[derive(Debug)]
pub enum SnippetPopup {
    /// Adding a new snippet.
    Add(SnippetForm),
    /// Editing an existing snippet.
    Edit {
        snippet_idx: usize,
        form: SnippetForm,
    },
    /// Asking for confirmation before deleting.
    DeleteConfirm(usize),
    /// Collecting values for `{{placeholder}}` params before executing.
    ParamInput {
        snippet_idx: usize,
        host_names: Vec<String>,
        param_names: Vec<String>,
        param_fields: Vec<FormField>,
        focused_field: usize,
    },
    /// Picking which hosts to broadcast to.
    BroadcastPicker {
        snippet_idx: usize,
        /// Indices into `AppState.hosts` that are checked.
        selected_host_indices: Vec<usize>,
        /// Highlighted row in the list.
        cursor: usize,
    },
    /// Single-line command input for quick-execute.
    QuickExecuteInput {
        host_name: String,
        command_field: FormField,
    },
    /// Scrollable results from one or more hosts.
    Results {
        entries: Vec<SnippetResultEntry>,
        scroll: usize,
    },
}

/// Labels for each field in the snippet add/edit form.
pub const SNIPPET_FORM_FIELD_LABELS: &[&str] = &[
    "Name",
    "Command",
    "Scope (global / host)",
    "Host (if scope=host)",
    "Tags (comma-sep)",
    "Params (comma-sep)",
];

/// The snippet add/edit form.
#[derive(Debug, Clone)]
pub struct SnippetForm {
    /// Parallel to `SNIPPET_FORM_FIELD_LABELS`.
    pub fields: Vec<FormField>,
    pub focused_field: usize,
}

impl SnippetForm {
    /// Creates an empty form for adding a new snippet.
    pub fn empty() -> Self {
        Self {
            fields: SNIPPET_FORM_FIELD_LABELS
                .iter()
                .map(|_| FormField::default())
                .collect(),
            focused_field: 0,
        }
    }

    /// Creates a form pre-filled from an existing snippet (for editing).
    pub fn from_snippet(s: &Snippet) -> Self {
        let mut form = Self::empty();
        form.fields[0] = FormField::with_value(&s.name);
        form.fields[1] = FormField::with_value(&s.command);
        form.fields[2] = FormField::with_value(match s.scope {
            SnippetScope::Global => "global",
            SnippetScope::Host => "host",
        });
        form.fields[3] = FormField::with_value(s.host.as_deref().unwrap_or(""));
        form.fields[4] = FormField::with_value(s.tags.as_deref().unwrap_or(&[]).join(", "));
        form.fields[5] = FormField::with_value(s.params.as_deref().unwrap_or(&[]).join(", "));
        form
    }

    /// Validates the form and converts it into a [`Snippet`].
    ///
    /// # Errors
    /// Returns a human-readable error string if validation fails.
    pub fn to_snippet(&self) -> Result<Snippet, String> {
        let name = self.fields[0].value.trim().to_string();
        if name.is_empty() {
            return Err("Name cannot be empty".to_string());
        }

        let command = self.fields[1].value.trim().to_string();
        if command.is_empty() {
            return Err("Command cannot be empty".to_string());
        }

        let scope_str = self.fields[2].value.trim().to_lowercase();
        let scope = match scope_str.as_str() {
            "host" => SnippetScope::Host,
            _ => SnippetScope::Global, // default to global
        };

        let host = {
            let v = self.fields[3].value.trim();
            if scope == SnippetScope::Host && v.is_empty() {
                return Err("Host cannot be empty when scope is 'host'".to_string());
            }
            if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            }
        };

        let tags: Vec<String> = self.fields[4]
            .value
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();

        let params: Vec<String> = self.fields[5]
            .value
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();

        Ok(Snippet {
            name,
            command,
            scope,
            host,
            tags: if tags.is_empty() { None } else { Some(tags) },
            params: if params.is_empty() {
                None
            } else {
                Some(params)
            },
        })
    }

    /// Move focus to the next field (wraps around).
    pub fn focus_next(&mut self) {
        self.focused_field = (self.focused_field + 1) % self.fields.len();
    }

    /// Move focus to the previous field (wraps around).
    pub fn focus_prev(&mut self) {
        if self.focused_field == 0 {
            self.focused_field = self.fields.len() - 1;
        } else {
            self.focused_field -= 1;
        }
    }
}

/// UI state specific to the Snippets screen.
#[derive(Debug, Default)]
pub struct SnippetsView {
    /// Selected row index within `filtered_indices`.
    pub selected: usize,
    /// True when the user is actively typing a search query.
    pub search_mode: bool,
    /// Current search query.
    pub search_query: String,
    /// Indices into `AppState.snippets` matching the current query.
    pub filtered_indices: Vec<usize>,
    /// Currently visible popup (if any).
    pub popup: Option<SnippetPopup>,
}

impl SnippetsView {
    /// Returns the index into `AppState.snippets` for the selected row.
    pub fn selected_snippet_idx(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected).copied()
    }

    /// Rebuilds `filtered_indices` with case-insensitive substring matching.
    pub fn rebuild_filter(&mut self, snippets: &[Snippet], query: &str) {
        self.filtered_indices = filter_snippets(snippets, query);
        if self.filtered_indices.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len() - 1;
        }
    }

    pub fn select_next(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        self.selected = (self.selected + 1).min(self.filtered_indices.len() - 1);
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
}

/// Case-insensitive substring filter over snippet name / command / tags.
pub fn filter_snippets(snippets: &[Snippet], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..snippets.len()).collect();
    }
    let q = query.to_lowercase();
    snippets
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.name.to_lowercase().contains(&q)
                || s.command.to_lowercase().contains(&q)
                || s.tags
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .any(|t| t.to_lowercase().contains(&q))
        })
        .map(|(i, _)| i)
        .collect()
}

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
    /// Directory entries as returned by the last listing.
    pub entries: Vec<FileEntry>,
    /// Absolute cursor index into `entries`.
    pub cursor: usize,
    /// Index of the first visible row (for scrolling).
    /// Uses [`std::cell::Cell`] so the render function can persist the computed
    /// scroll position through a shared `&FilePanelView` reference.
    pub scroll: std::cell::Cell<usize>,
    /// Set of `entry.path` values that are Space-marked.
    pub marked: HashSet<String>,
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

    /// Adjust `scroll` so that `cursor` is always visible in `visible_rows` rows.
    pub fn clamp_scroll(&mut self, visible_rows: usize) {
        if visible_rows == 0 {
            return;
        }
        let scroll = self.scroll.get();
        if self.cursor < scroll {
            self.scroll.set(self.cursor);
        } else if self.cursor >= scroll + visible_rows {
            self.scroll
                .set(self.cursor.saturating_sub(visible_rows - 1));
        }
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
    /// Number of queued transfer operations not yet completed.
    pub pending_ops: usize,
}

// ---------------------------------------------------------------------------
// Terminal multi-session view state
// ---------------------------------------------------------------------------

/// Direction of the split-view layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitDirection {
    /// Two panes side-by-side (left | right).
    Vertical,
    /// Two panes stacked (top / bottom).
    Horizontal,
}

/// Which pane currently has keyboard focus in split-view mode.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SplitFocus {
    #[default]
    Primary,
    Secondary,
}

/// Layout description when split-view is active.
#[derive(Debug, Clone)]
pub struct SplitView {
    pub direction: SplitDirection,
    /// Index into [`TerminalView::tabs`] shown in the secondary pane.
    pub secondary_tab: usize,
}

/// A single open SSH/PTY tab.
pub struct TermTab {
    /// Unique identifier matching the [`PtyManager`] session.
    pub session_id: SessionId,
    /// Display name (= `host.name`).
    pub host_name: String,
    /// Set to `true` when new output arrives while this tab is not focused,
    /// providing an unread-output indicator in the tab bar.
    pub has_activity: bool,
    /// Shared VT100 parser — written by the PTY reader thread, snapshotted by
    /// the render loop.  Stored here (in ViewState) so rendering does not
    /// require access to the PtyManager.
    pub parser: Arc<Mutex<vt100::Parser>>,
    /// Number of lines scrolled back from the live screen (0 = at the bottom).
    /// Set via mouse-wheel; reset to 0 when the user types anything.
    pub scroll_offset: usize,
}

/// Host-picker popup for opening a new terminal tab.
#[derive(Debug, Clone, Default)]
pub struct TermHostPicker {
    /// Index of the currently highlighted host in `AppState.hosts`.
    pub cursor: usize,
    /// If `true`, the picker is in "switch pane mode" — selecting a host replaces
    /// the focused pane's tab rather than creating a new tab.
    pub switch_pane_mode: bool,
}

/// All UI state for the Terminal screen.
#[derive(Default)]
pub struct TerminalView {
    /// Ordered list of open tabs.
    pub tabs: Vec<TermTab>,
    /// Index of the focused (primary) tab.
    pub active_tab: usize,
    /// Active split-view layout, if any.
    pub split: Option<SplitView>,
    /// Which pane has keyboard focus when split-view is active.
    pub split_focus: SplitFocus,
    /// Host-picker popup for creating a new tab (Ctrl+T).
    pub host_picker: Option<TermHostPicker>,
    /// When `true`, the next digit key 1–9 jumps directly to that tab.
    /// Activated by pressing Tab (which also cycles to the next tab).
    pub tab_select_mode: bool,
}

impl TerminalView {
    /// Returns the [`SessionId`] of the currently focused pane, or `None` if
    /// there are no open tabs.
    pub fn active_session_id(&self) -> Option<SessionId> {
        if self.tabs.is_empty() {
            return None;
        }
        let idx = match &self.split {
            Some(sv) if self.split_focus == SplitFocus::Secondary => sv.secondary_tab,
            _ => self.active_tab,
        };
        self.tabs.get(idx).map(|t| t.session_id)
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
    pub services: HashMap<String, Vec<crate::event::DetectedService>>,
    /// Active alerts per host.
    pub alerts: HashMap<String, Vec<crate::event::Alert>>,
    /// Discovery status per host.
    pub discovery_status: HashMap<String, crate::event::DiscoveryStatus>,
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
    /// Quick View popup state for Detail View service quick views.
    /// Contains the service kind if a Quick View is currently open.
    pub quick_view: Option<ServiceKind>,
    /// Scroll offset for Quick View popup content.
    pub quick_view_scroll: usize,
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
            quick_view: None,
            quick_view_scroll: 0,
        }
    }
}

impl Default for ViewState {
    fn default() -> Self {
        Self::default_inner()
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// Root application struct — owns the terminal, state, and event channel.
pub struct App {
    /// Shared state readable by background tokio tasks.
    pub state: Arc<RwLock<AppState>>,
    /// UI-only state, main thread only.
    pub view: ViewState,
    /// Sender half — given to the event thread and background tasks.
    event_tx: mpsc::Sender<AppEvent>,
    /// Receiver half — consumed by the main loop.
    event_rx: mpsc::Receiver<AppEvent>,
    /// Persistent SFTP session manager for the File Manager.
    sftp_manager: Option<SftpManager>,
    /// Monotone counter for assigning unique [`TransferId`] values.
    next_transfer_id: TransferId,
    /// Background metrics polling manager. Stored in `App` (not in
    /// `AppState`) to avoid a reference cycle through `Arc`. Dropping it
    /// signals all per-host tasks to stop.
    poll_manager: Option<PollManager>,
    /// PTY session manager for the Terminal multi-session screen.
    pty_manager: Option<PtyManager>,
    /// One heavyweight event (Key, etc.) that was pulled from the channel
    /// during a lightweight-event drain but could not be handled inline.
    /// Consumed at the top of the next main-loop iteration before blocking
    /// on `event_rx.recv()`.
    pending_event: Option<AppEvent>,
}

impl App {
    /// Creates a new `App` with the provided [`AppConfig`].
    ///
    /// The config is used to set the active theme and keybindings at startup.
    /// Call [`App::default`] to use a default config without loading a file.
    pub fn new(config: AppConfig) -> Self {
        let (tx, rx) = mpsc::channel(256);
        let theme = Theme::from_name(&config.ui.theme);
        let keybindings = ParsedKeybindings::from_config(&config.keybindings);
        Self {
            state: Arc::new(RwLock::new(AppState::default())),
            view: ViewState {
                theme,
                keybindings,
                ..ViewState::default_inner()
            },
            event_tx: tx,
            event_rx: rx,
            sftp_manager: None,
            next_transfer_id: 0,
            poll_manager: None,
            pty_manager: None,
            pending_event: None,
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
    ///
    /// The terminal is **always** restored before this function returns.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        // Terminal setup.
        crossterm::terminal::enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        crossterm::execute!(
            stdout,
            crossterm::terminal::EnterAlternateScreen,
            crate::utils::mouse::EnableMinimalMouseCapture,
        )?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Background event thread (keyboard + tick).
        spawn_event_thread(self.event_tx.clone())?;

        // Load hosts in a background task.
        {
            let tx = self.event_tx.clone();
            tokio::spawn(async move {
                match config::load_all_hosts() {
                    Ok(hosts) => {
                        let _ = tx.send(AppEvent::HostsLoaded(hosts)).await;
                    }
                    Err(e) => tracing::warn!("Failed to load hosts: {}", e),
                }
            });
        }

        // Load snippets in a background task.
        {
            let tx = self.event_tx.clone();
            tokio::spawn(async move {
                match config::snippets::load_snippets() {
                    Ok(snippets) => {
                        let _ = tx.send(AppEvent::SnippetsLoaded(snippets)).await;
                    }
                    Err(e) => tracing::warn!("Failed to load snippets: {}", e),
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
        if let Some(sftp) = self.sftp_manager.take() {
            sftp.disconnect();
        }
        // Gracefully shut down all PTY sessions.
        if let Some(mgr) = self.pty_manager.take() {
            mgr.shutdown();
        }

        // Terminal restore — always runs even if main_loop returned Err.
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(
            terminal.backend_mut(),
            crossterm::terminal::LeaveAlternateScreen,
            crate::utils::mouse::DisableMinimalMouseCapture,
        )?;
        terminal.show_cursor()?;

        result
    }

    /// Inner loop — separated from `run` so terminal restore always happens.
    async fn main_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> anyhow::Result<()> {
        loop {
            // ----------------------------------------------------------------
            // Handle a pending SSH connection *before* the next render so the
            // terminal is fully restored while SSH runs.
            // ----------------------------------------------------------------
            if let Some(host) = self.view.host_list.pending_connect.take() {
                self.connect_system_ssh(terminal, &host).await?;
                continue;
            }

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

                AppEvent::HostsLoaded(hosts) => {
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
                            self.event_tx.clone(),
                            Duration::from_secs(30),
                        ));
                    }
                    tracing::info!("Loaded {} host(s)", n);
                }

                AppEvent::Tick => {
                    // Increment tick counter for spinner animation.
                    self.view.tick_count = self.view.tick_count.wrapping_add(1);
                }

                AppEvent::MetricsUpdate(host_name, new_metrics) => {
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

                AppEvent::HostStatusChanged(host_name, status) => {
                    {
                        let mut state = self.state.write().await;
                        state
                            .connection_statuses
                            .insert(host_name.clone(), status.clone());
                    }

                    // Show notification for connection/metrics failures (extract essential message)
                    if let ConnectionStatus::Failed(ref error) = status {
                        let short_error =
                            error.split(':').next().unwrap_or(error).trim().to_string();

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
                AppEvent::DiscoveryQuickScanDone(host_name, services) => {
                    let mut state = self.state.write().await;
                    state.services.insert(host_name.clone(), services);
                    state
                        .discovery_status
                        .insert(host_name, crate::event::DiscoveryStatus::QuickScanDone);
                }

                AppEvent::DiscoveryDeepProbeDone(host_name, services) => {
                    let mut state = self.state.write().await;
                    state.services.insert(host_name.clone(), services);
                    state
                        .discovery_status
                        .insert(host_name, crate::event::DiscoveryStatus::DeepProbeDone);
                }

                AppEvent::DiscoveryFailed(host_name, error) => {
                    let mut state = self.state.write().await;
                    state.discovery_status.insert(
                        host_name.clone(),
                        crate::event::DiscoveryStatus::Failed(error.clone()),
                    );
                    tracing::debug!(host = %host_name, error = %error, "discovery failed");

                    // Show error notification in status bar (extract only the essential error message)
                    // Discovery errors often contain the full command after a colon, so extract just the first part
                    let short_error = error.split(':').next().unwrap_or(&error).trim().to_string();

                    self.view.status_message = Some(format!(
                        "Discovery failed for '{}': {}",
                        host_name, short_error
                    ));
                }

                AppEvent::AlertNew(host_name, alert) => {
                    let mut state = self.state.write().await;
                    state
                        .alerts
                        .entry(host_name)
                        .or_insert_with(Vec::new)
                        .push(alert);
                }

                // ----------------------------------------------------------------
                // Auto SSH Key Setup events
                // ----------------------------------------------------------------
                AppEvent::KeySetupProgress(host_name, step) => {
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

                AppEvent::KeySetupComplete(host_name, key_path) => {
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

                    // Close popup and show success.
                    self.view.host_list.popup = None;
                    self.view.status_message = Some(format!(
                        "✓ SSH key setup complete for '{}'. Key: {}",
                        host_name,
                        key_path.display()
                    ));
                }

                AppEvent::KeySetupFailed(host_name, error) => {
                    tracing::error!(host = %host_name, error = %error, "key setup failed");
                    // Close popup and show error.
                    self.view.host_list.popup = None;
                    self.view.status_message =
                        Some(format!("✗ Key setup failed for '{}': {}", host_name, error));
                }

                AppEvent::KeySetupRollback(host_name, result) => {
                    tracing::warn!(host = %host_name, result = %result, "key setup rollback");
                    // Close popup and show rollback result.
                    self.view.host_list.popup = None;
                    self.view.status_message = Some(format!(
                        "⚠ Key setup rolled back for '{}': {}",
                        host_name, result
                    ));
                }

                // ----------------------------------------------------------------
                // PTY terminal events
                // ----------------------------------------------------------------
                AppEvent::PtyOutput(session_id) => {
                    // Data already processed into the vt100 parser by the reader
                    // thread. Mark the tab as having unread activity if it is not
                    // the currently focused tab.
                    let active_id = self.view.terminal_view.active_session_id();
                    if active_id != Some(session_id) {
                        if let Some(tab) = self
                            .view
                            .terminal_view
                            .tabs
                            .iter_mut()
                            .find(|t| t.session_id == session_id)
                        {
                            tab.has_activity = true;
                        }
                    }
                }

                AppEvent::TermScroll(delta) => {
                    // Only scroll when the Terminal screen is active and there
                    // is a focused tab.  The offset is stored in the tab so each
                    // tab remembers its own scroll position independently.
                    let state = self.state.read().await;
                    if state.screen == crate::app::Screen::Terminal {
                        drop(state);
                        let tv = &mut self.view.terminal_view;
                        let focused_idx = match &tv.split {
                            Some(sv) if tv.split_focus == SplitFocus::Secondary => sv.secondary_tab,
                            _ => tv.active_tab,
                        };
                        if let Some(tab) = tv.tabs.get_mut(focused_idx) {
                            if delta > 0 {
                                // Cap at the configured vt100 scrollback capacity (1000
                                // lines — matches Parser::new(rows, cols, 1000) in pty.rs).
                                tab.scroll_offset =
                                    tab.scroll_offset.saturating_add(delta as usize).min(1000);
                            } else {
                                tab.scroll_offset =
                                    tab.scroll_offset.saturating_sub((-delta) as usize);
                            }
                        }
                    }
                }

                AppEvent::PtyExited(session_id) => {
                    // Remove the session from the manager and the tab bar.
                    if let Some(mgr) = &mut self.pty_manager {
                        mgr.close(session_id);
                    }
                    let tv = &mut self.view.terminal_view;
                    // Remove the tab.
                    if let Some(pos) = tv.tabs.iter().position(|t| t.session_id == session_id) {
                        tv.tabs.remove(pos);
                        // Collapse any split that referenced this tab.
                        tv.split = None;
                        tv.split_focus = SplitFocus::Primary;
                        if tv.tabs.is_empty() {
                            self.state.write().await.screen = Screen::Dashboard;
                            self.view.status_message = Some("SSH session closed.".to_string());
                        } else {
                            tv.active_tab = tv.active_tab.min(tv.tabs.len().saturating_sub(1));
                        }
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

                AppEvent::Error(_, msg) => {
                    self.view.status_message = Some(msg);
                }

                // ----------------------------------------------------------------
                // File Manager events
                // ----------------------------------------------------------------
                AppEvent::FileTransferProgress(tid, done, total) => {
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

                AppEvent::SftpConnected { host_name } => {
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

                AppEvent::SftpManagerReady { host_name, manager } => {
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

                AppEvent::SftpDisconnected {
                    host_name: _,
                    reason,
                } => {
                    self.sftp_manager = None;
                    self.view.file_manager.connected_host = None;
                    self.view.file_manager.sftp_connecting = false;
                    self.view.file_manager.remote = FilePanelView::default();
                    self.view.file_manager.popup = None;
                    self.view.status_message = Some(format!("SFTP: {reason}"));
                }

                AppEvent::FileDirListed { path, entries } => {
                    let rp = &mut self.view.file_manager.remote;
                    rp.cwd = path;
                    rp.entries = entries;
                    rp.cursor = 0;
                    rp.scroll.set(0);
                    rp.marked.clear();
                    self.request_preview_for_active();
                }

                AppEvent::LocalDirListed { path, entries } => {
                    let lp = &mut self.view.file_manager.local;
                    lp.cwd = path;
                    lp.entries = entries;
                    lp.cursor = 0;
                    lp.scroll.set(0);
                    lp.marked.clear();
                    self.request_preview_for_active();
                }

                AppEvent::FilePreviewReady { path, content } => {
                    self.view.file_manager.preview_content = Some(content);
                    self.view.file_manager.preview_path = Some(path);
                }

                AppEvent::SftpOpDone { kind: _, result } => {
                    self.view.file_manager.pending_ops =
                        self.view.file_manager.pending_ops.saturating_sub(1);
                    let remaining = self.view.file_manager.pending_ops;

                    match result {
                        Ok(()) => {
                            if remaining == 0 {
                                // All queued operations finished — close popup and refresh.
                                self.view.file_manager.popup = None;
                                self.view.file_manager.active_transfer = None;
                                self.view.status_message = None;
                                self.refresh_active_panels().await;
                            } else {
                                self.view.status_message =
                                    Some(format!("{remaining} file(s) remaining…"));
                            }
                        }
                        Err(e) => {
                            // Abort remaining: clear popup, show error, refresh.
                            self.view.file_manager.popup = None;
                            self.view.file_manager.active_transfer = None;
                            self.view.file_manager.pending_ops = 0;
                            self.view.status_message = Some(format!("Transfer failed: {e}"));
                            self.refresh_active_panels().await;
                        }
                    }
                }

                AppEvent::SnippetsLoaded(snippets) => {
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

                AppEvent::SnippetResult {
                    host_name,
                    snippet_name,
                    output,
                } => {
                    // Show error notification for failed snippet execution (extract essential message)
                    if let Err(ref error) = output {
                        let short_error =
                            error.split(':').next().unwrap_or(error).trim().to_string();

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
                        Ok(AppEvent::PtyOutput(sid)) => {
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
                            // Handle help popup scrolling first
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
                                let tv = &mut self.view.terminal_view;
                                let focused_idx = match &tv.split {
                                    Some(sv) if tv.split_focus == SplitFocus::Secondary => {
                                        sv.secondary_tab
                                    }
                                    _ => tv.active_tab,
                                };
                                if let Some(tab) = tv.tabs.get_mut(focused_idx) {
                                    if delta > 0 {
                                        tab.scroll_offset = tab
                                            .scroll_offset
                                            .saturating_add(delta as usize)
                                            .min(1000);
                                    } else {
                                        tab.scroll_offset =
                                            tab.scroll_offset.saturating_sub((-delta) as usize);
                                    }
                                }
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

    /// Translates a key event into an optional [`AppAction`].
    async fn handle_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<AppAction>> {
        let screen = self.state.read().await.screen.clone();

        // ----------------------------------------------------------------
        // Terminal screen intercepts ALL keys — including Ctrl+C which must
        // be forwarded to the PTY rather than quitting the application.
        // F1/F2/F3 are the escape hatch back to other screens and are
        // handled inside handle_terminal_key.
        // ----------------------------------------------------------------
        if matches!(screen, Screen::Terminal) {
            return Ok(self.handle_terminal_key(key));
        }

        // Ctrl+C always quits regardless of any other state (non-Terminal screens).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(Some(AppAction::Quit));
        }

        // Tag popup takes full priority over everything except Ctrl+C.
        if self.view.host_list.tag_popup_open && matches!(screen, Screen::Dashboard) {
            return Ok(ui::dashboard::handle_tag_popup_input(key, &mut self.view));
        }

        // File manager popup takes full priority on the File Manager screen.
        if self.view.file_manager.popup.is_some() && matches!(screen, Screen::FileManager) {
            return Ok(ui::file_manager::handle_input(key, &mut self.view));
        }

        // Snippet overlay popups (Results, QuickExecuteInput) are visible on
        // any screen and capture all input except Ctrl+C.
        let snip_overlay_active = matches!(
            self.view.snippets_view.popup,
            Some(SnippetPopup::Results { .. }) | Some(SnippetPopup::QuickExecuteInput { .. })
        );
        if snip_overlay_active {
            return Ok(ui::snippets::handle_input(key, &mut self.view));
        }

        // Snippet screen popups (Add/Edit/Delete/ParamInput/BroadcastPicker)
        // or search mode on the snippets screen — delegate to snippets handler.
        let snippet_popup_or_search =
            self.view.snippets_view.popup.is_some() || self.view.snippets_view.search_mode;
        if snippet_popup_or_search && matches!(screen, Screen::Snippets) {
            return Ok(ui::snippets::handle_input(key, &mut self.view));
        }

        // When a host-list popup is open or the user is searching, the screen
        // handler takes full priority (no global key interception except Ctrl+C).
        let popup_or_search =
            self.view.host_list.popup.is_some() || self.view.host_list.search_mode;

        if popup_or_search {
            return Ok(match screen {
                Screen::Dashboard => ui::dashboard::handle_input(key, &mut self.view),
                Screen::FileManager | Screen::Snippets | Screen::Terminal | Screen::DetailView => {
                    None
                }
            });
        }

        // ── Configurable global keys ───────────────────────────
        // These are checked before the main match so user-defined bindings
        // override the defaults without requiring changes to every branch.
        {
            let kb = &self.view.keybindings;
            if key.code == kb.quit {
                return Ok(Some(AppAction::Quit));
            }
            if key.code == kb.dashboard || key.code == KeyCode::Char('1') {
                self.state.write().await.screen = Screen::Dashboard;
                self.view.status_message = None;
                return Ok(None);
            }
            if key.code == kb.file_manager || key.code == KeyCode::Char('2') {
                self.state.write().await.screen = Screen::FileManager;
                self.view.status_message = None;
                self.bootstrap_file_manager().await;
                return Ok(None);
            }
            if key.code == kb.snippets || key.code == KeyCode::Char('3') {
                self.state.write().await.screen = Screen::Snippets;
                self.view.status_message = None;
                return Ok(None);
            }
        }

        // Global key handling.
        match key.code {
            // `q` is handled above via keybindings; kept here as dead arm to
            // avoid changing all the code below but effectively unreachable
            // when the default keybinding is used.
            KeyCode::Char('4') | KeyCode::F(4) => {
                // On DetailView, '4' is used for Quick View (Docker), not switching screens
                if matches!(screen, Screen::DetailView) {
                    // Delegate to DetailView handler for Quick View actions
                    return Ok(ui::detail_view::handle_input(key, &mut self.view));
                } else {
                    // Switch to Terminal screen; open host picker if no tabs are open.
                    self.state.write().await.screen = Screen::Terminal;
                    self.view.status_message = None;
                    if self.view.terminal_view.tabs.is_empty() {
                        self.view.terminal_view.host_picker = Some(TermHostPicker::default());
                    }
                }
            }

            KeyCode::Tab => {
                // On the File Manager screen Tab switches between the two panels
                // (local ↔ remote) rather than cycling to the next app screen.
                if matches!(screen, Screen::FileManager) {
                    return Ok(Some(AppAction::FmSwitchPanel));
                }

                let new_screen = {
                    let mut state = self.state.write().await;
                    state.screen = match state.screen {
                        Screen::Dashboard => Screen::FileManager,
                        Screen::DetailView => Screen::Dashboard, // Detail View → Dashboard
                        Screen::FileManager => Screen::Snippets,
                        Screen::Snippets => Screen::Terminal,
                        Screen::Terminal => Screen::Dashboard, // unreachable here (handled above)
                    };
                    state.screen.clone()
                };
                self.view.status_message = None;
                if matches!(new_screen, Screen::FileManager) {
                    self.bootstrap_file_manager().await;
                }
                if matches!(new_screen, Screen::Terminal) && self.view.terminal_view.tabs.is_empty()
                {
                    self.view.terminal_view.host_picker = Some(TermHostPicker::default());
                }
            }

            KeyCode::Char('?') => {
                self.view.show_help = !self.view.show_help;
                // Reset scroll when opening help
                if self.view.show_help {
                    self.view.help_scroll = 0;
                }
            }

            KeyCode::Esc => {
                // Close help popup if it's open
                if self.view.show_help {
                    self.view.show_help = false;
                    self.view.help_scroll = 0;
                    return Ok(None);
                }

                // Clear status message if present
                if self.view.status_message.is_some() {
                    self.view.status_message = None;
                    return Ok(None);
                }

                // Otherwise, delegate to screen handler (e.g., DetailView can return to Dashboard)
                return Ok(match screen {
                    Screen::Dashboard => ui::dashboard::handle_input(key, &mut self.view),
                    Screen::DetailView => ui::detail_view::handle_input(key, &mut self.view),
                    Screen::Snippets => ui::snippets::handle_input(key, &mut self.view),
                    Screen::FileManager => ui::file_manager::handle_input(key, &mut self.view),
                    Screen::Terminal => None,
                });
            }

            _ => {
                // Delegate to the current screen's input handler.
                return Ok(match screen {
                    Screen::Dashboard => ui::dashboard::handle_input(key, &mut self.view),
                    Screen::DetailView => ui::detail_view::handle_input(key, &mut self.view),
                    Screen::Snippets => ui::snippets::handle_input(key, &mut self.view),
                    Screen::FileManager => ui::file_manager::handle_input(key, &mut self.view),
                    // Terminal is handled at the very top of handle_key; unreachable here.
                    Screen::Terminal => None,
                });
            }
        }

        Ok(None)
    }

    /// Handles key events when the Terminal screen is active.
    ///
    /// Returns an [`AppAction`] to pass to `process_action`, or forwards the
    /// keystroke as raw bytes to the active PTY.
    fn handle_terminal_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // Host-picker popup has priority (Ctrl+T flow).
        if self.view.terminal_view.host_picker.is_some() {
            return ui::terminal::handle_host_picker_input(key, &mut self.view);
        }

        // F1/F2/F3 switch to other screens (escape hatch from Terminal).
        // ── Screen switching ────────────────────────────────────────────────
        // Ctrl+Q   → Dashboard  (works on macOS; Ctrl+letter always reliable)
        // F1/F2/F3 → Dashboard/Files/Snippets (Linux; macOS captures F-keys)
        match key.code {
            KeyCode::Char('q') if ctrl => {
                return Some(AppAction::SwitchScreen(Screen::Dashboard));
            }
            KeyCode::F(1) => return Some(AppAction::SwitchScreen(Screen::Dashboard)),
            KeyCode::F(2) => return Some(AppAction::SwitchScreen(Screen::FileManager)),
            KeyCode::F(3) => return Some(AppAction::SwitchScreen(Screen::Snippets)),
            _ => {}
        }

        // ── Terminal control combos ──────────────────────────────────────────
        // Ctrl+T      → new tab host picker
        // Ctrl+W      → close active tab
        // Ctrl+\      → toggle vertical split   (byte 0x1C → Char('4')+CONTROL)
        // Ctrl+]      → toggle horizontal split (byte 0x1D → Char('5')+CONTROL)
        //   Note: Ctrl+\ and Ctrl+] are physically adjacent keys on US layout.
        //   Ctrl+- maps to 0x0D (Enter) with no CONTROL modifier — unusable.
        // Ctrl+Right  → next tab in the focused pane (wraps around)
        // Ctrl+Left   → prev tab in the focused pane (wraps around)
        if ctrl {
            match key.code {
                KeyCode::Char('t') => return Some(AppAction::TermOpenHostPicker),
                KeyCode::Char('w') => return Some(AppAction::TermCloseTab),
                KeyCode::Char('h') => {
                    // Ctrl+H: Switch host in the focused pane (only in split mode)
                    if self.view.terminal_view.split.is_some() {
                        return Some(AppAction::TermSwitchPaneHost);
                    }
                    return None;
                }
                // Ctrl+\ sends byte 0x1C; crossterm decodes it as Char('4')+CONTROL
                KeyCode::Char('4') => return Some(AppAction::TermSplitVertical),
                // Ctrl+] sends byte 0x1D; crossterm decodes it as Char('5')+CONTROL
                KeyCode::Char('5') => return Some(AppAction::TermSplitHorizontal),
                KeyCode::Right => {
                    // Cycle within the focused pane (secondary or primary).
                    let tv = &mut self.view.terminal_view;
                    if tv.tabs.len() > 1 {
                        if let (Some(sv), SplitFocus::Secondary) =
                            (&mut tv.split, tv.split_focus.clone())
                        {
                            sv.secondary_tab = (sv.secondary_tab + 1) % tv.tabs.len();
                            return None; // already mutated
                        }
                        let next = (tv.active_tab + 1) % tv.tabs.len();
                        return Some(AppAction::TermSwitchTab(next));
                    }
                    return None;
                }
                KeyCode::Left => {
                    // Cycle within the focused pane (secondary or primary).
                    let tv = &mut self.view.terminal_view;
                    if tv.tabs.len() > 1 {
                        if let (Some(sv), SplitFocus::Secondary) =
                            (&mut tv.split, tv.split_focus.clone())
                        {
                            sv.secondary_tab = if sv.secondary_tab == 0 {
                                tv.tabs.len() - 1
                            } else {
                                sv.secondary_tab - 1
                            };
                            return None;
                        }
                        let prev = if tv.active_tab == 0 {
                            tv.tabs.len() - 1
                        } else {
                            tv.active_tab - 1
                        };
                        return Some(AppAction::TermSwitchTab(prev));
                    }
                    return None;
                }
                _ => {}
            }
        }

        // Tab key:
        //   • In split mode  → switch pane focus (existing behaviour).
        //   • Otherwise       → cycle to the next tab AND enter tab-select mode
        //                       (a subsequent digit 1–9 jumps to that tab directly).
        if key.code == KeyCode::Tab && !ctrl {
            if self.view.terminal_view.split.is_some() {
                return Some(AppAction::TermFocusNextPane);
            }
            // Cycle to next tab and enter select mode.
            let tv = &mut self.view.terminal_view;
            if tv.tabs.len() > 1 {
                tv.active_tab = (tv.active_tab + 1) % tv.tabs.len();
                // Mark the newly-active tab as seen.
                tv.tabs[tv.active_tab].has_activity = false;
            }
            tv.tab_select_mode = true;
            return None; // state already mutated; nothing to dispatch
        }

        // In tab-select mode a digit key 1–9 jumps to that tab.
        if self.view.terminal_view.tab_select_mode {
            if let KeyCode::Char(c @ '1'..='9') = key.code {
                let n = (c as u8 - b'1') as usize; // 0-based
                self.view.terminal_view.tab_select_mode = false;
                if n < self.view.terminal_view.tabs.len() {
                    return Some(AppAction::TermSwitchTab(n));
                }
                return None;
            }
            // Any other key exits select mode and falls through to normal handling.
            self.view.terminal_view.tab_select_mode = false;
        }

        // Forward everything else as raw bytes to the PTY.
        let bytes = ssh_pty::key_to_bytes(key);
        if bytes.is_empty() {
            None
        } else {
            Some(AppAction::TermInput(bytes))
        }
    }

    /// Executes an [`AppAction`] that requires access to shared state or the
    /// terminal (e.g. connecting to SSH).
    async fn process_action(&mut self, action: Option<AppAction>) -> anyhow::Result<()> {
        let Some(action) = action else { return Ok(()) };

        match action {
            // Quit is intercepted in main_loop before process_action is called.
            AppAction::Quit => {}

            AppAction::ConnectAt(idx) => {
                // Open a PTY tab in the Terminal screen instead of
                // the old system-SSH hand-off.
                self.open_term_tab(idx).await;
            }

            AppAction::OpenEditPopup => {
                let (idx, host) = {
                    let state = self.state.read().await;
                    let idx = self.view.host_list.selected_host_idx();
                    let host = idx.and_then(|i| state.hosts.get(i)).cloned();
                    (idx, host)
                };
                if let (Some(idx), Some(host)) = (idx, host) {
                    let form = HostForm::from_host(&host);
                    self.view.host_list.popup = Some(HostPopup::Edit {
                        host_idx: idx,
                        form,
                    });
                }
            }

            AppAction::ConfirmForm => {
                self.handle_confirm_form().await;
            }

            AppAction::ConfirmDelete => {
                self.handle_confirm_delete().await;
            }

            AppAction::ReloadHosts => {
                let tx = self.event_tx.clone();
                tokio::spawn(async move {
                    match config::load_all_hosts() {
                        Ok(hosts) => {
                            let _ = tx.send(AppEvent::HostsLoaded(hosts)).await;
                        }
                        Err(e) => tracing::warn!("Reload failed: {}", e),
                    }
                });
                self.view.status_message = Some("Reloading hosts…".to_string());
            }

            AppAction::SearchQueryChanged => {
                let state = self.state.read().await;
                self.view.host_list.rebuild_filter(
                    &state.hosts,
                    &state.metrics,
                    &state.connection_statuses,
                );
            }

            AppAction::RefreshMetrics => {
                if let Some(mgr) = &self.poll_manager {
                    mgr.refresh_all();
                }
                self.view.status_message = Some("Refreshing metrics…".to_string());
            }

            AppAction::CycleSortOrder => {
                let new_order = self.view.host_list.sort_order.next();
                self.view.host_list.sort_order = new_order;
                let state = self.state.read().await;
                self.view.host_list.rebuild_filter(
                    &state.hosts,
                    &state.metrics,
                    &state.connection_statuses,
                );
            }

            AppAction::OpenTagFilter => {
                self.view.host_list.tag_popup_open = !self.view.host_list.tag_popup_open;
                self.view.host_list.tag_popup_selected = 0;
            }

            AppAction::TagFilterSelected(tag_opt) => {
                self.view.host_list.tag_filter = tag_opt;
                self.view.host_list.tag_popup_open = false;
                let state = self.state.read().await;
                self.view.host_list.rebuild_filter(
                    &state.hosts,
                    &state.metrics,
                    &state.connection_statuses,
                );
            }

            AppAction::DashboardNav(dir) => {
                // The number of grid columns is computed identically here and
                // in the render function. Keep these two in sync.
                const CARD_W: u16 = 34; // Match CARD_MIN_WIDTH from card.rs
                const GAP: u16 = 1;
                // Use a conservative estimate for term width when not rendering.
                let approx_cols = {
                    let w = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);
                    ((w + GAP) / (CARD_W + GAP)).max(1) as usize
                };
                let len = self.view.host_list.filtered_indices.len();
                if len == 0 {
                    return Ok(());
                }
                let sel = self.view.host_list.selected;
                self.view.host_list.selected = match dir {
                    NavDir::Up => sel.saturating_sub(approx_cols),
                    NavDir::Down => (sel + approx_cols).min(len - 1),
                    NavDir::Left => sel.saturating_sub(1),
                    NavDir::Right => (sel + 1).min(len - 1),
                };
            }

            // ---------------------------------------------------------------
            // SSH Key Setup actions
            // ---------------------------------------------------------------
            AppAction::StartKeySetup => {
                let state = self.state.read().await;
                if let Some(idx) = self.view.host_list.selected_host_idx() {
                    if let Some(host) = state.hosts.get(idx) {
                        // Only offer key setup for hosts that use password auth
                        // (have a password field set and no identity_file).
                        if host.password.is_some() && host.identity_file.is_none() {
                            self.view.host_list.popup = Some(HostPopup::KeySetupConfirm(idx));
                        } else if host.identity_file.is_some() {
                            self.view.status_message =
                                Some(format!("'{}' already uses key authentication.", host.name));
                        } else {
                            self.view.status_message = Some(
                                "No password set for this host. Add a password first to enable key setup."
                                    .to_string(),
                            );
                        }
                    }
                }
            }

            AppAction::ConfirmKeySetup(idx) => {
                let state = self.state.read().await;
                if let Some(host) = state.hosts.get(idx).cloned() {
                    // Transition to progress popup.
                    self.view.host_list.popup = Some(HostPopup::KeySetupProgress {
                        host_idx: idx,
                        host_name: host.name.clone(),
                        current_step: None,
                    });
                    drop(state);

                    // Spawn background key setup task.
                    let tx = self.event_tx.clone();
                    let host_clone = host.clone();
                    tokio::spawn(async move {
                        use crate::ssh::key_setup::{setup_key_for_host, KeySetupStep, KeyType};

                        // Create a channel for progress updates.
                        let (progress_tx, mut progress_rx) = mpsc::channel::<KeySetupStep>(10);
                        let event_tx = tx.clone();
                        let host_name = host_clone.name.clone();

                        // Spawn task to forward progress events.
                        tokio::spawn(async move {
                            while let Some(step) = progress_rx.recv().await {
                                let _ = event_tx
                                    .send(AppEvent::KeySetupProgress(host_name.clone(), step))
                                    .await;
                            }
                        });

                        // Connect to the host using password to run setup commands.
                        let session = match SshSession::connect(&host_clone).await {
                            Ok(s) => s,
                            Err(e) => {
                                let _ = tx
                                    .send(AppEvent::KeySetupFailed(
                                        host_clone.name.clone(),
                                        format!("Connection failed: {}", e),
                                    ))
                                    .await;
                                return;
                            }
                        };

                        match setup_key_for_host(
                            &host_clone,
                            &session,
                            KeyType::Ed25519,
                            Some(progress_tx),
                        )
                        .await
                        {
                            Ok(result) => {
                                use crate::ssh::key_setup::KeySetupState;
                                match result.state {
                                    KeySetupState::Success | KeySetupState::PartialSuccess => {
                                        let _ = tx
                                            .send(AppEvent::KeySetupComplete(
                                                host_clone.name.clone(),
                                                result.key_path,
                                            ))
                                            .await;
                                    }
                                    KeySetupState::RolledBack => {
                                        let msg = result
                                            .error_message
                                            .unwrap_or_else(|| "Rolled back.".to_string());
                                        let _ = tx
                                            .send(AppEvent::KeySetupRollback(
                                                host_clone.name.clone(),
                                                msg,
                                            ))
                                            .await;
                                    }
                                    _ => {
                                        let msg = result
                                            .error_message
                                            .unwrap_or_else(|| "Unknown failure.".to_string());
                                        let _ = tx
                                            .send(AppEvent::KeySetupFailed(
                                                host_clone.name.clone(),
                                                msg,
                                            ))
                                            .await;
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(AppEvent::KeySetupFailed(
                                        host_clone.name.clone(),
                                        format!("{:#}", e),
                                    ))
                                    .await;
                            }
                        }

                        session.disconnect().await;
                    });
                }
            }

            AppAction::CancelKeySetup => {
                self.view.host_list.popup = None;
            }

            // ---------------------------------------------------------------
            // Detail View actions
            // ---------------------------------------------------------------
            AppAction::OpenDetailView => {
                // Open Detail View screen for the currently selected host
                let mut state = self.state.write().await;
                state.screen = Screen::DetailView;
            }

            AppAction::CloseDetailView => {
                // Return to Dashboard
                let mut state = self.state.write().await;
                state.screen = Screen::Dashboard;
            }

            AppAction::ConnectFromDetailView => {
                // Same as ConnectAt — open PTY tab for selected host
                let idx = self.view.host_list.selected_host_idx();
                if let Some(idx) = idx {
                    self.open_term_tab(idx).await;
                }
            }

            AppAction::ShowQuickView(service_kind) => {
                // Execute a quick command for the specified service
                self.execute_quick_view(service_kind).await;
            }

            AppAction::CloseQuickView => {
                // Close Quick View popup
                self.view.quick_view = None;
                self.view.quick_view_scroll = 0;
            }

            // ---------------------------------------------------------------
            // Snippet actions
            // ---------------------------------------------------------------
            AppAction::ReloadSnippets => {
                let tx = self.event_tx.clone();
                tokio::spawn(async move {
                    match config::snippets::load_snippets() {
                        Ok(s) => {
                            let _ = tx.send(AppEvent::SnippetsLoaded(s)).await;
                        }
                        Err(e) => tracing::warn!("Snippet reload failed: {}", e),
                    }
                });
            }

            AppAction::OpenSnippetAdd => {
                self.view.snippets_view.popup = Some(SnippetPopup::Add(SnippetForm::empty()));
            }

            AppAction::OpenSnippetEdit => {
                let idx = self.view.snippets_view.selected_snippet_idx();
                if let Some(i) = idx {
                    let snippet = self.state.read().await.snippets.get(i).cloned();
                    if let Some(s) = snippet {
                        let form = SnippetForm::from_snippet(&s);
                        self.view.snippets_view.popup = Some(SnippetPopup::Edit {
                            snippet_idx: i,
                            form,
                        });
                    }
                }
            }

            AppAction::OpenSnippetDeleteConfirm => {
                if let Some(idx) = self.view.snippets_view.selected_snippet_idx() {
                    self.view.snippets_view.popup = Some(SnippetPopup::DeleteConfirm(idx));
                }
            }

            AppAction::ConfirmSnippetForm => {
                self.handle_confirm_snippet_form().await;
            }

            AppAction::ConfirmSnippetDelete => {
                self.handle_confirm_snippet_delete().await;
            }

            AppAction::SnippetSearchChanged => {
                let state = self.state.read().await;
                let q = self.view.snippets_view.search_query.clone();
                self.view.snippets_view.rebuild_filter(&state.snippets, &q);
            }

            AppAction::ExecuteSnippet {
                snippet_idx,
                host_names,
            } => {
                // Resolve target host(s) if none were provided.
                let resolved = if !host_names.is_empty() {
                    host_names
                } else {
                    let state = self.state.read().await;
                    if let Some(s) = state.snippets.get(snippet_idx) {
                        if s.scope == SnippetScope::Host {
                            s.host.iter().cloned().collect()
                        } else {
                            self.view
                                .host_list
                                .selected_host_idx()
                                .and_then(|i| state.hosts.get(i))
                                .map(|h| vec![h.name.clone()])
                                .unwrap_or_default()
                        }
                    } else {
                        vec![]
                    }
                };

                if resolved.is_empty() {
                    // Open the broadcast picker so the user can choose hosts.
                    self.view.snippets_view.popup = Some(SnippetPopup::BroadcastPicker {
                        snippet_idx,
                        selected_host_indices: vec![],
                        cursor: 0,
                    });
                } else {
                    self.execute_snippet(snippet_idx, resolved).await;
                }
            }

            AppAction::ConfirmParamInput => {
                self.handle_confirm_param_input().await;
            }

            AppAction::OpenBroadcastPicker => {
                if let Some(idx) = self.view.snippets_view.selected_snippet_idx() {
                    self.view.snippets_view.popup = Some(SnippetPopup::BroadcastPicker {
                        snippet_idx: idx,
                        selected_host_indices: vec![],
                        cursor: 0,
                    });
                }
            }

            AppAction::ToggleBroadcastHost(host_idx) => {
                if let Some(SnippetPopup::BroadcastPicker {
                    selected_host_indices,
                    ..
                }) = &mut self.view.snippets_view.popup
                {
                    if let Some(pos) = selected_host_indices.iter().position(|&i| i == host_idx) {
                        selected_host_indices.remove(pos);
                    } else {
                        selected_host_indices.push(host_idx);
                    }
                }
            }

            AppAction::ConfirmBroadcast => {
                self.handle_confirm_broadcast().await;
            }

            AppAction::OpenQuickExecute => {
                let host_name = {
                    let state = self.state.read().await;
                    self.view
                        .host_list
                        .selected_host_idx()
                        .and_then(|i| state.hosts.get(i))
                        .map(|h| h.name.clone())
                };
                if let Some(name) = host_name {
                    self.view.snippets_view.popup = Some(SnippetPopup::QuickExecuteInput {
                        host_name: name,
                        command_field: FormField::default(),
                    });
                } else {
                    self.view.status_message = Some("No host selected.".to_string());
                }
            }

            AppAction::QuickExecute { host_name, command } => {
                self.run_quick_execute(host_name, command).await;
            }

            AppAction::DismissSnippetResult => {
                if matches!(
                    self.view.snippets_view.popup,
                    Some(SnippetPopup::Results { .. })
                ) {
                    self.view.snippets_view.popup = None;
                }
            }

            // ---------------------------------------------------------------
            // File Manager actions
            // ---------------------------------------------------------------
            AppAction::FmNavUp => {
                self.active_fm_panel_mut().select_prev();
                self.request_preview_for_active();
            }

            AppAction::FmNavDown => {
                self.active_fm_panel_mut().select_next();
                self.request_preview_for_active();
            }

            AppAction::FmSwitchPanel => {
                self.view.file_manager.active_panel = match self.view.file_manager.active_panel {
                    FmPanel::Local => FmPanel::Remote,
                    FmPanel::Remote => FmPanel::Local,
                };
                self.request_preview_for_active();
            }

            AppAction::FmEnterDir => {
                self.fm_enter_dir().await;
            }

            AppAction::FmParentDir => {
                self.fm_parent_dir().await;
            }

            AppAction::FmMarkFile => {
                let panel = self.active_fm_panel_mut();
                if let Some(entry) = panel.cursor_entry() {
                    if entry.name == ".." {
                        return Ok(());
                    }
                    let path = entry.path.clone();
                    if panel.marked.contains(&path) {
                        panel.marked.remove(&path);
                    } else {
                        panel.marked.insert(path);
                    }
                }
            }

            AppAction::FmCopy => {
                let (paths, source) = {
                    let panel = self.active_fm_panel_ref();
                    (
                        panel.marked_or_cursor_paths(),
                        self.view.file_manager.active_panel.clone(),
                    )
                };
                if paths.is_empty() {
                    self.view.status_message = Some("Nothing to copy.".to_string());
                } else {
                    self.view.file_manager.clipboard = Some(FmClipboard {
                        paths,
                        source_panel: source,
                    });
                    self.view.status_message =
                        Some("Copied to clipboard. Switch panel and press p to paste.".to_string());
                }
            }

            AppAction::FmPaste => {
                self.fm_paste().await;
            }

            AppAction::FmOpenDeleteConfirm => {
                let paths = self.active_fm_panel_ref().marked_or_cursor_paths();
                if paths.is_empty() {
                    self.view.status_message = Some("Nothing to delete.".to_string());
                } else {
                    self.view.file_manager.popup = Some(FileManagerPopup::DeleteConfirm { paths });
                }
            }

            AppAction::FmConfirmDelete => {
                self.fm_delete().await;
            }

            AppAction::FmOpenMkDir => {
                self.view.file_manager.popup = Some(FileManagerPopup::MkDir(FormField::default()));
            }

            AppAction::FmConfirmMkDir(name) => {
                self.fm_mkdir(name).await;
            }

            AppAction::FmOpenRename => {
                if let Some(entry) = self.active_fm_panel_ref().cursor_entry() {
                    if entry.name != ".." {
                        let original_name = entry.name.clone();
                        let field = FormField::with_value(&original_name);
                        self.view.file_manager.popup = Some(FileManagerPopup::Rename {
                            original_name,
                            field,
                        });
                    }
                }
            }

            AppAction::FmConfirmRename(name) => {
                self.fm_rename(name).await;
            }

            AppAction::FmClosePopup => {
                self.view.file_manager.popup = None;
            }

            AppAction::FmOpenHostPicker => {
                self.view.file_manager.popup = Some(FileManagerPopup::HostPicker { cursor: 0 });
            }

            AppAction::FmHostPickerSelect(idx) => {
                self.fm_connect_host(idx).await;
            }

            AppAction::FmHostPickerNav(delta) => {
                if let Some(FileManagerPopup::HostPicker { cursor }) =
                    &mut self.view.file_manager.popup
                {
                    let hosts_len = self.state.read().await.hosts.len();
                    if hosts_len == 0 {
                        return Ok(());
                    }
                    if delta > 0 {
                        *cursor = (*cursor + 1).min(hosts_len - 1);
                    } else {
                        *cursor = cursor.saturating_sub(1);
                    }
                }
            }

            // ---------------------------------------------------------------
            // Terminal multi-session actions
            // ---------------------------------------------------------------
            AppAction::TermOpenTab(host_idx) => {
                self.open_term_tab(host_idx).await;
            }

            AppAction::TermInput(bytes) => {
                let active_id = self.view.terminal_view.active_session_id();
                if let (Some(id), Some(mgr)) = (active_id, &mut self.pty_manager) {
                    // Jump back to the live screen when the user types anything.
                    let tv = &mut self.view.terminal_view;
                    let focused_idx = match &tv.split {
                        Some(sv) if tv.split_focus == SplitFocus::Secondary => sv.secondary_tab,
                        _ => tv.active_tab,
                    };
                    if let Some(tab) = tv.tabs.get_mut(focused_idx) {
                        tab.scroll_offset = 0;
                    }
                    if let Err(e) = mgr.write(id, &bytes) {
                        tracing::warn!("PTY write error for session {id}: {e}");
                    }
                }
            }

            AppAction::TermCloseTab => {
                let tv = &mut self.view.terminal_view;
                if tv.tabs.is_empty() {
                    return Ok(());
                }
                let id = tv.tabs[tv.active_tab].session_id;
                if let Some(mgr) = &mut self.pty_manager {
                    mgr.close(id);
                }
                tv.tabs.remove(tv.active_tab);
                tv.split = None;
                tv.split_focus = SplitFocus::Primary;
                if tv.tabs.is_empty() {
                    self.state.write().await.screen = Screen::Dashboard;
                } else {
                    tv.active_tab = tv.active_tab.min(tv.tabs.len().saturating_sub(1));
                }
            }

            AppAction::TermSwitchTab(n) => {
                let tv = &mut self.view.terminal_view;
                if n < tv.tabs.len() {
                    tv.active_tab = n;
                    tv.tabs[n].has_activity = false;
                }
            }

            AppAction::TermSplitVertical => {
                let tv = &mut self.view.terminal_view;
                // Same key while already in vertical split → close split.
                if matches!(&tv.split, Some(sv) if sv.direction == SplitDirection::Vertical) {
                    tv.split = None;
                    tv.split_focus = SplitFocus::Primary;
                } else if tv.tabs.len() >= 2 {
                    // Already in horizontal split → switch direction, keep secondary tab.
                    let secondary = tv
                        .split
                        .as_ref()
                        .map(|sv| sv.secondary_tab)
                        .unwrap_or_else(|| (tv.active_tab + 1) % tv.tabs.len());
                    tv.split = Some(SplitView {
                        direction: SplitDirection::Vertical,
                        secondary_tab: secondary,
                    });
                    tv.split_focus = SplitFocus::Primary;
                } else {
                    self.view.status_message = Some(
                        "Need at least 2 tabs to split. Open another tab with Ctrl+T.".to_string(),
                    );
                }
            }

            AppAction::TermSplitHorizontal => {
                let tv = &mut self.view.terminal_view;
                // Same key while already in horizontal split → close split.
                if matches!(&tv.split, Some(sv) if sv.direction == SplitDirection::Horizontal) {
                    tv.split = None;
                    tv.split_focus = SplitFocus::Primary;
                } else if tv.tabs.len() >= 2 {
                    // Already in vertical split → switch direction, keep secondary tab.
                    let secondary = tv
                        .split
                        .as_ref()
                        .map(|sv| sv.secondary_tab)
                        .unwrap_or_else(|| (tv.active_tab + 1) % tv.tabs.len());
                    tv.split = Some(SplitView {
                        direction: SplitDirection::Horizontal,
                        secondary_tab: secondary,
                    });
                    tv.split_focus = SplitFocus::Primary;
                } else {
                    self.view.status_message = Some(
                        "Need at least 2 tabs to split. Open another tab with Ctrl+T.".to_string(),
                    );
                }
            }

            AppAction::TermFocusNextPane => {
                let tv = &mut self.view.terminal_view;
                if tv.split.is_some() {
                    tv.split_focus = match tv.split_focus {
                        SplitFocus::Primary => SplitFocus::Secondary,
                        SplitFocus::Secondary => SplitFocus::Primary,
                    };
                }
            }

            AppAction::TermOpenHostPicker => {
                self.view.terminal_view.host_picker = Some(TermHostPicker::default());
            }

            AppAction::TermHostPickerNav(delta) => {
                if let Some(picker) = &mut self.view.terminal_view.host_picker {
                    let hosts_len = self.state.read().await.hosts.len();
                    if hosts_len == 0 {
                        return Ok(());
                    }
                    if delta > 0 {
                        picker.cursor = (picker.cursor + 1).min(hosts_len - 1);
                    } else {
                        picker.cursor = picker.cursor.saturating_sub(1);
                    }
                }
            }

            AppAction::TermHostPickerSelect(idx) => {
                let switch_pane_mode = self
                    .view
                    .terminal_view
                    .host_picker
                    .as_ref()
                    .map(|p| p.switch_pane_mode)
                    .unwrap_or(false);

                self.view.terminal_view.host_picker = None;

                if switch_pane_mode {
                    // Replace the focused pane's tab with a new connection
                    self.switch_focused_pane_host(idx).await;
                } else {
                    // Normal mode: create a new tab
                    self.open_term_tab(idx).await;
                }
            }

            AppAction::TermCloseHostPicker => {
                self.view.terminal_view.host_picker = None;
                // If no tabs are open, return to Dashboard.
                if self.view.terminal_view.tabs.is_empty() {
                    self.state.write().await.screen = Screen::Dashboard;
                }
            }

            AppAction::TermSwitchPaneHost => {
                // Open host picker in "switch pane mode"
                self.view.terminal_view.host_picker = Some(TermHostPicker {
                    cursor: 0,
                    switch_pane_mode: true,
                });
            }

            AppAction::SwitchScreen(target) => {
                let bootstrap_fm = matches!(target, Screen::FileManager);
                let open_picker =
                    matches!(target, Screen::Terminal) && self.view.terminal_view.tabs.is_empty();
                self.state.write().await.screen = target;
                self.view.status_message = None;
                if bootstrap_fm {
                    self.bootstrap_file_manager().await;
                }
                if open_picker {
                    self.view.terminal_view.host_picker = Some(TermHostPicker::default());
                }
            }
        }

        Ok(())
    }

    /// Opens a new PTY terminal tab for `AppState.hosts[host_idx]`.
    ///
    /// Switches to the Terminal screen and sets `active_tab` to the new tab.
    /// Reports errors in the status bar without panicking.
    async fn open_term_tab(&mut self, host_idx: usize) {
        let host = {
            let state = self.state.read().await;
            state.hosts.get(host_idx).cloned()
        };
        let Some(host) = host else {
            self.view.status_message = Some("No such host.".to_string());
            return;
        };
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        // Reserve rows: status bar (1) + tab bar (1) + pane border top+bottom (2) = 4.
        // Reserve cols: pane border left+right (2) = 2.
        let pty_rows = rows.saturating_sub(4);
        let pty_cols = cols.saturating_sub(2);
        let mgr = self.pty_manager.get_or_insert_with(PtyManager::new);
        match mgr.open(&host, pty_cols, pty_rows, self.event_tx.clone()) {
            Ok(session_id) => {
                let Some(parser) = mgr.parser_for(session_id) else {
                    tracing::error!(
                        session = session_id,
                        "parser not found for freshly created session"
                    );
                    return;
                };
                self.view.terminal_view.tabs.push(TermTab {
                    session_id,
                    host_name: host.name.clone(),
                    has_activity: false,
                    parser,
                    scroll_offset: 0,
                });
                self.view.terminal_view.active_tab =
                    self.view.terminal_view.tabs.len().saturating_sub(1);
                self.state.write().await.screen = Screen::Terminal;
                tracing::info!(
                    "Opened terminal tab for '{}' (session {})",
                    host.name,
                    session_id
                );
            }
            Err(e) => {
                self.view.status_message = Some(format!("PTY error: {e}"));
            }
        }
    }

    /// Switches the focused pane's host connection to a new host.
    /// Closes the existing session for that pane and opens a new one.
    async fn switch_focused_pane_host(&mut self, host_idx: usize) {
        let host = {
            let state = self.state.read().await;
            state.hosts.get(host_idx).cloned()
        };
        let Some(host) = host else {
            self.view.status_message = Some("No such host.".to_string());
            return;
        };

        let tv = &mut self.view.terminal_view;

        // Determine which tab index to replace based on split focus
        let tab_idx = match &tv.split {
            Some(sv) if tv.split_focus == SplitFocus::Secondary => sv.secondary_tab,
            _ => tv.active_tab,
        };

        // Close the old session
        if let Some(old_tab) = tv.tabs.get(tab_idx) {
            if let Some(mgr) = &mut self.pty_manager {
                mgr.close(old_tab.session_id);
                tracing::info!(
                    "Closed terminal session {} for '{}'",
                    old_tab.session_id,
                    old_tab.host_name
                );
            }
        }

        // Open new session
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let pty_rows = rows.saturating_sub(4);
        let pty_cols = cols.saturating_sub(2);
        let mgr = self.pty_manager.get_or_insert_with(PtyManager::new);

        match mgr.open(&host, pty_cols, pty_rows, self.event_tx.clone()) {
            Ok(session_id) => {
                let Some(parser) = mgr.parser_for(session_id) else {
                    tracing::error!(
                        session = session_id,
                        "parser not found for freshly created session"
                    );
                    return;
                };

                // Replace the tab at the current position
                let new_tab = TermTab {
                    session_id,
                    host_name: host.name.clone(),
                    has_activity: false,
                    parser,
                    scroll_offset: 0,
                };

                if let Some(slot) = tv.tabs.get_mut(tab_idx) {
                    *slot = new_tab;
                }

                tracing::info!(
                    "Switched pane {} to host '{}' (session {})",
                    tab_idx,
                    host.name,
                    session_id
                );
            }
            Err(e) => {
                self.view.status_message = Some(format!("PTY error: {e}"));
            }
        }
    }

    /// Handles the user confirming the Add or Edit host form.
    async fn handle_confirm_form(&mut self) {
        match self.view.host_list.popup.take() {
            Some(HostPopup::Add(form)) => {
                match form.to_host(HostSource::Manual) {
                    Ok(host) => {
                        let has_password = host.password.is_some();
                        let host_name = host.name.clone();

                        {
                            let mut state = self.state.write().await;
                            state.hosts.push(host);
                        }
                        self.save_manual_hosts().await;

                        // Restart poll_manager to include the new host
                        {
                            let state = self.state.read().await;
                            if let Some(old) = self.poll_manager.take() {
                                old.shutdown();
                            }
                            self.poll_manager = Some(PollManager::start(
                                state.hosts.clone(),
                                self.event_tx.clone(),
                                Duration::from_secs(30),
                            ));
                        }

                        let state = self.state.read().await;
                        self.view.host_list.rebuild_filter(
                            &state.hosts,
                            &state.metrics,
                            &state.connection_statuses,
                        );
                        self.view.host_list.rebuild_tags(&state.hosts);

                        // Suggest SSH key setup if host was added with password
                        if has_password {
                            self.view.status_message = Some(format!(
                                "Host '{}' added. Press 'Shift+K' to set up SSH key authentication (recommended).",
                                host_name
                            ));
                        } else {
                            self.view.status_message = Some("Host added.".to_string());
                        }
                    }
                    Err(e) => {
                        // Restore popup so the user can correct the input.
                        self.view.host_list.popup = Some(HostPopup::Add(form));
                        self.view.status_message = Some(format!("Error: {e}"));
                    }
                }
            }

            Some(HostPopup::Edit { host_idx, form }) => match form.to_host(HostSource::Manual) {
                Ok(mut host) => {
                    let (old_name, _was_ssh_config) = {
                        let mut state = self.state.write().await;
                        let old_host = state.hosts.get(host_idx);
                        let old_name = old_host.map(|h| h.name.clone());
                        let was_ssh_config = old_host
                            .map(|h| h.source == HostSource::SshConfig)
                            .unwrap_or(false);

                        // If editing a SSH config host, preserve original name for duplicate prevention
                        if was_ssh_config && old_name.is_some() {
                            host.original_ssh_host = old_name.clone();
                        }

                        if let Some(slot) = state.hosts.get_mut(host_idx) {
                            *slot = host.clone();
                        }
                        (old_name, was_ssh_config)
                    };

                    // If the host name changed, migrate all associated data
                    if let Some(old_name) = old_name {
                        if old_name != host.name {
                            let mut state = self.state.write().await;

                            // Migrate metrics
                            if let Some(metrics) = state.metrics.remove(&old_name) {
                                state.metrics.insert(host.name.clone(), metrics);
                            }

                            // Migrate connection status
                            if let Some(status) = state.connection_statuses.remove(&old_name) {
                                state.connection_statuses.insert(host.name.clone(), status);
                            }

                            // Migrate services
                            if let Some(services) = state.services.remove(&old_name) {
                                state.services.insert(host.name.clone(), services);
                            }

                            // Migrate alerts
                            if let Some(alerts) = state.alerts.remove(&old_name) {
                                state.alerts.insert(host.name.clone(), alerts);
                            }

                            // Migrate discovery status
                            if let Some(discovery) = state.discovery_status.remove(&old_name) {
                                state.discovery_status.insert(host.name.clone(), discovery);
                            }
                        }
                    }

                    self.save_manual_hosts().await;
                    let state = self.state.read().await;
                    self.view.host_list.rebuild_filter(
                        &state.hosts,
                        &state.metrics,
                        &state.connection_statuses,
                    );
                    self.view.host_list.rebuild_tags(&state.hosts);
                    self.view.status_message = Some("Host updated.".to_string());
                }
                Err(e) => {
                    self.view.host_list.popup = Some(HostPopup::Edit { host_idx, form });
                    self.view.status_message = Some(format!("Error: {e}"));
                }
            },

            other => {
                // Wrong popup type — put it back unchanged.
                self.view.host_list.popup = other;
            }
        }
    }

    /// Handles the user confirming deletion.
    async fn handle_confirm_delete(&mut self) {
        if let Some(HostPopup::DeleteConfirm(idx)) = self.view.host_list.popup.take() {
            {
                let mut state = self.state.write().await;
                if idx < state.hosts.len() {
                    let removed = state.hosts.remove(idx);
                    self.view.status_message = Some(format!("Deleted '{}'.", removed.name));
                }
            }
            self.save_manual_hosts().await;
            let state = self.state.read().await;
            self.view.host_list.rebuild_filter(
                &state.hosts,
                &state.metrics,
                &state.connection_statuses,
            );
            self.view.host_list.rebuild_tags(&state.hosts);
        }
    }

    /// Saves the manually-added hosts to `hosts.toml`.
    async fn save_manual_hosts(&mut self) {
        let hosts = self.state.read().await.hosts.clone();
        if let Err(e) = config::save_hosts(&hosts) {
            self.view.status_message = Some(format!("Save failed: {e}"));
        }
    }

    /// Temporarily restores the terminal, runs the system SSH binary for the
    /// given host, then re-initialises the TUI.
    async fn connect_system_ssh(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        host: &Host,
    ) -> anyhow::Result<()> {
        // 1. Leave TUI mode.
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crate::utils::mouse::DisableMinimalMouseCapture,
        )?;

        // 2. Build SSH command (ConnectTimeout=10).
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.args(["-o", "ConnectTimeout=10"]);
        if host.port != 22 {
            cmd.args(["-p", &host.port.to_string()]);
        }
        if let Some(ref key) = host.identity_file {
            cmd.args(["-i", key]);
        }
        if let Some(ref jump) = host.proxy_jump {
            cmd.args(["-J", jump]);
        }
        cmd.arg(format!("{}@{}", host.user, host.hostname));

        // 3. Hand off terminal control to SSH.
        tracing::info!("Connecting to {} via system SSH", host.name);
        let status = cmd.spawn()?.wait().await?;

        // 4. Re-enter TUI mode.
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crate::utils::mouse::EnableMinimalMouseCapture,
        )?;
        terminal.clear()?;

        // 5. Show connection result.
        self.view.status_message = Some(if status.success() {
            format!("Disconnected from '{}'.", host.name)
        } else {
            format!(
                "SSH to '{}' exited with code {:?}.",
                host.name,
                status.code()
            )
        });

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Snippet execution private methods
    // -----------------------------------------------------------------------

    /// Checks whether the snippet requires param input; if yes, opens the
    /// `ParamInput` popup, otherwise fires the execution tasks immediately.
    async fn execute_snippet(&mut self, snippet_idx: usize, host_names: Vec<String>) {
        let snippet = {
            let state = self.state.read().await;
            state.snippets.get(snippet_idx).cloned()
        };
        let Some(snippet) = snippet else { return };

        let param_names: Vec<String> = snippet.params.as_deref().unwrap_or(&[]).to_vec();

        if !param_names.is_empty() {
            // Need values for placeholders — open the param input popup.
            let param_fields = param_names.iter().map(|_| FormField::default()).collect();
            self.view.snippets_view.popup = Some(SnippetPopup::ParamInput {
                snippet_idx,
                host_names,
                param_names,
                param_fields,
                focused_field: 0,
            });
        } else {
            self.spawn_snippet_tasks(&snippet, &host_names, &[]).await;
        }
    }

    /// Called when the user confirms the `ParamInput` popup.  Collects the
    /// filled values and fires the execution tasks.
    async fn handle_confirm_param_input(&mut self) {
        let popup = self.view.snippets_view.popup.take();
        match popup {
            Some(SnippetPopup::ParamInput {
                snippet_idx,
                host_names,
                param_names,
                param_fields,
                ..
            }) => {
                let param_values: Vec<String> = param_fields
                    .iter()
                    .map(|f| f.value.trim().to_string())
                    .collect();

                let snippet = {
                    let state = self.state.read().await;
                    state.snippets.get(snippet_idx).cloned()
                };
                if let Some(snippet) = snippet {
                    self.spawn_snippet_tasks(&snippet, &host_names, &param_values)
                        .await;
                }
                // `self.view.snippets_view.popup` is already `None` from `.take()`.
                // spawn_snippet_tasks will replace it with Results.
                let _ = (param_names,); // suppress unused warning
            }
            other => {
                // Wrong popup type — restore.
                self.view.snippets_view.popup = other;
            }
        }
    }

    /// Substitutes `{{placeholder}}` values in the command, opens a `Results`
    /// popup with pending entries, and spawns one tokio task per host.
    async fn spawn_snippet_tasks(
        &mut self,
        snippet: &Snippet,
        host_names: &[String],
        param_values: &[String],
    ) {
        let command = substitute_params(&snippet.command, snippet.params.as_deref(), param_values);

        // Pre-populate the Results popup with pending entries.
        let entries: Vec<SnippetResultEntry> = host_names
            .iter()
            .map(|h| SnippetResultEntry {
                host_name: h.clone(),
                snippet_name: snippet.name.clone(),
                output: Ok(String::new()),
                pending: true,
            })
            .collect();
        self.view.snippets_view.popup = Some(SnippetPopup::Results { entries, scroll: 0 });

        // Collect Host structs for the requested names.
        let hosts: Vec<Host> = {
            let state = self.state.read().await;
            host_names
                .iter()
                .filter_map(|name| state.hosts.iter().find(|h| &h.name == name).cloned())
                .collect()
        };

        // Spawn one task per host.
        for host in hosts {
            let tx = self.event_tx.clone();
            let cmd = command.clone();
            let sname = snippet.name.clone();
            tokio::spawn(async move {
                let result = run_command_on_host(&host, &cmd).await;
                let _ = tx
                    .send(AppEvent::SnippetResult {
                        host_name: host.name.clone(),
                        snippet_name: sname,
                        output: result,
                    })
                    .await;
            });
        }
    }

    /// Confirms the broadcast-picker and runs the snippet on all checked hosts.
    async fn handle_confirm_broadcast(&mut self) {
        let popup = self.view.snippets_view.popup.take();
        match popup {
            Some(SnippetPopup::BroadcastPicker {
                snippet_idx,
                selected_host_indices,
                ..
            }) => {
                if selected_host_indices.is_empty() {
                    self.view.status_message = Some("No hosts selected.".to_string());
                    return;
                }
                let host_names: Vec<String> = {
                    let state = self.state.read().await;
                    selected_host_indices
                        .iter()
                        .filter_map(|&i| state.hosts.get(i))
                        .map(|h| h.name.clone())
                        .collect()
                };
                self.execute_snippet(snippet_idx, host_names).await;
            }
            other => {
                self.view.snippets_view.popup = other;
            }
        }
    }

    /// Runs an ad-hoc quick-execute command.
    async fn run_quick_execute(&mut self, host_name: String, command: String) {
        let host = {
            let state = self.state.read().await;
            state.hosts.iter().find(|h| h.name == host_name).cloned()
        };

        let Some(host) = host else {
            self.view.snippets_view.popup = Some(SnippetPopup::Results {
                entries: vec![SnippetResultEntry {
                    host_name: host_name.clone(),
                    snippet_name: "(quick-execute)".to_string(),
                    output: Err(format!("Host '{}' not found.", host_name)),
                    pending: false,
                }],
                scroll: 0,
            });
            return;
        };

        // Open Results popup with a single pending entry.
        self.view.snippets_view.popup = Some(SnippetPopup::Results {
            entries: vec![SnippetResultEntry {
                host_name: host_name.clone(),
                snippet_name: "(quick-execute)".to_string(),
                output: Ok(String::new()),
                pending: true,
            }],
            scroll: 0,
        });

        let tx = self.event_tx.clone();
        let cmd = command.clone();
        tokio::spawn(async move {
            let result = run_command_on_host(&host, &cmd).await;
            let _ = tx
                .send(AppEvent::SnippetResult {
                    host_name: host.name.clone(),
                    snippet_name: "(quick-execute)".to_string(),
                    output: result,
                })
                .await;
        });
    }

    /// Executes a quick view command for a specific service.
    /// This is similar to quick-execute but uses predefined commands per service type.
    async fn execute_quick_view(&mut self, service_kind: ServiceKind) {
        // Get the currently selected host from the dashboard/detail view
        let (host, host_name) = {
            let state = self.state.read().await;
            match self.view.host_list.selected_host_idx() {
                Some(idx) => match state.hosts.get(idx) {
                    Some(h) => (Some(h.clone()), h.name.clone()),
                    None => (None, "(unknown)".to_string()),
                },
                None => (None, "(no selection)".to_string()),
            }
        };

        let Some(host) = host else {
            self.view.snippets_view.popup = Some(SnippetPopup::Results {
                entries: vec![SnippetResultEntry {
                    host_name: host_name.clone(),
                    snippet_name: format!("Quick View: {:?}", service_kind),
                    output: Err("No host selected.".to_string()),
                    pending: false,
                }],
                scroll: 0,
            });
            return;
        };

        // Determine the command based on service kind
        let (command, service_name) = match service_kind {
            ServiceKind::Docker => (
                "docker compose ps -a 2>/dev/null || docker ps -a",
                "Docker Containers",
            ),
            ServiceKind::Nginx => (
                "echo '=== Nginx Status ===' && systemctl status nginx --no-pager || service nginx status",
                "Nginx Status",
            ),
            ServiceKind::PostgreSQL => (
                "echo '=== PostgreSQL Connections ===' && sudo -u postgres psql -c 'SELECT count(*) as connections, state FROM pg_stat_activity GROUP BY state;' 2>/dev/null || echo 'No access to PostgreSQL'",
                "PostgreSQL Connections",
            ),
            ServiceKind::Redis => (
                "echo '=== Redis Info ===' && redis-cli info server 2>/dev/null | head -20 || echo 'Redis not accessible'",
                "Redis Info",
            ),
            ServiceKind::NodeJS => (
                "echo '=== PM2 Status ===' && pm2 status 2>/dev/null || (echo '=== Node Processes ===' && ps aux | grep -E '[n]ode ' | head -10)",
                "Node.js Processes",
            ),
        };

        // Open Results popup with a single pending entry
        self.view.snippets_view.popup = Some(SnippetPopup::Results {
            entries: vec![SnippetResultEntry {
                host_name: host_name.clone(),
                snippet_name: format!("Quick View: {}", service_name),
                output: Ok(String::new()),
                pending: true,
            }],
            scroll: 0,
        });

        let tx = self.event_tx.clone();
        let cmd = command.to_string();
        let sname = format!("Quick View: {}", service_name);
        tokio::spawn(async move {
            let result = run_command_on_host(&host, &cmd).await;
            let _ = tx
                .send(AppEvent::SnippetResult {
                    host_name: host.name.clone(),
                    snippet_name: sname,
                    output: result,
                })
                .await;
        });
    }

    /// Confirms the snippet add/edit form and saves.
    async fn handle_confirm_snippet_form(&mut self) {
        match self.view.snippets_view.popup.take() {
            Some(SnippetPopup::Add(form)) => match form.to_snippet() {
                Ok(snippet) => {
                    {
                        let mut state = self.state.write().await;
                        state.snippets.push(snippet);
                    }
                    self.save_snippets().await;
                    let state = self.state.read().await;
                    let q = self.view.snippets_view.search_query.clone();
                    self.view.snippets_view.rebuild_filter(&state.snippets, &q);
                    self.view.status_message = Some("Snippet added.".to_string());
                }
                Err(e) => {
                    self.view.snippets_view.popup = Some(SnippetPopup::Add(form));
                    self.view.status_message = Some(format!("Error: {e}"));
                }
            },

            Some(SnippetPopup::Edit { snippet_idx, form }) => match form.to_snippet() {
                Ok(snippet) => {
                    {
                        let mut state = self.state.write().await;
                        if let Some(slot) = state.snippets.get_mut(snippet_idx) {
                            *slot = snippet;
                        }
                    }
                    self.save_snippets().await;
                    let state = self.state.read().await;
                    let q = self.view.snippets_view.search_query.clone();
                    self.view.snippets_view.rebuild_filter(&state.snippets, &q);
                    self.view.status_message = Some("Snippet updated.".to_string());
                }
                Err(e) => {
                    self.view.snippets_view.popup = Some(SnippetPopup::Edit { snippet_idx, form });
                    self.view.status_message = Some(format!("Error: {e}"));
                }
            },

            other => {
                self.view.snippets_view.popup = other;
            }
        }
    }

    /// Confirms snippet deletion.
    async fn handle_confirm_snippet_delete(&mut self) {
        if let Some(SnippetPopup::DeleteConfirm(idx)) = self.view.snippets_view.popup.take() {
            {
                let mut state = self.state.write().await;
                if idx < state.snippets.len() {
                    let removed = state.snippets.remove(idx);
                    self.view.status_message = Some(format!("Deleted snippet '{}'.", removed.name));
                }
            }
            self.save_snippets().await;
            let state = self.state.read().await;
            let q = self.view.snippets_view.search_query.clone();
            self.view.snippets_view.rebuild_filter(&state.snippets, &q);
        }
    }

    /// Persists `AppState.snippets` to `snippets.toml`.
    async fn save_snippets(&mut self) {
        let snippets = self.state.read().await.snippets.clone();
        if let Err(e) = config::snippets::save_snippets(&snippets) {
            self.view.status_message = Some(format!("Save failed: {e}"));
        }
    }

    // -----------------------------------------------------------------------
    // File Manager private helper methods
    // -----------------------------------------------------------------------

    /// Returns a mutable reference to the active file panel view.
    fn active_fm_panel_mut(&mut self) -> &mut FilePanelView {
        match self.view.file_manager.active_panel {
            FmPanel::Local => &mut self.view.file_manager.local,
            FmPanel::Remote => &mut self.view.file_manager.remote,
        }
    }

    /// Returns a shared reference to the active file panel view.
    fn active_fm_panel_ref(&self) -> &FilePanelView {
        match self.view.file_manager.active_panel {
            FmPanel::Local => &self.view.file_manager.local,
            FmPanel::Remote => &self.view.file_manager.remote,
        }
    }

    /// Initialises the file manager when the user first switches to it.
    ///
    /// - Loads the local panel from `home_dir` (or `/`) if it is empty.
    /// - Opens the host-picker popup if no remote session is active.
    async fn bootstrap_file_manager(&mut self) {
        // Load local panel if not yet populated.
        if self.view.file_manager.local.cwd.is_empty() {
            let start = dirs::home_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".to_string());
            let tx = self.event_tx.clone();
            let path = start.clone();
            tokio::spawn(async move {
                match sftp::list_local_dir(&path).await {
                    Ok(entries) => {
                        let _ = tx.send(AppEvent::LocalDirListed { path, entries }).await;
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
    fn request_preview_for_active(&mut self) {
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
            let tx = self.event_tx.clone();
            tokio::spawn(async move {
                if let Ok(content) = sftp::preview_local_file(&path).await {
                    let _ = tx.send(AppEvent::FilePreviewReady { path, content }).await;
                }
            });
        }
    }

    /// Re-lists both panels after a mutating operation completes.
    async fn refresh_active_panels(&mut self) {
        let local_path = self.view.file_manager.local.cwd.clone();
        if !local_path.is_empty() {
            let tx = self.event_tx.clone();
            tokio::spawn(async move {
                match sftp::list_local_dir(&local_path).await {
                    Ok(entries) => {
                        let _ = tx
                            .send(AppEvent::LocalDirListed {
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
    async fn fm_connect_host(&mut self, idx: usize) {
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
        self.view.file_manager.connected_host = None;
        self.view.file_manager.remote = FilePanelView::default();

        self.view.status_message = Some(format!("Connecting to '{}'… (30s timeout)", host.name));
        self.view.file_manager.sftp_connecting = true;

        // Spawn connection in background with 30s timeout to prevent UI freeze
        let tx = self.event_tx.clone();
        let host_clone = host.clone();
        tokio::spawn(async move {
            let connect_future = SftpManager::connect(&host_clone, tx.clone());
            let timeout_future = tokio::time::sleep(Duration::from_secs(30));

            tokio::select! {
                result = connect_future => {
                    match result {
                        Ok(mgr) => {
                            // Send the manager through a new event type
                            let _ = tx
                                .send(AppEvent::SftpManagerReady {
                                    host_name: host_clone.name.clone(),
                                    manager: Box::new(mgr),
                                })
                                .await;
                        }
                        Err(e) => {
                            let _ = tx
                                .send(AppEvent::SftpDisconnected {
                                    host_name: host_clone.name.clone(),
                                    reason: e.to_string(),
                                })
                                .await;
                        }
                    }
                }
                _ = timeout_future => {
                    let _ = tx
                        .send(AppEvent::SftpDisconnected {
                            host_name: host_clone.name.clone(),
                            reason: "connection timed out (30s)".to_string(),
                        })
                        .await;
                }
            }
        });
    }

    /// Enters the directory under the cursor in the active panel.
    async fn fm_enter_dir(&mut self) {
        let is_remote = self.view.file_manager.active_panel == FmPanel::Remote;
        let entry = self.active_fm_panel_ref().cursor_entry().cloned();
        let Some(entry) = entry else { return };
        if !entry.is_dir {
            return;
        }

        if is_remote {
            if let Some(mgr) = &self.sftp_manager {
                mgr.send(SftpCommand::ListDir(entry.path.clone()));
            }
        } else {
            let path = entry.path.clone();
            let tx = self.event_tx.clone();
            tokio::spawn(async move {
                match sftp::list_local_dir(&path).await {
                    Ok(entries) => {
                        let _ = tx.send(AppEvent::LocalDirListed { path, entries }).await;
                    }
                    Err(e) => {
                        let _ = tx
                            .send(AppEvent::Error("local".to_string(), e.to_string()))
                            .await;
                    }
                }
            });
        }
    }

    /// Navigates to the parent of the current working directory.
    async fn fm_parent_dir(&mut self) {
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

        if is_remote {
            if let Some(mgr) = &self.sftp_manager {
                mgr.send(SftpCommand::ListDir(parent));
            }
        } else {
            let tx = self.event_tx.clone();
            tokio::spawn(async move {
                match sftp::list_local_dir(&parent).await {
                    Ok(entries) => {
                        let _ = tx
                            .send(AppEvent::LocalDirListed {
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

    /// Pastes all clipboard contents into the active panel (upload / download).
    ///
    /// All files are queued as individual SFTP commands and processed sequentially
    /// by the background task. `pending_ops` tracks how many are still in flight.
    async fn fm_paste(&mut self) {
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

        let dst_cwd = match &dst_panel {
            FmPanel::Local => self.view.file_manager.local.cwd.clone(),
            FmPanel::Remote => self.view.file_manager.remote.cwd.clone(),
        };

        let count = clipboard.paths.len();
        let first_tid = self.next_transfer_id;
        self.next_transfer_id += count as u64;
        self.view.file_manager.pending_ops = count;

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

        // Queue every file as a separate SFTP command.
        for (i, src_path) in clipboard.paths.iter().enumerate() {
            let tid = first_tid + i as u64;
            let fname = filename_of(src_path);
            let dst = format!("{}/{}", dst_cwd.trim_end_matches('/'), fname);

            match (&clipboard.source_panel, &dst_panel) {
                (FmPanel::Local, FmPanel::Remote) => {
                    if let Some(mgr) = &self.sftp_manager {
                        mgr.send(SftpCommand::Upload {
                            local: src_path.clone(),
                            remote: dst,
                            transfer_id: tid,
                        });
                    }
                }
                (FmPanel::Remote, FmPanel::Local) => {
                    if let Some(mgr) = &self.sftp_manager {
                        mgr.send(SftpCommand::Download {
                            remote: src_path.clone(),
                            local: dst,
                            transfer_id: tid,
                        });
                    }
                }
                _ => unreachable!("same-panel case handled above"),
            }
        }

        if count > 1 {
            self.view.status_message = Some(format!("Queued {count} files for transfer…"));
        }
    }

    /// Deletes items listed in the `DeleteConfirm` popup.
    async fn fm_delete(&mut self) {
        let popup = self.view.file_manager.popup.take();
        let Some(FileManagerPopup::DeleteConfirm { paths }) = popup else {
            return;
        };

        let is_remote = self.view.file_manager.active_panel == FmPanel::Remote;

        if is_remote {
            // Send delete commands for all paths.
            for path in paths {
                if let Some(mgr) = &self.sftp_manager {
                    mgr.send(SftpCommand::Delete(path));
                }
            }
        } else {
            let tx = self.event_tx.clone();
            tokio::spawn(async move {
                let mut last_err: Option<String> = None;
                for path in paths {
                    let result = tokio::fs::remove_file(&path).await.or_else(|_| {
                        // Might be a directory — try remove_dir (empty only).
                        // Using blocking version since remove_dir_all is destructive.
                        std::fs::remove_dir(&path)
                            .map_err(|e| std::io::Error::new(e.kind(), e.to_string()))
                    });
                    if let Err(e) = result {
                        last_err = Some(e.to_string());
                    }
                }
                let result = last_err.map_or(Ok(()), Err);
                let _ = tx
                    .send(AppEvent::SftpOpDone {
                        kind: SftpOpKind::Delete,
                        result: result.map_err(|e: String| e),
                    })
                    .await;
            });
        }
    }

    /// Creates a new directory in the active panel.
    async fn fm_mkdir(&mut self, name: String) {
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
            let tx = self.event_tx.clone();
            tokio::spawn(async move {
                let result = tokio::fs::create_dir(&new_path)
                    .await
                    .map_err(|e| e.to_string());
                let _ = tx
                    .send(AppEvent::SftpOpDone {
                        kind: SftpOpKind::MkDir,
                        result,
                    })
                    .await;
            });
        }
    }

    /// Renames the file or directory under the cursor.
    async fn fm_rename(&mut self, new_name: String) {
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
            let tx = self.event_tx.clone();
            tokio::spawn(async move {
                let result = tokio::fs::rename(&old_path, &new_path)
                    .await
                    .map_err(|e| e.to_string());
                let _ = tx
                    .send(AppEvent::SftpOpDone {
                        kind: SftpOpKind::Rename,
                        result,
                    })
                    .await;
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Free helper functions for snippet execution
// ---------------------------------------------------------------------------

/// Extracts the file name from an absolute path string.
fn filename_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string()
}

/// Opens a fresh SSH connection, runs `command`, closes the connection, and
/// returns `Ok(stdout)` or `Err(error_message)`.
///
/// A new connection is opened for each invocation.  Connection pooling with
/// the metrics poller is a future optimisation.
async fn run_command_on_host(host: &Host, command: &str) -> Result<String, String> {
    let session = SshSession::connect(host)
        .await
        .map_err(|e| format!("Connect failed: {e}"))?;
    let output = session
        .run_command(command)
        .await
        .map_err(|e| format!("Command failed: {e}"))?;
    let _ = session.disconnect().await;
    Ok(output)
}

/// Replaces `{{param_name}}` placeholders in `command` with the
/// corresponding values from `param_values` (parallel to `param_names`).
fn substitute_params(
    command: &str,
    param_names: Option<&[String]>,
    param_values: &[String],
) -> String {
    let mut result = command.to_string();
    if let Some(names) = param_names {
        for (name, value) in names.iter().zip(param_values.iter()) {
            let placeholder = format!("{{{{{}}}}}", name);
            result = result.replace(&placeholder, value);
        }
    }
    result
}
