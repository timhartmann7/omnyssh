//! System clipboard for the terminal's PuTTY-style copy/paste (copy on select,
//! right-click / middle-click / Shift+Insert paste). The webview's own
//! `navigator.clipboard` only works inside a key or click gesture on WebKit, and
//! `readText` can raise a permission prompt in WebView2, so the terminal goes through
//! the native clipboard here instead. The clipboard-manager plugin is registered
//! Rust-side only: none of its own commands are granted to the frontend, just these two.

use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::error::CommandError;

/// Read the system clipboard as text. An empty clipboard reads as `""`, which the
/// terminal treats as nothing to paste; a clipboard it cannot read at all (no text on
/// it, or no clipboard the platform layer can reach) is an error, so the frontend can
/// fall back to the webview's own paste.
#[tauri::command]
#[specta::specta]
pub fn clipboard_read_text(app: AppHandle) -> Result<String, CommandError> {
    app.clipboard().read_text().map_err(|e| CommandError {
        message: e.to_string(),
    })
}

/// Put `text` on the system clipboard.
#[tauri::command]
#[specta::specta]
pub fn clipboard_write_text(app: AppHandle, text: String) -> Result<(), CommandError> {
    app.clipboard().write_text(text).map_err(|e| CommandError {
        message: e.to_string(),
    })
}
