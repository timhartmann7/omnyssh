//! Dashboard screen — grid of server cards with live metrics.
//!
//! Layout (schematic):
//!
//! ```text
//! ┌ header bar (1 line) ──────────────────────────────────────┐
//! │ Dashboard  [sort: name]  [filter: prod]  r:refresh s:sort  │
//! ├───────────────────────────────────────────────────────────┤
//! │ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐       │
//! │ │ web-prod-1 ● │ │ db-master  ◐ │ │ staging    ✗ │       │
//! │ │ ...          │ │ ...          │ │ ...          │       │
//! │ └──────────────┘ └──────────────┘ └──────────────┘       │
//! └───────────────────────────────────────────────────────────┘
//! ```
//!
//! All host-form popups (add / edit / delete) are still handled via the
//! [`host_list`] module and rendered on top of the grid unchanged.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

use crate::app::{AppAction, AppState, GridRow, GroupHeader, NavDir, ViewState};
use crate::ui::card::{render_card, CardData, CARD_HEIGHT, CARD_MIN_WIDTH};
use crate::ui::{host_list, popup};

const CARD_GAP: u16 = 1;
/// Height of a section title row when the grid is grouped by tag.
const GROUP_HEADER_HEIGHT: u16 = 1;

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Renders the Dashboard screen.
pub fn render(frame: &mut Frame, area: Rect, state: &AppState, view: &ViewState) {
    // Minimum terminal size guard.
    if area.width < 40 || area.height < 10 {
        frame.render_widget(
            Paragraph::new("Terminal too small for dashboard.")
                .style(Style::default().fg(view.theme.text_error)),
            area,
        );
        return;
    }

    // Split: header (1 line) + grid.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    let header_area = chunks[0];
    let grid_area = chunks[1];

    render_header(frame, header_area, state, view);
    render_grid(frame, grid_area, state, view);

    // Overlay host-form popups (add / edit / delete confirm).
    // Tag popup is handled separately below.
    let hlv = &view.host_list;
    if let Some(popup) = &hlv.popup {
        use crate::app::HostPopup;
        match popup {
            HostPopup::Add(form) => popup::render_host_form(frame, form, "Add Host", &view.theme),
            HostPopup::Edit { form, .. } => {
                popup::render_host_form(frame, form, "Edit Host", &view.theme)
            }
            HostPopup::DeleteConfirm(idx) => {
                let name = state
                    .hosts
                    .get(*idx)
                    .map(|h| h.name.as_str())
                    .unwrap_or("?");
                popup::render_delete_confirm(frame, name, &view.theme);
            }
            HostPopup::KeySetupConfirm(idx) => {
                let host = state.hosts.get(*idx);
                popup::render_key_setup_confirm(frame, host, &view.theme);
            }
            HostPopup::KeySetupProgress {
                host_name,
                current_step,
                ..
            } => {
                popup::render_key_setup_progress(
                    frame,
                    host_name,
                    current_step.as_ref(),
                    &view.theme,
                );
            }
        }
    }

    // Tag filter popup.
    if hlv.tag_popup_open {
        popup::render_tag_filter_popup(
            frame,
            &hlv.available_tags,
            hlv.tag_popup_selected,
            hlv.tag_filter.as_deref(),
            &view.theme,
        );
    }
    // Note: help popup is rendered by ui::mod on top of everything.
}

// ---------------------------------------------------------------------------
// Header bar
// ---------------------------------------------------------------------------

fn render_header(frame: &mut Frame, area: Rect, state: &AppState, view: &ViewState) {
    let hlv = &view.host_list;
    let mut spans = vec![
        Span::styled(
            " Dashboard ",
            Style::default()
                .fg(view.theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("[sort: {}]", hlv.sort_order.label()),
            Style::default().fg(view.theme.accent),
        ),
        Span::styled("  ", Style::default()),
    ];
    if let Some(tag) = &hlv.tag_filter {
        spans.push(Span::styled(
            format!("[filter: {}]", tag),
            Style::default().fg(view.theme.text_warning),
        ));
        spans.push(Span::styled("  ", Style::default()));
    }
    if !hlv.search_query.is_empty() {
        spans.push(Span::styled(
            format!("[search: {}]", hlv.search_query),
            Style::default().fg(view.theme.highlight),
        ));
        spans.push(Span::styled("  ", Style::default()));
    }

    // Build key hints.
    let mut hints =
        String::from("r:refresh  s:sort  t:tags  g:group  /:search  a:add  x:execute  f:tunnel");

    // Check if selected host needs SSH key setup.
    // Show "Shift+K:ssh-setup" hint if selected host has password but no identity_file.
    if let Some(idx) = hlv.selected_host_idx() {
        if let Some(host) = state.hosts.get(idx) {
            if host.password.is_some() && host.identity_file.is_none() {
                hints.push_str("  Shift+K:ssh-setup");
            }
        }
    }

    spans.push(Span::styled(
        hints,
        Style::default().fg(view.theme.text_muted),
    ));

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Reset)),
        area,
    );
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

fn render_grid(frame: &mut Frame, area: Rect, state: &AppState, view: &ViewState) {
    let hlv = &view.host_list;

    if state.hosts.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "\n  No hosts configured.\n\n  Press  a  to add a host, or  r  to reload from ~/.ssh/config.",
            )
            .style(Style::default().fg(view.theme.text_muted)),
            area,
        );
        return;
    }

    if hlv.filtered_indices.is_empty() {
        frame.render_widget(
            Paragraph::new("\n  No hosts match the current filter.")
                .style(Style::default().fg(view.theme.text_muted)),
            area,
        );
        return;
    }

    // Guard against a terminal that is too short to show even one card row.
    if area.height < CARD_HEIGHT + 2 {
        return;
    }

    // Compute number of columns.
    let cols = compute_columns(area.width);
    let card_w = compute_card_width(area.width, cols);

    let selected = hlv.selected;
    let rows = hlv.grid_rows(cols as usize);
    let row_height = |row: &GridRow| match row {
        GridRow::Header(_) => GROUP_HEADER_HEIGHT,
        GridRow::Cards(_) => CARD_HEIGHT + CARD_GAP,
    };

    // Compute scroll: the first visible row is the earliest one from which
    // the selected card row still fits (pulling its section header into view
    // when there is room).
    let selected_row = rows
        .iter()
        .position(|r| matches!(r, GridRow::Cards(range) if range.contains(&selected)))
        .unwrap_or(0);
    let mut first_row = selected_row;
    let mut used = CARD_HEIGHT;
    while first_row > 0 {
        let h = row_height(&rows[first_row - 1]);
        if used + h > area.height {
            break;
        }
        used += h;
        first_row -= 1;
    }

    // Draw scrollbar if needed.
    // Saturating: a huge host list must not overflow (render never panics).
    let total_height = rows
        .iter()
        .fold(0u16, |acc, r| acc.saturating_add(row_height(r)));
    if total_height > area.height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        let mut sb_state = ScrollbarState::new(rows.len()).position(first_row);
        let sb_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };
        frame.render_stateful_widget(scrollbar, sb_area, &mut sb_state);
    }

    // Render visible rows.
    let mut y = area.y;
    for row in &rows[first_row..] {
        let range = match row {
            GridRow::Header(g) => {
                // A header only shows with its first row of cards under it.
                if y + GROUP_HEADER_HEIGHT + CARD_HEIGHT > area.y + area.height {
                    break;
                }
                if let Some(group) = hlv.groups.get(*g) {
                    render_group_header(frame, area, y, group, view);
                }
                y += GROUP_HEADER_HEIGHT;
                continue;
            }
            GridRow::Cards(range) => range.clone(),
        };
        if y + CARD_HEIGHT > area.y + area.height {
            break;
        }

        let mut x = area.x;
        for card_idx in range {
            let Some(host) = hlv
                .filtered_indices
                .get(card_idx)
                .and_then(|&i| state.hosts.get(i))
            else {
                continue;
            };
            let metrics = state.metrics.get(&host.name);
            let status = state.connection_statuses.get(&host.name);
            let is_selected = card_idx == selected;

            let card_rect = Rect {
                x,
                y,
                width: card_w,
                height: CARD_HEIGHT,
            };

            // Only render if the card fits in the area.
            if card_rect.x + card_rect.width <= area.x + area.width {
                render_card(
                    frame,
                    card_rect,
                    &CardData {
                        host_name: &host.name,
                        hostname: &host.hostname,
                        user: &host.user,
                        port: host.port,
                        tags: &host.tags,
                        metrics,
                        status,
                        services: state.services.get(&host.name).map(|s| s.as_slice()),
                        monitoring: host.monitoring,
                        monitor_port: host.monitor_port,
                        has_forwards: !host.local_forwards.is_empty(),
                        tunnel: state.tunnel_statuses.get(&host.name),
                    },
                    is_selected,
                    &view.theme,
                );
            }

            x += card_w + CARD_GAP;
        }
        y += CARD_HEIGHT + CARD_GAP;
    }
}

/// Draws a one-line section title (`tag (count) ───`) at row `y`.
fn render_group_header(
    frame: &mut Frame,
    area: Rect,
    y: u16,
    group: &GroupHeader,
    view: &ViewState,
) {
    let title = format!("{} ({}) ", group.title(), group.len);
    let fill = (area.width as usize).saturating_sub(title.chars().count() + 1);
    let title_style = if group.label.is_some() {
        Style::default()
            .fg(view.theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(view.theme.text_muted)
            .add_modifier(Modifier::BOLD | Modifier::ITALIC)
    };
    let line = Line::from(vec![
        Span::styled(title, title_style),
        Span::styled("─".repeat(fill), Style::default().fg(view.theme.text_muted)),
    ]);
    frame.render_widget(
        Paragraph::new(line),
        Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        },
    );
}

// ---------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------

fn compute_columns(width: u16) -> u16 {
    ((width + CARD_GAP) / (CARD_MIN_WIDTH + CARD_GAP)).max(1)
}

fn compute_card_width(total_width: u16, cols: u16) -> u16 {
    if cols == 0 {
        return total_width;
    }
    (total_width.saturating_sub((cols - 1) * CARD_GAP)) / cols
}

// ---------------------------------------------------------------------------
// Input handlers
// ---------------------------------------------------------------------------

/// Handles Dashboard-specific key events (normal mode + popup-open mode).
///
/// Returns an [`AppAction`] if the event triggers a loop-level effect.
pub fn handle_input(key: KeyEvent, view: &mut ViewState) -> Option<AppAction> {
    let hlv = &mut view.host_list;

    // ---- Search mode ----
    if hlv.search_mode {
        return host_list::handle_input(key, view);
    }

    // ---- Popup mode (add / edit / delete) ----
    if hlv.popup.is_some() {
        return host_list::handle_input(key, view);
    }

    // ---- Normal grid navigation ----
    match key.code {
        // Navigation
        KeyCode::Char('j') | KeyCode::Down => Some(AppAction::DashboardNav(NavDir::Down)),
        KeyCode::Char('k') | KeyCode::Up => Some(AppAction::DashboardNav(NavDir::Up)),
        KeyCode::Char('h') | KeyCode::Left => Some(AppAction::DashboardNav(NavDir::Left)),
        KeyCode::Char('l') | KeyCode::Right => Some(AppAction::DashboardNav(NavDir::Right)),

        // Open Detail View for selected host.
        // Note: Ctrl+Enter to connect directly is not supported yet,
        // user can press Enter in Detail View to connect.
        KeyCode::Enter => Some(AppAction::OpenDetailView),

        // Host management.
        KeyCode::Char('a') => {
            use crate::app::{HostForm, HostPopup};
            view.host_list.popup = Some(HostPopup::Add(HostForm::empty()));
            None
        }
        KeyCode::Char('e') => Some(AppAction::OpenEditPopup),
        KeyCode::Char('d') => {
            use crate::app::HostPopup;
            if let Some(idx) = view.host_list.selected_host_idx() {
                view.host_list.popup = Some(HostPopup::DeleteConfirm(idx));
            }
            None
        }

        // Search — also accepts the user-configured search key.
        kc if kc == KeyCode::Char('/') || kc == view.keybindings.search => {
            view.host_list.search_mode = true;
            None
        }

        // Refresh and sort keys.
        KeyCode::Char('r') => Some(AppAction::RefreshMetrics),
        KeyCode::Char('s') => Some(AppAction::CycleSortOrder),
        KeyCode::Char('t') => Some(AppAction::OpenTagFilter),
        KeyCode::Char('g') => Some(AppAction::ToggleGroupByTag),

        // Quick-execute snippet on selected host.
        KeyCode::Char('x') => Some(AppAction::OpenQuickExecute),

        // SSH key setup for selected host.
        KeyCode::Char('K') => Some(AppAction::StartKeySetup),

        // Start / stop the selected host's tunnel.
        KeyCode::Char('f') => Some(AppAction::ToggleTunnel),

        // Esc: clear status message / search query.
        KeyCode::Esc => {
            if !view.host_list.search_query.is_empty() {
                view.host_list.search_query.clear();
                return Some(AppAction::SearchQueryChanged);
            }
            None
        }

        _ => None,
    }
}

/// Handles key events when the tag filter popup is open.
pub fn handle_tag_popup_input(key: KeyEvent, view: &mut ViewState) -> Option<AppAction> {
    let hlv = &mut view.host_list;
    // +1 for the "All (clear)" entry at the top.
    let total = hlv.available_tags.len() + 1;

    match key.code {
        KeyCode::Esc | KeyCode::Char('t') => {
            hlv.tag_popup_open = false;
            None
        }
        KeyCode::Char('j') | KeyCode::Down => {
            hlv.tag_popup_selected = (hlv.tag_popup_selected + 1).min(total.saturating_sub(1));
            None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            hlv.tag_popup_selected = hlv.tag_popup_selected.saturating_sub(1);
            None
        }
        KeyCode::Enter => {
            let sel = hlv.tag_popup_selected;
            let chosen = if sel == 0 {
                None // "All" = clear filter
            } else {
                hlv.available_tags.get(sel - 1).cloned()
            };
            Some(AppAction::TagFilterSelected(chosen))
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            hlv.tag_popup_open = false;
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnyssh_core::ssh::client::Host;
    use std::collections::HashMap;

    /// Draws the grouped grid into a `width`x`height` area and returns its text.
    fn grouped_screen(selected: usize, width: u16, height: u16) -> String {
        let hosts = ["alpha", "beta"]
            .map(|tag| Host {
                name: format!("{tag}-1"),
                tags: vec![tag.to_string()],
                ..Host::default()
            })
            .to_vec();
        let state = AppState {
            hosts,
            ..AppState::default()
        };
        let mut view = ViewState::default();
        view.host_list.group_by_tag = true;
        view.host_list
            .rebuild_filter(&state.hosts, &HashMap::new(), &HashMap::new());
        view.host_list.selected = selected;
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_grid(frame, frame.area(), &state, &view))
            .expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn a_section_header_never_shows_without_its_cards() {
        // Room for "alpha" and its card, then one spare line: "beta" stays off.
        let screen = grouped_screen(0, 40, 15);
        assert!(screen.starts_with("alpha (1) ───"));
        assert!(!screen.contains("beta (1)"));
    }

    #[test]
    fn the_selected_card_shows_with_its_section_header() {
        let screen = grouped_screen(1, 40, 15);
        assert!(screen.contains("beta (1)"));
        assert!(screen.contains("beta-1"));
        assert!(!screen.contains("alpha-1"));
    }
}
