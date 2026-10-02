use std::ops::Range;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::app::{
    FormField, HostForm, PassphrasePrompt, PasswordPrompt, SnippetForm, SnippetResultEntry,
    UpdateButton, UpdatePopup, UpdatePopupPhase, FORM_FIELD_LABELS, SNIPPET_FORM_FIELD_LABELS,
    UPDATE_BUTTONS,
};
use crate::ui::theme::Theme;
use omnyssh_core::ssh::client::Host;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Computes a centred rectangle of the given percentage dimensions inside
/// `area`. Used to position popups.
pub fn centred_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let layout_v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(layout_v[1])[1]
}

// ---------------------------------------------------------------------------
// Help popup
// ---------------------------------------------------------------------------

/// Renders the built-in help popup listing all key bindings organized by screen.
pub fn render_help(frame: &mut Frame, theme: &Theme) {
    let area = centred_rect(95, 85, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Help — Keyboard Shortcuts ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.warning_border));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Section header style
    let section_style = Style::default()
        .fg(theme.text_warning)
        .add_modifier(Modifier::BOLD);
    let key_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(theme.text_primary);

    // Split into 3 columns
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(34),
        ])
        .split(inner);

    // Column 1: Global Navigation + Dashboard
    let mut col1_lines = vec![Line::from("")];
    col1_lines.push(Line::from(Span::styled(
        " GLOBAL NAVIGATION",
        section_style,
    )));
    col1_lines.push(Line::from(vec![
        Span::styled("  1", key_style),
        Span::styled("        Dashboard", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  2", key_style),
        Span::styled("        File Manager", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  3", key_style),
        Span::styled("        Snippets", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  4", key_style),
        Span::styled("        Terminal", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  ?", key_style),
        Span::styled("        Help", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  q", key_style),
        Span::styled("        Quit", desc_style),
    ]));
    col1_lines.push(Line::from(""));

    col1_lines.push(Line::from(Span::styled(" DASHBOARD", section_style)));
    col1_lines.push(Line::from(vec![
        Span::styled("  Enter", key_style),
        Span::styled("    Open details", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  a", key_style),
        Span::styled("        Add host", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  e", key_style),
        Span::styled("        Edit host", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  d", key_style),
        Span::styled("        Delete host", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  r", key_style),
        Span::styled("        Refresh", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  s", key_style),
        Span::styled("        Sort", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  t", key_style),
        Span::styled("        Filter tags", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  g", key_style),
        Span::styled("        Group by tag", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  /", key_style),
        Span::styled("        Search", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  x", key_style),
        Span::styled("        Quick exec", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  K", key_style),
        Span::styled("        Setup SSH key", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  f", key_style),
        Span::styled("        Tunnel on/off", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  hjkl", key_style),
        Span::styled("    Navigate", desc_style),
    ]));

    frame.render_widget(Paragraph::new(col1_lines), columns[0]);

    // Column 2: Detail View + File Manager + Snippets
    let mut col2_lines = vec![Line::from("")];
    col2_lines.push(Line::from(Span::styled(" DETAIL VIEW", section_style)));
    col2_lines.push(Line::from(vec![
        Span::styled("  Enter", key_style),
        Span::styled("    Connect", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  r", key_style),
        Span::styled("        Refresh", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  Esc", key_style),
        Span::styled("      Back", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  4-9", key_style),
        Span::styled("      Quick view", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  f", key_style),
        Span::styled("        Tunnel on/off", desc_style),
    ]));
    col2_lines.push(Line::from(""));

    col2_lines.push(Line::from(Span::styled(" FILE MANAGER", section_style)));
    col2_lines.push(Line::from(vec![
        Span::styled("  hjkl", key_style),
        Span::styled("    Navigate", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  Tab", key_style),
        Span::styled("      Switch panel", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  Space", key_style),
        Span::styled("    Mark file", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  c", key_style),
        Span::styled("        Copy", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  p", key_style),
        Span::styled("        Paste", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  n", key_style),
        Span::styled("        New dir", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  R", key_style),
        Span::styled("        Rename", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  D", key_style),
        Span::styled("        Delete", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  H", key_style),
        Span::styled("        Connect", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  .", key_style),
        Span::styled("        Toggle hidden", desc_style),
    ]));
    if cfg!(windows) {
        col2_lines.push(Line::from(vec![
            Span::styled("  d", key_style),
            Span::styled("        Next drive", desc_style),
        ]));
    }
    col2_lines.push(Line::from(""));

    col2_lines.push(Line::from(Span::styled(" SNIPPETS", section_style)));
    col2_lines.push(Line::from(vec![
        Span::styled("  Enter", key_style),
        Span::styled("    Run snippet", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  n", key_style),
        Span::styled("        New", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  e", key_style),
        Span::styled("        Edit", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  d", key_style),
        Span::styled("        Delete", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  b", key_style),
        Span::styled("        Broadcast", desc_style),
    ]));
    col2_lines.push(Line::from(vec![
        Span::styled("  /", key_style),
        Span::styled("        Search", desc_style),
    ]));

    frame.render_widget(Paragraph::new(col2_lines), columns[1]);

    // Column 3: Terminal + Footer
    let mut col3_lines = vec![Line::from("")];
    col3_lines.push(Line::from(Span::styled(" TERMINAL", section_style)));
    col3_lines.push(Line::from(vec![
        Span::styled("  Ctrl+T", key_style),
        Span::styled("   New tab", desc_style),
    ]));
    col3_lines.push(Line::from(vec![
        Span::styled("  Ctrl+W", key_style),
        Span::styled("   Close tab", desc_style),
    ]));
    col3_lines.push(Line::from(vec![
        Span::styled("  Ctrl+N", key_style),
        Span::styled("   Next tab", desc_style),
    ]));
    col3_lines.push(Line::from(vec![
        Span::styled("  Ctrl+\\", key_style),
        Span::styled("   V-split", desc_style),
    ]));
    col3_lines.push(Line::from(vec![
        Span::styled("  Ctrl+]", key_style),
        Span::styled("   H-split", desc_style),
    ]));
    col3_lines.push(Line::from(vec![
        Span::styled("  Ctrl+Q", key_style),
        Span::styled("   Exit", desc_style),
    ]));
    col3_lines.push(Line::from(""));
    col3_lines.push(Line::from(Span::styled(" COPY TEXT", section_style)));
    col3_lines.push(Line::from(vec![
        Span::styled("  Mouse drag", key_style),
        Span::styled(" Select text", desc_style),
    ]));
    col3_lines.push(Line::from(vec![
        Span::styled("  Cmd+C/Ctrl+C", key_style),
        Span::styled(" Copy", desc_style),
    ]));
    col3_lines.push(Line::from(vec![Span::styled(
        "  (Terminal screen only)",
        Style::default()
            .fg(theme.text_secondary)
            .add_modifier(Modifier::ITALIC),
    )]));
    col3_lines.push(Line::from(""));
    col3_lines.push(Line::from(""));
    col3_lines.push(Line::from(""));
    col3_lines.push(Line::from(""));
    col3_lines.push(Line::from(""));
    col3_lines.push(Line::from(Span::styled(
        " Press Esc or ? to close",
        Style::default()
            .fg(theme.text_muted)
            .add_modifier(Modifier::ITALIC),
    )));

    frame.render_widget(Paragraph::new(col3_lines), columns[2]);
}

// ---------------------------------------------------------------------------
// Host form (Add / Edit)
// ---------------------------------------------------------------------------

/// Renders the host add/edit form popup.
///
/// `title` is either `"Add Host"` or `"Edit Host"`.
pub fn render_host_form(frame: &mut Frame, form: &HostForm, title: &str, theme: &Theme) {
    // Taller popup to fit all fields.
    // One row for each label and input, plus top padding, the hint and its spacer,
    // and the border. Sizing to that rather than to a fixed share of the screen
    // keeps the last field on screen when the terminal is short.
    let frame_area = frame.area();
    let needed = u32::from(FORM_FIELD_LABELS.len() as u16) * 2 + 5;
    let percent = (needed * 100)
        .div_ceil(u32::from(frame_area.height.max(1)))
        .clamp(50, 100) as u16;
    let area = centred_rect(70, percent, frame_area);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", title))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.success_border));

    // Split: inner area = top padding + one row per field + bottom hint.
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // A terminal too short for every field shows the ones around the focus;
    // the padding and spacer rows point at the ones scrolled out.
    let capacity = usize::from(inner.height.saturating_sub(3) / 2);
    let window = field_window(FORM_FIELD_LABELS.len(), form.focused_field, capacity);
    let num_fields = window.len();
    // 1 blank line top + 2 lines per field (label + input) + 2 hint lines
    let mut constraints: Vec<Constraint> = Vec::with_capacity(num_fields * 2 + 3);
    constraints.push(Constraint::Length(1)); // top padding
    for _ in 0..num_fields {
        constraints.push(Constraint::Length(1)); // label
        constraints.push(Constraint::Length(1)); // input box
    }
    constraints.push(Constraint::Length(1)); // spacer
    constraints.push(Constraint::Length(1)); // hint
    constraints.push(Constraint::Min(0)); // remainder

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let focused_style = Style::default()
        .fg(theme.form_focused_fg)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let normal_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.selected_bg);
    let label_style = Style::default().fg(theme.text_secondary);
    let focused_label_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    let more_style = Style::default().fg(theme.text_muted);
    if window.start > 0 {
        let more = format!("  ↑ {} more", window.start);
        frame.render_widget(Paragraph::new(Span::styled(more, more_style)), rows[0]);
    }
    if window.end < FORM_FIELD_LABELS.len() {
        let more = format!("  ↓ {} more", FORM_FIELD_LABELS.len() - window.end);
        frame.render_widget(
            Paragraph::new(Span::styled(more, more_style)),
            rows[1 + num_fields * 2],
        );
    }

    for (row, i) in window.enumerate() {
        let label = FORM_FIELD_LABELS[i];
        let label_row = rows[1 + row * 2];
        let input_row = rows[2 + row * 2];
        let is_focused = i == form.focused_field;

        // Label
        let lbl_span = Span::styled(
            format!("  {}: ", label),
            if is_focused {
                focused_label_style
            } else {
                label_style
            },
        );
        frame.render_widget(Paragraph::new(Line::from(lbl_span)), label_row);

        // Input value with simulated cursor
        let field = &form.fields[i];
        let value_style = if is_focused {
            focused_style
        } else {
            normal_style
        };

        let display = if is_focused {
            // Show cursor as a blinking block by inserting '|' at cursor pos.
            let (before, after) = field.value.split_at(field.cursor.min(field.value.len()));
            format!("  {}|{} ", before, after)
        } else {
            format!("  {} ", field.value)
        };

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(display, value_style))),
            input_row,
        );
    }

    // Hint row at bottom.
    let hint_row_idx = 1 + num_fields * 2 + 1;
    if hint_row_idx < rows.len() {
        let hint = Line::from(vec![
            Span::styled(
                "  Tab",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":next  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(theme.text_success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":save  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(theme.text_warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":cancel", Style::default().fg(theme.text_muted)),
        ]);
        frame.render_widget(Paragraph::new(hint), rows[hint_row_idx]);
    }
}

/// The run of `count` form fields that fits in `capacity` slots and holds the
/// `focused` one. Scrolls only once the focus passes the last slot.
fn field_window(count: usize, focused: usize, capacity: usize) -> Range<usize> {
    let capacity = capacity.clamp(1, count.max(1));
    let start = focused
        .saturating_sub(capacity - 1)
        .min(count.saturating_sub(capacity));
    start..start + capacity.min(count)
}

// ---------------------------------------------------------------------------
// Delete confirmation popup
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Tag filter popup
// ---------------------------------------------------------------------------

/// Renders the tag filter picker popup.
///
/// `selected_idx` is 0-based within the list (0 = "All", 1+ = tag entries).
/// `active_filter` is the currently active tag filter (highlighted in title).
pub fn render_tag_filter_popup(
    frame: &mut Frame,
    tags: &[String],
    selected_idx: usize,
    active_filter: Option<&str>,
    theme: &Theme,
) {
    let area = centred_rect(40, 60, frame.area());
    frame.render_widget(Clear, area);

    let title = match active_filter {
        Some(t) => format!(" Filter by tag [{}] ", t),
        None => " Filter by tag ".to_string(),
    };

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.popup_border));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Build list items: "All" first, then each tag.
    let mut items: Vec<ListItem> = vec![ListItem::new(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            "All (clear filter)",
            Style::default()
                .fg(theme.text_secondary)
                .add_modifier(Modifier::ITALIC),
        ),
    ]))];

    for tag in tags {
        items.push(ListItem::new(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(tag.as_str(), Style::default().fg(theme.text_primary)),
        ])));
    }

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(theme.form_focused_fg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut list_state = ListState::default().with_selected(Some(selected_idx));
    frame.render_stateful_widget(list, inner, &mut list_state);

    // Hint at the bottom if space allows.
    if inner.height > 3 {
        let hint_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "Enter",
                    Style::default()
                        .fg(theme.text_success)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(":select  ", Style::default().fg(theme.text_muted)),
                Span::styled(
                    "Esc",
                    Style::default()
                        .fg(theme.text_warning)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(":close", Style::default().fg(theme.text_muted)),
            ])),
            hint_area,
        );
    }
}

/// Renders a small delete-confirmation popup.
pub fn render_delete_confirm(frame: &mut Frame, host_name: &str, theme: &Theme) {
    let area = centred_rect(50, 25, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Confirm Delete ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.danger_border));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  Delete host '{}'?", host_name),
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ))),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "  This cannot be undone.  ",
            Style::default().fg(theme.text_muted),
        )])),
        rows[2],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                "y",
                Style::default()
                    .fg(theme.text_error)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":Yes  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "n / Esc",
                Style::default()
                    .fg(theme.text_success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":No", Style::default().fg(theme.text_muted)),
        ])),
        rows[3],
    );
}

// ---------------------------------------------------------------------------
// Snippet popups
// ---------------------------------------------------------------------------

/// Renders the snippet add/edit form popup.
pub fn render_snippet_form(frame: &mut Frame, form: &SnippetForm, title: &str, theme: &Theme) {
    let area = centred_rect(70, 85, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", title))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.success_border));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let num_fields = SNIPPET_FORM_FIELD_LABELS.len();
    // 1 blank + 2 lines per field (label + input) + spacer + hint
    let mut constraints: Vec<Constraint> = Vec::with_capacity(num_fields * 2 + 3);
    constraints.push(Constraint::Length(1));
    for _ in 0..num_fields {
        constraints.push(Constraint::Length(1));
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Min(0));

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let focused_style = Style::default()
        .fg(theme.form_focused_fg)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let normal_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.selected_bg);
    let label_style = Style::default().fg(theme.text_secondary);
    let focused_label_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    for (i, label) in SNIPPET_FORM_FIELD_LABELS.iter().enumerate() {
        let label_row = rows[1 + i * 2];
        let input_row = rows[2 + i * 2];
        let is_focused = i == form.focused_field;

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {}: ", label),
                if is_focused {
                    focused_label_style
                } else {
                    label_style
                },
            ))),
            label_row,
        );

        let field = &form.fields[i];
        let display = if is_focused {
            let (before, after) = field.value.split_at(field.cursor.min(field.value.len()));
            format!("  {}|{} ", before, after)
        } else {
            format!("  {} ", field.value)
        };

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                display,
                if is_focused {
                    focused_style
                } else {
                    normal_style
                },
            ))),
            input_row,
        );
    }

    let hint_row_idx = 1 + num_fields * 2 + 1;
    if hint_row_idx < rows.len() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "  Tab",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(":next  ", Style::default().fg(theme.text_muted)),
                Span::styled(
                    "Enter",
                    Style::default()
                        .fg(theme.text_success)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(":save  ", Style::default().fg(theme.text_muted)),
                Span::styled(
                    "Esc",
                    Style::default()
                        .fg(theme.text_warning)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(":cancel", Style::default().fg(theme.text_muted)),
            ])),
            rows[hint_row_idx],
        );
    }
}

/// Renders a delete-confirmation popup for a snippet.
pub fn render_snippet_delete_confirm(frame: &mut Frame, snippet_name: &str, theme: &Theme) {
    let area = centred_rect(55, 25, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Confirm Delete Snippet ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.danger_border));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  Delete snippet '{}'?", snippet_name),
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ))),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  This cannot be undone.",
            Style::default().fg(theme.text_muted),
        ))),
        rows[2],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                "y",
                Style::default()
                    .fg(theme.text_error)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":Yes  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "n / Esc",
                Style::default()
                    .fg(theme.text_success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":No", Style::default().fg(theme.text_muted)),
        ])),
        rows[3],
    );
}

/// Renders the parameter input form for parameterised snippets.
pub fn render_param_input(
    frame: &mut Frame,
    snippet_name: &str,
    param_names: &[String],
    param_fields: &[FormField],
    focused_field: usize,
    theme: &Theme,
) {
    let area = centred_rect(60, 70, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Parameters — {} ", snippet_name))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.text_warning));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let n = param_names.len();
    let mut constraints = vec![Constraint::Length(1)];
    for _ in 0..n {
        constraints.push(Constraint::Length(1));
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Min(0));

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let focused_style = Style::default()
        .fg(theme.form_focused_fg)
        .bg(theme.warning_border)
        .add_modifier(Modifier::BOLD);
    let normal_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.selected_bg);

    for i in 0..n {
        let label_row = rows[1 + i * 2];
        let input_row = rows[2 + i * 2];
        let is_focused = i == focused_field;

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {{{{{}}}}}:", param_names[i]),
                Style::default()
                    .fg(if is_focused {
                        Color::Yellow
                    } else {
                        Color::Gray
                    })
                    .add_modifier(if is_focused {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ))),
            label_row,
        );

        let field = &param_fields[i];
        let display = if is_focused {
            let (before, after) = field.value.split_at(field.cursor.min(field.value.len()));
            format!("  {}|{} ", before, after)
        } else {
            format!("  {} ", field.value)
        };

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                display,
                if is_focused {
                    focused_style
                } else {
                    normal_style
                },
            ))),
            input_row,
        );
    }

    let hint_row_idx = 1 + n * 2 + 1;
    if hint_row_idx < rows.len() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "  Tab",
                    Style::default()
                        .fg(theme.text_warning)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(":next  ", Style::default().fg(theme.text_muted)),
                Span::styled(
                    "Enter",
                    Style::default()
                        .fg(theme.text_success)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(":run  ", Style::default().fg(theme.text_muted)),
                Span::styled(
                    "Esc",
                    Style::default()
                        .fg(theme.text_warning)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(":cancel", Style::default().fg(theme.text_muted)),
            ])),
            rows[hint_row_idx],
        );
    }
}

/// Renders the broadcast host-picker popup.
pub fn render_broadcast_picker(
    frame: &mut Frame,
    hosts: &[Host],
    selected_host_indices: &[usize],
    cursor: usize,
    theme: &Theme,
) {
    let area = centred_rect(60, 75, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Broadcast — Select Hosts ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.popup_border));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Reserve bottom 1 line for hint.
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

    // Scroll to keep cursor visible.
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
            let checked = selected_host_indices.contains(&i);
            let is_cursor = i == cursor;
            let checkbox = if checked {
                Span::styled(
                    "[x] ",
                    Style::default()
                        .fg(theme.text_success)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled("[ ] ", Style::default().fg(theme.text_muted))
            };
            let name = Span::styled(
                h.name.as_str(),
                if is_cursor {
                    Style::default()
                        .fg(theme.text_primary)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text_secondary)
                },
            );
            let item = ListItem::new(Line::from(vec![Span::raw("  "), checkbox, name]));
            if is_cursor {
                item.style(Style::default().bg(theme.selected_bg))
            } else {
                item
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, list_area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Space",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":toggle  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(theme.text_success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":run  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(theme.text_warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":cancel", Style::default().fg(theme.text_muted)),
        ])),
        hint_area,
    );
}

/// Renders the passphrase prompt for an encrypted identity file.
pub fn render_passphrase_prompt(frame: &mut Frame, prompt: &PassphrasePrompt, theme: &Theme) {
    let input = if prompt.unlocking {
        String::from("  Unlocking…")
    } else {
        masked(&prompt.field)
    };
    render_secret_prompt(
        frame,
        theme,
        SecretPrompt {
            title: format!(" Unlock SSH key — {} ", prompt.host_name),
            detail: &prompt.key_path,
            label: "  Passphrase (kept in memory until OmnySSH exits):",
            input,
            error: prompt.error.clone(),
            action: ":unlock  ",
        },
    );
}

/// Renders the prompt for a login password a connection waits on.
pub fn render_password_prompt(frame: &mut Frame, prompt: &PasswordPrompt, theme: &Theme) {
    render_secret_prompt(
        frame,
        theme,
        SecretPrompt {
            title: format!(" SSH login — {} ", prompt.host_name),
            detail: &prompt.login,
            label: "  Password (kept in memory until OmnySSH exits):",
            input: masked(&prompt.field),
            error: prompt
                .retry
                .then(|| String::from("Permission denied, please try again."))
                .or_else(|| {
                    prompt
                        .new_host_key
                        .as_ref()
                        .map(|key| format!("New host key recorded: {key}"))
                }),
            action: ":log in  ",
        },
    );
}

fn masked(field: &FormField) -> String {
    format!("  {}| ", "*".repeat(field.value.chars().count()))
}

/// What a prompt for a secret shows; the secret itself only as stars.
struct SecretPrompt<'a> {
    title: String,
    detail: &'a str,
    label: &'a str,
    input: String,
    error: Option<String>,
    /// The Enter hint, e.g. `":unlock  "`.
    action: &'a str,
}

fn render_secret_prompt(frame: &mut Frame, theme: &Theme, prompt: SecretPrompt<'_>) {
    // Six text rows plus the border, whatever the frame height.
    let screen = frame.area();
    let width = (screen.width * 7 / 10).max(60).min(screen.width);
    let height = 8.min(screen.height);
    let area = Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + (screen.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(prompt.title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1); 6])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {}", prompt.detail),
            Style::default().fg(theme.text_secondary),
        ))),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            prompt.label,
            Style::default().fg(theme.text_primary),
        ))),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            prompt.input,
            Style::default()
                .fg(theme.form_focused_fg)
                .bg(theme.success_border)
                .add_modifier(Modifier::BOLD),
        ))),
        rows[2],
    );

    if let Some(error) = prompt.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {error}"),
                Style::default().fg(theme.text_error),
            ))),
            rows[3],
        );
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "  Enter",
                Style::default()
                    .fg(theme.text_success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(prompt.action, Style::default().fg(theme.text_muted)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(theme.text_warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":cancel", Style::default().fg(theme.text_muted)),
        ])),
        rows[5],
    );
}

/// Renders the single-line quick-execute command-input popup.
pub fn render_quick_execute_input(
    frame: &mut Frame,
    host_name: &str,
    command_field: &FormField,
    theme: &Theme,
) {
    let area = centred_rect(65, 20, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Quick Execute — {} ", host_name))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.success_border));

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

    // Command input row with cursor.
    let (before, after) = command_field
        .value
        .split_at(command_field.cursor.min(command_field.value.len()));
    let display = format!("  {}|{} ", before, after);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            display,
            Style::default()
                .fg(theme.form_focused_fg)
                .bg(theme.success_border)
                .add_modifier(Modifier::BOLD),
        ))),
        rows[0],
    );

    // Hint row.
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "  Enter",
                Style::default()
                    .fg(theme.text_success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":run  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(theme.text_warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":cancel", Style::default().fg(theme.text_muted)),
        ])),
        rows[1],
    );
}

/// Spinner frames for pending execution indicator.
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const _: () = assert!(
    !SPINNER_FRAMES.is_empty(),
    "SPINNER_FRAMES must not be empty"
);

/// Renders the snippet execution results popup.
pub fn render_snippet_results(
    frame: &mut Frame,
    entries: &[SnippetResultEntry],
    scroll: usize,
    tick_count: u64,
    theme: &Theme,
) {
    let area = centred_rect(80, 85, frame.area());
    frame.render_widget(Clear, area);

    let spinner = SPINNER_FRAMES[(tick_count as usize / 2) % SPINNER_FRAMES.len()];

    let block = Block::default()
        .title(" Results ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.popup_border));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if entries.is_empty() {
        return;
    }

    if entries.len() == 1 {
        // Single host — full inner area with scrollable text.
        render_single_result(frame, inner, &entries[0], scroll, spinner, theme);
    } else {
        // Multiple hosts — split vertically, one section per host.
        let n = entries.len().min(6); // cap at 6 to avoid tiny sections
        let constraints: Vec<Constraint> = (0..n).map(|_| Constraint::Min(3)).collect();
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);
        for (i, entry) in entries.iter().take(n).enumerate() {
            render_single_result(frame, sections[i], entry, 0, spinner, theme);
        }
    }
}

/// Helper: renders one host's result into `area`.
fn render_single_result(
    frame: &mut Frame,
    area: Rect,
    entry: &SnippetResultEntry,
    scroll: usize,
    spinner: char,
    theme: &Theme,
) {
    if area.height < 2 {
        return;
    }

    // Header line.
    let header_area = Rect { height: 1, ..area };
    let body_area = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };

    let status_span = if entry.pending {
        Span::styled(
            format!(" {} Running… ", spinner),
            Style::default().fg(theme.text_warning),
        )
    } else if entry.output.is_ok() {
        Span::styled(" ✓ Done ", Style::default().fg(theme.text_success))
    } else {
        Span::styled(" ✗ Error ", Style::default().fg(theme.text_error))
    };

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {} — {} ", entry.host_name, entry.snippet_name),
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            status_span,
        ])),
        header_area,
    );

    if area.height < 3 {
        return;
    }

    // Body: output text or error message.
    let text = match &entry.output {
        Ok(out) if !out.is_empty() => out.as_str(),
        Ok(_) if entry.pending => "",
        Ok(_) => "(no output)",
        Err(err) => err.as_str(),
    };

    let text_color = if entry.output.is_err() {
        theme.text_error
    } else {
        theme.text_secondary
    };

    let lines: Vec<Line> = text
        .lines()
        .skip(scroll)
        .map(|l| {
            Line::from(Span::styled(
                format!(" {}", l),
                Style::default().fg(text_color),
            ))
        })
        .collect();

    let hint = if !entry.pending {
        Line::from(vec![
            Span::styled(
                "j/k",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":scroll  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(theme.text_warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":close", Style::default().fg(theme.text_muted)),
        ])
    } else {
        Line::from(Span::styled(
            "  Waiting for result…",
            Style::default().fg(theme.text_muted),
        ))
    };

    // Reserve last line for hint.
    let text_area = Rect {
        height: body_area.height.saturating_sub(1),
        ..body_area
    };
    let hint_area = Rect {
        y: body_area.y + body_area.height.saturating_sub(1),
        height: 1,
        ..body_area
    };

    frame.render_widget(Paragraph::new(lines), text_area);
    frame.render_widget(Paragraph::new(hint), hint_area);
}

// ---------------------------------------------------------------------------
// SSH Key Setup popups
// ---------------------------------------------------------------------------

/// Renders the SSH key setup confirmation dialog.
///
/// Shows host name, warns about the operation (disabling password auth),
/// and offers y/Enter to confirm or n/Esc to cancel.
pub fn render_key_setup_confirm(
    frame: &mut Frame,
    host: Option<&omnyssh_core::ssh::client::Host>,
    theme: &Theme,
) {
    let area = centred_rect(55, 40, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" SSH Key Setup ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.warning_border));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // top padding
            Constraint::Length(1), // title
            Constraint::Length(1), // blank
            Constraint::Length(1), // host name
            Constraint::Length(1), // blank
            Constraint::Length(1), // warning 1
            Constraint::Length(1), // warning 2
            Constraint::Length(1), // warning 3
            Constraint::Length(1), // blank
            Constraint::Length(1), // hint
            Constraint::Min(0),
        ])
        .split(inner);

    let host_name = host.map(|h| h.name.as_str()).unwrap_or("?");
    let host_addr = host.map(|h| h.hostname.as_str()).unwrap_or("?");

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  Configure SSH key authentication",
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ))),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  Host: ", Style::default().fg(theme.text_secondary)),
            Span::styled(
                format!("{} ({})", host_name, host_addr),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        rows[3],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  This will:",
            Style::default().fg(theme.text_secondary),
        ))),
        rows[5],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  • Generate an Ed25519 key pair",
            Style::default().fg(theme.text_secondary),
        ))),
        rows[6],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  • Disable password auth on server (if sudo available)",
            Style::default().fg(theme.text_warning),
        ))),
        rows[7],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                "y / Enter",
                Style::default()
                    .fg(theme.text_success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":confirm  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "n / Esc",
                Style::default()
                    .fg(theme.text_warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":cancel", Style::default().fg(theme.text_muted)),
        ])),
        rows[9],
    );
}

/// Renders the SSH key setup progress dialog.
///
/// Shows all 6 steps with status indicators:
/// - `✓` for completed steps (green)
/// - Animated spinner for the current step (yellow)
/// - `·` for pending steps (dimmed)
pub fn render_key_setup_progress(
    frame: &mut Frame,
    host_name: &str,
    current_step: Option<&omnyssh_core::ssh::key_setup::KeySetupStep>,
    theme: &Theme,
) {
    use omnyssh_core::ssh::key_setup::KeySetupStep;

    let area = centred_rect(55, 55, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Key Setup — {} ", host_name))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.warning_border));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let all_steps = KeySetupStep::all_steps();

    // 1 top padding + 1 header + 1 blank + 6 steps + 1 blank + 1 hint + remainder
    let mut constraints = vec![
        Constraint::Length(1), // top padding
        Constraint::Length(1), // header
        Constraint::Length(1), // blank
    ];
    for _ in &all_steps {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // blank
    constraints.push(Constraint::Length(1)); // hint
    constraints.push(Constraint::Min(0));

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    // Header
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  Setting up SSH key authentication…",
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ))),
        rows[1],
    );

    // Determine the numeric index of the current step (1-based).
    let current_idx = current_step.map(|s| *s as usize);

    // Spinner character for the active step.
    let spinner_char = SPINNER_FRAMES[(frame.count() / 2) % SPINNER_FRAMES.len()];

    for (i, step) in all_steps.iter().enumerate() {
        let step_num = *step as usize; // 1-based
        let row = rows[3 + i];

        let (icon, icon_style, desc_style) = if let Some(cur) = current_idx {
            if step_num < cur {
                // Completed
                (
                    "  ✓ ".to_string(),
                    Style::default()
                        .fg(theme.text_success)
                        .add_modifier(Modifier::BOLD),
                    Style::default().fg(theme.text_secondary),
                )
            } else if step_num == cur {
                // Current — animated spinner
                (
                    format!("  {} ", spinner_char),
                    Style::default()
                        .fg(theme.text_warning)
                        .add_modifier(Modifier::BOLD),
                    Style::default()
                        .fg(theme.text_warning)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                // Pending
                (
                    "  · ".to_string(),
                    Style::default().fg(theme.text_muted),
                    Style::default().fg(theme.text_muted),
                )
            }
        } else {
            // No step started yet — all pending
            (
                "  · ".to_string(),
                Style::default().fg(theme.text_muted),
                Style::default().fg(theme.text_muted),
            )
        };

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(icon, icon_style),
                Span::styled(step.description(), desc_style),
            ])),
            row,
        );
    }

    // Hint at the bottom
    let hint_row = rows[3 + all_steps.len() + 1];
    let is_done = current_step.is_some_and(|s| *s as usize >= 6);
    if is_done {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "  Esc",
                    Style::default()
                        .fg(theme.text_warning)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(":close", Style::default().fg(theme.text_muted)),
            ])),
            hint_row,
        );
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  Please wait…",
                Style::default()
                    .fg(theme.text_muted)
                    .add_modifier(Modifier::ITALIC),
            ))),
            hint_row,
        );
    }
}

// ---------------------------------------------------------------------------
// Update popup
// ---------------------------------------------------------------------------

/// Label shown on an update-popup button.
fn update_button_label(button: UpdateButton, can_self_update: bool) -> &'static str {
    match button {
        UpdateButton::Primary if can_self_update => "Update now",
        UpdateButton::Primary => "Remind me later",
        UpdateButton::Skip => "Skip this version",
        UpdateButton::Disable => "Don't check again",
    }
}

/// Renders the startup "update available" popup.
pub fn render_update(frame: &mut Frame, popup: &UpdatePopup, theme: &Theme) {
    let area = centred_rect(62, 42, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Update Available ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Reserve the last two rows for the action buttons and the hint.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    // ── Body text ──────────────────────────────────────────────
    let mut lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Version  ", Style::default().fg(theme.text_secondary)),
            Span::styled(
                popup.info.current.clone(),
                Style::default().fg(theme.text_muted),
            ),
            Span::styled("  →  ", Style::default().fg(theme.text_secondary)),
            Span::styled(
                popup.info.latest.clone(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    let info_style = Style::default().fg(theme.text_primary);
    let emphasis_style = Style::default()
        .fg(theme.text_warning)
        .add_modifier(Modifier::BOLD);

    match &popup.phase {
        UpdatePopupPhase::Prompt { .. } => {
            if popup.info.can_self_update {
                lines.push(Line::from(Span::styled(
                    "  This update can be downloaded and installed now.",
                    info_style,
                )));
                lines.push(Line::from(Span::styled(
                    "  The archive checksum is verified before installing.",
                    Style::default().fg(theme.text_muted),
                )));
            } else if let Some(cmd) = popup.info.method.upgrade_command() {
                lines.push(Line::from(Span::styled(
                    "  Update it with your package manager:",
                    info_style,
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("    {cmd}"),
                    emphasis_style,
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "  Download the new release from:",
                    info_style,
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("    {}", popup.info.release_url()),
                    emphasis_style,
                )));
            }
        }
        UpdatePopupPhase::Installing => {
            let spinner = SPINNER_FRAMES[(frame.count() / 2) % SPINNER_FRAMES.len()];
            lines.push(Line::from(Span::styled(
                format!("  {spinner} Downloading and installing — please wait…"),
                Style::default().fg(theme.text_warning),
            )));
        }
        UpdatePopupPhase::Done { message, ok } => {
            let color = if *ok {
                theme.text_success
            } else {
                theme.text_error
            };
            for line in message.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default().fg(color),
                )));
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), rows[0]);

    // ── Action row ─────────────────────────────────────────────
    if let UpdatePopupPhase::Prompt { selected } = &popup.phase {
        let mut spans: Vec<Span> = vec![Span::raw("  ")];
        for (i, &button) in UPDATE_BUTTONS.iter().enumerate() {
            let label = update_button_label(button, popup.info.can_self_update);
            let style = if i == *selected {
                Style::default()
                    .fg(theme.form_focused_fg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_secondary)
            };
            spans.push(Span::styled(format!(" {label} "), style));
            spans.push(Span::raw("  "));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), rows[1]);
    }

    // ── Hint row ───────────────────────────────────────────────
    let hint = match &popup.phase {
        UpdatePopupPhase::Prompt { .. } => Line::from(vec![
            Span::styled(
                "  ←/→",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":select  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(theme.text_success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":confirm  ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(theme.text_warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":dismiss", Style::default().fg(theme.text_muted)),
        ]),
        UpdatePopupPhase::Installing => Line::from(Span::styled(
            "  Please wait…",
            Style::default()
                .fg(theme.text_muted)
                .add_modifier(Modifier::ITALIC),
        )),
        UpdatePopupPhase::Done { .. } => Line::from(vec![
            Span::styled(
                "  Enter / Esc",
                Style::default()
                    .fg(theme.text_warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":close", Style::default().fg(theme.text_muted)),
        ]),
    };
    frame.render_widget(Paragraph::new(hint), rows[2]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_window_always_holds_the_focused_field() {
        for count in 1..=12 {
            for capacity in 0..=13 {
                for focused in 0..count {
                    let window = field_window(count, focused, capacity);
                    assert!(
                        window.contains(&focused),
                        "{count} fields, {capacity} slots, focus {focused}: {window:?}"
                    );
                    assert_eq!(window.len(), capacity.clamp(1, count));
                    assert!(window.end <= count);
                }
            }
        }
    }

    #[test]
    fn the_form_window_scrolls_only_past_the_last_slot() {
        // 12 fields at the 80x24 minimum leave room for 9.
        assert_eq!(field_window(12, 0, 9), 0..9);
        assert_eq!(field_window(12, 8, 9), 0..9);
        assert_eq!(field_window(12, 9, 9), 1..10);
        assert_eq!(field_window(12, 11, 9), 3..12);
        assert_eq!(field_window(12, 11, 20), 0..12);
    }

    #[test]
    fn the_passphrase_prompt_fits_the_smallest_screen() {
        let prompt = PassphrasePrompt {
            host_name: String::from("lab"),
            key_path: String::from("/home/me/.ssh/test_key"),
            field: FormField::with_value("secret"),
            error: Some(String::from("wrong passphrase")),
            unlocking: false,
        };
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_passphrase_prompt(frame, &prompt, &Theme::default()))
            .expect("draw");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for text in [
            "Unlock SSH key — lab",
            "/home/me/.ssh/test_key",
            "******|",
            "wrong passphrase",
            "Enter:unlock",
        ] {
            assert!(screen.contains(text), "{text:?} is not on screen");
        }
        assert!(!screen.contains("secret"), "the passphrase is never drawn");
    }

    #[test]
    fn the_password_prompt_names_the_login_and_hides_the_password() {
        let prompt = PasswordPrompt {
            request_id: 1,
            host_name: String::from("udm"),
            login: String::from("root@192.168.1.1"),
            retry: true,
            new_host_key: None,
            field: FormField::with_value("hunter2"),
        };
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_password_prompt(frame, &prompt, &Theme::default()))
            .expect("draw");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for text in [
            "SSH login — udm",
            "root@192.168.1.1",
            "*******|",
            "Permission denied, please try again.",
            "Enter:log in",
        ] {
            assert!(screen.contains(text), "{text:?} is not on screen");
        }
        assert!(!screen.contains("hunter2"), "the password is never drawn");
    }
}
