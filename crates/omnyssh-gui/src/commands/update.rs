//! Update-checker + self-update commands (tech-gui.md §4.3). `check_update` and the
//! startup check query GitHub through the core `update` module and map to
//! `UpdateInfoDto`; the banner is driven by those. `install_update` performs the desktop
//! self-update via `tauri-plugin-updater` — its endpoints + signing key land in Stage 5
//! (§3.7), so until then it surfaces a clear "not available yet" rather than ever
//! self-replacing the binary with the core's TUI archive. Update prefs round-trip
//! through the shared config's `[update]` section.

use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;
use tokio::sync::mpsc;

use omnyssh_core::config::app_config::{
    load_app_config, save_general_config as core_save_general_config,
    save_update_config as core_save_update_config,
};
use omnyssh_core::event::CoreEvent;
use omnyssh_core::update;

use crate::dto::{GeneralConfigDto, UpdateConfigDto, UpdateInfoDto};
use crate::error::CommandError;

/// Query GitHub for a newer release (tech-gui.md §4.2). `None` means up to date — the
/// core swallows network/parse errors so a failed check never disrupts.
#[tauri::command]
#[specta::specta]
pub async fn check_update() -> Result<Option<UpdateInfoDto>, CommandError> {
    Ok(update::check().await.as_ref().map(UpdateInfoDto::from))
}

/// Download and install the latest desktop bundle via `tauri-plugin-updater` (tech-gui.md
/// §4.3/§3.7). Until Stage 5 configures the updater endpoints + signing key the plugin
/// has nothing to point at, so this reports "not available yet" rather than touching the
/// running binary.
#[tauri::command]
#[specta::specta]
pub async fn install_update(app: AppHandle) -> Result<(), CommandError> {
    let updater = app.updater().map_err(|e| CommandError {
        message: format!("Self-update is not available yet: {e}"),
    })?;
    match updater.check().await {
        Ok(Some(update)) => {
            update
                .download_and_install(|_, _| {}, || {})
                .await
                .map_err(|e| CommandError {
                    message: format!("Update failed: {e}"),
                })?;
            app.restart();
        }
        Ok(None) => Err(CommandError {
            message: "No update is currently available to install.".to_string(),
        }),
        Err(e) => Err(CommandError {
            message: format!("Self-update is not available yet: {e}"),
        }),
    }
}

/// Read the update-checker preferences from the shared config (tech-gui.md §4.3).
#[tauri::command]
#[specta::specta]
pub async fn load_update_config() -> Result<UpdateConfigDto, CommandError> {
    let config = tauri::async_runtime::spawn_blocking(|| load_app_config(None))
        .await
        .map_err(|e| CommandError {
            message: format!("config load task failed: {e}"),
        })?
        .map_err(|e| CommandError {
            message: e.to_string(),
        })?;
    Ok((&config.update).into())
}

/// Persist the update-checker preferences to the shared config's `[update]` section
/// (tech-gui.md §4.3). Writes off the async worker — parse + atomic write are blocking.
#[tauri::command]
#[specta::specta]
pub async fn save_update_config(config: UpdateConfigDto) -> Result<(), CommandError> {
    let update = config.into();
    tauri::async_runtime::spawn_blocking(move || core_save_update_config(&update))
        .await
        .map_err(|e| CommandError {
            message: format!("config save task failed: {e}"),
        })?
        .map_err(|e| CommandError {
            message: e.to_string(),
        })
}

/// Read the shared `[general]` configuration.
#[tauri::command]
#[specta::specta]
pub async fn load_general_config() -> Result<GeneralConfigDto, CommandError> {
    let config = tauri::async_runtime::spawn_blocking(|| load_app_config(None))
        .await
        .map_err(|e| CommandError {
            message: format!("config load task failed: {e}"),
        })?
        .map_err(|e| CommandError {
            message: e.to_string(),
        })?;

    Ok((&config.general).into())
}

/// Persist the shared `[general]` configuration.
#[tauri::command]
#[specta::specta]
pub async fn save_general_config(config: GeneralConfigDto) -> Result<(), CommandError> {
    let general = config.into();

    tauri::async_runtime::spawn_blocking(move || core_save_general_config(&general))
        .await
        .map_err(|e| CommandError {
            message: format!("config save task failed: {e}"),
        })?
        .map_err(|e| CommandError {
            message: e.to_string(),
        })
}

/// The startup update check (tech-gui.md §3.4/§4.3): honour `check_on_startup` and the
/// user's skipped version, then emit `UpdateAvailable` on the shared engine channel (the
/// bridge maps it to `update-available`). Errors are swallowed — a failed check never
/// disrupts startup.
pub async fn startup_update_check(engine_tx: mpsc::Sender<CoreEvent>) {
    let config = load_app_config(None).unwrap_or_default().update;
    if !config.check_on_startup {
        return;
    }
    if let Some(info) = update::check().await {
        if info.latest != config.skip_version {
            let _ = engine_tx.send(CoreEvent::UpdateAvailable(info)).await;
        }
    }
}
