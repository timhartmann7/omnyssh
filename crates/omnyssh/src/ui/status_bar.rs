use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::app::{AppState, FileManagerPopup, Screen, SnippetPopup, ViewState};

/// Renders the bottom status bar with context-sensitive key hints.
pub fn render(frame: &mut Frame, area: Rect, state: &AppState, view: &ViewState) {
    // Apply active theme colours.
    let key_style = Style::default()
        .fg(view.theme.key_badge_fg)
        .bg(view.theme.key_badge_bg)
        .add_modifier(Modifier::BOLD);
    let sep_style = Style::default().fg(view.theme.separator_fg);
    let hint_style = Style::default().fg(view.theme.hint_fg);

    macro_rules! key {
        ($k:expr) => {
            Span::styled(format!(" {} ", $k), key_style)
        };
    }
    macro_rules! hint {
        ($h:expr) => {
            Span::styled(format!(" {} ", $h), hint_style)
        };
    }
    macro_rules! sep {
        () => {
            Span::styled(" │ ", sep_style)
        };
    }

    // If a status message is set, show it instead of key hints.
    if let Some(msg) = &view.status_message {
        let line = Line::from(Span::styled(
            format!(" {}", msg),
            Style::default().fg(Color::Yellow),
        ));
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(Color::Reset)),
            area,
        );
        return;
    }

    let hlv = &view.host_list;

    // Search mode: show search-specific hints.
    if hlv.search_mode {
        let line = Line::from(vec![
            Span::styled(
                " [SEARCH] ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" type to filter  ", Style::default().fg(Color::Gray)),
            key!("Enter"),
            hint!("confirm"),
            sep!(),
            key!("Esc"),
            hint!("clear"),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(Color::Reset)),
            area,
        );
        return;
    }

    // Popup mode: show popup-specific hints.
    if let Some(popup) = &hlv.popup {
        use crate::app::HostPopup;
        let line = match popup {
            HostPopup::Add(_) | HostPopup::Edit { .. } => Line::from(vec![
                key!("Tab"),
                hint!("next field"),
                sep!(),
                key!("Shift+Tab"),
                hint!("prev field"),
                sep!(),
                key!("Enter"),
                hint!("save"),
                sep!(),
                key!("Esc"),
                hint!("cancel"),
            ]),
            HostPopup::DeleteConfirm(_) => Line::from(vec![
                key!("y"),
                hint!("confirm delete"),
                sep!(),
                key!("n / Esc"),
                hint!("cancel"),
            ]),
            HostPopup::KeySetupConfirm(_) => Line::from(vec![
                key!("y / Enter"),
                hint!("confirm setup"),
                sep!(),
                key!("n / Esc"),
                hint!("cancel"),
            ]),
            HostPopup::KeySetupProgress { .. } => Line::from(vec![
                Span::styled(" Setting up SSH keys… ", hint_style),
                sep!(),
                key!("Esc"),
                hint!("close"),
            ]),
        };
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(Color::Reset)),
            area,
        );
        return;
    }

    // Snippets search mode.
    if view.snippets_view.search_mode {
        let line = Line::from(vec![
            Span::styled(
                " [SEARCH] ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" type to filter  ", Style::default().fg(Color::Gray)),
            key!("Enter"),
            hint!("confirm"),
            sep!(),
            key!("Esc"),
            hint!("clear"),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(Color::Reset)),
            area,
        );
        return;
    }

    // Snippets popup hints.
    if let Some(popup) = &view.snippets_view.popup {
        let line = match popup {
            SnippetPopup::Add(_) | SnippetPopup::Edit { .. } => Line::from(vec![
                key!("Tab"),
                hint!("next field"),
                sep!(),
                key!("Shift+Tab"),
                hint!("prev field"),
                sep!(),
                key!("Enter"),
                hint!("save"),
                sep!(),
                key!("Esc"),
                hint!("cancel"),
            ]),
            SnippetPopup::DeleteConfirm(_) => Line::from(vec![
                key!("y"),
                hint!("confirm delete"),
                sep!(),
                key!("n / Esc"),
                hint!("cancel"),
            ]),
            SnippetPopup::ParamInput { .. } => Line::from(vec![
                key!("Tab"),
                hint!("next param"),
                sep!(),
                key!("Enter"),
                hint!("run"),
                sep!(),
                key!("Esc"),
                hint!("cancel"),
            ]),
            SnippetPopup::BroadcastPicker { .. } => Line::from(vec![
                key!("j/k"),
                hint!("navigate"),
                sep!(),
                key!("Space"),
                hint!("toggle"),
                sep!(),
                key!("Enter"),
                hint!("run"),
                sep!(),
                key!("Esc"),
                hint!("cancel"),
            ]),
            SnippetPopup::QuickExecuteInput { .. } => Line::from(vec![
                key!("Enter"),
                hint!("run"),
                sep!(),
                key!("Esc"),
                hint!("cancel"),
            ]),
            SnippetPopup::Results { .. } => Line::from(vec![
                key!("j/k"),
                hint!("scroll"),
                sep!(),
                key!("Esc"),
                hint!("close"),
            ]),
        };
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(Color::Reset)),
            area,
        );
        return;
    }

    // File manager popup hints.
    if matches!(state.screen, Screen::FileManager) {
        if let Some(fm_popup) = &view.file_manager.popup {
            let line = match fm_popup {
                FileManagerPopup::HostPicker { .. } => Line::from(vec![
                    key!("j/k"),
                    hint!("navigate"),
                    sep!(),
                    key!("Enter"),
                    hint!("connect"),
                    sep!(),
                    key!("Esc"),
                    hint!("cancel"),
                ]),
                FileManagerPopup::DeleteConfirm { .. } => Line::from(vec![
                    key!("y"),
                    hint!("confirm delete"),
                    sep!(),
                    key!("n / Esc"),
                    hint!("cancel"),
                ]),
                FileManagerPopup::MkDir(_) | FileManagerPopup::Rename { .. } => Line::from(vec![
                    key!("Enter"),
                    hint!("confirm"),
                    sep!(),
                    key!("Esc"),
                    hint!("cancel"),
                ]),
                FileManagerPopup::TransferProgress {
                    filename,
                    done,
                    total,
                    ..
                } => {
                    let pct = if *total > 0 {
                        ((*done as f64 / *total as f64) * 100.0) as u64
                    } else {
                        0
                    };
                    Line::from(vec![Span::styled(
                        format!(" Transferring: {}  {}% ", filename, pct),
                        hint_style,
                    )])
                }

                FileManagerPopup::ExternalEditConfirm { .. } => Line::from(vec![
                    key!("Enter / y"),
                    hint!("open external editor"),
                    sep!(),
                    key!("n / Esc"),
                    hint!("cancel"),
                ]),
            };
            frame.render_widget(
                Paragraph::new(line).style(Style::default().bg(Color::Reset)),
                area,
            );
            return;
        }
    }

    // Tag popup: show its own hints.
    if hlv.tag_popup_open {
        let line = Line::from(vec![
            key!("j/k"),
            hint!("navigate"),
            sep!(),
            key!("Enter"),
            hint!("select"),
            sep!(),
            key!("Esc"),
            hint!("close"),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(Color::Reset)),
            area,
        );
        return;
    }

    // Global hints only - screen-specific hints are now in page headers.
    let spans = vec![
        key!("1"),
        hint!("Dashboard"),
        sep!(),
        key!("2"),
        hint!("Files"),
        sep!(),
        key!("3"),
        hint!("Snippets"),
        sep!(),
        key!("4"),
        hint!("Terminal"),
        sep!(),
        key!("?"),
        hint!("Help"),
        sep!(),
        key!("q"),
        hint!("Quit"),
    ];

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Reset)),
        area,
    );
}
