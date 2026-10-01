use ratatui::{
    layout::{Constraint, Direction, Layout},
    Frame,
};

use crate::app::{AppState, Screen, SnippetPopup, ViewState};

pub mod card;
pub mod dashboard;
pub mod detail_view;
pub mod file_editor;
pub mod file_manager;
pub mod host_list;
pub mod popup;
pub mod snippets;
pub mod status_bar;
pub mod terminal;
pub mod theme;

/// Top-level render function. Called once per frame from the main loop.
///
/// Dispatches to the active screen renderer, then overlays the
/// status bar and any visible popups. Never panics — missing data is shown
/// as placeholders.
pub fn render(frame: &mut Frame, state: &AppState, view: &ViewState) {
    // Check minimum terminal size.
    let area = frame.area();
    if area.width < 80 || area.height < 24 {
        let msg = ratatui::widgets::Paragraph::new(
            "Terminal too small — please resize to at least 80×24.",
        )
        .style(ratatui::style::Style::default().fg(ratatui::style::Color::Red));
        frame.render_widget(msg, area);
        return;
    }

    // Split into content area (top) + status bar (bottom, 1 line).
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let content_area = layout[0];
    let status_area = layout[1];

    // In-app editor overlays the active screen.
    if view.file_editor.is_some() {
        file_editor::render(frame, content_area, view);
    } else {
        match state.screen {
            Screen::Dashboard => dashboard::render(frame, content_area, state, view),
            Screen::DetailView => detail_view::render(frame, content_area, state, view),
            Screen::FileManager => file_manager::render(frame, content_area, state, view),
            Screen::Snippets => snippets::render(frame, content_area, state, view),
            Screen::Terminal => terminal::render(frame, content_area, state, view),
        }
    }

    // Always render status bar.
    status_bar::render(frame, status_area, state, view);

    // Snippet overlay popups — visible regardless of active screen.
    // These are rendered here so they appear on top even when triggered from
    // the Dashboard (e.g. quick-execute via `x`).
    if let Some(snip_popup) = &view.snippets_view.popup {
        match snip_popup {
            SnippetPopup::Results { entries, scroll } => {
                popup::render_snippet_results(
                    frame,
                    entries,
                    *scroll,
                    view.tick_count,
                    &view.theme,
                );
            }
            SnippetPopup::QuickExecuteInput {
                host_name,
                command_field,
            } => {
                popup::render_quick_execute_input(frame, host_name, command_field, &view.theme);
            }
            // All other snippet popups are rendered inside snippets::render.
            _ => {}
        }
    }

    // Render help popup on top if requested.
    if view.show_help {
        popup::render_help(frame, &view.theme);
    }

    // The startup update popup sits above everything else but a passphrase
    // prompt, which takes the keys first.
    if let Some(update_popup) = &view.update_popup {
        popup::render_update(frame, update_popup, &view.theme);
    }

    // Kept off the terminal screen, whose keys belong to the remote shell.
    if !matches!(state.screen, Screen::Terminal) {
        if let Some(prompt) = view.passphrase_prompts.first() {
            popup::render_passphrase_prompt(frame, prompt, &view.theme);
        } else if let Some(prompt) = view.password_prompts.first() {
            popup::render_password_prompt(frame, prompt, &view.theme);
        }
    }
}
