//! Terminal multi-session screen state and the `App` methods that open,
//! switch, scroll, and paste into PTY-backed tabs.

use std::sync::{Arc, Mutex};

use super::*;
use omnyssh_core::event::SessionId;
use omnyssh_core::ssh::pty::PtyManager;

// ---------------------------------------------------------------------------
// Terminal multi-session view state
// ---------------------------------------------------------------------------

/// Direction of the split-view layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitDirection {
    /// Two panes side-by-side (left | right).
    Vertical,
    /// Two panes stacked (top / bottom).
    Horizontal,
}

/// Which pane currently has keyboard focus in split-view mode.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SplitFocus {
    #[default]
    Primary,
    Secondary,
}

/// Layout description when split-view is active.
#[derive(Debug, Clone)]
pub struct SplitView {
    pub direction: SplitDirection,
    /// Index into [`TerminalView::tabs`] shown in the secondary pane.
    pub secondary_tab: usize,
}

/// A single open SSH/PTY tab.
pub struct TermTab {
    /// Unique identifier matching the [`PtyManager`] session.
    pub session_id: SessionId,
    /// Display name (= `host.name`).
    pub host_name: String,
    /// Set to `true` when new output arrives while this tab is not focused,
    /// providing an unread-output indicator in the tab bar.
    pub has_activity: bool,
    /// Shared VT100 parser — written by the PTY reader thread, snapshotted by
    /// the render loop.  Stored here (in ViewState) so rendering does not
    /// require access to the PtyManager.
    pub parser: Arc<Mutex<vt100::Parser>>,
    /// Number of lines scrolled back from the live screen (0 = at the bottom).
    /// Set via mouse-wheel; reset to 0 when the user types anything.
    pub scroll_offset: usize,
    /// Any output reached the tab, a password prompt included.
    pub saw_output: bool,
    /// The remote side ended the session; the tab stays until Enter or Esc.
    pub ended: bool,
}

/// Written under a session's last output when it ends, byte for byte as in the
/// GUI: dim, then the cursor hidden and any mouse reporting the dead app left on
/// turned off, since the tab takes no more input.
const END_LINE: &[u8] = b"\r\n\x1b[2m[Connection closed. Press Enter to close this tab.]\x1b[0m\
    \x1b[?25l\x1b[?9;1000;1002;1003l";

impl TermTab {
    /// Marks the tab ended and writes the end line under its output, so whatever
    /// the server said last stays readable. `false` for a tab that never showed
    /// anything (a failed connect): there is nothing to keep.
    pub fn end(&mut self) -> bool {
        if !self.saw_output {
            return false;
        }
        let Ok(mut parser) = self.parser.lock() else {
            return false;
        };
        parser.process(END_LINE);
        self.ended = true;
        true
    }
}

/// Host-picker popup for opening a new terminal tab.
#[derive(Debug, Clone, Default)]
pub struct TermHostPicker {
    /// Index of the currently highlighted host in `AppState.hosts`.
    pub cursor: usize,
    /// If `true`, the picker is in "switch pane mode" — selecting a host replaces
    /// the focused pane's tab rather than creating a new tab.
    pub switch_pane_mode: bool,
}

/// All UI state for the Terminal screen.
#[derive(Default)]
pub struct TerminalView {
    /// Ordered list of open tabs.
    pub tabs: Vec<TermTab>,
    /// Index of the focused (primary) tab.
    pub active_tab: usize,
    /// Active split-view layout, if any.
    pub split: Option<SplitView>,
    /// Which pane has keyboard focus when split-view is active.
    pub split_focus: SplitFocus,
    /// Host-picker popup for creating a new tab (Ctrl+T).
    pub host_picker: Option<TermHostPicker>,
    /// When `true`, the next digit key 1–9 jumps directly to that tab.
    /// Activated by the next-tab key (`Ctrl+N` by default, which also cycles
    /// to the next tab).
    pub tab_select_mode: bool,
    /// When Enter or Esc last closed an ended tab. A second press right after
    /// would otherwise act on what took its place.
    pub closed_ended_at: Option<std::time::Instant>,
}

/// How long after an ended tab closes Enter and Esc still count as closing it.
pub const CLOSE_KEY_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

impl TerminalView {
    /// Returns the [`SessionId`] of the currently focused pane, or `None` if
    /// there are no open tabs.
    pub fn active_session_id(&self) -> Option<SessionId> {
        if self.tabs.is_empty() {
            return None;
        }
        let idx = match &self.split {
            Some(sv) if self.split_focus == SplitFocus::Secondary => sv.secondary_tab,
            _ => self.active_tab,
        };
        self.tabs.get(idx).map(|t| t.session_id)
    }

    /// Index of the focused tab when its session has ended.
    pub fn focused_ended_tab(&self) -> Option<usize> {
        let focused = self.active_session_id()?;
        self.tabs
            .iter()
            .position(|t| t.ended && t.session_id == focused)
    }
}

impl App {
    /// Handles a mouse-wheel notch in the terminal screen.
    ///
    /// On the normal screen the focused tab's local scrollback is moved. On the
    /// alternate screen (vim, less, htop, ...) the notch is forwarded to the
    /// foreground application instead, since the local scrollback is empty.
    pub(crate) fn handle_term_scroll(&mut self, delta: i16) {
        let tv = &mut self.view.terminal_view;
        let focused_idx = match &tv.split {
            Some(sv) if tv.split_focus == SplitFocus::Secondary => sv.secondary_tab,
            _ => tv.active_tab,
        };
        let Some(tab) = tv.tabs.get_mut(focused_idx) else {
            return;
        };
        // Inspect the foreground app under a brief parser lock, then release it.
        let action = match tab.parser.lock() {
            Ok(parser) => omnyssh_core::utils::scroll::resolve_scroll(delta, parser.screen()),
            Err(_) => return,
        };
        match action {
            omnyssh_core::utils::scroll::ScrollAction::Scrollback(d) => {
                if d > 0 {
                    // Cap at the vt100 scrollback capacity (1000 lines, see pty.rs).
                    tab.scroll_offset = tab.scroll_offset.saturating_add(d as usize).min(1000);
                } else {
                    tab.scroll_offset = tab.scroll_offset.saturating_sub((-d) as usize);
                }
            }
            omnyssh_core::utils::scroll::ScrollAction::Forward(bytes) => {
                let id = tab.session_id;
                if let Some(mgr) = &mut self.pty_manager {
                    if let Err(e) = mgr.write(id, &bytes) {
                        tracing::warn!("PTY scroll-forward write error for session {id}: {e}");
                    }
                }
            }
        }
    }

    /// Forwards pasted text to the focused terminal tab's PTY.
    ///
    /// The payload is wrapped in bracketed-paste markers when the foreground
    /// application requested them (so `vim` inserts it verbatim without
    /// auto-indent), otherwise it is sent as plain input.
    pub(crate) fn handle_term_paste(&mut self, text: &str) {
        let tv = &mut self.view.terminal_view;
        let focused_idx = match &tv.split {
            Some(sv) if tv.split_focus == SplitFocus::Secondary => sv.secondary_tab,
            _ => tv.active_tab,
        };
        let Some(tab) = tv.tabs.get_mut(focused_idx) else {
            return;
        };
        // Paste is input — jump back to the live screen, like typing.
        tab.scroll_offset = 0;
        let id = tab.session_id;
        // Read the foreground app's bracketed-paste mode under a brief lock.
        let bracketed = tab
            .parser
            .lock()
            .map(|p| p.screen().bracketed_paste())
            .unwrap_or(false);
        let bytes = crate::utils::paste::encode_paste(text, bracketed);
        if let Some(mgr) = &mut self.pty_manager {
            if let Err(e) = mgr.write(id, &bytes) {
                tracing::warn!("PTY paste write error for session {id}: {e}");
            }
        }
    }

    /// Removes the tab of a session the remote side ended, collapsing any split
    /// that referenced it; the last one returns to the Dashboard.
    pub(crate) async fn remove_ended_tab(&mut self, pos: usize) {
        let tv = &mut self.view.terminal_view;
        tv.tabs.remove(pos);
        tv.split = None;
        tv.split_focus = SplitFocus::Primary;
        // Keep the primary tab itself focused, not whichever one slid into its index.
        if pos < tv.active_tab {
            tv.active_tab -= 1;
        }
        if tv.tabs.is_empty() {
            self.state.write().await.screen = Screen::Dashboard;
            self.view.status_message = Some("SSH session closed.".to_string());
        } else {
            tv.active_tab = tv.active_tab.min(tv.tabs.len().saturating_sub(1));
        }
    }

    /// Opens a new PTY terminal tab for `AppState.hosts[host_idx]`.
    ///
    /// Switches to the Terminal screen and sets `active_tab` to the new tab.
    /// Reports errors in the status bar without panicking.
    pub(crate) async fn open_term_tab(&mut self, host_idx: usize) {
        let host = {
            let state = self.state.read().await;
            state.hosts.get(host_idx).cloned()
        };
        let Some(host) = host else {
            self.view.status_message = Some("No such host.".to_string());
            return;
        };
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        // Reserve rows: status bar (1) + tab bar (1) + pane border top+bottom (2) = 4.
        // Reserve cols: pane border left+right (2) = 2.
        let pty_rows = rows.saturating_sub(4);
        let pty_cols = cols.saturating_sub(2);
        let mgr = self.pty_manager.get_or_insert_with(PtyManager::new);
        match mgr.open(&host, pty_cols, pty_rows, self.core_tx.clone()) {
            Ok(session_id) => {
                let Some(parser) = mgr.parser_for(session_id) else {
                    tracing::error!(
                        session = session_id,
                        "parser not found for freshly created session"
                    );
                    return;
                };
                self.view.terminal_view.tabs.push(TermTab {
                    session_id,
                    host_name: host.name.clone(),
                    has_activity: false,
                    parser,
                    scroll_offset: 0,
                    saw_output: false,
                    ended: false,
                });
                self.view.terminal_view.active_tab =
                    self.view.terminal_view.tabs.len().saturating_sub(1);
                self.state.write().await.screen = Screen::Terminal;
                tracing::info!(
                    "Opened terminal tab for '{}' (session {})",
                    host.name,
                    session_id
                );
            }
            Err(e) => {
                self.view.status_message = Some(format!("PTY error: {e}"));
            }
        }
    }

    /// Switches the focused pane's host connection to a new host.
    /// Closes the existing session for that pane and opens a new one.
    pub(crate) async fn switch_focused_pane_host(&mut self, host_idx: usize) {
        let host = {
            let state = self.state.read().await;
            state.hosts.get(host_idx).cloned()
        };
        let Some(host) = host else {
            self.view.status_message = Some("No such host.".to_string());
            return;
        };

        let tv = &mut self.view.terminal_view;

        // Determine which tab index to replace based on split focus
        let tab_idx = match &tv.split {
            Some(sv) if tv.split_focus == SplitFocus::Secondary => sv.secondary_tab,
            _ => tv.active_tab,
        };

        // Close the old session
        if let Some(old_tab) = tv.tabs.get(tab_idx) {
            if let Some(mgr) = &mut self.pty_manager {
                mgr.close(old_tab.session_id);
                tracing::info!(
                    "Closed terminal session {} for '{}'",
                    old_tab.session_id,
                    old_tab.host_name
                );
            }
        }

        // Open new session
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let pty_rows = rows.saturating_sub(4);
        let pty_cols = cols.saturating_sub(2);
        let mgr = self.pty_manager.get_or_insert_with(PtyManager::new);

        match mgr.open(&host, pty_cols, pty_rows, self.core_tx.clone()) {
            Ok(session_id) => {
                let Some(parser) = mgr.parser_for(session_id) else {
                    tracing::error!(
                        session = session_id,
                        "parser not found for freshly created session"
                    );
                    return;
                };

                // Replace the tab at the current position
                let new_tab = TermTab {
                    session_id,
                    host_name: host.name.clone(),
                    has_activity: false,
                    parser,
                    scroll_offset: 0,
                    saw_output: false,
                    ended: false,
                };

                if let Some(slot) = tv.tabs.get_mut(tab_idx) {
                    *slot = new_tab;
                }

                tracing::info!(
                    "Switched pane {} to host '{}' (session {})",
                    tab_idx,
                    host.name,
                    session_id
                );
            }
            Err(e) => {
                self.view.status_message = Some(format!("PTY error: {e}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn tab(session_id: SessionId) -> TermTab {
        TermTab {
            session_id,
            host_name: String::new(),
            has_activity: false,
            parser: Arc::new(Mutex::new(vt100::Parser::new(24, 80, 0))),
            scroll_offset: 0,
            saw_output: false,
            ended: false,
        }
    }

    // --- TerminalView::active_session_id (P1.7) ---------------------------

    #[test]
    fn active_session_id_none_when_no_tabs() {
        let view = TerminalView::default();
        assert_eq!(view.active_session_id(), None);
    }

    #[test]
    fn active_session_id_returns_active_tab() {
        let view = TerminalView {
            tabs: vec![tab(10), tab(20)],
            active_tab: 1,
            ..Default::default()
        };
        assert_eq!(view.active_session_id(), Some(20));
    }

    #[test]
    fn active_session_id_uses_secondary_pane_when_focused() {
        let view = TerminalView {
            tabs: vec![tab(10), tab(20), tab(30)],
            active_tab: 0,
            split: Some(SplitView {
                direction: SplitDirection::Vertical,
                secondary_tab: 2,
            }),
            split_focus: SplitFocus::Secondary,
            ..Default::default()
        };
        assert_eq!(view.active_session_id(), Some(30));
    }

    #[test]
    fn active_session_id_uses_primary_pane_when_focused() {
        let view = TerminalView {
            tabs: vec![tab(10), tab(20), tab(30)],
            active_tab: 0,
            split: Some(SplitView {
                direction: SplitDirection::Vertical,
                secondary_tab: 2,
            }),
            split_focus: SplitFocus::Primary,
            ..Default::default()
        };
        assert_eq!(view.active_session_id(), Some(10));
    }

    #[test]
    fn active_session_id_none_when_secondary_index_out_of_range() {
        let view = TerminalView {
            tabs: vec![tab(10)],
            split: Some(SplitView {
                direction: SplitDirection::Horizontal,
                secondary_tab: 99,
            }),
            split_focus: SplitFocus::Secondary,
            ..Default::default()
        };
        assert_eq!(view.active_session_id(), None);
    }

    // --- Ended sessions ------------------------------------------------------

    /// A tab whose session printed `text`.
    fn spoke(session_id: SessionId, text: &str) -> TermTab {
        let mut tab = tab(session_id);
        tab.parser.lock().unwrap().process(text.as_bytes());
        tab.saw_output = true;
        tab
    }

    fn screen_text(tab: &TermTab) -> String {
        tab.parser.lock().unwrap().screen().contents()
    }

    async fn terminal_app(tabs: Vec<TermTab>) -> App {
        let mut app = App::default();
        app.state.write().await.screen = Screen::Terminal;
        app.view.terminal_view.tabs = tabs;
        app
    }

    fn test_terminal() -> Terminal<CrosstermBackend<Stdout>> {
        Terminal::new(CrosstermBackend::new(std::io::stdout())).unwrap()
    }

    async fn screen(app: &App) -> Screen {
        app.state.read().await.screen.clone()
    }

    async fn exit(app: &mut App, session_id: SessionId) {
        let event = CoreEvent::PtyExited(session_id);
        let mut terminal = test_terminal();
        app.handle_core_event(event, &mut terminal).await.unwrap();
    }

    async fn action(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Option<AppAction> {
        let key = KeyEvent::new(code, mods);
        app.handle_key(key).await.unwrap()
    }

    async fn press(app: &mut App, code: KeyCode) {
        let action = action(app, code, KeyModifiers::NONE).await;
        app.process_action(action).await.unwrap();
    }

    #[test]
    fn a_tab_that_showed_output_ends_under_the_end_line() {
        let mut tab = spoke(1, "Permission denied, please try again.");
        assert!(tab.end());
        assert!(tab.ended);
        let text = screen_text(&tab);
        assert!(text.contains("Permission denied, please try again."));
        assert!(text.contains("[Connection closed. Press Enter to close this tab.]"));
        // Input is off, so no cursor blinks under the message.
        assert!(tab.parser.lock().unwrap().screen().hide_cursor());
    }

    #[test]
    fn a_blank_tab_is_not_kept() {
        let mut tab = tab(2);
        assert!(!tab.end());
        assert!(!tab.ended);
        assert!(screen_text(&tab).trim().is_empty());
    }

    // As in the GUI: output that was cleared off the screen still counts.
    #[test]
    fn a_tab_cleared_before_the_exit_is_kept() {
        let mut tab = spoke(3, "bye\x1b[2J\x1b[H");
        assert!(screen_text(&tab).trim().is_empty());
        assert!(tab.end());
    }

    #[tokio::test]
    async fn output_events_mark_the_tab() {
        let mut app = terminal_app(vec![tab(1), tab(2)]).await;
        let mut terminal = test_terminal();

        for id in [1, 2] {
            let event = CoreEvent::PtyOutput(id);
            app.handle_core_event(event, &mut terminal).await.unwrap();
        }
        let tv = &app.view.terminal_view;
        assert!(tv.tabs.iter().all(|t| t.saw_output));
        assert!(
            !tv.tabs[0].has_activity,
            "the focused tab has nothing unread"
        );
        assert!(tv.tabs[1].has_activity);
    }

    #[tokio::test]
    async fn a_remote_exit_keeps_a_tab_with_output_until_enter() {
        let mut app = terminal_app(vec![spoke(1, "Permission denied."), tab(2)]).await;

        // A failed connect printed nothing: its tab goes as before.
        exit(&mut app, 2).await;
        assert_eq!(app.view.terminal_view.tabs.len(), 1);

        exit(&mut app, 1).await;
        let tv = &app.view.terminal_view;
        assert_eq!(tv.tabs.len(), 1, "kept for its last words");
        assert!(tv.tabs[0].ended);
        assert_eq!(screen(&app).await, Screen::Terminal);

        // Nothing is sent to the ended session.
        for (code, modifiers) in [
            (KeyCode::Char('x'), KeyModifiers::NONE),
            (KeyCode::Char('c'), KeyModifiers::CONTROL),
            (KeyCode::Enter, KeyModifiers::SHIFT),
        ] {
            assert!(action(&mut app, code, modifiers).await.is_none());
        }

        press(&mut app, KeyCode::Enter).await;
        assert!(app.view.terminal_view.tabs.is_empty());
        assert_eq!(screen(&app).await, Screen::Dashboard);
        assert_eq!(
            app.view.status_message.as_deref(),
            Some("SSH session closed.")
        );
    }

    #[tokio::test]
    async fn esc_closes_the_ended_pane_that_has_focus() {
        let mut app = terminal_app(vec![spoke(1, "$ "), spoke(2, "bye")]).await;
        let tv = &mut app.view.terminal_view;
        tv.split = Some(SplitView {
            direction: SplitDirection::Vertical,
            secondary_tab: 1,
        });
        tv.split_focus = SplitFocus::Secondary;
        exit(&mut app, 2).await;

        press(&mut app, KeyCode::Esc).await;
        let tv = &app.view.terminal_view;
        assert_eq!(tv.tabs.len(), 1);
        assert_eq!(tv.tabs[0].session_id, 1, "the live tab stays");
        assert!(tv.split.is_none());
        assert_eq!(tv.split_focus, SplitFocus::Primary);
        assert_eq!(screen(&app).await, Screen::Terminal);
    }

    // The ended pane sits before the primary tab: removing it must not hand the
    // keyboard to the live tab after the primary one.
    #[tokio::test]
    async fn closing_an_ended_pane_keeps_the_primary_tab_focused() {
        let tabs = vec![spoke(1, "bye"), spoke(2, "$ "), spoke(3, "$ ")];
        let mut app = terminal_app(tabs).await;
        let tv = &mut app.view.terminal_view;
        tv.active_tab = 1;
        tv.split = Some(SplitView {
            direction: SplitDirection::Horizontal,
            secondary_tab: 0,
        });
        tv.split_focus = SplitFocus::Secondary;
        exit(&mut app, 1).await;

        press(&mut app, KeyCode::Enter).await;
        assert_eq!(app.view.terminal_view.active_session_id(), Some(2));
    }

    #[test]
    fn the_end_line_turns_off_mouse_reporting_left_on() {
        let mut tab = spoke(1, "\x1b[?1002htop");
        let mode = |tab: &TermTab| tab.parser.lock().unwrap().screen().mouse_protocol_mode();
        assert_ne!(mode(&tab), vt100::MouseProtocolMode::None);
        assert!(tab.end());
        assert_eq!(mode(&tab), vt100::MouseProtocolMode::None);
    }

    #[tokio::test]
    async fn keys_still_reach_a_live_tab_next_to_an_ended_one() {
        let mut app = terminal_app(vec![spoke(1, "$ "), spoke(2, "bye")]).await;
        exit(&mut app, 2).await;

        let sent = action(&mut app, KeyCode::Enter, KeyModifiers::NONE).await;
        assert!(matches!(sent, Some(AppAction::TermInput(bytes)) if bytes == b"\r"));
        assert_eq!(app.view.terminal_view.tabs.len(), 2);
    }

    // A double or held Enter must not land in the live shell that takes the
    // ended tab's place; typing there still works.
    #[tokio::test]
    async fn enter_right_after_closing_an_ended_tab_goes_nowhere() {
        let mut app = terminal_app(vec![spoke(1, "$ "), spoke(2, "bye")]).await;
        app.view.terminal_view.active_tab = 1;
        exit(&mut app, 2).await;

        press(&mut app, KeyCode::Enter).await;
        assert_eq!(app.view.terminal_view.active_session_id(), Some(1));
        assert!(action(&mut app, KeyCode::Enter, KeyModifiers::NONE)
            .await
            .is_none());
        let typed = action(&mut app, KeyCode::Char('l'), KeyModifiers::NONE).await;
        assert!(matches!(typed, Some(AppAction::TermInput(bytes)) if bytes == b"l"));

        app.view.terminal_view.closed_ended_at = Some(std::time::Instant::now() - CLOSE_KEY_GRACE);
        let sent = action(&mut app, KeyCode::Enter, KeyModifiers::NONE).await;
        assert!(matches!(sent, Some(AppAction::TermInput(bytes)) if bytes == b"\r"));
    }

    // Closing the last tab lands on the Dashboard, where Enter would open a host.
    #[tokio::test]
    async fn enter_right_after_closing_the_last_ended_tab_opens_nothing() {
        let mut app = terminal_app(vec![spoke(1, "bye")]).await;
        exit(&mut app, 1).await;

        press(&mut app, KeyCode::Enter).await;
        assert_eq!(screen(&app).await, Screen::Dashboard);
        assert!(action(&mut app, KeyCode::Enter, KeyModifiers::NONE)
            .await
            .is_none());
    }
}
