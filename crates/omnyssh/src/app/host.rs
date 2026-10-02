//! Host list state: add/edit form, list view, popups, and the `App` methods
//! that create, update, delete, and connect to hosts.

use std::ops::Range;
use std::time::Duration;

use super::*;
use omnyssh_core::ssh::client::{HostSource, MonitorMode};
use omnyssh_core::ssh::tunnel::LocalForward;

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
    "Password (login, optional)",
    "Tags (comma-sep)",
    "Notes",
    "Monitoring (ssh | tcp | tcp:PORT)",
    "Port forwards (port:host:hostport, ...)",
    "Start tunnel on launch (y/n)",
    #[cfg(unix)]
    "Forward SSH agent (y/n)",
    // Kept so a hosts.toml shared with another machine round-trips it.
    #[cfg(not(unix))]
    "Forward SSH agent (y/n, not on Windows yet)",
];

/// Whether an edit changed anything a running poller reads. Everything else on
/// the form — tags, notes — is display-only, and restarting the pool for it drops
/// and re-authenticates every host's live monitoring session.
///
/// `original_ssh_host` is in here because it is read for *other* hosts: a jump chain
/// resolves a `ProxyJump` alias against it, so changing it changes where a different
/// host connects.
fn poller_inputs_changed(before: &Host, after: &Host) -> bool {
    before.name != after.name
        || before.hostname != after.hostname
        || before.user != after.user
        || before.port != after.port
        || before.identity_file != after.identity_file
        || before.password != after.password
        || before.proxy_jump != after.proxy_jump
        || before.original_ssh_host != after.original_ssh_host
        || before.monitoring != after.monitoring
        || before.monitor_port != after.monitor_port
}

/// Renders a host's monitoring mode back into its form field.
fn monitoring_value(host: &Host) -> String {
    match (host.monitoring, host.monitor_port) {
        (MonitorMode::Ssh, _) => String::new(),
        (MonitorMode::TcpPort, Some(port)) => format!("tcp:{port}"),
        (MonitorMode::TcpPort, None) => String::from("tcp"),
    }
}

/// Parses the monitoring field: empty or `ssh` keeps the SSH poller, `tcp`
/// probes the host's SSH port, `tcp:PORT` probes another one.
fn parse_monitoring(value: &str) -> Result<(MonitorMode, Option<u16>), String> {
    match value {
        "" | "ssh" => Ok((MonitorMode::Ssh, None)),
        "tcp" => Ok((MonitorMode::TcpPort, None)),
        other => other
            .strip_prefix("tcp:")
            .and_then(|p| p.parse::<u16>().ok())
            .filter(|&p| p != 0)
            .map(|p| (MonitorMode::TcpPort, Some(p)))
            .ok_or_else(|| format!("Monitoring must be 'ssh', 'tcp' or 'tcp:PORT', got '{other}'")),
    }
}

/// Renders a host's port forwards the way their form field holds them.
pub fn forwards_value(host: &Host) -> String {
    host.local_forwards
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parses the port forwards field: comma-separated `ssh -L` rules.
fn parse_forwards(value: &str) -> Result<Vec<LocalForward>, String> {
    let forwards: Vec<LocalForward> = value
        .split(',')
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
        .map(|rule| rule.parse().map_err(|e| format!("Port forward: {e}")))
        .collect::<Result<_, _>>()?;
    // Two rules on one local port would fail the whole tunnel when it binds.
    let mut listening = std::collections::HashSet::new();
    for forward in &forwards {
        let address = forward.bind_address.as_deref().unwrap_or("localhost");
        if !listening.insert((address, forward.bind_port)) {
            return Err(format!(
                "Port forward: two rules listen on port {}",
                forward.bind_port
            ));
        }
    }
    Ok(forwards)
}

/// Renders a y/n flag into its form field; off stays blank like the other
/// optional fields.
fn yes_no_value(on: bool) -> &'static str {
    if on {
        "y"
    } else {
        ""
    }
}

/// Parses a y/n field; `label` names it in the error.
fn parse_yes_no(label: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "n" | "no" => Ok(false),
        "y" | "yes" => Ok(true),
        _ => Err(format!("{label} must be y or n, got '{}'", value.trim())),
    }
}

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
        form.fields[8] = FormField::with_value(monitoring_value(host));
        form.fields[9] = FormField::with_value(forwards_value(host));
        form.fields[10] = FormField::with_value(yes_no_value(host.tunnel_autostart));
        form.fields[11] = FormField::with_value(yes_no_value(host.forward_agent));
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
                v.parse::<u16>().ok().filter(|&p| p != 0).ok_or_else(|| {
                    format!("Port must be a number between 1 and 65535, got '{v}'")
                })?
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

        let (monitoring, monitor_port) = parse_monitoring(self.fields[8].value.trim())?;
        let local_forwards = parse_forwards(&self.fields[9].value)?;
        let tunnel_autostart = parse_yes_no("Start tunnel on launch", &self.fields[10].value)?;
        let forward_agent = parse_yes_no("Forward SSH agent", &self.fields[11].value)?;

        Ok(Host {
            name,
            hostname,
            user,
            port,
            identity_file,
            identities_only: false,
            password,
            proxy_jump: None,
            tags,
            notes,
            source,
            original_ssh_host: None,
            monitoring,
            monitor_port,
            local_forwards,
            tunnel_autostart,
            forward_agent,
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
        host_name: String,
        current_step: Option<omnyssh_core::ssh::key_setup::KeySetupStep>,
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
    /// When true the grid is split into sections by each host's first tag.
    pub group_by_tag: bool,
    /// Sections of `filtered_indices` when `group_by_tag` is on (empty otherwise).
    pub groups: Vec<GroupHeader>,
}

/// One section of the grouped dashboard grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupHeader {
    /// Tag name, or `None` for the "Untagged" section.
    pub label: Option<String>,
    /// First position of the section within `filtered_indices`.
    pub start: usize,
    /// Number of cards in the section.
    pub len: usize,
}

impl GroupHeader {
    /// Display name of the section.
    pub fn title(&self) -> &str {
        self.label.as_deref().unwrap_or("Untagged")
    }
}

/// One visual row of the dashboard grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridRow {
    /// Section header; the value indexes `HostListView::groups`.
    Header(usize),
    /// A row of cards; the range indexes `filtered_indices`.
    Cards(Range<usize>),
}

impl HostListView {
    /// Returns the index into `AppState.hosts` for the selected filtered row.
    pub fn selected_host_idx(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected).copied()
    }

    /// Lays the filtered hosts out in rows of at most `cols` cards, with a
    /// header row before each section when grouping is on.
    pub fn grid_rows(&self, cols: usize) -> Vec<GridRow> {
        let cols = cols.max(1);
        let chunk = |start: usize, len: usize, rows: &mut Vec<GridRow>| {
            let end = start + len;
            let mut s = start;
            while s < end {
                let e = (s + cols).min(end);
                rows.push(GridRow::Cards(s..e));
                s = e;
            }
        };
        let mut rows = Vec::new();
        if self.group_by_tag {
            for (g, group) in self.groups.iter().enumerate() {
                rows.push(GridRow::Header(g));
                chunk(group.start, group.len, &mut rows);
            }
        } else {
            chunk(0, self.filtered_indices.len(), &mut rows);
        }
        rows
    }

    /// Moves the selection one step in `dir` over a grid of `cols` columns.
    /// Up/down jump to the same column of the previous/next card row, across
    /// section headers, clamping to shorter rows. Past the top or bottom row
    /// they land on the first or last card.
    pub fn navigate(&mut self, dir: &NavDir, cols: usize) {
        let len = self.filtered_indices.len();
        if len == 0 {
            return;
        }
        let sel = self.selected.min(len - 1);
        self.selected = match dir {
            NavDir::Left => sel.saturating_sub(1),
            NavDir::Right => (sel + 1).min(len - 1),
            NavDir::Up | NavDir::Down => {
                let rows: Vec<Range<usize>> = self
                    .grid_rows(cols)
                    .into_iter()
                    .filter_map(|r| match r {
                        GridRow::Cards(range) => Some(range),
                        GridRow::Header(_) => None,
                    })
                    .collect();
                let Some(cur) = rows.iter().position(|r| r.contains(&sel)) else {
                    return;
                };
                let col = sel - rows[cur].start;
                let target = match dir {
                    NavDir::Up => cur.checked_sub(1),
                    _ => Some(cur + 1).filter(|&t| t < rows.len()),
                };
                match target.and_then(|t| rows.get(t)) {
                    Some(row) => (row.start + col).min(row.end - 1),
                    None if matches!(dir, NavDir::Up) => 0,
                    None => len - 1,
                }
            }
        };
    }

    /// Flips grouping by tag, keeping the cursor on the same host.
    pub fn toggle_group_by_tag(
        &mut self,
        hosts: &[Host],
        metrics: &HashMap<String, Metrics>,
        statuses: &HashMap<String, ConnectionStatus>,
    ) {
        let kept = self.selected_host_idx();
        self.group_by_tag = !self.group_by_tag;
        self.rebuild_filter(hosts, metrics, statuses);
        if let Some(pos) = kept.and_then(|i| self.filtered_indices.iter().position(|&j| j == i)) {
            self.selected = pos;
        }
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

        // 4. Group by first tag, Untagged last. The sort is stable, so the
        // order from step 3 holds inside each section.
        self.groups.clear();
        if self.group_by_tag {
            indices.sort_by_cached_key(|&i| {
                let tag = group_tag(&hosts[i]);
                (tag.is_none(), tag.map(str::to_lowercase), tag)
            });
            for (pos, &i) in indices.iter().enumerate() {
                let tag = group_tag(&hosts[i]);
                match self.groups.last_mut() {
                    Some(group) if group.label.as_deref() == tag => group.len += 1,
                    _ => self.groups.push(GroupHeader {
                        label: tag.map(str::to_string),
                        start: pos,
                        len: 1,
                    }),
                }
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

/// The section a host goes under: its first non-blank tag.
fn group_tag(host: &Host) -> Option<&str> {
    host.tags.iter().map(|t| t.trim()).find(|t| !t.is_empty())
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

impl App {
    /// Handles the user confirming the Add or Edit host form.
    pub(crate) async fn handle_confirm_form(&mut self) {
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
                                self.core_tx.clone(),
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
                    let (old_name, _was_ssh_config, before) = {
                        let mut state = self.state.write().await;
                        let old_host = state.hosts.get(host_idx);
                        let old_name = old_host.map(|h| h.name.clone());
                        let before = old_host.cloned();
                        let was_ssh_config = old_host
                            .map(|h| h.source == HostSource::SshConfig)
                            .unwrap_or(false);

                        // ProxyJump has no form field, so carry it over: editing
                        // an imported host would otherwise drop its bastion and
                        // the saved copy would try to connect direct.
                        host.proxy_jump = old_host.and_then(|h| h.proxy_jump.clone());
                        // Nor has IdentitiesOnly, and losing it brings back a login
                        // that offers every agent key.
                        host.identities_only = old_host.is_some_and(|h| h.identities_only);

                        // An import is adopted under the name it was imported by; a copy
                        // already adopted keeps the one it carries. Dropping it brings the
                        // import back as a duplicate card, and takes with it the alias any
                        // other host's `ProxyJump` resolves through.
                        host.original_ssh_host = if was_ssh_config {
                            old_name.clone()
                        } else {
                            old_host.and_then(|h| h.original_ssh_host.clone())
                        };

                        if let Some(slot) = state.hosts.get_mut(host_idx) {
                            *slot = host.clone();
                        }
                        (old_name, was_ssh_config, before)
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
                        }
                    }

                    self.save_manual_hosts().await;
                    self.sync_tunnels().await;

                    // The edit can change the address, the port or the monitoring
                    // mode, none of which a running poller picks up. The pool has no
                    // per-host restart, so this costs every other host its session —
                    // only worth it when a poller input actually moved.
                    if before
                        .as_ref()
                        .is_none_or(|b| poller_inputs_changed(b, &host))
                    {
                        let state = self.state.read().await;
                        if let Some(old) = self.poll_manager.take() {
                            old.shutdown();
                        }
                        self.poll_manager = Some(PollManager::start(
                            state.hosts.clone(),
                            self.core_tx.clone(),
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
    pub(crate) async fn handle_confirm_delete(&mut self) {
        if let Some(HostPopup::DeleteConfirm(idx)) = self.view.host_list.popup.take() {
            {
                let mut state = self.state.write().await;
                if idx < state.hosts.len() {
                    let removed = state.hosts.remove(idx);
                    self.view.status_message = Some(format!("Deleted '{}'.", removed.name));
                }
            }
            self.save_manual_hosts().await;
            self.sync_tunnels().await;
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
    pub(crate) async fn save_manual_hosts(&mut self) {
        let hosts = self.state.read().await.hosts.clone();
        if let Err(e) = config::save_hosts(&hosts) {
            self.view.status_message = Some(format!("Save failed: {e}"));
        }
    }

    /// Brings the running tunnels in line with the host list. A failed tunnel
    /// reports nothing more, so its status is dropped here once its host is
    /// gone — a later host of that name must not inherit it.
    pub(crate) async fn sync_tunnels(&mut self) {
        let mut state = self.state.write().await;
        if let Some(tunnels) = &mut self.tunnel_manager {
            tunnels.sync(&state.hosts);
        }
        let AppState {
            hosts,
            tunnel_statuses,
            ..
        } = &mut *state;
        tunnel_statuses.retain(|name, _| hosts.iter().any(|h| &h.name == name));
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    /// Builds a host form from the field values, in `FORM_FIELD_LABELS` order.
    fn host_form(values: [&str; 8]) -> HostForm {
        let mut form = HostForm::empty();
        for (i, v) in values.iter().enumerate() {
            form.fields[i] = FormField::with_value(*v);
        }
        form
    }

    #[test]
    fn the_monitoring_field_round_trips_through_the_form() {
        for (text, mode, port) in [
            ("", MonitorMode::Ssh, None),
            ("ssh", MonitorMode::Ssh, None),
            ("tcp", MonitorMode::TcpPort, None),
            ("tcp:8443", MonitorMode::TcpPort, Some(8443)),
        ] {
            let parsed = parse_monitoring(text).expect("valid monitoring value");
            assert_eq!(parsed, (mode, port), "parsing '{text}'");

            let host = Host {
                monitoring: mode,
                monitor_port: port,
                ..Host::default()
            };
            assert_eq!(parse_monitoring(&monitoring_value(&host)), Ok((mode, port)));
        }
    }

    #[test]
    fn only_a_connection_field_edit_restarts_the_pool() {
        let base = Host {
            name: String::from("web"),
            hostname: String::from("10.0.0.1"),
            ..Host::default()
        };

        // Display-only edits: the running poller would produce the same session.
        let mut described = base.clone();
        described.tags = vec![String::from("prod")];
        described.notes = Some(String::from("the billing box"));
        assert!(!poller_inputs_changed(&base, &described));

        // The guard holds only because the form round-trips every compared field
        // untouched. Masking the password field — a plausible hardening — would make
        // each of these hosts look edited and quietly restore the old behaviour.
        let populated = Host {
            user: String::from("deploy"),
            port: 2222,
            identity_file: Some(String::from("~/.ssh/id_ed25519")),
            password: Some(String::from("hunter2")),
            tags: vec![String::from("prod"), String::from("web")],
            notes: Some(String::from("the billing box")),
            monitoring: MonitorMode::TcpPort,
            monitor_port: Some(8443),
            ..base.clone()
        };
        let reopened = HostForm::from_host(&populated)
            .to_host(HostSource::Manual)
            .expect("the form round-trips a valid host");
        assert!(
            !poller_inputs_changed(&populated, &reopened),
            "opening and confirming the form unchanged must not restart the pool"
        );

        // Everything a poller dials, authenticates or watches with.
        let moved = [
            Host {
                name: String::from("web-1"),
                ..base.clone()
            },
            Host {
                hostname: String::from("10.0.0.2"),
                ..base.clone()
            },
            Host {
                user: String::from("deploy"),
                ..base.clone()
            },
            Host {
                port: 2222,
                ..base.clone()
            },
            Host {
                identity_file: Some(String::from("~/.ssh/id_ed25519")),
                ..base.clone()
            },
            Host {
                password: Some(String::from("hunter2")),
                ..base.clone()
            },
            Host {
                proxy_jump: Some(String::from("bastion")),
                ..base.clone()
            },
            Host {
                monitoring: MonitorMode::TcpPort,
                ..base.clone()
            },
            Host {
                monitor_port: Some(8443),
                ..base.clone()
            },
        ];
        for after in moved {
            assert!(
                poller_inputs_changed(&base, &after),
                "a changed poller input went unnoticed"
            );
        }
    }

    fn forwarded_host() -> Host {
        Host {
            name: String::from("web"),
            hostname: String::from("10.0.0.1"),
            local_forwards: [
                "9443:127.0.0.1:9443",
                "0.0.0.0:5432:db.internal:5432",
                "[::1]:8080:[fe80::1]:80",
            ]
            .iter()
            .map(|rule| rule.parse().expect("valid rule"))
            .collect(),
            tunnel_autostart: true,
            ..Host::default()
        }
    }

    #[test]
    fn the_forwards_field_round_trips_through_the_form() {
        let host = forwarded_host();
        assert_eq!(
            forwards_value(&host),
            "9443:127.0.0.1:9443, 0.0.0.0:5432:db.internal:5432, [::1]:8080:[fe80::1]:80"
        );
        assert_eq!(
            parse_forwards(&forwards_value(&host)),
            Ok(host.local_forwards)
        );
        assert_eq!(
            parse_forwards(" 9443:localhost:9443 , ,"),
            Ok(vec!["9443:localhost:9443".parse().unwrap()])
        );
        assert_eq!(parse_forwards(""), Ok(Vec::new()));
    }

    #[test]
    fn two_rules_on_one_local_port_are_rejected() {
        for text in ["8080:a:80, 8080:b:80", "8080:a:80, localhost:8080:b:80"] {
            assert_eq!(
                parse_forwards(text),
                Err(String::from("Port forward: two rules listen on port 8080"))
            );
        }
        // The same port on two separate addresses is two sockets.
        assert!(parse_forwards("10.0.0.5:8080:a:80, 10.0.0.6:8080:b:80").is_ok());
    }

    #[test]
    fn a_bad_forward_is_rejected_as_one() {
        for text in ["9443", "9443:localhost:0", "9443:localhost:9443, nonsense"] {
            let err = parse_forwards(text).expect_err(text);
            assert!(err.starts_with("Port forward: "), "'{text}' gave '{err}'");
        }
    }

    #[test]
    fn the_autostart_field_round_trips_through_the_form() {
        for (text, on) in [
            ("", false),
            ("n", false),
            (" No ", false),
            ("y", true),
            ("YES", true),
        ] {
            assert_eq!(
                parse_yes_no("Start tunnel on launch", text),
                Ok(on),
                "parsing '{text}'"
            );
        }
        for on in [false, true] {
            let host = Host {
                tunnel_autostart: on,
                ..Host::default()
            };
            let form = HostForm::from_host(&host);
            assert_eq!(
                parse_yes_no("Start tunnel on launch", &form.fields[10].value),
                Ok(on)
            );
        }
        for text in ["maybe", "1", "true"] {
            assert_eq!(
                parse_yes_no("Start tunnel on launch", text),
                Err(format!(
                    "Start tunnel on launch must be y or n, got '{text}'"
                ))
            );
        }
    }

    #[test]
    fn the_agent_field_round_trips_and_edits() {
        let host = Host {
            name: String::from("lab"),
            hostname: String::from("10.0.0.7"),
            forward_agent: true,
            ..Host::default()
        };
        let parsed = |form: &HostForm| form.to_host(HostSource::Manual).map(|h| h.forward_agent);
        let mut form = HostForm::from_host(&host);
        assert_eq!(form.fields[11].value, "y");
        assert_eq!(parsed(&form), Ok(true));

        form.fields[11] = FormField::with_value("n");
        assert_eq!(parsed(&form), Ok(false));

        form.fields[11] = FormField::with_value("sometimes");
        let error = "Forward SSH agent must be y or n, got 'sometimes'";
        assert_eq!(parsed(&form), Err(String::from(error)));
    }

    #[test]
    fn an_edit_keeps_the_forwards_and_autostart() {
        // Forwards set in the GUI or read from ~/.ssh/config have to survive an
        // edit that never touches them.
        let host = forwarded_host();
        let edited = HostForm::from_host(&host)
            .to_host(HostSource::Manual)
            .expect("the form round-trips a valid host");
        assert_eq!(edited.local_forwards, host.local_forwards);
        assert!(edited.tunnel_autostart);

        let mut form = HostForm::from_host(&host);
        form.fields[9] = FormField::with_value("9443:localhost:9443, 99999:localhost:1");
        assert!(form.to_host(HostSource::Manual).is_err());
    }

    #[test]
    fn a_tunnel_edit_does_not_restart_the_pool() {
        let base = Host {
            name: String::from("web"),
            hostname: String::from("10.0.0.1"),
            ..Host::default()
        };
        assert!(!poller_inputs_changed(&base, &forwarded_host()));
    }

    #[test]
    fn an_unusable_monitoring_value_is_rejected() {
        for text in ["tcp:0", "tcp:99999", "http", "tcp:"] {
            assert!(
                parse_monitoring(text).is_err(),
                "'{text}' should be rejected"
            );
        }
    }

    // --- HostForm::to_host (P0.2) -----------------------------------------

    #[test]
    fn to_host_empty_name_errs() {
        let form = host_form(["", "h", "u", "22", "", "", "", ""]);
        assert!(form.to_host(HostSource::Manual).is_err());
    }

    #[test]
    fn to_host_whitespace_name_errs() {
        let form = host_form(["   ", "h", "u", "22", "", "", "", ""]);
        assert!(form.to_host(HostSource::Manual).is_err());
    }

    #[test]
    fn to_host_empty_hostname_errs() {
        let form = host_form(["n", "", "u", "22", "", "", "", ""]);
        assert!(form.to_host(HostSource::Manual).is_err());
    }

    #[test]
    fn to_host_empty_user_defaults_root() {
        let host = host_form(["n", "h", "", "22", "", "", "", ""])
            .to_host(HostSource::Manual)
            .unwrap();
        assert_eq!(host.user, "root");
    }

    #[test]
    fn to_host_whitespace_user_defaults_root() {
        let host = host_form(["n", "h", "  ", "22", "", "", "", ""])
            .to_host(HostSource::Manual)
            .unwrap();
        assert_eq!(host.user, "root");
    }

    #[test]
    fn to_host_empty_port_defaults_22() {
        let host = host_form(["n", "h", "u", "", "", "", "", ""])
            .to_host(HostSource::Manual)
            .unwrap();
        assert_eq!(host.port, 22);
    }

    #[test]
    fn to_host_valid_port_parsed() {
        let host = host_form(["n", "h", "u", "2222", "", "", "", ""])
            .to_host(HostSource::Manual)
            .unwrap();
        assert_eq!(host.port, 2222);
    }

    #[test]
    fn to_host_port_max_accepted() {
        let host = host_form(["n", "h", "u", "65535", "", "", "", ""])
            .to_host(HostSource::Manual)
            .unwrap();
        assert_eq!(host.port, 65535);
    }

    #[test]
    fn to_host_port_zero_errs() {
        // Port 0 is invalid for SSH; the form rejects it.
        assert!(host_form(["n", "h", "u", "0", "", "", "", ""])
            .to_host(HostSource::Manual)
            .is_err());
    }

    #[test]
    fn to_host_port_overflow_errs() {
        let err = host_form(["n", "h", "u", "65536", "", "", "", ""])
            .to_host(HostSource::Manual)
            .unwrap_err();
        assert!(err.contains("between 1 and 65535"));
    }

    #[test]
    fn to_host_port_non_numeric_errs() {
        assert!(host_form(["n", "h", "u", "abc", "", "", "", ""])
            .to_host(HostSource::Manual)
            .is_err());
    }

    #[test]
    fn to_host_port_negative_errs() {
        assert!(host_form(["n", "h", "u", "-1", "", "", "", ""])
            .to_host(HostSource::Manual)
            .is_err());
    }

    #[test]
    fn to_host_optional_fields_none_when_empty() {
        let host = host_form(["n", "h", "u", "22", "", "", "", ""])
            .to_host(HostSource::Manual)
            .unwrap();
        assert!(host.identity_file.is_none());
        assert!(host.password.is_none());
        assert!(host.notes.is_none());
        assert!(host.tags.is_empty());
    }

    #[test]
    fn to_host_tags_split_and_trimmed() {
        let host = host_form(["n", "h", "u", "22", "", "", "a, b ,c,,", ""])
            .to_host(HostSource::Manual)
            .unwrap();
        assert_eq!(host.tags, vec!["a", "b", "c"]);
    }

    #[test]
    fn to_host_source_and_metadata_propagated() {
        let host = host_form(["n", "h", "u", "22", "", "", "", ""])
            .to_host(HostSource::SshConfig)
            .unwrap();
        assert_eq!(host.source, HostSource::SshConfig);
        assert!(host.original_ssh_host.is_none());
        assert!(host.key_setup_date.is_none());
        assert!(host.password_auth_disabled.is_none());
    }

    // --- FormField insert/backspace, UTF-8 safety (P1.1) ------------------

    #[test]
    fn formfield_insert_ascii() {
        let mut f = FormField::default();
        f.insert_char('a');
        assert_eq!(f.value, "a");
        assert_eq!(f.cursor, 1);
    }

    #[test]
    fn formfield_insert_multibyte_advances_cursor_by_byte_len() {
        let mut f = FormField::default();
        f.insert_char('é');
        assert_eq!(f.value, "é");
        assert_eq!(f.cursor, 2);
    }

    #[test]
    fn formfield_insert_emoji_advances_cursor_by_four() {
        let mut f = FormField::default();
        f.insert_char('😀');
        assert_eq!(f.cursor, 4);
    }

    #[test]
    fn formfield_insert_at_start() {
        let mut f = FormField::with_value("bc");
        f.cursor = 0;
        f.insert_char('a');
        assert_eq!(f.value, "abc");
        assert_eq!(f.cursor, 1);
    }

    #[test]
    fn formfield_insert_on_multibyte_boundary_stays_valid() {
        // Cursor sits between 'a' and 'é' — a valid char boundary.
        let mut f = FormField::with_value("aé");
        f.cursor = 1;
        f.insert_char('X');
        assert_eq!(f.value, "aXé");
    }

    #[test]
    fn formfield_backspace_ascii() {
        let mut f = FormField::with_value("ab");
        f.backspace();
        assert_eq!(f.value, "a");
        assert_eq!(f.cursor, 1);
    }

    #[test]
    fn formfield_backspace_removes_whole_multibyte_char() {
        let mut f = FormField::with_value("é");
        f.backspace();
        assert_eq!(f.value, "");
        assert_eq!(f.cursor, 0);
    }

    #[test]
    fn formfield_backspace_at_zero_is_noop() {
        let mut f = FormField::default();
        f.backspace();
        assert_eq!(f.value, "");
        assert_eq!(f.cursor, 0);
    }

    #[test]
    fn formfield_with_value_cursor_at_byte_len() {
        let f = FormField::with_value("héllo");
        assert_eq!(f.cursor, 6);
    }

    // --- filter_hosts (P1.3) ----------------------------------------------

    fn host(name: &str, hostname: &str, tags: &[&str], notes: Option<&str>) -> Host {
        Host {
            name: name.to_string(),
            hostname: hostname.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            notes: notes.map(|n| n.to_string()),
            ..Host::default()
        }
    }

    #[test]
    fn filter_hosts_empty_query_returns_all() {
        let hosts = [host("a", "1", &[], None), host("b", "2", &[], None)];
        assert_eq!(filter_hosts(&hosts, ""), vec![0, 1]);
    }

    #[test]
    fn filter_hosts_matches_name() {
        let hosts = [host("web-prod", "1", &[], None), host("db", "2", &[], None)];
        assert_eq!(filter_hosts(&hosts, "web"), vec![0]);
    }

    #[test]
    fn filter_hosts_matches_hostname() {
        let hosts = [
            host("a", "10.0.0.1", &[], None),
            host("b", "10.0.0.2", &[], None),
        ];
        assert_eq!(filter_hosts(&hosts, "0.0.1"), vec![0]);
    }

    #[test]
    fn filter_hosts_matches_tag() {
        let hosts = [
            host("a", "1", &["prod"], None),
            host("b", "2", &["dev"], None),
        ];
        assert_eq!(filter_hosts(&hosts, "prod"), vec![0]);
    }

    #[test]
    fn filter_hosts_matches_notes() {
        let hosts = [
            host("a", "1", &[], Some("bastion host")),
            host("b", "2", &[], None),
        ];
        assert_eq!(filter_hosts(&hosts, "bastion"), vec![0]);
    }

    #[test]
    fn filter_hosts_case_insensitive() {
        let hosts = [host("Web-Prod", "1", &[], None)];
        assert_eq!(filter_hosts(&hosts, "WEB"), vec![0]);
    }

    #[test]
    fn filter_hosts_no_match_returns_empty() {
        let hosts = [host("a", "1", &[], None)];
        assert!(filter_hosts(&hosts, "zzz").is_empty());
    }

    // --- HostListView::rebuild_filter and selection (P1.4) ----------------

    fn filtered_names<'a>(view: &HostListView, hosts: &'a [Host]) -> Vec<&'a str> {
        view.filtered_indices
            .iter()
            .map(|&i| hosts[i].name.as_str())
            .collect()
    }

    fn metrics_cpu(value: f64) -> Metrics {
        Metrics {
            cpu_percent: Some(value),
            ..Metrics::default()
        }
    }

    fn metrics_ram(value: f64) -> Metrics {
        Metrics {
            ram_percent: Some(value),
            ..Metrics::default()
        }
    }

    #[test]
    fn rebuild_sort_name_alphabetical() {
        let hosts = [
            host("c", "1", &[], None),
            host("a", "2", &[], None),
            host("b", "3", &[], None),
        ];
        let mut view = HostListView::default();
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(filtered_names(&view, &hosts), ["a", "b", "c"]);
    }

    #[test]
    fn rebuild_sort_cpu_descending_missing_metrics_last() {
        let hosts = [
            host("a", "1", &[], None),
            host("b", "2", &[], None),
            host("c", "3", &[], None),
        ];
        let metrics = HashMap::from([
            ("a".to_string(), metrics_cpu(90.0)),
            ("b".to_string(), metrics_cpu(10.0)),
        ]);
        let mut view = HostListView {
            sort_order: SortOrder::Cpu,
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &metrics, &HashMap::new());
        assert_eq!(filtered_names(&view, &hosts), ["a", "b", "c"]);
    }

    #[test]
    fn rebuild_sort_ram_descending() {
        let hosts = [host("a", "1", &[], None), host("b", "2", &[], None)];
        let metrics = HashMap::from([
            ("a".to_string(), metrics_ram(20.0)),
            ("b".to_string(), metrics_ram(80.0)),
        ]);
        let mut view = HostListView {
            sort_order: SortOrder::Ram,
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &metrics, &HashMap::new());
        assert_eq!(filtered_names(&view, &hosts), ["b", "a"]);
    }

    #[test]
    fn rebuild_sort_status_priority() {
        let hosts = [
            host("a", "1", &[], None),
            host("b", "2", &[], None),
            host("c", "3", &[], None),
            host("d", "4", &[], None),
        ];
        let statuses = HashMap::from([
            ("a".to_string(), ConnectionStatus::Failed("x".to_string())),
            ("b".to_string(), ConnectionStatus::Connected),
            ("c".to_string(), ConnectionStatus::Connecting),
        ]);
        let mut view = HostListView {
            sort_order: SortOrder::Status,
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &HashMap::new(), &statuses);
        // Connected, Connecting, Unknown/None, Failed.
        assert_eq!(filtered_names(&view, &hosts), ["b", "c", "d", "a"]);
    }

    #[test]
    fn rebuild_tag_filter_includes_only_matching() {
        let hosts = [
            host("a", "1", &["prod"], None),
            host("b", "2", &["dev"], None),
        ];
        let mut view = HostListView {
            tag_filter: Some("prod".to_string()),
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(filtered_names(&view, &hosts), ["a"]);
    }

    fn group_titles(view: &HostListView) -> Vec<(String, usize)> {
        view.groups
            .iter()
            .map(|g| (g.title().to_string(), g.len))
            .collect()
    }

    #[test]
    fn rebuild_group_by_first_tag_with_untagged_last() {
        let hosts = [
            host("web", "1", &["prod", "Web"], None),
            host("lab", "2", &[], None),
            host("db", "3", &["prod"], None),
            host("dev1", "4", &["dev"], None),
            host("www", "5", &["Web"], None),
        ];
        let mut view = HostListView {
            group_by_tag: true,
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(
            group_titles(&view),
            [
                ("dev".to_string(), 1),
                ("prod".to_string(), 2),
                ("Web".to_string(), 1),
                ("Untagged".to_string(), 1),
            ]
        );
        // One card per host; the name sort holds inside a section.
        assert_eq!(
            filtered_names(&view, &hosts),
            ["dev1", "db", "web", "www", "lab"]
        );
    }

    #[test]
    fn rebuild_group_skips_blank_and_repeated_tags() {
        let hosts = [
            host("a", "1", &["", " db "], None),
            host("b", "2", &["  "], None),
            host("c", "3", &["db", "db"], None),
        ];
        let mut view = HostListView {
            group_by_tag: true,
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(
            group_titles(&view),
            [("db".to_string(), 2), ("Untagged".to_string(), 1)]
        );
        assert_eq!(filtered_names(&view, &hosts), ["a", "c", "b"]);
    }

    #[test]
    fn rebuild_group_by_tag_within_the_tag_filter() {
        let hosts = [
            host("web", "1", &["web", "prod"], None),
            host("dev1", "2", &["dev"], None),
            host("db", "3", &["prod"], None),
        ];
        let mut view = HostListView {
            group_by_tag: true,
            tag_filter: Some("prod".to_string()),
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(
            group_titles(&view),
            [("prod".to_string(), 1), ("web".to_string(), 1)]
        );
        assert_eq!(filtered_names(&view, &hosts), ["db", "web"]);
    }

    #[test]
    fn toggling_group_by_tag_keeps_the_selected_host() {
        let hosts = [
            host("a", "1", &["z"], None),
            host("b", "2", &[], None),
            host("c", "3", &["m"], None),
        ];
        let mut view = HostListView::default();
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        view.selected = 0; // "a": first when flat, second when grouped
        view.toggle_group_by_tag(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(filtered_names(&view, &hosts), ["c", "a", "b"]);
        assert_eq!(view.selected, 1);
        view.toggle_group_by_tag(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn rebuild_without_grouping_has_no_sections() {
        let hosts = [host("a", "1", &["prod"], None)];
        let mut view = HostListView::default();
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        assert!(view.groups.is_empty());
    }

    /// 3 hosts in "a", 1 in "b", 2 untagged.
    fn grouped_view() -> (Vec<Host>, HostListView) {
        let hosts = vec![
            host("a1", "1", &["a"], None),
            host("a2", "2", &["a"], None),
            host("a3", "3", &["a"], None),
            host("b1", "4", &["b"], None),
            host("u1", "5", &[], None),
            host("u2", "6", &[], None),
        ];
        let mut view = HostListView {
            group_by_tag: true,
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        (hosts, view)
    }

    #[test]
    fn grid_rows_puts_headers_before_each_section() {
        let (_, view) = grouped_view();
        assert_eq!(
            view.grid_rows(2),
            [
                GridRow::Header(0),
                GridRow::Cards(0..2),
                GridRow::Cards(2..3),
                GridRow::Header(1),
                GridRow::Cards(3..4),
                GridRow::Header(2),
                GridRow::Cards(4..6),
            ]
        );
    }

    #[test]
    fn grid_rows_flat_when_not_grouped() {
        let view = HostListView {
            filtered_indices: vec![0, 1, 2],
            ..Default::default()
        };
        assert_eq!(
            view.grid_rows(2),
            [GridRow::Cards(0..2), GridRow::Cards(2..3)]
        );
    }

    #[test]
    fn navigate_down_crosses_sections_and_clamps_column() {
        let (_, mut view) = grouped_view();
        view.selected = 1; // a2, column 1
        view.navigate(&NavDir::Down, 2);
        assert_eq!(view.selected, 2); // a3, short row: clamped to column 0
        view.navigate(&NavDir::Down, 2);
        assert_eq!(view.selected, 3); // b1, next section
        view.navigate(&NavDir::Down, 2);
        assert_eq!(view.selected, 4); // u1
        view.navigate(&NavDir::Down, 2);
        assert_eq!(view.selected, 5); // past the last row: the last card
    }

    #[test]
    fn navigate_up_keeps_column_across_sections() {
        let (_, mut view) = grouped_view();
        view.selected = 5; // u2, column 1
        view.navigate(&NavDir::Up, 2);
        assert_eq!(view.selected, 3); // b1 row has one card
        view.navigate(&NavDir::Up, 2);
        assert_eq!(view.selected, 2);
        view.navigate(&NavDir::Up, 2);
        assert_eq!(view.selected, 0);
        view.navigate(&NavDir::Up, 2);
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn navigate_left_right_walk_positions() {
        let (_, mut view) = grouped_view();
        view.selected = 2;
        view.navigate(&NavDir::Right, 2);
        assert_eq!(view.selected, 3);
        view.navigate(&NavDir::Left, 2);
        view.navigate(&NavDir::Left, 2);
        assert_eq!(view.selected, 1);
    }

    #[test]
    fn navigate_flat_grid_moves_by_columns() {
        let mut view = HostListView {
            filtered_indices: vec![0, 1, 2, 3, 4],
            ..Default::default()
        };
        view.navigate(&NavDir::Down, 3);
        assert_eq!(view.selected, 3);
        view.selected = 2;
        view.navigate(&NavDir::Down, 3);
        assert_eq!(view.selected, 4); // clamped to the shorter last row
        view.selected = 3;
        view.navigate(&NavDir::Down, 3);
        assert_eq!(view.selected, 4); // past the last row: the last card
        view.selected = 2;
        view.navigate(&NavDir::Up, 3);
        assert_eq!(view.selected, 0); // past the first row: the first card
    }

    #[test]
    fn navigate_noop_when_empty() {
        let mut view = HostListView::default();
        view.navigate(&NavDir::Down, 3);
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn rebuild_tag_filter_none_shows_all() {
        let hosts = [
            host("a", "1", &["prod"], None),
            host("b", "2", &["dev"], None),
        ];
        let mut view = HostListView::default();
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(view.filtered_indices.len(), 2);
    }

    #[test]
    fn rebuild_combines_text_tag_and_sort() {
        let hosts = [
            host("web1", "1", &["prod"], None),
            host("web2", "2", &["dev"], None),
            host("db1", "3", &["prod"], None),
        ];
        let mut view = HostListView {
            search_query: "web".to_string(),
            tag_filter: Some("prod".to_string()),
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(filtered_names(&view, &hosts), ["web1"]);
    }

    #[test]
    fn rebuild_clamps_selection_when_list_shrinks() {
        let hosts = [host("a", "1", &[], None), host("b", "2", &[], None)];
        let mut view = HostListView {
            selected: 5,
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(view.selected, 1);
    }

    #[test]
    fn rebuild_resets_selection_to_zero_when_empty() {
        let hosts = [host("a", "1", &[], None)];
        let mut view = HostListView {
            selected: 3,
            search_query: "zzz".to_string(),
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn rebuild_preserves_selection_in_range() {
        let hosts = [
            host("a", "1", &[], None),
            host("b", "2", &[], None),
            host("c", "3", &[], None),
        ];
        let mut view = HostListView {
            selected: 1,
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(view.selected, 1);
    }

    #[test]
    fn rebuild_handles_shrunk_host_list_without_panic() {
        let many = [
            host("a", "1", &[], None),
            host("b", "2", &[], None),
            host("c", "3", &[], None),
        ];
        let mut view = HostListView::default();
        view.rebuild_filter(&many, &HashMap::new(), &HashMap::new());
        let few = [host("a", "1", &[], None)];
        view.rebuild_filter(&few, &HashMap::new(), &HashMap::new());
        assert!(view.filtered_indices.iter().all(|&i| i < few.len()));
    }

    #[test]
    fn rebuild_cpu_sort_with_empty_metrics_does_not_panic() {
        let hosts = [host("a", "1", &[], None), host("b", "2", &[], None)];
        let mut view = HostListView {
            sort_order: SortOrder::Cpu,
            ..Default::default()
        };
        view.rebuild_filter(&hosts, &HashMap::new(), &HashMap::new());
        assert_eq!(view.filtered_indices.len(), 2);
    }

    #[test]
    fn host_select_next_clamps_at_last() {
        let mut view = HostListView {
            filtered_indices: vec![0, 1],
            ..Default::default()
        };
        view.select_next();
        view.select_next();
        view.select_next();
        assert_eq!(view.selected, 1);
    }

    #[test]
    fn host_select_prev_saturates_at_zero() {
        let mut view = HostListView {
            filtered_indices: vec![0, 1],
            ..Default::default()
        };
        view.select_prev();
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn host_select_next_noop_when_empty() {
        let mut view = HostListView::default();
        view.select_next();
        assert_eq!(view.selected, 0);
    }
}
