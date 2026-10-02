use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout},
    Frame,
};

use crate::app::{AppState, Screen, SnippetPopup, ViewState};

pub mod card;
pub mod dashboard;
pub mod detail_view;
pub mod file_manager;
pub mod host_list;
pub mod popup;
pub mod snippets;
pub mod status_bar;
pub mod terminal;
pub mod theme;

/// Top-level render function. Called once per frame from the main loop.
///
/// Server text such as file and process names lands in cells verbatim, and the
/// backend prints each cell as is, so the finished frame is cleaned here rather
/// than in every widget that might show such a name. A frame drawn any other
/// way skips that cleanup.
pub fn render(frame: &mut Frame, state: &AppState, view: &ViewState) {
    draw(frame, state, view);
    defuse_controls(frame.buffer_mut());
}

/// Replaces each cell holding a C0 or C1 control or DEL with U+FFFD, so no
/// escape sequence reaches the terminal. A tab, common in command output,
/// becomes a space. ratatui gives a control a cell of its own, one column
/// wide, so the layout stays as drawn.
fn defuse_controls(buf: &mut Buffer) {
    for cell in &mut buf.content {
        if cell.symbol() == "\t" {
            cell.set_symbol(" ");
        } else if cell.symbol().contains(char::is_control) {
            cell.set_symbol("\u{fffd}");
        }
    }
}

/// Dispatches to the active screen renderer, then overlays the
/// status bar and any visible popups. Never panics — missing data is shown
/// as placeholders.
fn draw(frame: &mut Frame, state: &AppState, view: &ViewState) {
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

    // Render active screen.
    match state.screen {
        Screen::Dashboard => dashboard::render(frame, content_area, state, view),
        Screen::DetailView => detail_view::render(frame, content_area, state, view),
        Screen::FileManager => file_manager::render(frame, content_area, state, view),
        Screen::Snippets => snippets::render(frame, content_area, state, view),
        Screen::Terminal => terminal::render(frame, content_area, state, view),
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use omnyssh_core::event::{Metrics, ProcessInfo};
    use omnyssh_core::ssh::client::Host;
    use omnyssh_core::ssh::sftp::FileEntry;
    use ratatui::{backend::TestBackend, buffer::Cell, style::Color, Terminal};

    use super::*;
    use crate::app::{FmPanel, SnippetResultEntry, TermTab};

    // Names a hostile server can give a file or a process.
    const HOSTILE: [&str; 3] = ["\x1b]0;PWNED\x07", "\x1b[31mRED", "\x1b[2J"];

    fn draw_app(state: &AppState, view: &ViewState) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal.draw(|f| render(f, state, view)).expect("draw");
        terminal.backend().buffer().clone()
    }

    // The buffer is what the backend prints to the terminal.
    fn assert_no_controls(buf: &Buffer) {
        for cell in &buf.content {
            let symbol = cell.symbol();
            assert!(!symbol.contains(char::is_control), "{symbol:?}");
        }
    }

    fn text(buf: &Buffer) -> String {
        buf.content.iter().map(Cell::symbol).collect()
    }

    #[test]
    fn file_names_reach_the_terminal_defused() {
        let state = AppState {
            screen: Screen::FileManager,
            ..AppState::default()
        };
        let mut view = ViewState::default();
        let fm = &mut view.file_manager;
        fm.active_panel = FmPanel::Remote;
        fm.connected_host = Some("web".into());
        fm.remote.cwd = "/tmp".into();
        fm.remote.entries = HOSTILE
            .iter()
            .map(|name| FileEntry {
                name: name.to_string(),
                path: format!("/tmp/{name}"),
                size: 14,
                is_dir: false,
            })
            .collect();
        // The preview repeats the name in its title.
        fm.preview_path = Some("/tmp/\x1b[2J".into());
        fm.preview_content = Some("hello".into());

        let buf = draw_app(&state, &view);
        assert_no_controls(&buf);
        let text = text(&buf);
        for shown in [
            "\u{fffd}]0;PWNED\u{fffd}",
            "\u{fffd}[31mRED",
            "\u{fffd}[2J ─",
        ] {
            assert!(text.contains(shown), "{shown:?}");
        }
    }

    #[test]
    fn process_names_reach_the_terminal_defused() {
        let mut state = AppState {
            screen: Screen::DetailView,
            ..AppState::default()
        };
        state.hosts.push(Host {
            name: "web".into(),
            ..Host::default()
        });
        let top_processes = HOSTILE
            .iter()
            .map(|name| ProcessInfo {
                name: name.to_string(),
                cpu_percent: 50.0,
                mem_percent: 1.0,
            })
            .collect();
        state.metrics.insert(
            "web".into(),
            Metrics {
                top_processes: Some(top_processes),
                ..Metrics::default()
            },
        );
        let mut view = ViewState::default();
        view.host_list.filtered_indices = vec![0];

        let buf = draw_app(&state, &view);
        assert_no_controls(&buf);
        assert!(text(&buf).contains("\u{fffd}[31mRED"));
    }

    #[test]
    fn tabs_in_snippet_output_draw_as_spaces() {
        let mut view = ViewState::default();
        view.snippets_view.popup = Some(SnippetPopup::Results {
            entries: vec![SnippetResultEntry {
                host_name: "web".into(),
                snippet_name: "hosts".into(),
                output: Ok("127.0.0.1\tlocalhost".into()),
                pending: false,
            }],
            scroll: 0,
        });

        let buf = draw_app(&AppState::default(), &view);
        assert_no_controls(&buf);
        assert!(text(&buf).contains("127.0.0.1 localhost"));
    }

    #[test]
    fn terminal_tabs_keep_their_colours() {
        let state = AppState {
            screen: Screen::Terminal,
            ..AppState::default()
        };
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"\x1b[31mRED\x1b[0m plain");
        let mut view = ViewState::default();
        view.terminal_view.tabs.push(TermTab {
            session_id: 1,
            host_name: "web".into(),
            has_activity: false,
            parser: Arc::new(Mutex::new(parser)),
            scroll_offset: 0,
            saw_output: true,
            ended: false,
        });

        let buf = draw_app(&state, &view);
        assert_no_controls(&buf);
        let at = buf
            .content
            .windows(9)
            .position(|w| w.iter().map(Cell::symbol).collect::<String>() == "RED plain")
            .expect("output drawn");
        let colours: Vec<Color> = buf.content[at..at + 9].iter().map(|c| c.fg).collect();
        assert_eq!(colours[..3], [Color::Red; 3]);
        assert_eq!(colours[3..], [Color::Reset; 6]);
    }
}
