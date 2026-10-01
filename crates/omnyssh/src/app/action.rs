//! Actions returned by UI input handlers and processed in the main loop.

use super::*;

// ---------------------------------------------------------------------------
// Actions — returned by UI input handlers, processed in main_loop.
// ---------------------------------------------------------------------------

/// Actions that a UI input handler can request from the main event loop.
/// Separating them from events allows the main loop to remain the sole
/// owner of state mutations.
#[derive(Debug)]
pub enum AppAction {
    /// Application should exit.
    Quit,
    /// Connect to `AppState.hosts[idx]` using the system SSH binary.
    ConnectAt(usize),
    /// Open the edit popup pre-filled with the currently selected host.
    OpenEditPopup,
    /// The user confirmed deletion (from inside the DeleteConfirm popup).
    ConfirmDelete,
    /// The user confirmed the add/edit form (pressed Enter).
    ConfirmForm,
    /// Reload hosts from disk + `~/.ssh/config` in a background task.
    ReloadHosts,
    /// The search query was modified — rebuild `filtered_indices`.
    SearchQueryChanged,
    /// Trigger an immediate metrics refresh on all polled hosts.
    RefreshMetrics,
    /// Cycle through sort orders on the dashboard grid.
    CycleSortOrder,
    /// Open or close the tag-filter popup.
    OpenTagFilter,
    /// Apply (or clear) the given tag filter. `None` clears the filter.
    TagFilterSelected(Option<String>),
    /// Navigate the dashboard grid.
    DashboardNav(NavDir),
    /// Start SSH key setup for the selected host (Dashboard 'k' key).
    StartKeySetup,
    /// User confirmed the key setup prompt.
    ConfirmKeySetup(usize),
    /// User cancelled the key setup prompt.
    CancelKeySetup,
    /// Start or stop the selected host's port-forwarding tunnel.
    ToggleTunnel,
    /// Submit the passphrase entered for an encrypted identity file.
    SubmitPassphrase,
    /// Dismiss the passphrase prompt without unlocking the key.
    DismissPassphrase,
    /// Send the password typed for the login a connection waits on.
    SubmitPassword,
    /// Cancel that login.
    DismissPassword,

    // -----------------------------------------------------------------------
    // Detail View actions
    // -----------------------------------------------------------------------
    /// Open the Detail View for the currently selected host on Dashboard.
    OpenDetailView,
    /// Close Detail View and return to Dashboard.
    CloseDetailView,
    /// Connect to host from Detail View (same as Dashboard Enter).
    ConnectFromDetailView,
    /// Show Quick View popup for a specific service (keys 4-9 in Detail View).
    ShowQuickView(ServiceKind),

    // -----------------------------------------------------------------------
    // Snippets actions
    // -----------------------------------------------------------------------
    /// Open the snippet add form on the Snippets screen.
    OpenSnippetAdd,
    /// Open the snippet edit form for the selected snippet.
    OpenSnippetEdit,
    /// Open the delete-confirm popup for the selected snippet.
    OpenSnippetDeleteConfirm,
    /// User confirmed the snippet add/edit form.
    ConfirmSnippetForm,
    /// User confirmed snippet deletion.
    ConfirmSnippetDelete,
    /// The snippet search query changed — rebuild filtered list.
    SnippetSearchChanged,
    /// Execute snippet at `snippet_idx` on the given hosts.
    /// If `host_names` is empty, resolve the target host automatically.
    ExecuteSnippet {
        snippet_idx: usize,
        host_names: Vec<String>,
    },
    /// Open the broadcast host-picker popup for the selected snippet.
    OpenBroadcastPicker,
    /// Toggle selection of host at `host_idx` (into `AppState.hosts`) in
    /// the broadcast picker.
    ToggleBroadcastHost(usize),
    /// Confirm and start a broadcast execution with the currently-checked hosts.
    ConfirmBroadcast,
    /// Open the quick-execute command-input popup for the dashboard's selected host.
    OpenQuickExecute,
    /// Execute an ad-hoc quick-execute command on the given host.
    QuickExecute { host_name: String, command: String },
    /// Confirm parameterized snippet inputs and execute.
    ConfirmParamInput,
    /// Dismiss the snippet results popup.
    DismissSnippetResult,

    // -----------------------------------------------------------------------
    // File Manager actions
    // -----------------------------------------------------------------------
    /// Navigate the cursor up (k / Up arrow) in the active panel.
    FmNavUp,
    /// Navigate the cursor down (j / Down arrow) in the active panel.
    FmNavDown,
    /// Switch focus between left (Local) and right (Remote) panel.
    FmSwitchPanel,
    /// Enter the directory under the cursor (l / Enter).
    FmEnterDir,
    /// Navigate to the parent directory (Backspace).
    FmParentDir,
    /// Move the local panel to the next drive (d; Windows has one per letter).
    FmNextDrive,
    /// Toggle the marked state of the entry under the cursor (Space).
    FmMarkFile,
    /// Open the host-picker popup to connect the remote panel (H).
    FmOpenHostPicker,
    /// User selected host at index `usize` in the host-picker popup.
    FmHostPickerSelect(usize),
    /// Copy the marked (or cursor) items to the clipboard (c).
    FmCopy,
    /// Paste clipboard contents into the active panel (p).
    FmPaste,
    /// Open the delete-confirmation popup for marked / cursor items (D).
    FmOpenDeleteConfirm,
    /// User confirmed deletion.
    FmConfirmDelete,
    /// Open the new-directory popup (n).
    FmOpenMkDir,
    /// Toggle visibility of hidden (dot-prefixed) entries in the active panel (`.`).
    FmToggleHidden,
    /// User confirmed the new directory name.
    FmConfirmMkDir(String),
    /// Open the rename popup for the cursor item (R).
    FmOpenRename,
    /// User confirmed the new name.
    FmConfirmRename(String),
    /// Close the active file-manager popup (Esc).
    FmClosePopup,
    /// Navigate the cursor inside the host-picker popup (j/k).
    FmHostPickerNav(i8), // +1 = down, -1 = up

    /// Edit the remote file under the cursor (in-app or external).
    FmEditFile,
    /// User confirmed opening a large file in the external editor.
    FmConfirmExternalEdit,
    /// Save the in-app editor buffer to the remote path.
    EditorSave,
    /// Quit the in-app editor (second Esc discards a dirty buffer).
    EditorQuit,

    // -----------------------------------------------------------------------
    // Terminal multi-session actions
    // -----------------------------------------------------------------------
    /// Open the host-picker popup for creating a new terminal tab (Ctrl+T).
    TermOpenHostPicker,
    /// Navigate the host-picker cursor. `+1` = down, `-1` = up.
    TermHostPickerNav(i8),
    /// Confirm host selection at `cursor` index in the host-picker popup.
    TermHostPickerSelect(usize),
    /// Close the host-picker popup without connecting (Esc).
    TermCloseHostPicker,
    /// Close the active terminal tab (Ctrl+W).
    TermCloseTab,
    /// Close the focused tab once its session has ended (Enter or Esc).
    TermCloseEnded,
    /// Switch to the tab at the given 0-based index (Ctrl+1..9).
    TermSwitchTab(usize),
    /// Toggle vertical split-view between primary and the next tab (Ctrl+\).
    TermSplitVertical,
    /// Toggle horizontal split-view between primary and the next tab (Ctrl+-).
    TermSplitHorizontal,
    /// Switch keyboard focus between the primary and secondary pane (Tab in split mode).
    TermFocusNextPane,
    /// Forward raw bytes to the active PTY session's stdin.
    TermInput(Vec<u8>),
    /// Switch to the named screen from within the Terminal screen (F1/F2/F3).
    SwitchScreen(Screen),
    /// Switch the host for the currently focused pane (replaces its tab with a new connection).
    /// Only available in split view mode. Opens the host picker.
    TermSwitchPaneHost,
}

/// Direction for dashboard grid navigation.
#[derive(Debug)]
pub enum NavDir {
    Up,
    Down,
    Left,
    Right,
}
