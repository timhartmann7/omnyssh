//! The central action dispatcher: `process_action` applies an [`AppAction`] to
//! shared state, spawns background tasks, and delegates to feature methods.

use super::*;
use omnyssh_core::config::snippets::SnippetScope;
use omnyssh_core::ssh::session::SshSession;

impl App {
    /// Executes an [`AppAction`] that requires access to shared state or the
    /// terminal (e.g. connecting to SSH).
    pub(crate) async fn process_action(&mut self, action: Option<AppAction>) -> anyhow::Result<()> {
        let Some(action) = action else { return Ok(()) };

        match action {
            // Quit is intercepted in main_loop before process_action is called.
            AppAction::Quit => {}

            AppAction::ConnectAt(idx) => {
                // Open a PTY tab in the Terminal screen instead of
                // the old system-SSH hand-off.
                self.open_term_tab(idx).await;
            }

            AppAction::OpenEditPopup => {
                let (idx, host) = {
                    let state = self.state.read().await;
                    let idx = self.view.host_list.selected_host_idx();
                    let host = idx.and_then(|i| state.hosts.get(i)).cloned();
                    (idx, host)
                };
                if let (Some(idx), Some(host)) = (idx, host) {
                    let form = HostForm::from_host(&host);
                    self.view.host_list.popup = Some(HostPopup::Edit {
                        host_idx: idx,
                        form,
                    });
                }
            }

            AppAction::ConfirmForm => {
                self.handle_confirm_form().await;
            }

            AppAction::ConfirmDelete => {
                self.handle_confirm_delete().await;
            }

            AppAction::ReloadHosts => {
                let tx = self.core_tx.clone();
                tokio::spawn(async move {
                    match config::load_all_hosts() {
                        Ok(hosts) => {
                            let _ = tx.send(CoreEvent::HostsLoaded(hosts)).await;
                        }
                        Err(e) => tracing::warn!("Reload failed: {}", e),
                    }
                });
                self.view.status_message = Some("Reloading hosts…".to_string());
            }

            AppAction::SearchQueryChanged => {
                let state = self.state.read().await;
                self.view.host_list.rebuild_filter(
                    &state.hosts,
                    &state.metrics,
                    &state.connection_statuses,
                );
            }

            AppAction::RefreshMetrics => {
                if let Some(mgr) = &self.poll_manager {
                    mgr.refresh_all();
                }
                self.view.status_message = Some("Refreshing metrics…".to_string());
            }

            AppAction::CycleSortOrder => {
                let new_order = self.view.host_list.sort_order.next();
                self.view.host_list.sort_order = new_order;
                let state = self.state.read().await;
                self.view.host_list.rebuild_filter(
                    &state.hosts,
                    &state.metrics,
                    &state.connection_statuses,
                );
            }

            AppAction::OpenTagFilter => {
                self.view.host_list.tag_popup_open = !self.view.host_list.tag_popup_open;
                self.view.host_list.tag_popup_selected = 0;
            }

            AppAction::TagFilterSelected(tag_opt) => {
                self.view.host_list.tag_filter = tag_opt;
                self.view.host_list.tag_popup_open = false;
                let state = self.state.read().await;
                self.view.host_list.rebuild_filter(
                    &state.hosts,
                    &state.metrics,
                    &state.connection_statuses,
                );
            }

            AppAction::DashboardNav(dir) => {
                // The number of grid columns is computed identically here and
                // in the render function. Keep these two in sync.
                const CARD_W: u16 = 34; // Match CARD_MIN_WIDTH from card.rs
                const GAP: u16 = 1;
                // Use a conservative estimate for term width when not rendering.
                let approx_cols = {
                    let w = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);
                    ((w + GAP) / (CARD_W + GAP)).max(1) as usize
                };
                let len = self.view.host_list.filtered_indices.len();
                if len == 0 {
                    return Ok(());
                }
                let sel = self.view.host_list.selected;
                self.view.host_list.selected = match dir {
                    NavDir::Up => sel.saturating_sub(approx_cols),
                    NavDir::Down => (sel + approx_cols).min(len - 1),
                    NavDir::Left => sel.saturating_sub(1),
                    NavDir::Right => (sel + 1).min(len - 1),
                };
            }

            // ---------------------------------------------------------------
            // SSH Key Setup actions
            // ---------------------------------------------------------------
            AppAction::StartKeySetup => {
                let state = self.state.read().await;
                if let Some(idx) = self.view.host_list.selected_host_idx() {
                    if let Some(host) = state.hosts.get(idx) {
                        // Only offer key setup for hosts that use password auth
                        // (have a password field set and no identity_file).
                        if host.password.is_some() && host.identity_file.is_none() {
                            self.view.host_list.popup = Some(HostPopup::KeySetupConfirm(idx));
                        } else if host.identity_file.is_some() {
                            self.view.status_message =
                                Some(format!("'{}' already uses key authentication.", host.name));
                        } else {
                            self.view.status_message = Some(
                                "No password set for this host. Add a password first to enable key setup."
                                    .to_string(),
                            );
                        }
                    }
                }
            }

            AppAction::ConfirmKeySetup(idx) => {
                let state = self.state.read().await;
                if let Some(host) = state.hosts.get(idx).cloned() {
                    // Transition to progress popup.
                    self.view.host_list.popup = Some(HostPopup::KeySetupProgress {
                        host_name: host.name.clone(),
                        current_step: None,
                    });
                    drop(state);

                    // Spawn background key setup task.
                    let tx = self.core_tx.clone();
                    let host_clone = host.clone();
                    tokio::spawn(async move {
                        use omnyssh_core::ssh::key_setup::{
                            setup_key_for_host, KeySetupStep, KeyType,
                        };

                        // Create a channel for progress updates.
                        let (progress_tx, mut progress_rx) = mpsc::channel::<KeySetupStep>(10);
                        let event_tx = tx.clone();
                        let host_name = host_clone.name.clone();

                        // Spawn task to forward progress events.
                        tokio::spawn(async move {
                            while let Some(step) = progress_rx.recv().await {
                                let _ = event_tx
                                    .send(CoreEvent::KeySetupProgress(host_name.clone(), step))
                                    .await;
                            }
                        });

                        // Connect to the host using password to run setup commands.
                        let session = match SshSession::connect(&host_clone).await {
                            Ok(s) => s,
                            Err(e) => {
                                let _ = tx
                                    .send(CoreEvent::KeySetupFailed(
                                        host_clone.name.clone(),
                                        format!("Connection failed: {}", e),
                                    ))
                                    .await;
                                return;
                            }
                        };

                        match setup_key_for_host(
                            &host_clone,
                            &session,
                            KeyType::Ed25519,
                            Some(progress_tx),
                        )
                        .await
                        {
                            Ok(result) => {
                                use omnyssh_core::ssh::key_setup::KeySetupState;
                                match result.state {
                                    KeySetupState::Success | KeySetupState::PartialSuccess => {
                                        let _ = tx
                                            .send(CoreEvent::KeySetupComplete(
                                                host_clone.name.clone(),
                                                result.key_path,
                                            ))
                                            .await;
                                    }
                                    KeySetupState::RolledBack => {
                                        let msg = result
                                            .error_message
                                            .unwrap_or_else(|| "Rolled back.".to_string());
                                        let _ = tx
                                            .send(CoreEvent::KeySetupRollback(
                                                host_clone.name.clone(),
                                                msg,
                                            ))
                                            .await;
                                    }
                                    _ => {
                                        let msg = result
                                            .error_message
                                            .unwrap_or_else(|| "Unknown failure.".to_string());
                                        let _ = tx
                                            .send(CoreEvent::KeySetupFailed(
                                                host_clone.name.clone(),
                                                msg,
                                            ))
                                            .await;
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(CoreEvent::KeySetupFailed(
                                        host_clone.name.clone(),
                                        format!("{:#}", e),
                                    ))
                                    .await;
                            }
                        }

                        session.disconnect().await;
                    });
                }
            }

            AppAction::CancelKeySetup => {
                self.view.host_list.popup = None;
            }

            AppAction::ToggleTunnel => {
                let host = {
                    let state = self.state.read().await;
                    self.view
                        .host_list
                        .selected_host_idx()
                        .and_then(|i| state.hosts.get(i))
                        .cloned()
                };
                let (Some(host), Some(tunnels)) = (host, &mut self.tunnel_manager) else {
                    return Ok(());
                };
                if host.local_forwards.is_empty() {
                    self.view.status_message = Some(format!(
                        "No port forwards set for '{}' — add them with e",
                        host.name
                    ));
                } else if tunnels.is_running(&host.name) {
                    tunnels.stop(&host.name);
                    self.view.status_message = Some(format!("Tunnel for '{}' stopped", host.name));
                } else {
                    self.view.status_message =
                        Some(format!("Starting tunnel for '{}'…", host.name));
                    tunnels.start(host);
                }
            }

            AppAction::SubmitPassphrase => {
                let Some(prompt) = self.view.passphrase_prompts.first_mut() else {
                    return Ok(());
                };
                if prompt.unlocking || prompt.field.value.is_empty() {
                    return Ok(());
                }
                prompt.unlocking = true;
                prompt.error = None;
                let key_path = prompt.key_path.clone();
                let passphrase = std::mem::take(&mut prompt.field).value;
                let tx = self.event_tx.clone();
                // Decrypting runs the key's KDF, which can take a while: keep it
                // off the render loop.
                tokio::task::spawn_blocking(move || {
                    let result = omnyssh_core::ssh::identity::unlock(&key_path, &passphrase)
                        .map_err(|e| e.to_string());
                    let _ = tx.blocking_send(AppEvent::PassphraseUnlocked { key_path, result });
                });
            }

            AppAction::DismissPassphrase => {
                if !self.view.passphrase_prompts.is_empty() {
                    self.view.passphrase_prompts.remove(0);
                }
            }

            AppAction::SubmitPassword | AppAction::DismissPassword => {
                if self.view.password_prompts.is_empty() {
                    return Ok(());
                }
                let prompt = self.view.password_prompts.remove(0);
                let password =
                    matches!(action, AppAction::SubmitPassword).then_some(prompt.field.value);
                // The connection checks it with the server and asks again if
                // it is refused.
                if omnyssh_core::ssh::password::answer(prompt.request_id, password).is_err() {
                    self.view.status_message = Some(format!(
                        "{} is no longer waiting for a password",
                        prompt.login
                    ));
                }
            }

            // ---------------------------------------------------------------
            // Detail View actions
            // ---------------------------------------------------------------
            AppAction::OpenDetailView => {
                // Open Detail View screen for the currently selected host
                let mut state = self.state.write().await;
                state.screen = Screen::DetailView;
            }

            AppAction::CloseDetailView => {
                // Return to Dashboard
                let mut state = self.state.write().await;
                state.screen = Screen::Dashboard;
            }

            AppAction::ConnectFromDetailView => {
                // Same as ConnectAt — open PTY tab for selected host
                let idx = self.view.host_list.selected_host_idx();
                if let Some(idx) = idx {
                    self.open_term_tab(idx).await;
                }
            }

            AppAction::ShowQuickView(service_kind) => {
                // Execute a quick command for the specified service
                self.execute_quick_view(service_kind).await;
            }

            // ---------------------------------------------------------------
            // Snippet actions
            // ---------------------------------------------------------------
            AppAction::OpenSnippetAdd => {
                self.view.snippets_view.popup = Some(SnippetPopup::Add(SnippetForm::empty()));
            }

            AppAction::OpenSnippetEdit => {
                let idx = self.view.snippets_view.selected_snippet_idx();
                if let Some(i) = idx {
                    let snippet = self.state.read().await.snippets.get(i).cloned();
                    if let Some(s) = snippet {
                        let form = SnippetForm::from_snippet(&s);
                        self.view.snippets_view.popup = Some(SnippetPopup::Edit {
                            snippet_idx: i,
                            form,
                        });
                    }
                }
            }

            AppAction::OpenSnippetDeleteConfirm => {
                if let Some(idx) = self.view.snippets_view.selected_snippet_idx() {
                    self.view.snippets_view.popup = Some(SnippetPopup::DeleteConfirm(idx));
                }
            }

            AppAction::ConfirmSnippetForm => {
                self.handle_confirm_snippet_form().await;
            }

            AppAction::ConfirmSnippetDelete => {
                self.handle_confirm_snippet_delete().await;
            }

            AppAction::SnippetSearchChanged => {
                let state = self.state.read().await;
                let q = self.view.snippets_view.search_query.clone();
                self.view.snippets_view.rebuild_filter(&state.snippets, &q);
            }

            AppAction::ExecuteSnippet {
                snippet_idx,
                host_names,
            } => {
                // Resolve target host(s) if none were provided.
                let resolved = if !host_names.is_empty() {
                    host_names
                } else {
                    let state = self.state.read().await;
                    if let Some(s) = state.snippets.get(snippet_idx) {
                        if s.scope == SnippetScope::Host {
                            s.host.iter().cloned().collect()
                        } else {
                            self.view
                                .host_list
                                .selected_host_idx()
                                .and_then(|i| state.hosts.get(i))
                                .map(|h| vec![h.name.clone()])
                                .unwrap_or_default()
                        }
                    } else {
                        vec![]
                    }
                };

                if resolved.is_empty() {
                    // Open the broadcast picker so the user can choose hosts.
                    self.view.snippets_view.popup = Some(SnippetPopup::BroadcastPicker {
                        snippet_idx,
                        selected_host_indices: vec![],
                        cursor: 0,
                    });
                } else {
                    self.execute_snippet(snippet_idx, resolved).await;
                }
            }

            AppAction::ConfirmParamInput => {
                self.handle_confirm_param_input().await;
            }

            AppAction::OpenBroadcastPicker => {
                if let Some(idx) = self.view.snippets_view.selected_snippet_idx() {
                    self.view.snippets_view.popup = Some(SnippetPopup::BroadcastPicker {
                        snippet_idx: idx,
                        selected_host_indices: vec![],
                        cursor: 0,
                    });
                }
            }

            AppAction::ToggleBroadcastHost(host_idx) => {
                if let Some(SnippetPopup::BroadcastPicker {
                    selected_host_indices,
                    ..
                }) = &mut self.view.snippets_view.popup
                {
                    if let Some(pos) = selected_host_indices.iter().position(|&i| i == host_idx) {
                        selected_host_indices.remove(pos);
                    } else {
                        selected_host_indices.push(host_idx);
                    }
                }
            }

            AppAction::ConfirmBroadcast => {
                self.handle_confirm_broadcast().await;
            }

            AppAction::OpenQuickExecute => {
                let host_name = {
                    let state = self.state.read().await;
                    self.view
                        .host_list
                        .selected_host_idx()
                        .and_then(|i| state.hosts.get(i))
                        .map(|h| h.name.clone())
                };
                if let Some(name) = host_name {
                    self.view.snippets_view.popup = Some(SnippetPopup::QuickExecuteInput {
                        host_name: name,
                        command_field: FormField::default(),
                    });
                } else {
                    self.view.status_message = Some("No host selected.".to_string());
                }
            }

            AppAction::QuickExecute { host_name, command } => {
                self.run_quick_execute(host_name, command).await;
            }

            AppAction::DismissSnippetResult => {
                if matches!(
                    self.view.snippets_view.popup,
                    Some(SnippetPopup::Results { .. })
                ) {
                    self.view.snippets_view.popup = None;
                }
            }

            // ---------------------------------------------------------------
            // File Manager actions
            // ---------------------------------------------------------------
            AppAction::FmNavUp => {
                self.active_fm_panel_mut().select_prev();
                self.request_preview_for_active();
            }

            AppAction::FmNavDown => {
                self.active_fm_panel_mut().select_next();
                self.request_preview_for_active();
            }

            AppAction::FmSwitchPanel => {
                self.view.file_manager.active_panel = match self.view.file_manager.active_panel {
                    FmPanel::Local => FmPanel::Remote,
                    FmPanel::Remote => FmPanel::Local,
                };
                self.request_preview_for_active();
            }

            AppAction::FmEnterDir => {
                self.fm_enter_dir().await;
            }

            AppAction::FmParentDir => {
                self.fm_parent_dir().await;
            }

            AppAction::FmNextDrive => {
                self.fm_next_drive().await;
            }

            AppAction::FmToggleHidden => {
                let panel = self.active_fm_panel_mut();
                panel.show_hidden = !panel.show_hidden;
                panel.apply_hidden_filter();
            }

            AppAction::FmMarkFile => {
                let panel = self.active_fm_panel_mut();
                if let Some(entry) = panel.cursor_entry() {
                    if entry.name == ".." {
                        return Ok(());
                    }
                    let path = entry.path.clone();
                    if panel.marked.contains(&path) {
                        panel.marked.remove(&path);
                    } else {
                        panel.marked.insert(path);
                    }
                }
            }

            AppAction::FmCopy => {
                let (paths, source) = {
                    let panel = self.active_fm_panel_ref();
                    (
                        panel.marked_or_cursor_paths(),
                        self.view.file_manager.active_panel.clone(),
                    )
                };
                if paths.is_empty() {
                    self.view.status_message = Some("Nothing to copy.".to_string());
                } else {
                    self.view.file_manager.clipboard = Some(FmClipboard {
                        paths,
                        source_panel: source,
                    });
                    self.view.status_message =
                        Some("Copied to clipboard. Switch panel and press p to paste.".to_string());
                }
            }

            AppAction::FmPaste => {
                self.fm_paste().await;
            }

            AppAction::FmOpenDeleteConfirm => {
                let paths = self.active_fm_panel_ref().marked_or_cursor_paths();
                if paths.is_empty() {
                    self.view.status_message = Some("Nothing to delete.".to_string());
                } else {
                    self.view.file_manager.popup = Some(FileManagerPopup::DeleteConfirm { paths });
                }
            }

            AppAction::FmConfirmDelete => {
                self.fm_delete().await;
            }

            AppAction::FmOpenMkDir => {
                self.view.file_manager.popup = Some(FileManagerPopup::MkDir(FormField::default()));
            }

            AppAction::FmConfirmMkDir(name) => {
                self.fm_mkdir(name).await;
            }

            AppAction::FmOpenRename => {
                if let Some(entry) = self.active_fm_panel_ref().cursor_entry() {
                    if entry.name != ".." {
                        let original_name = entry.name.clone();
                        let field = FormField::with_value(&original_name);
                        self.view.file_manager.popup = Some(FileManagerPopup::Rename {
                            original_name,
                            field,
                        });
                    }
                }
            }

            AppAction::FmConfirmRename(name) => {
                self.fm_rename(name).await;
            }

            AppAction::FmClosePopup => {
                self.view.file_manager.popup = None;
            }

            AppAction::FmOpenHostPicker => {
                self.view.file_manager.popup = Some(FileManagerPopup::HostPicker { cursor: 0 });
            }

            AppAction::FmHostPickerSelect(idx) => {
                self.fm_connect_host(idx).await;
            }

            AppAction::FmHostPickerNav(delta) => {
                if let Some(FileManagerPopup::HostPicker { cursor }) =
                    &mut self.view.file_manager.popup
                {
                    let hosts_len = self.state.read().await.hosts.len();
                    if hosts_len == 0 {
                        return Ok(());
                    }
                    if delta > 0 {
                        *cursor = (*cursor + 1).min(hosts_len - 1);
                    } else {
                        *cursor = cursor.saturating_sub(1);
                    }
                }
            }

            // ---- Remote file editing ----
            AppAction::FmEditFile => {
                self.fm_start_edit();
            }
            AppAction::FmConfirmExternalEdit => {
                self.fm_confirm_external_edit();
            }
            AppAction::EditorSave => {
                self.editor_save();
            }
            AppAction::EditorQuit => {
                self.editor_quit();
            }

            // ---------------------------------------------------------------
            // Terminal multi-session actions
            // ---------------------------------------------------------------
            AppAction::TermInput(bytes) => {
                let active_id = self.view.terminal_view.active_session_id();
                if let (Some(id), Some(mgr)) = (active_id, &mut self.pty_manager) {
                    // Jump back to the live screen when the user types anything.
                    let tv = &mut self.view.terminal_view;
                    let focused_idx = match &tv.split {
                        Some(sv) if tv.split_focus == SplitFocus::Secondary => sv.secondary_tab,
                        _ => tv.active_tab,
                    };
                    if let Some(tab) = tv.tabs.get_mut(focused_idx) {
                        tab.scroll_offset = 0;
                    }
                    if let Err(e) = mgr.write(id, &bytes) {
                        tracing::warn!("PTY write error for session {id}: {e}");
                    }
                }
            }

            AppAction::TermCloseTab => {
                let tv = &mut self.view.terminal_view;
                if tv.tabs.is_empty() {
                    return Ok(());
                }
                let id = tv.tabs[tv.active_tab].session_id;
                if let Some(mgr) = &mut self.pty_manager {
                    mgr.close(id);
                }
                tv.tabs.remove(tv.active_tab);
                tv.split = None;
                tv.split_focus = SplitFocus::Primary;
                if tv.tabs.is_empty() {
                    self.state.write().await.screen = Screen::Dashboard;
                } else {
                    tv.active_tab = tv.active_tab.min(tv.tabs.len().saturating_sub(1));
                }
            }

            AppAction::TermCloseEnded => {
                if let Some(pos) = self.view.terminal_view.focused_ended_tab() {
                    self.remove_ended_tab(pos).await;
                    self.view.terminal_view.closed_ended_at = Some(std::time::Instant::now());
                }
            }

            AppAction::TermSwitchTab(n) => {
                let tv = &mut self.view.terminal_view;
                if n < tv.tabs.len() {
                    tv.active_tab = n;
                    tv.tabs[n].has_activity = false;
                }
            }

            AppAction::TermSplitVertical => {
                let tv = &mut self.view.terminal_view;
                // Same key while already in vertical split → close split.
                if matches!(&tv.split, Some(sv) if sv.direction == SplitDirection::Vertical) {
                    tv.split = None;
                    tv.split_focus = SplitFocus::Primary;
                } else if tv.tabs.len() >= 2 {
                    // Already in horizontal split → switch direction, keep secondary tab.
                    let secondary = tv
                        .split
                        .as_ref()
                        .map(|sv| sv.secondary_tab)
                        .unwrap_or_else(|| (tv.active_tab + 1) % tv.tabs.len());
                    tv.split = Some(SplitView {
                        direction: SplitDirection::Vertical,
                        secondary_tab: secondary,
                    });
                    tv.split_focus = SplitFocus::Primary;
                } else {
                    self.view.status_message = Some(
                        "Need at least 2 tabs to split. Open another tab with Ctrl+T.".to_string(),
                    );
                }
            }

            AppAction::TermSplitHorizontal => {
                let tv = &mut self.view.terminal_view;
                // Same key while already in horizontal split → close split.
                if matches!(&tv.split, Some(sv) if sv.direction == SplitDirection::Horizontal) {
                    tv.split = None;
                    tv.split_focus = SplitFocus::Primary;
                } else if tv.tabs.len() >= 2 {
                    // Already in vertical split → switch direction, keep secondary tab.
                    let secondary = tv
                        .split
                        .as_ref()
                        .map(|sv| sv.secondary_tab)
                        .unwrap_or_else(|| (tv.active_tab + 1) % tv.tabs.len());
                    tv.split = Some(SplitView {
                        direction: SplitDirection::Horizontal,
                        secondary_tab: secondary,
                    });
                    tv.split_focus = SplitFocus::Primary;
                } else {
                    self.view.status_message = Some(
                        "Need at least 2 tabs to split. Open another tab with Ctrl+T.".to_string(),
                    );
                }
            }

            AppAction::TermFocusNextPane => {
                let tv = &mut self.view.terminal_view;
                if tv.split.is_some() {
                    tv.split_focus = match tv.split_focus {
                        SplitFocus::Primary => SplitFocus::Secondary,
                        SplitFocus::Secondary => SplitFocus::Primary,
                    };
                }
            }

            AppAction::TermOpenHostPicker => {
                self.view.terminal_view.host_picker = Some(TermHostPicker::default());
            }

            AppAction::TermHostPickerNav(delta) => {
                if let Some(picker) = &mut self.view.terminal_view.host_picker {
                    let hosts_len = self.state.read().await.hosts.len();
                    if hosts_len == 0 {
                        return Ok(());
                    }
                    if delta > 0 {
                        picker.cursor = (picker.cursor + 1).min(hosts_len - 1);
                    } else {
                        picker.cursor = picker.cursor.saturating_sub(1);
                    }
                }
            }

            AppAction::TermHostPickerSelect(idx) => {
                let switch_pane_mode = self
                    .view
                    .terminal_view
                    .host_picker
                    .as_ref()
                    .map(|p| p.switch_pane_mode)
                    .unwrap_or(false);

                self.view.terminal_view.host_picker = None;

                if switch_pane_mode {
                    // Replace the focused pane's tab with a new connection
                    self.switch_focused_pane_host(idx).await;
                } else {
                    // Normal mode: create a new tab
                    self.open_term_tab(idx).await;
                }
            }

            AppAction::TermCloseHostPicker => {
                self.view.terminal_view.host_picker = None;
                // If no tabs are open, return to Dashboard.
                if self.view.terminal_view.tabs.is_empty() {
                    self.state.write().await.screen = Screen::Dashboard;
                }
            }

            AppAction::TermSwitchPaneHost => {
                // Open host picker in "switch pane mode"
                self.view.terminal_view.host_picker = Some(TermHostPicker {
                    cursor: 0,
                    switch_pane_mode: true,
                });
            }

            AppAction::SwitchScreen(target) => {
                let bootstrap_fm = matches!(target, Screen::FileManager);
                let open_picker =
                    matches!(target, Screen::Terminal) && self.view.terminal_view.tabs.is_empty();
                self.state.write().await.screen = target;
                self.view.status_message = None;
                if bootstrap_fm {
                    self.bootstrap_file_manager().await;
                }
                if open_picker {
                    self.view.terminal_view.host_picker = Some(TermHostPicker::default());
                }
            }
        }

        Ok(())
    }
}

impl App {
    /// Applies the result of a background unlock to its prompt. Connections
    /// waiting on the key retry by themselves; a closed terminal or file tab
    /// has to be opened again.
    pub(crate) fn finish_unlock(&mut self, key_path: String, result: Result<(), String>) {
        // Only the prompt still waiting on it: one dismissed mid-unlock may have
        // been replaced by a fresh prompt for the same key.
        let prompts = &mut self.view.passphrase_prompts;
        let Some(pos) = prompts
            .iter()
            .position(|p| p.key_path == key_path && p.unlocking)
        else {
            return;
        };
        match result {
            Ok(()) => {
                prompts.remove(pos);
                self.view.status_message = Some(format!("Unlocked {key_path}"));
            }
            Err(message) => {
                prompts[pos].unlocking = false;
                prompts[pos].error = Some(message);
            }
        }
    }
}
