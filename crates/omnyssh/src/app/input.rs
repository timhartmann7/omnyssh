//! Keyboard input routing: the global key handler and the Terminal-screen key
//! handler that decide which `AppAction` (if any) a keystroke produces.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::term_input;
use crate::ui;

impl App {
    pub(crate) async fn handle_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<AppAction>> {
        // Enter or Esc just closed an ended tab: a second press must not act on
        // what took its place, a live shell or, after the last tab, the Dashboard.
        let tv = &self.view.terminal_view;
        if matches!(key.code, KeyCode::Enter | KeyCode::Esc)
            && key.modifiers.is_empty()
            && tv
                .closed_ended_at
                .is_some_and(|at| at.elapsed() < super::terminal::CLOSE_KEY_GRACE)
        {
            return Ok(None);
        }

        let screen = self.state.read().await.screen.clone();

        // A passphrase prompt is modal everywhere but the terminal screen, whose
        // keys belong to the remote shell. It sits above the update popup too, so
        // a passphrase being typed never lands on an update button. Ctrl+C
        // cancels it rather than quitting.
        if !self.view.passphrase_prompts.is_empty() && !matches!(screen, Screen::Terminal) {
            return Ok(self.handle_passphrase_key(key));
        }
        // Same rules for a login password.
        if !self.view.password_prompts.is_empty() && !matches!(screen, Screen::Terminal) {
            return Ok(self.handle_password_key(key));
        }

        // The update popup is modal — it captures all input until dismissed.
        // Ctrl+C still quits as an escape hatch.
        if self.view.update_popup.is_some() {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                return Ok(Some(AppAction::Quit));
            }
            self.handle_update_popup_key(key).await;
            return Ok(None);
        }

        // In-app file editor captures all keys while open.
        if self.view.file_editor.is_some() {
            return Ok(ui::file_editor::handle_input(key, &mut self.view));
        }

        let screen = self.state.read().await.screen.clone();

        // ----------------------------------------------------------------
        // Terminal screen intercepts ALL keys — including Ctrl+C which must
        // be forwarded to the PTY rather than quitting the application.
        // F1/F2/F3 are the escape hatch back to other screens and are
        // handled inside handle_terminal_key.
        // ----------------------------------------------------------------
        if matches!(screen, Screen::Terminal) {
            return Ok(self.handle_terminal_key(key));
        }

        // Ctrl+C always quits regardless of any other state (non-Terminal screens).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(Some(AppAction::Quit));
        }

        // Tag popup takes full priority over everything except Ctrl+C.
        if self.view.host_list.tag_popup_open && matches!(screen, Screen::Dashboard) {
            return Ok(ui::dashboard::handle_tag_popup_input(key, &mut self.view));
        }

        // File manager popup takes full priority on the File Manager screen.
        if self.view.file_manager.popup.is_some() && matches!(screen, Screen::FileManager) {
            return Ok(ui::file_manager::handle_input(key, &mut self.view));
        }

        // Snippet overlay popups (Results, QuickExecuteInput) are visible on
        // any screen and capture all input except Ctrl+C.
        let snip_overlay_active = matches!(
            self.view.snippets_view.popup,
            Some(SnippetPopup::Results { .. }) | Some(SnippetPopup::QuickExecuteInput { .. })
        );
        if snip_overlay_active {
            return Ok(ui::snippets::handle_input(key, &mut self.view));
        }

        // Snippet screen popups (Add/Edit/Delete/ParamInput/BroadcastPicker)
        // or search mode on the snippets screen — delegate to snippets handler.
        let snippet_popup_or_search =
            self.view.snippets_view.popup.is_some() || self.view.snippets_view.search_mode;
        if snippet_popup_or_search && matches!(screen, Screen::Snippets) {
            return Ok(ui::snippets::handle_input(key, &mut self.view));
        }

        // When a host-list popup is open or the user is searching, the screen
        // handler takes full priority (no global key interception except Ctrl+C).
        let popup_or_search =
            self.view.host_list.popup.is_some() || self.view.host_list.search_mode;

        if popup_or_search {
            return Ok(match screen {
                Screen::Dashboard => ui::dashboard::handle_input(key, &mut self.view),
                Screen::FileManager | Screen::Snippets | Screen::Terminal | Screen::DetailView => {
                    None
                }
            });
        }

        // ── Configurable global keys ───────────────────────────
        // These are checked before the main match so user-defined bindings
        // override the defaults without requiring changes to every branch.
        {
            let kb = &self.view.keybindings;
            if key.code == kb.quit {
                return Ok(Some(AppAction::Quit));
            }
            if key.code == kb.dashboard || key.code == KeyCode::Char('1') {
                self.state.write().await.screen = Screen::Dashboard;
                self.view.status_message = None;
                return Ok(None);
            }
            if key.code == kb.file_manager || key.code == KeyCode::Char('2') {
                self.state.write().await.screen = Screen::FileManager;
                self.view.status_message = None;
                self.bootstrap_file_manager().await;
                return Ok(None);
            }
            if key.code == kb.snippets || key.code == KeyCode::Char('3') {
                self.state.write().await.screen = Screen::Snippets;
                self.view.status_message = None;
                return Ok(None);
            }
        }

        // Global key handling.
        match key.code {
            // `q` is handled above via keybindings; kept here as dead arm to
            // avoid changing all the code below but effectively unreachable
            // when the default keybinding is used.
            KeyCode::Char('4') | KeyCode::F(4) => {
                // On DetailView, '4' is used for Quick View (Docker), not switching screens
                if matches!(screen, Screen::DetailView) {
                    // Delegate to DetailView handler for Quick View actions
                    return Ok(ui::detail_view::handle_input(key, &mut self.view));
                } else {
                    // Switch to Terminal screen; open host picker if no tabs are open.
                    self.state.write().await.screen = Screen::Terminal;
                    self.view.status_message = None;
                    if self.view.terminal_view.tabs.is_empty() {
                        self.view.terminal_view.host_picker = Some(TermHostPicker::default());
                    }
                }
            }

            _code if self.view.keybindings.next_screen.matches(key) => {
                // On the File Manager screen this key switches between the two
                // panels (local ↔ remote) rather than cycling to the next screen.
                if matches!(screen, Screen::FileManager) {
                    return Ok(Some(AppAction::FmSwitchPanel));
                }

                let new_screen = {
                    let mut state = self.state.write().await;
                    state.screen = match state.screen {
                        Screen::Dashboard => Screen::FileManager,
                        Screen::DetailView => Screen::Dashboard, // Detail View → Dashboard
                        Screen::FileManager => Screen::Snippets,
                        Screen::Snippets => Screen::Terminal,
                        Screen::Terminal => Screen::Dashboard, // unreachable here (handled above)
                    };
                    state.screen.clone()
                };
                self.view.status_message = None;
                if matches!(new_screen, Screen::FileManager) {
                    self.bootstrap_file_manager().await;
                }
                if matches!(new_screen, Screen::Terminal) && self.view.terminal_view.tabs.is_empty()
                {
                    self.view.terminal_view.host_picker = Some(TermHostPicker::default());
                }
            }

            KeyCode::Char('?') => {
                self.view.show_help = !self.view.show_help;
                // Reset scroll when opening help
                if self.view.show_help {
                    self.view.help_scroll = 0;
                }
            }

            KeyCode::Esc => {
                // Close help popup if it's open
                if self.view.show_help {
                    self.view.show_help = false;
                    self.view.help_scroll = 0;
                    return Ok(None);
                }

                // Clear status message if present
                if self.view.status_message.is_some() {
                    self.view.status_message = None;
                    return Ok(None);
                }

                // Otherwise, delegate to screen handler (e.g., DetailView can return to Dashboard)
                return Ok(match screen {
                    Screen::Dashboard => ui::dashboard::handle_input(key, &mut self.view),
                    Screen::DetailView => ui::detail_view::handle_input(key, &mut self.view),
                    Screen::Snippets => ui::snippets::handle_input(key, &mut self.view),
                    Screen::FileManager => ui::file_manager::handle_input(key, &mut self.view),
                    Screen::Terminal => None,
                });
            }

            _ => {
                // Delegate to the current screen's input handler.
                return Ok(match screen {
                    Screen::Dashboard => ui::dashboard::handle_input(key, &mut self.view),
                    Screen::DetailView => ui::detail_view::handle_input(key, &mut self.view),
                    Screen::Snippets => ui::snippets::handle_input(key, &mut self.view),
                    Screen::FileManager => ui::file_manager::handle_input(key, &mut self.view),
                    // Terminal is handled at the very top of handle_key; unreachable here.
                    Screen::Terminal => None,
                });
            }
        }

        Ok(None)
    }

    fn handle_passphrase_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        let prompt = self.view.passphrase_prompts.first_mut()?;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => Some(AppAction::DismissPassphrase),
            KeyCode::Char('c') if ctrl => Some(AppAction::DismissPassphrase),
            _ if prompt.unlocking => None,
            KeyCode::Enter => Some(AppAction::SubmitPassphrase),
            KeyCode::Backspace => {
                prompt.field.backspace();
                None
            }
            KeyCode::Char(c) if !ctrl => {
                prompt.field.insert_char(c);
                None
            }
            _ => None,
        }
    }

    fn handle_password_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        let prompt = self.view.password_prompts.first_mut()?;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => Some(AppAction::DismissPassword),
            KeyCode::Char('c') if ctrl => Some(AppAction::DismissPassword),
            KeyCode::Enter => Some(AppAction::SubmitPassword),
            KeyCode::Backspace => {
                prompt.field.backspace();
                None
            }
            KeyCode::Char(c) if !ctrl => {
                prompt.field.insert_char(c);
                None
            }
            _ => None,
        }
    }

    /// Handles key events when the Terminal screen is active.
    ///
    /// Returns an [`AppAction`] to pass to `process_action`, or forwards the
    /// keystroke as raw bytes to the active PTY.
    fn handle_terminal_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // Host-picker popup has priority (Ctrl+T flow).
        if self.view.terminal_view.host_picker.is_some() {
            return ui::terminal::handle_host_picker_input(key, &mut self.view);
        }

        // F1/F2/F3 switch to other screens (escape hatch from Terminal).
        // ── Screen switching ────────────────────────────────────────────────
        // Ctrl+Q   → Dashboard  (works on macOS; Ctrl+letter always reliable)
        // F1/F2/F3 → Dashboard/Files/Snippets (Linux; macOS captures F-keys)
        match key.code {
            KeyCode::Char('q') if ctrl => {
                return Some(AppAction::SwitchScreen(Screen::Dashboard));
            }
            KeyCode::F(1) => return Some(AppAction::SwitchScreen(Screen::Dashboard)),
            KeyCode::F(2) => return Some(AppAction::SwitchScreen(Screen::FileManager)),
            KeyCode::F(3) => return Some(AppAction::SwitchScreen(Screen::Snippets)),
            _ => {}
        }

        // ── Terminal control combos ──────────────────────────────────────────
        // Ctrl+T      → new tab host picker
        // Ctrl+W      → close active tab
        // Ctrl+\      → toggle vertical split   (byte 0x1C → Char('4')+CONTROL)
        // Ctrl+]      → toggle horizontal split (byte 0x1D → Char('5')+CONTROL)
        //   Note: Ctrl+\ and Ctrl+] are physically adjacent keys on US layout.
        //   Ctrl+- maps to 0x0D (Enter) with no CONTROL modifier — unusable.
        // Ctrl+Right  → next tab in the focused pane (wraps around)
        // Ctrl+Left   → prev tab in the focused pane (wraps around)
        if ctrl {
            match key.code {
                KeyCode::Char('t') => return Some(AppAction::TermOpenHostPicker),
                KeyCode::Char('w') => return Some(AppAction::TermCloseTab),
                KeyCode::Char('h') => {
                    // Ctrl+H: Switch host in the focused pane (only in split mode)
                    if self.view.terminal_view.split.is_some() {
                        return Some(AppAction::TermSwitchPaneHost);
                    }
                    return None;
                }
                // Ctrl+\ sends byte 0x1C; crossterm decodes it as Char('4')+CONTROL
                KeyCode::Char('4') => return Some(AppAction::TermSplitVertical),
                // Ctrl+] sends byte 0x1D; crossterm decodes it as Char('5')+CONTROL
                KeyCode::Char('5') => return Some(AppAction::TermSplitHorizontal),
                KeyCode::Right => {
                    // Cycle within the focused pane (secondary or primary).
                    let tv = &mut self.view.terminal_view;
                    if tv.tabs.len() > 1 {
                        if let (Some(sv), SplitFocus::Secondary) =
                            (&mut tv.split, tv.split_focus.clone())
                        {
                            sv.secondary_tab = (sv.secondary_tab + 1) % tv.tabs.len();
                            return None; // already mutated
                        }
                        let next = (tv.active_tab + 1) % tv.tabs.len();
                        return Some(AppAction::TermSwitchTab(next));
                    }
                    return None;
                }
                KeyCode::Left => {
                    // Cycle within the focused pane (secondary or primary).
                    let tv = &mut self.view.terminal_view;
                    if tv.tabs.len() > 1 {
                        if let (Some(sv), SplitFocus::Secondary) =
                            (&mut tv.split, tv.split_focus.clone())
                        {
                            sv.secondary_tab = if sv.secondary_tab == 0 {
                                tv.tabs.len() - 1
                            } else {
                                sv.secondary_tab - 1
                            };
                            return None;
                        }
                        let prev = if tv.active_tab == 0 {
                            tv.tabs.len() - 1
                        } else {
                            tv.active_tab - 1
                        };
                        return Some(AppAction::TermSwitchTab(prev));
                    }
                    return None;
                }
                _ => {}
            }
        }

        // ── Tab / next-tab keybinding ──────────────────────────────────────
        //   • In split mode  → switch pane focus.
        //   • Otherwise       → cycle to the next tab AND enter tab-select mode
        //                       (a subsequent digit 1–9 jumps to that tab directly).
        if self.view.keybindings.next_tab.matches(key) {
            if self.view.terminal_view.split.is_some() {
                return Some(AppAction::TermFocusNextPane);
            }
            // Cycle to next tab and enter select mode.
            let tv = &mut self.view.terminal_view;
            if tv.tabs.len() > 1 {
                tv.active_tab = (tv.active_tab + 1) % tv.tabs.len();
                // Mark the newly-active tab as seen.
                tv.tabs[tv.active_tab].has_activity = false;
            }
            tv.tab_select_mode = true;
            return None; // state already mutated; nothing to dispatch
        }

        // In tab-select mode a digit key 1–9 jumps to that tab.
        if self.view.terminal_view.tab_select_mode {
            if let KeyCode::Char(c @ '1'..='9') = key.code {
                let n = (c as u8 - b'1') as usize; // 0-based
                self.view.terminal_view.tab_select_mode = false;
                if n < self.view.terminal_view.tabs.len() {
                    return Some(AppAction::TermSwitchTab(n));
                }
                return None;
            }
            // Any other key exits select mode and falls through to normal handling.
            self.view.terminal_view.tab_select_mode = false;
        }

        // An ended session takes no input: Enter or Esc closes its tab.
        if self.view.terminal_view.focused_ended_tab().is_some() {
            let closes =
                matches!(key.code, KeyCode::Enter | KeyCode::Esc) && key.modifiers.is_empty();
            return closes.then_some(AppAction::TermCloseEnded);
        }

        // Forward everything else as raw bytes to the PTY.
        let bytes = term_input::key_to_bytes(key);
        if bytes.is_empty() {
            None
        } else {
            Some(AppAction::TermInput(bytes))
        }
    }
}
