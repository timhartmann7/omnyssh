//! File Manager screen — split-panel SFTP file manager.
//!
//! Three-zone layout:
//!   - Top 65% (or Min(3)): horizontal split — local panel (left) + remote panel (right)
//!   - Bottom 35%: preview zone or transfer progress bar
//!
//! Key bindings are documented in [`handle_input`].

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Gauge, List, ListItem, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState,
    },
    Frame,
};

use crate::app::{
    AppAction, FileManagerPopup, FileManagerView, FilePanelView, FmPanel, FormField, ViewState,
};
use omnyssh_core::ssh::client::Host;

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Top-level render function for the File Manager screen.
pub fn render(frame: &mut Frame, area: Rect, state: &crate::app::AppState, view: &ViewState) {
    let fm = &view.file_manager;

    // Vertical split — hints (1 line) + panels on top, preview on bottom.
    let (panel_constraint, preview_constraint) = if area.height >= 30 {
        (Constraint::Percentage(65), Constraint::Percentage(35))
    } else {
        (Constraint::Min(3), Constraint::Length(0))
    };

    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), panel_constraint, preview_constraint])
        .split(area);

    let hints_area = vert[0];
    let panels_area = vert[1];
    let preview_area = vert[2];

    // Horizontal split — local (left) + remote (right).
    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(panels_area);

    let local_area = horiz[0];
    let remote_area = horiz[1];

    // Determine panel titles.
    // Render key hints header
    render_hints_header(frame, hints_area, &view.theme);

    let local_cwd = if fm.local.cwd.is_empty() {
        "~".to_string()
    } else {
        fm.local.cwd.clone()
    };
    let remote_title = match &fm.connected_host {
        Some(h) => format!("REMOTE  {} — {}", h, fm.remote.cwd),
        None => "REMOTE  (not connected)".to_string(),
    };

    // Render panels.
    let local_active = fm.active_panel == FmPanel::Local;
    render_panel(
        frame,
        local_area,
        &fm.local,
        &format!("LOCAL  {local_cwd}"),
        local_active,
        &view.theme,
    );
    render_panel(
        frame,
        remote_area,
        &fm.remote,
        &remote_title,
        !local_active,
        &view.theme,
    );

    // Render preview / progress zone.
    if preview_area.height > 0 {
        render_preview_zone(frame, preview_area, fm, &view.theme);
    }

    // Render active popup on top.
    if let Some(popup) = &fm.popup {
        render_fm_popup(frame, area, popup, state, view);
    }
}

/// Handles key events on the File Manager screen.
///
/// Returns `Some(AppAction)` when the key triggered an action, `None` when
/// the key was consumed (e.g. text field input) but no further action is
/// needed, or when the key is unrelated.
/// Discriminant-only enum to avoid holding a borrow on the popup while
/// also passing `&mut view` into handlers.
#[derive(Clone, Copy)]
enum PopupKind {
    HostPicker,
    DeleteConfirm,
    MkDir,
    Rename,
    TransferProgress,
    ExternalEditConfirm,
}

fn popup_kind(view: &ViewState) -> Option<PopupKind> {
    match &view.file_manager.popup {
        Some(FileManagerPopup::HostPicker { .. }) => Some(PopupKind::HostPicker),
        Some(FileManagerPopup::DeleteConfirm { .. }) => Some(PopupKind::DeleteConfirm),
        Some(FileManagerPopup::MkDir(_)) => Some(PopupKind::MkDir),
        Some(FileManagerPopup::Rename { .. }) => Some(PopupKind::Rename),
        Some(FileManagerPopup::TransferProgress { .. }) => Some(PopupKind::TransferProgress),
        Some(FileManagerPopup::ExternalEditConfirm { .. }) => Some(PopupKind::ExternalEditConfirm),
        None => None,
    }
}

pub fn handle_input(key: KeyEvent, view: &mut ViewState) -> Option<AppAction> {
    // Popup interception — all keys go to the popup first.
    if let Some(kind) = popup_kind(view) {
        return handle_popup_input(key, kind, view);
    }

    // Normal file-panel key dispatch.
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(AppAction::FmNavDown),
        KeyCode::Char('k') | KeyCode::Up => Some(AppAction::FmNavUp),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => Some(AppAction::FmEnterDir),
        KeyCode::Char('h') | KeyCode::Left => Some(AppAction::FmParentDir),
        KeyCode::Backspace => Some(AppAction::FmParentDir),
        KeyCode::Char('d') => Some(AppAction::FmNextDrive),
        KeyCode::Char(' ') => Some(AppAction::FmMarkFile),
        KeyCode::Tab => Some(AppAction::FmSwitchPanel),
        KeyCode::Char('c') => Some(AppAction::FmCopy),
        KeyCode::Char('p') => Some(AppAction::FmPaste),
        KeyCode::Char('D') => Some(AppAction::FmOpenDeleteConfirm),
        KeyCode::Char('n') => Some(AppAction::FmOpenMkDir),
        KeyCode::Char('.') => Some(AppAction::FmToggleHidden),
        KeyCode::Char('R') => Some(AppAction::FmOpenRename),
        KeyCode::Char('H') => Some(AppAction::FmOpenHostPicker),
        KeyCode::Char('e') => Some(AppAction::FmEditFile),
        KeyCode::Esc => Some(AppAction::FmClosePopup),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Key hints header
// ---------------------------------------------------------------------------

fn render_hints_header(frame: &mut Frame, area: Rect, theme: &crate::ui::theme::Theme) {
    let mut hints = Line::from(vec![
        Span::styled(
            " hjkl",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Navigate", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            "Tab",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Switch", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            "Space",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Mark", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            "c",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Copy", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            "p",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Paste", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            "n",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":MkDir", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            "Shift+R",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Rename", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            "Shift+D",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Delete", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            ".",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Hidden", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            "Shift+H",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Host", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            "e",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Edit", Style::default().fg(theme.text_muted)),
    ]);
    // Only Windows has more than one root to switch between.
    if cfg!(windows) {
        hints.spans.extend([
            Span::raw("  "),
            Span::styled(
                "d",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":Drive", Style::default().fg(theme.text_muted)),
        ]);
    }

    frame.render_widget(Paragraph::new(hints), area);
}

// ---------------------------------------------------------------------------
// Popup input handling
// ---------------------------------------------------------------------------

fn handle_popup_input(key: KeyEvent, kind: PopupKind, view: &mut ViewState) -> Option<AppAction> {
    match kind {
        PopupKind::HostPicker => handle_host_picker_input(key, view),
        PopupKind::DeleteConfirm => handle_delete_confirm_input(key),
        PopupKind::MkDir => handle_text_input_popup(key, view, TextPopupKind::MkDir),
        PopupKind::Rename => handle_text_input_popup(key, view, TextPopupKind::Rename),
        PopupKind::TransferProgress => {
            if key.code == KeyCode::Esc {
                Some(AppAction::FmClosePopup)
            } else {
                None
            }
        }
        PopupKind::ExternalEditConfirm => match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                Some(AppAction::FmConfirmExternalEdit)
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => Some(AppAction::FmClosePopup),
            _ => None,
        },
    }
}

fn handle_host_picker_input(key: KeyEvent, view: &mut ViewState) -> Option<AppAction> {
    let cursor = match &view.file_manager.popup {
        Some(FileManagerPopup::HostPicker { cursor }) => *cursor,
        _ => return None,
    };

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(AppAction::FmHostPickerNav(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(AppAction::FmHostPickerNav(-1)),
        KeyCode::Enter => Some(AppAction::FmHostPickerSelect(cursor)),
        KeyCode::Esc => Some(AppAction::FmClosePopup),
        _ => None,
    }
}

fn handle_delete_confirm_input(key: KeyEvent) -> Option<AppAction> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => Some(AppAction::FmConfirmDelete),
        KeyCode::Char('n') | KeyCode::Esc => Some(AppAction::FmClosePopup),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum TextPopupKind {
    MkDir,
    Rename,
}

fn handle_text_input_popup(
    key: KeyEvent,
    view: &mut ViewState,
    kind: TextPopupKind,
) -> Option<AppAction> {
    // Get the field from the popup (mutable borrow of view.file_manager.popup).
    let field = match &mut view.file_manager.popup {
        Some(FileManagerPopup::MkDir(f)) => f,
        Some(FileManagerPopup::Rename { field, .. }) => field,
        _ => return None,
    };

    match key.code {
        KeyCode::Char(c) => {
            field.insert_char(c);
            None
        }
        KeyCode::Backspace => {
            field.backspace();
            None
        }
        KeyCode::Enter => {
            // Take the value, close the popup, and dispatch the action.
            let value = field.value.trim().to_string();
            view.file_manager.popup = None;
            if value.is_empty() {
                None
            } else {
                match kind {
                    TextPopupKind::MkDir => Some(AppAction::FmConfirmMkDir(value)),
                    TextPopupKind::Rename => Some(AppAction::FmConfirmRename(value)),
                }
            }
        }
        KeyCode::Esc => {
            view.file_manager.popup = None;
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Panel renderer
// ---------------------------------------------------------------------------

fn render_panel(
    frame: &mut Frame,
    area: Rect,
    panel: &FilePanelView,
    title: &str,
    is_active: bool,
    theme: &crate::ui::theme::Theme,
) {
    let border_color = if is_active {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let block = Block::default()
        .title(format!(" {} ", title))
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let visible_rows = inner.height as usize;
    let mut scroll = panel.scroll.get();

    // Clamp scroll so cursor stays visible.
    let cursor = panel.cursor;
    if cursor < scroll {
        scroll = cursor;
    } else if cursor >= scroll + visible_rows {
        scroll = cursor.saturating_sub(visible_rows - 1);
    }
    panel.scroll.set(scroll);

    // Build list items.
    let items: Vec<ListItem> = panel
        .entries
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_rows)
        .map(|(original_idx, entry)| {
            // Compare original index from entries list with cursor position
            let is_cursor = original_idx == cursor;
            let is_marked = panel.marked.contains(&entry.path);

            // Nerd-font icon. Renders as a single cell width; falls back to
            // empty boxes if the user does not have a Nerd Font installed.
            let icon = crate::utils::file_icons::icon_for(&entry.name, entry.is_dir);
            let icon_color = if entry.is_dir {
                Color::Cyan
            } else {
                Color::Gray
            };

            // Cursor indicator.
            let cursor_prefix = if is_cursor { "▶ " } else { "  " };

            // Marked indicator (with trailing space so the glyph doesn't
            // touch the file icon and visually overlap with it).
            let mark_str = if is_marked { "●  " } else { "   " };

            // Size string (right-aligned, skip for dirs and "..").
            let size_str = if entry.is_dir || entry.name == ".." {
                String::new()
            } else {
                format_size(entry.size)
            };

            // Width budget: cursor(2) + mark(3, with trailing padding) +
            // icon(1) + space(1) + name + space-before-size(1) + size.
            // The nerd-font glyph occupies a single cell even though its
            // codepoint is wide.
            let name_width = inner
                .width
                .saturating_sub(2 + 3 + 1 + 1 + 1 + size_str.len() as u16)
                as usize;
            let name_display: String = if entry.name.chars().count() > name_width {
                let truncated = entry
                    .name
                    .char_indices()
                    .nth(name_width.saturating_sub(1))
                    .map(|(i, _)| &entry.name[..i])
                    .unwrap_or(&entry.name);
                format!("{}…", truncated)
            } else {
                format!("{:<width$}", entry.name, width = name_width)
            };

            let mut spans = vec![
                Span::styled(
                    cursor_prefix,
                    if is_cursor {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                ),
                Span::styled(mark_str, Style::default().fg(theme.text_warning)),
                Span::styled(format!("{} ", icon), Style::default().fg(icon_color)),
                Span::styled(
                    name_display,
                    if is_marked {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else if is_cursor {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else if entry.is_dir {
                        Style::default().fg(theme.accent)
                    } else {
                        Style::default().fg(theme.text_secondary)
                    },
                ),
            ];

            if !size_str.is_empty() {
                spans.push(Span::styled(
                    format!(" {}", size_str),
                    Style::default().fg(theme.text_muted),
                ));
            }

            let item = ListItem::new(Line::from(spans));
            if is_cursor {
                item.style(Style::default().bg(theme.selected_bg))
            } else {
                item
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, inner);

    // Scrollbar if entries overflow.
    if panel.entries.len() > visible_rows {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        let mut scrollbar_state = ScrollbarState::new(panel.entries.len()).position(scroll);
        let scrollbar_area = Rect {
            x: area.x + area.width - 1,
            y: area.y + 1,
            width: 1,
            height: area.height.saturating_sub(2),
        };
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }

    // Empty-panel hint.
    if panel.entries.is_empty() {
        let hint = if panel.cwd.is_empty() {
            "(loading…)"
        } else {
            "(empty)"
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                hint,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )),
            inner,
        );
    }
}

// ---------------------------------------------------------------------------
// Preview zone renderer
// ---------------------------------------------------------------------------

fn render_preview_zone(
    frame: &mut Frame,
    area: Rect,
    fm: &FileManagerView,
    theme: &crate::ui::theme::Theme,
) {
    // If there's an active transfer, show progress gauge.
    if let Some(FileManagerPopup::TransferProgress {
        filename,
        done,
        total,
        ..
    }) = &fm.popup
    {
        render_transfer_progress(frame, area, filename, *done, *total, theme);
        return;
    }

    let block = Block::default()
        .title(" Preview ")
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.text_muted));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(content) = &fm.preview_content {
        // Show the preview text.
        let path_hint = fm
            .preview_path
            .as_deref()
            .map(|p| {
                // Show only the filename part.
                std::path::Path::new(p)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.to_string())
            })
            .unwrap_or_default();

        // Sanitize content to prevent overflow: remove non-printable chars and limit line width
        let max_line_width = inner.width.saturating_sub(3) as usize; // Reserve space for "  " prefix
        let sanitized_lines = sanitize_preview_content(
            content,
            max_line_width,
            inner.height.saturating_sub(1) as usize,
        );

        let lines: Vec<Line> = std::iter::once(Line::from(Span::styled(
            format!("  {} ─────", path_hint),
            Style::default().fg(theme.accent),
        )))
        .chain(sanitized_lines.into_iter().map(|l| {
            Line::from(Span::styled(
                format!("  {}", l),
                Style::default().fg(theme.text_secondary),
            ))
        }))
        .collect();

        frame.render_widget(Paragraph::new(lines), inner);
    } else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  (no preview — select a text file)",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )),
            inner,
        );
    }
}

fn render_transfer_progress(
    frame: &mut Frame,
    area: Rect,
    filename: &str,
    done: u64,
    total: u64,
    theme: &crate::ui::theme::Theme,
) {
    let block = Block::default()
        .title(" Transfer Progress ")
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.text_success));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 2 {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    // Filename.
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!("  {}", filename),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        rows[0],
    );

    // Progress gauge.
    let percent = if total > 0 {
        ((done as f64 / total as f64) * 100.0) as u16
    } else {
        0
    };

    let label = if total > 0 {
        format!(
            "  {} / {}  ({}%)",
            format_size(done),
            format_size(total),
            percent
        )
    } else {
        format!("  {} transferred…", format_size(done))
    };

    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(theme.text_success).bg(Color::DarkGray))
        .percent(percent)
        .label(label);

    frame.render_widget(gauge, rows[1]);
}

// ---------------------------------------------------------------------------
// Popup renderers
// ---------------------------------------------------------------------------

fn render_fm_popup(
    frame: &mut Frame,
    area: Rect,
    popup: &FileManagerPopup,
    state: &crate::app::AppState,
    view: &ViewState,
) {
    let theme = &view.theme;
    match popup {
        FileManagerPopup::HostPicker { cursor } => {
            render_fm_host_picker(frame, area, &state.hosts, *cursor, theme);
        }
        FileManagerPopup::DeleteConfirm { paths } => {
            render_fm_delete_confirm(frame, area, paths, theme);
        }
        FileManagerPopup::MkDir(field) => {
            render_fm_text_input(frame, area, " New Directory ", field, theme);
        }
        FileManagerPopup::Rename {
            field,
            original_name,
        } => {
            let title = format!(" Rename '{}' ", original_name);
            render_fm_text_input(frame, area, &title, field, theme);
        }
        FileManagerPopup::TransferProgress { .. } => {
            // Transfer progress is rendered in the preview zone, not as a floating popup.
        }
        FileManagerPopup::ExternalEditConfirm { path, size } => {
            render_fm_external_edit_confirm(frame, area, path, *size, theme);
        }
    }
}

/// Renders the host-picker popup for connecting the remote panel.
fn render_fm_host_picker(
    frame: &mut Frame,
    area: Rect,
    hosts: &[Host],
    cursor: usize,
    theme: &crate::ui::theme::Theme,
) {
    let popup_area = crate::ui::popup::centred_rect(60, 70, area);
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Connect Remote Panel — Select Host ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if hosts.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  No hosts configured. Add a host from the Dashboard (1).",
                Style::default().fg(theme.text_muted),
            )),
            inner,
        );
        return;
    }

    let list_height = inner.height.saturating_sub(1) as usize;
    let list_area = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };
    let hint_area = Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1,
        ..inner
    };

    let offset = if cursor >= list_height {
        cursor - list_height + 1
    } else {
        0
    };

    let items: Vec<ListItem> = hosts
        .iter()
        .enumerate()
        .skip(offset)
        .take(list_height)
        .map(|(i, h)| {
            let is_cursor = i == cursor;
            let name_span = Span::styled(
                format!("  {} ", h.name),
                if is_cursor {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text_primary)
                },
            );
            let host_span = Span::styled(
                format!("{}@{}", h.user, h.hostname),
                Style::default().fg(theme.text_muted),
            );
            ListItem::new(Line::from(vec![name_span, host_span]))
        })
        .collect();

    frame.render_widget(List::new(items), list_area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "  j/k",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":navigate  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":connect  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":cancel", Style::default().fg(theme.text_muted)),
        ])),
        hint_area,
    );
}

/// Renders a single-line text-input popup (mkdir / rename).
fn render_fm_text_input(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    field: &FormField,
    theme: &crate::ui::theme::Theme,
) {
    let popup_area = crate::ui::popup::centred_rect(60, 20, area);
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.text_success));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.height < 2 {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    // Input field with cursor.
    let (before, after) = field.value.split_at(field.cursor.min(field.value.len()));
    let display = format!("  {}|{} ", before, after);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            display,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ))),
        rows[0],
    );

    // Hint.
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "  Enter",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":confirm  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":cancel", Style::default().fg(theme.text_muted)),
        ])),
        rows[1],
    );
}

/// Confirm opening a large remote file in the external editor.
fn render_fm_external_edit_confirm(
    frame: &mut Frame,
    area: Rect,
    path: &str,
    size: u64,
    theme: &crate::ui::theme::Theme,
) {
    let popup_area = crate::ui::popup::centred_rect(70, 40, area);
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" External Editor ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let lines = vec![
        Line::from(Span::styled(
            "  File is too large for the in-app editor.",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  Path: {path}"),
            Style::default().fg(theme.text_muted),
        )),
        Line::from(Span::styled(
            format!("  Size: {size} bytes"),
            Style::default().fg(theme.text_muted),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Download to a temp file and open your configured editor?",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                "Enter / y",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(": open editor   ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "n / Esc",
                Style::default()
                    .fg(theme.text_error)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(": cancel", Style::default().fg(theme.text_muted)),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_fm_delete_confirm(
    frame: &mut Frame,
    area: Rect,
    paths: &[String],
    theme: &crate::ui::theme::Theme,
) {
    let popup_area = crate::ui::popup::centred_rect(60, 35, area);
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Confirm Delete ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.text_error));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.height < 2 {
        return;
    }

    // Build content: up to 5 paths + ellipsis if more.
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "  Delete the following items?",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    let max_show = inner.height.saturating_sub(4) as usize;
    for path in paths.iter().take(max_show) {
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        lines.push(Line::from(Span::styled(
            format!("    • {}", name),
            Style::default().fg(theme.text_warning),
        )));
    }
    if paths.len() > max_show {
        lines.push(Line::from(Span::styled(
            format!("    … and {} more", paths.len() - max_show),
            Style::default().fg(theme.text_muted),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  This cannot be undone.",
        Style::default().fg(theme.text_muted),
    )));

    let hint_row = Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1,
        ..inner
    };
    let content_area = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };

    frame.render_widget(Paragraph::new(lines), content_area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                "y",
                Style::default()
                    .fg(theme.text_error)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":Yes — delete  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "n / Esc",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":No", Style::default().fg(theme.text_muted)),
        ])),
        hint_row,
    );
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Human-readable file size string (e.g. "45.3K", "1.2M", "3.0G").
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}M", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}K", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

/// Sanitizes preview content to prevent UI overflow.
///
/// - Filters out non-printable characters (except newline and tab)
/// - Truncates lines that exceed max_width
/// - Limits total number of lines to max_lines
/// - Replaces control characters with placeholders
fn sanitize_preview_content(content: &str, max_width: usize, max_lines: usize) -> Vec<String> {
    content
        .lines()
        .take(max_lines)
        .map(|line| {
            // Filter and truncate each line
            let sanitized: String = line
                .chars()
                .map(|c| {
                    if c == '\t' {
                        // Replace tab with 4 spaces
                        "    "
                    } else if c.is_control() {
                        // Replace control characters with placeholder
                        "�"
                    } else if c.is_ascii_graphic() || c == ' ' {
                        // Keep printable ASCII characters
                        return c.to_string();
                    } else {
                        // Replace other non-printable with placeholder
                        "�"
                    }
                    .to_string()
                })
                .collect();

            // Truncate to max_width using char boundaries
            if sanitized.chars().count() > max_width {
                let truncated: String = sanitized
                    .chars()
                    .take(max_width.saturating_sub(1))
                    .collect();
                format!("{}…", truncated)
            } else {
                sanitized
            }
        })
        .collect()
}
