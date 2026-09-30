//! Full-screen in-app editor for remote files under the size cap.
//!
//! Cursor positions are **byte indexes** into UTF-8 strings, but all movement
//! and edits snap to **character boundaries** so multi-byte glyphs (e.g. ╔═║)
//! do not panic on `split_at` / `insert` / `remove`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::{AppAction, ViewState};

// ---------------------------------------------------------------------------
// UTF-8 helpers (byte index ↔ char boundary)
// ---------------------------------------------------------------------------

fn next_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }

    let mut i = idx + 1;

    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }

    i
}

fn prev_char_boundary(s: &str, idx: usize) -> usize {
    if idx == 0 {
        return 0;
    }

    let mut i = idx.min(s.len()).saturating_sub(1);

    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }

    i
}

fn clamp_col(s: &str, col: usize) -> usize {
    let col = col.min(s.len());

    if s.is_char_boundary(col) {
        col
    } else {
        prev_char_boundary(s, col)
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

pub fn render(frame: &mut Frame, area: Rect, view: &ViewState) {
    let Some(ed) = &view.file_editor else {
        return;
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    let title = format!(
        " EDIT {}{} ",
        ed.remote_path,
        if ed.dirty { " [modified]" } else { "" }
    );

    frame.render_widget(
        Paragraph::new(title).style(Style::default().add_modifier(Modifier::BOLD)),
        chunks[0],
    );

    // Number of actual text rows available inside the bordered editor.
    let inner_h = chunks[1].height.saturating_sub(2) as usize;
    let visible_rows = inner_h.max(1);

    // ---------------------------------------------------------------
    // Keep the cursor visible.
    //
    // If the cursor is near the bottom of the file, move the displayed
    // window down so the cursor remains on screen.
    // ---------------------------------------------------------------
    let start = ed.cursor_row.saturating_sub(visible_rows.saturating_sub(1));

    let end = (start + visible_rows).min(ed.lines.len());

    let text: Vec<Line> = ed.lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let row = start + i;
            let mut spans = Vec::new();

            if row == ed.cursor_row {
                let col = clamp_col(line, ed.cursor_col);
                let (left, rest) = line.split_at(col);

                let (mid, right) = if rest.is_empty() {
                    (" ", "")
                } else {
                    let ch = rest.chars().next().unwrap();
                    let n = ch.len_utf8();
                    (&rest[..n], &rest[n..])
                };

                spans.push(Span::raw(left.to_string()));

                spans.push(Span::styled(
                    mid.to_string(),
                    Style::default().add_modifier(Modifier::REVERSED),
                ));

                spans.push(Span::raw(right.to_string()));
            } else {
                spans.push(Span::raw(line.clone()));
            }

            Line::from(spans)
        })
        .collect();

    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(" buffer "))
            .wrap(Wrap { trim: false }),
        chunks[1],
    );

    let hints = if ed.saving {
        "Saving…".to_string()
    } else {
        "Ctrl+S save · Esc close · arrows move · type to insert".to_string()
    };

    frame.render_widget(Paragraph::new(hints), chunks[2]);
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

pub fn handle_input(key: KeyEvent, view: &mut ViewState) -> Option<AppAction> {
    let ed = view.file_editor.as_mut()?;

    if ed.saving {
        return None;
    }

    match key.code {
        KeyCode::Esc => Some(AppAction::EditorQuit),

        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(AppAction::EditorSave)
        }

        KeyCode::Up => {
            ed.cursor_row = ed.cursor_row.saturating_sub(1);
            ed.clamp_cursor();
            None
        }

        KeyCode::Down => {
            if ed.cursor_row + 1 < ed.lines.len() {
                ed.cursor_row += 1;
            }

            ed.clamp_cursor();
            None
        }

        KeyCode::Left => {
            ed.clamp_cursor();

            let line = &ed.lines[ed.cursor_row];

            if ed.cursor_col > 0 {
                ed.cursor_col = prev_char_boundary(line, ed.cursor_col);
            } else if ed.cursor_row > 0 {
                ed.cursor_row -= 1;
                ed.cursor_col = ed.lines[ed.cursor_row].len();
            }

            None
        }

        KeyCode::Right => {
            ed.clamp_cursor();

            let line = &ed.lines[ed.cursor_row];

            if ed.cursor_col < line.len() {
                ed.cursor_col = next_char_boundary(line, ed.cursor_col);
            } else if ed.cursor_row + 1 < ed.lines.len() {
                ed.cursor_row += 1;
                ed.cursor_col = 0;
            }

            None
        }

        KeyCode::Home => {
            ed.cursor_col = 0;
            None
        }

        KeyCode::End => {
            ed.clamp_cursor();
            ed.cursor_col = ed.lines[ed.cursor_row].len();
            None
        }

        KeyCode::Enter => {
            ed.clamp_cursor();

            let row = ed.cursor_row;
            let col = ed.cursor_col;

            let rest = ed.lines[row].split_off(col);

            ed.lines.insert(row + 1, rest);

            ed.cursor_row += 1;
            ed.cursor_col = 0;
            ed.dirty = true;

            None
        }

        KeyCode::Backspace => {
            ed.clamp_cursor();

            if ed.cursor_col > 0 {
                let row = ed.cursor_row;
                let col = ed.cursor_col;

                let line = &mut ed.lines[row];
                let start = prev_char_boundary(line, col);

                line.replace_range(start..col, "");

                ed.cursor_col = start;
                ed.dirty = true;
            } else if ed.cursor_row > 0 {
                let row = ed.cursor_row;

                let cur = ed.lines.remove(row);

                ed.cursor_row -= 1;
                ed.cursor_col = ed.lines[ed.cursor_row].len();

                ed.lines[ed.cursor_row].push_str(&cur);

                ed.dirty = true;
            }

            None
        }

        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            ed.clamp_cursor();

            let row = ed.cursor_row;
            let col = ed.cursor_col;
            let ch_len = c.len_utf8();

            ed.lines[row].insert(col, c);

            ed.cursor_col = col + ch_len;
            ed.dirty = true;

            None
        }

        _ => None,
    }
}
