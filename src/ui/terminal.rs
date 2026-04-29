//! Terminal multi-session screen.
//!
//! Renders PTY-backed SSH sessions as VT100-parsed character grids, a tab bar
//! at the top, optional split-view layout, and a
//! host-picker popup for opening new tabs.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{AppAction, AppState, SplitFocus, TermHostPicker, TermTab, ViewState};

// ---------------------------------------------------------------------------
// Top-level render
// ---------------------------------------------------------------------------

/// Renders the Terminal screen: tab bar + one or two PTY panes.
///
/// Never panics — missing or locked parser data shows a placeholder instead.
pub fn render(frame: &mut Frame, area: Rect, state: &AppState, view: &ViewState) {
    let tv = &view.terminal_view;

    // Split area into tab bar (1 row) + content.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    let tab_area = chunks[0];
    let content_area = chunks[1];

    // Render tab bar.
    render_tab_bar(frame, tab_area, tv, &view.theme);

    if tv.tabs.is_empty() {
        // No sessions open — show a hint.
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No SSH sessions open.",
                Style::default().fg(view.theme.text_muted),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press Ctrl+N to connect to a host.",
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(view.theme.text_muted)),
        );
        frame.render_widget(msg, content_area);
    } else {
        match &tv.split {
            None => {
                // Single pane.
                if let Some(tab) = tv.tabs.get(tv.active_tab) {
                    render_pty_pane(frame, content_area, tab, true, &view.theme);
                }
            }
            Some(sv) => {
                // Split view: two panes.
                let (primary_constraint, secondary_constraint) = match sv.direction {
                    crate::app::SplitDirection::Vertical => {
                        let half = content_area.width / 2;
                        (Constraint::Length(half), Constraint::Min(1))
                    }
                    crate::app::SplitDirection::Horizontal => {
                        let half = content_area.height / 2;
                        (Constraint::Length(half), Constraint::Min(1))
                    }
                };
                let split_dir = match sv.direction {
                    crate::app::SplitDirection::Vertical => Direction::Horizontal,
                    crate::app::SplitDirection::Horizontal => Direction::Vertical,
                };
                let pane_areas = Layout::default()
                    .direction(split_dir)
                    .constraints([primary_constraint, secondary_constraint])
                    .split(content_area);

                let primary_focused = matches!(tv.split_focus, SplitFocus::Primary);

                if let Some(primary_tab) = tv.tabs.get(tv.active_tab) {
                    render_pty_pane(
                        frame,
                        pane_areas[0],
                        primary_tab,
                        primary_focused,
                        &view.theme,
                    );
                }
                if let Some(secondary_tab) = tv.tabs.get(sv.secondary_tab) {
                    render_pty_pane(
                        frame,
                        pane_areas[1],
                        secondary_tab,
                        !primary_focused,
                        &view.theme,
                    );
                }
            }
        }
    }

    // Host-picker popup renders on top.
    if let Some(picker) = &tv.host_picker {
        render_host_picker(frame, area, picker, state, &view.theme);
    }
}

// ---------------------------------------------------------------------------
// Tab bar
// ---------------------------------------------------------------------------

/// Renders the single-line tab bar above the pane area.
fn render_tab_bar(
    frame: &mut Frame,
    area: Rect,
    tv: &crate::app::TerminalView,
    theme: &crate::ui::theme::Theme,
) {
    let mut spans: Vec<Span> = Vec::new();

    // Check if we're in split view to highlight both visible tabs
    let secondary_tab_idx = tv.split.as_ref().map(|sv| sv.secondary_tab);

    for (i, tab) in tv.tabs.iter().enumerate() {
        let is_primary = i == tv.active_tab;
        let is_secondary = secondary_tab_idx == Some(i);
        let is_visible_in_split = is_primary || is_secondary;

        // When tab-select mode is active, prefix each tab with its 1-based number
        // so the user can see which digit to press for a direct jump.
        let label = if tv.tab_select_mode {
            if tab.has_activity && !is_visible_in_split {
                format!(" [{}] ● {} ", i + 1, tab.host_name)
            } else {
                format!(" [{}] {} ", i + 1, tab.host_name)
            }
        } else if tab.has_activity && !is_visible_in_split {
            format!(" ● {} ", tab.host_name)
        } else {
            format!("  {}  ", tab.host_name)
        };

        // Primary tab gets cyan, secondary gets green, others are gray
        let style = if is_primary && tv.split_focus == crate::app::SplitFocus::Primary {
            // Primary tab with focus
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if is_secondary && tv.split_focus == crate::app::SplitFocus::Secondary {
            // Secondary tab with focus
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else if is_primary || is_secondary {
            // Visible in split but not focused
            Style::default()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else if tab.has_activity {
            Style::default().fg(theme.text_warning).bg(Color::DarkGray)
        } else {
            Style::default()
                .fg(theme.text_secondary)
                .bg(Color::DarkGray)
        };

        spans.push(Span::styled(label, style));
        // Separator between tabs.
        spans.push(Span::styled("│", Style::default().fg(theme.text_muted)));
    }

    // "[+]" hint for new tab + copy instruction.
    let hint = if tv.split.is_some() {
        " Ctrl+N:new  Ctrl+W:close  Ctrl+H:switch-pane  Ctrl+\\ :split-v  Ctrl+]:split-h  │  Opt/Shift+Drag to select"
    } else {
        " Ctrl+N:new  Ctrl+W:close  Ctrl+\\ :split-v  Ctrl+]:split-h  │  Opt/Shift+Drag to select"
    };
    spans.push(Span::styled(hint, Style::default().fg(theme.text_muted)));

    let line = Line::from(spans);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(Color::Reset)),
        area,
    );
}

// ---------------------------------------------------------------------------
// PTY pane renderer
// ---------------------------------------------------------------------------

/// Renders one SSH session's VT100 screen content into `area`.
///
/// Locks the parser mutex for a snapshot, then releases it immediately before
/// iterating cells (avoids holding the lock during the entire render pass).
fn render_pty_pane(
    frame: &mut Frame,
    area: Rect,
    tab: &TermTab,
    focused: bool,
    theme: &crate::ui::theme::Theme,
) {
    let border_style = if focused {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.text_muted)
    };

    let title = if tab.scroll_offset > 0 {
        format!(" {}  ↑ scroll — type to return ", tab.host_name)
    } else {
        format!(" {} ", tab.host_name)
    };
    let block = Block::default()
        .title(title.as_str())
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let render_rows = inner.height;
    let render_cols = inner.width;

    // Snapshot only the visible viewport (render_rows × render_cols) plus
    // cursor state.  Avoids cloning the entire vt100 parser — which includes
    // up to 1000 scrollback rows × terminal width cells — on every frame.
    // With a large scrollback that clone allocates several MB per render and
    // is the root cause of the "freeze while fast-scrolling" stutter.
    //
    // Instead we hold the lock just long enough to read each visible cell via
    // screen.visible_row(r).get(c), which is O(rows) iterator work total
    // (one nth(r) call per row, not per cell), then release immediately.
    struct CellSnap {
        display: String,
        style: Style,
    }
    let (rows_snap, cursor_pos, hide_cursor) = {
        let mut guard = match tab.parser.lock() {
            Ok(g) => g,
            Err(_) => {
                // Poisoned mutex — show a placeholder.
                frame.render_widget(
                    Paragraph::new("  [parser error]").style(Style::default().fg(theme.text_error)),
                    inner,
                );
                return;
            }
        };
        guard.set_scrollback(tab.scroll_offset);
        let screen = guard.screen();

        // Single-pass iteration via visible_rows_iter() — O(scrollback_skip + rows_len).
        // The previous approach called visible_row(r) per row, which recreates the
        // iterator each time and costs O(scrollback_skip + r) per call — O(rows²) total.
        // At scroll_offset ≈ 0 (bottom), scrollback_skip ≈ 1000, so the old code did
        // ~46 × 1000 = 46 000 VecDeque steps per frame; the new code does ~1 046.
        let mut rows_snap: Vec<Vec<CellSnap>> = Vec::with_capacity(render_rows as usize);
        for vt_row in screen.visible_rows_iter().take(render_rows as usize) {
            let mut row_snap: Vec<CellSnap> = Vec::with_capacity(render_cols as usize);
            for c in 0..render_cols {
                let snap = match vt_row.get(c) {
                    None => CellSnap {
                        display: " ".into(),
                        style: Style::default(),
                    },
                    Some(cell) => {
                        let text = cell.contents();
                        CellSnap {
                            display: if text.is_empty() { " ".into() } else { text },
                            style: cell_to_style(cell),
                        }
                    }
                };
                row_snap.push(snap);
            }
            rows_snap.push(row_snap);
        }
        // Pad with blank rows if visible_rows_iter yielded fewer than render_rows
        // (can happen when the scrollback buffer isn't full yet).
        while rows_snap.len() < render_rows as usize {
            rows_snap.push(
                (0..render_cols)
                    .map(|_| CellSnap {
                        display: " ".into(),
                        style: Style::default(),
                    })
                    .collect(),
            );
        }

        let hide_cursor = screen.hide_cursor();
        let cursor_pos = screen.cursor_position();
        (rows_snap, cursor_pos, hide_cursor)
        // parser lock released here
    };

    // Render each row as a one-line Paragraph.
    // Accumulate same-style characters into a single String buffer and flush
    // with std::mem::take when the style changes — O(cols) allocations per row.
    for (row, row_cells) in rows_snap.into_iter().enumerate() {
        let row = row as u16;
        let mut spans: Vec<Span> = Vec::new();
        let mut buf_style: Option<Style> = None;
        let mut buf_text = String::new();

        for cell in row_cells {
            if buf_style == Some(cell.style) {
                buf_text.push_str(&cell.display);
            } else {
                if let Some(s) = buf_style {
                    spans.push(Span::styled(std::mem::take(&mut buf_text), s));
                }
                buf_style = Some(cell.style);
                buf_text = cell.display;
            }
        }
        if let Some(s) = buf_style {
            spans.push(Span::styled(buf_text, s));
        }

        let line_area = Rect {
            x: inner.x,
            y: inner.y + row,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(Line::from(spans)), line_area);
    }

    // Render cursor for the focused pane only when showing the live screen.
    // When scrolled back, the cursor belongs to the live screen which is not
    // visible, so skip it to avoid rendering it at the wrong position.
    if focused && tab.scroll_offset == 0 && !hide_cursor {
        let (cur_row, cur_col) = cursor_pos;
        if cur_row < render_rows && cur_col < render_cols {
            frame.set_cursor_position((inner.x + cur_col, inner.y + cur_row));
        }
    }
}

// ---------------------------------------------------------------------------
// Style mapping: vt100 → ratatui
// ---------------------------------------------------------------------------

/// Converts vt100 cell attributes to a ratatui [`Style`].
fn cell_to_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    match cell.fgcolor() {
        vt100::Color::Default => {}
        vt100::Color::Idx(i) => style = style.fg(ansi_idx_to_color(i)),
        vt100::Color::Rgb(r, g, b) => style = style.fg(Color::Rgb(r, g, b)),
    }
    match cell.bgcolor() {
        vt100::Color::Default => {}
        vt100::Color::Idx(i) => style = style.bg(ansi_idx_to_color(i)),
        vt100::Color::Rgb(r, g, b) => style = style.bg(Color::Rgb(r, g, b)),
    }
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

/// Maps a standard ANSI palette index (0–15) to a ratatui [`Color`].
///
/// Indices 0–7 are the standard colours; 8–15 are the bright variants.
fn ansi_idx_to_color(idx: u8) -> Color {
    match idx {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::White,
        8 => Color::DarkGray, // bright black
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        15 => Color::White, // bright white
        // 256-colour palette — fall back to reset for higher indices.
        _ => Color::Reset,
    }
}

// ---------------------------------------------------------------------------
// Host-picker popup
// ---------------------------------------------------------------------------

/// Renders the host-picker popup for opening a new terminal tab (Ctrl+N flow).
fn render_host_picker(
    frame: &mut Frame,
    area: Rect,
    picker: &TermHostPicker,
    state: &AppState,
    theme: &crate::ui::theme::Theme,
) {
    let popup_w = 60u16.min(area.width.saturating_sub(4)).max(20);
    let list_h = (state.hosts.len() as u16 + 2)
        .min(20)
        .min(area.height.saturating_sub(4))
        .max(3);
    let popup_h = list_h + 2; // +2 for block borders
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_w,
        height: popup_h,
    };

    // Clear background behind popup.
    frame.render_widget(Clear, popup_area);

    let title = if picker.switch_pane_mode {
        " Switch pane to host (Enter to switch) "
    } else {
        " Connect to host (Enter to open tab) "
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if state.hosts.is_empty() {
        frame.render_widget(
            Paragraph::new("  No hosts configured. Add one on the Dashboard (a)."),
            inner,
        );
        return;
    }

    let visible = inner.height as usize;
    // Clamp cursor so it never exceeds the host list length.
    let cursor = picker.cursor.min(state.hosts.len().saturating_sub(1));
    let scroll = cursor.saturating_sub(visible.saturating_sub(1));

    let items: Vec<ListItem> = state
        .hosts
        .iter()
        .skip(scroll)
        .take(visible)
        .enumerate()
        .map(|(i, h)| {
            let actual_idx = i + scroll;
            let tag_str = if h.tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", h.tags.join(", "))
            };
            // Show host name first, then user@hostname:port, then tags
            let label = format!(
                " {} — {}@{}:{}{}",
                h.name, h.user, h.hostname, h.port, tag_str
            );
            let style = if actual_idx == cursor {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_primary)
            };
            ListItem::new(label).style(style)
        })
        .collect();

    frame.render_widget(List::new(items), inner);
}

// ---------------------------------------------------------------------------
// Input handler for the host-picker popup
// ---------------------------------------------------------------------------

/// Handles key events while the terminal host-picker popup is open.
///
/// Called from [`crate::app::App::handle_terminal_key`] when
/// `view.terminal_view.host_picker` is `Some`.
pub fn handle_host_picker_input(key: KeyEvent, view: &mut ViewState) -> Option<AppAction> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(AppAction::TermHostPickerNav(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(AppAction::TermHostPickerNav(-1)),
        KeyCode::Enter => {
            let cursor = view.terminal_view.host_picker.as_ref()?.cursor;
            Some(AppAction::TermHostPickerSelect(cursor))
        }
        KeyCode::Esc => Some(AppAction::TermCloseHostPicker),
        _ => None,
    }
}
