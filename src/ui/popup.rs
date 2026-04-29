use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::app::{
    FormField, HostForm, SnippetForm, SnippetResultEntry, FORM_FIELD_LABELS,
    SNIPPET_FORM_FIELD_LABELS,
};
use crate::ssh::client::Host;
use crate::ui::theme::Theme;

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
// Generic popup
// ---------------------------------------------------------------------------

/// Renders a centred modal popup with `title` and `content` text lines.
pub fn render_popup(frame: &mut Frame, title: &str, content: &[&str], theme: &Theme) {
    let area = centred_rect(60, 40, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", title))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.popup_border));

    let lines: Vec<Line> = content
        .iter()
        .map(|&s| {
            Line::from(Span::styled(
                format!("  {}", s),
                Style::default().fg(theme.text_primary),
            ))
        })
        .collect();

    frame.render_widget(Paragraph::new(lines).block(block), area);
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
        Span::styled("  /", key_style),
        Span::styled("        Search", desc_style),
    ]));
    col1_lines.push(Line::from(vec![
        Span::styled("  x", key_style),
        Span::styled("        Quick exec", desc_style),
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
        Span::styled("  Ctrl+N", key_style),
        Span::styled("   New tab", desc_style),
    ]));
    col3_lines.push(Line::from(vec![
        Span::styled("  Ctrl+W", key_style),
        Span::styled("   Close tab", desc_style),
    ]));
    col3_lines.push(Line::from(vec![
        Span::styled("  Tab", key_style),
        Span::styled("      Next tab", desc_style),
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
    let area = centred_rect(70, 80, frame.area());
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

    let num_fields = FORM_FIELD_LABELS.len();
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

    for (i, label) in FORM_FIELD_LABELS.iter().enumerate() {
        let label_row = rows[1 + i * 2];
        let input_row = rows[2 + i * 2];
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
// Quick View popup
// ---------------------------------------------------------------------------

/// Renders a Quick View popup for service-specific details.
///
/// Shows a scrollable view of service metrics, status, and suggested actions.
/// This is a simplified implementation showing a placeholder that can be
/// enhanced to display actual service data.
pub fn render_quick_view(
    frame: &mut Frame,
    service_kind: &crate::event::ServiceKind,
    _scroll: usize,
    theme: &Theme,
) {
    let area = centred_rect(70, 70, frame.area());
    frame.render_widget(Clear, area);

    let service_name = match service_kind {
        crate::event::ServiceKind::Docker => "Docker Containers",
        crate::event::ServiceKind::Nginx => "Nginx Status",
        crate::event::ServiceKind::PostgreSQL => "PostgreSQL Connections",
        crate::event::ServiceKind::Redis => "Redis Info",
        crate::event::ServiceKind::NodeJS => "Node.js Processes",
    };

    let block = Block::default()
        .title(format!(" Quick View: {} ", service_name))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.popup_border));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Simple placeholder content
    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {} Quick View", service_name),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Detailed service metrics will appear here.",
            Style::default().fg(theme.text_secondary),
        )),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "  Press Esc to close",
            Style::default()
                .fg(theme.text_muted)
                .add_modifier(Modifier::ITALIC),
        )),
    ];

    frame.render_widget(Paragraph::new(content), inner);
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
    host: Option<&crate::ssh::client::Host>,
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
    current_step: Option<&crate::ssh::key_setup::KeySetupStep>,
    theme: &Theme,
) {
    use crate::ssh::key_setup::KeySetupStep;

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
