// Release builds link as a Windows GUI binary, so launching the app never opens a
// console window alongside it. Debug builds keep the console for cargo output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! OmnySSH Desktop entry point. Wires the tauri-specta IPC boundary (commands +
//! events), regenerates the TypeScript bindings in dev, spawns the core-event
//! bridge, and boots the window (tech-gui.md §3.3–§3.4).

mod bridge;
mod commands;
mod dto;
mod error;
mod events;
#[cfg(target_os = "macos")]
mod menu;
mod state;
mod tray;

use commands::auth::{answer_password, unlock_identity};
use commands::hosts::{delete_host, list_hosts, refresh_metrics, reload_hosts, save_host};
use commands::keysetup::start_key_setup;
use commands::sftp::{
    list_local_dir, list_local_roots, preview_local_file, sftp_cancel, sftp_close, sftp_delete,
    sftp_download, sftp_list, sftp_mkdir, sftp_open, sftp_preview, sftp_rename, sftp_upload,
};
use commands::snippets::{delete_snippet, execute_snippet, list_snippets, save_snippet};
use commands::terminal::{
    terminal_close, terminal_open, terminal_paste, terminal_resize, terminal_write,
};
use commands::tray::set_tray_behavior;
use commands::tunnels::{tunnel_start, tunnel_stop};
use commands::update::{check_update, install_update, load_update_config, save_update_config};
use omnyssh_core::event::{CoreEvent, SessionId};
use omnyssh_core::ssh::pty::PtyManager;
use state::GuiState;
use tauri::webview::PageLoadEvent;
use tauri::Manager;
use tauri_plugin_window_state::StateFlags;
use tauri_specta::{collect_commands, collect_events, Builder};

// Absolute at build time, so the export target is independent of the run CWD.
const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui/src/lib/bindings.ts");

/// How long the hidden window may wait for the page before it is revealed anyway.
/// The tray is off until the page turns it on, so a frontend that never loads must
/// not leave a running process the user cannot see or reach.
const REVEAL_FALLBACK: std::time::Duration = std::time::Duration::from_secs(3);

/// Set once the document is up. `is_visible()` stops answering that question the moment
/// the fallback reveals the window, so the render check reads this instead.
static PAGE_LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A page still missing this long into a launch is a broken graphics stack, not a slow
/// disk. Deliberately far past `REVEAL_FALLBACK`: that one only reveals a window, this
/// one restarts the process.
#[cfg(all(target_os = "linux", not(debug_assertions)))]
const RENDER_HEAL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(12);

// The heal must outlast the reveal, or it would judge a page that is merely still
// loading. Prose in the doc comment above cannot fail a build; this can.
#[cfg(all(target_os = "linux", not(debug_assertions)))]
const _: () = assert!(RENDER_HEAL_DEADLINE.as_secs() > REVEAL_FALLBACK.as_secs());

/// Marks the child of a software-rendering retry so it can only ever happen once.
/// Exporting it by hand disables the retry — the intended escape hatch.
#[cfg(all(target_os = "linux", not(debug_assertions)))]
const RETRY_MARKER: &str = "OMNYSSH_SOFTWARE_RENDER_RETRY";

/// What the window-state plugin is allowed to restore. Geometry only: it applies every
/// flag from `on_window_ready`, before the page exists, and only sizing and moving leave
/// a hidden window hidden. `VISIBLE` shows it outright, undoing the reveal that keeps the
/// blank webview off screen; `MAXIMIZED` reaches `ShowWindow(SW_MAXIMIZE)` on Windows,
/// which carries no visibility guard of its own; `FULLSCREEN` touches the same hidden
/// window; `DECORATIONS` re-derives the macOS style mask the overlay title bar needs.
///
/// Dropping `MAXIMIZED` costs a maximised window its position: the plugin records a move
/// without checking for one, so the maximised origin becomes the saved position, and the
/// pre-maximise origin it keeps alongside is read back only for a window it also restores
/// maximised. Such a window reopens at its own size in the corner of the display.
const WINDOW_STATE_FLAGS: StateFlags = StateFlags::SIZE.union(StateFlags::POSITION);

// The reveal is the only path to a visible window until the page turns the tray on, so
// the exclusions above are too load-bearing to live in prose alone.
const _: () = assert!(!WINDOW_STATE_FLAGS.intersects(
    StateFlags::VISIBLE
        .union(StateFlags::MAXIMIZED)
        .union(StateFlags::FULLSCREEN)
        .union(StateFlags::DECORATIONS)
));

/// The single definition of the IPC surface. Shared by `main` (dev export +
/// wiring) and the drift test so they can never disagree.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            list_hosts,
            reload_hosts,
            save_host,
            delete_host,
            list_snippets,
            save_snippet,
            delete_snippet,
            execute_snippet,
            terminal_open,
            terminal_write,
            terminal_resize,
            terminal_close,
            terminal_paste,
            sftp_open,
            sftp_list,
            sftp_upload,
            sftp_download,
            sftp_mkdir,
            sftp_rename,
            sftp_delete,
            sftp_preview,
            sftp_cancel,
            sftp_close,
            list_local_dir,
            list_local_roots,
            preview_local_file,
            start_key_setup,
            tunnel_start,
            tunnel_stop,
            refresh_metrics,
            unlock_identity,
            answer_password,
            set_tray_behavior,
            check_update,
            install_update,
            load_update_config,
            save_update_config
        ])
        .events(collect_events![
            events::HostsLoaded,
            events::HostStatusChanged,
            events::MetricsUpdated,
            events::ServicesDetected,
            events::ServicesFailed,
            events::TunnelStatusChanged,
            events::SnippetResult,
            events::TerminalExited,
            events::SftpConnected,
            events::SftpDirListed,
            events::SftpOpDone,
            events::SftpDisconnected,
            events::FilePreview,
            events::TransferProgress,
            events::KeySetupProgress,
            events::KeySetupComplete,
            events::KeySetupFailed,
            events::KeySetupRollback,
            events::UpdateAvailable,
            events::KeyPassphraseRequired,
            events::PasswordRequired,
            events::Error
        ])
}

/// Writes `bindings.ts` for the current IPC surface. Dev/test only — release
/// builds ship no exporter and never regenerate.
#[cfg(debug_assertions)]
fn export_bindings(path: impl AsRef<std::path::Path>) {
    // Emit `number` for u64 fields (`ageSeconds`, later the session/transfer ids);
    // their values stay well within JS's safe-integer range.
    let ts = specta_typescript::Typescript::default()
        .bigint(specta_typescript::BigIntExportBehavior::Number);
    specta_builder()
        .export(ts, path)
        .expect("failed to export TypeScript bindings");
}

/// Whether a blank launch should be retried once with WebKit's software renderer.
///
/// Deliberately not restricted to the AppImage: the DMA-BUF failure this heals is a
/// WebKitGTK-and-driver problem, so a `.deb` or `.rpm` on the same machine blanks the
/// same way — and `install.sh` now prefers the `.rpm` on the distributions where it is
/// reported most. What keeps the retry safe is that it can only ever happen once
/// (`RETRY_MARKER`) and never overrides a renderer the user chose himself.
// Compiled everywhere and exercised by the tests, but called only from the release
// Linux heal, so every other build has to waive the unused warning.
#[cfg_attr(any(not(target_os = "linux"), debug_assertions), allow(dead_code))]
fn should_retry_software_rendering(
    page_loaded: bool,
    already_retried: bool,
    dmabuf_disabled: bool,
) -> bool {
    !page_loaded && !already_retried && !dmabuf_disabled
}

/// Whether the single-instance plugin can start. On Linux it unwraps the session bus
/// address, so one zbus cannot read — `disabled:`, as some sandboxes set — would abort
/// the launch. With no bus at all it just stands down, and so does this app: each launch
/// is then its own.
#[cfg(target_os = "linux")]
fn single_instance_can_start() -> bool {
    zbus::Address::session().is_ok()
}

#[cfg(not(target_os = "linux"))]
fn single_instance_can_start() -> bool {
    true
}

/// Re-exec ourselves with WebKit's software renderer. Returns only on failure.
///
/// `/proc/self/exe`, not `$APPIMAGE`: inside an AppImage the runtime already put the
/// AppDir's `LD_LIBRARY_PATH` and `PATH` into this process and exec keeps them, while
/// re-running the AppImage would mount a second copy of the bundle whose first mount can
/// no longer be released — and the file may have been moved since launch (`install.sh`
/// does that). For a packaged install it is simply the installed binary.
#[cfg(all(target_os = "linux", not(debug_assertions)))]
fn exec_software_render_retry() -> std::io::Error {
    use std::os::unix::process::CommandExt;

    // `args_os`: a non-UTF-8 argument must not panic the retry.
    let mut args = std::env::args_os();
    let mut cmd = std::process::Command::new("/proc/self/exe");
    if let Some(arg0) = args.next() {
        cmd.arg0(arg0);
    }
    cmd.args(args);
    // On the child only: `env::set_var` is unsound with other threads running, and
    // clearing the environment would throw away the AppDir's library paths.
    cmd.env("WEBKIT_DISABLE_DMABUF_RENDERER", "1")
        .env(RETRY_MARKER, "1");

    let failure = cmd.exec();
    // `exec` sets up the child in *this* process before `execvp`, which includes resetting
    // SIGPIPE to SIG_DFL, and it does not undo that when `execvp` fails. Left alone, the
    // first SSH socket to close under a write would kill the app instead of erroring.
    // SAFETY: restoring the disposition the Rust runtime installs at startup.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    failure
}

fn main() {
    let builder = specta_builder();

    #[cfg(debug_assertions)]
    export_bindings(BINDINGS_PATH);

    let mut app = tauri::Builder::default();
    // First, so a second launch exits before anything else starts; it brings the running
    // window forward, from the tray too.
    if single_instance_can_start() {
        app = app.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::reveal(app)
        }));
    }
    // Replaces Tauri's default, whose ⌘W and ⌘Q close and quit without asking.
    #[cfg(target_os = "macos")]
    {
        app = app.menu(menu::build);
    }
    app
        // Persists UI prefs (theme, sidebar collapse, refresh interval) from the
        // frontend JS API — no bespoke command (tech-gui.md §4.2, §5.1).
        .plugin(tauri_plugin_store::Builder::new().build())
        // Desktop self-update (tech-gui.md §4.3); endpoints/signing land in Stage 5.
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Opens the support dialog's GitHub/Telegram links in the default browser.
        .plugin(tauri_plugin_opener::init())
        // Restores the window's size and position between launches; the flags keep it
        // away from everything that would touch the window itself (WINDOW_STATE_FLAGS).
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(WINDOW_STATE_FLAGS)
                .build(),
        )
        .invoke_handler(builder.invoke_handler())
        .on_menu_event(tray::on_menu_event)
        .on_window_event(tray::on_window_event)
        // The window is created hidden (tauri.conf.json `visible: false`) so the
        // launch never shows the webview's blank base colour; reveal it once the
        // document — stylesheet included — is up.
        .on_page_load(|webview, payload| {
            if matches!(payload.event(), PageLoadEvent::Finished) {
                PAGE_LOADED.store(true, std::sync::atomic::Ordering::Release);
                // Reveal once: a later page load must not raise the window over
                // whatever the user is doing, nor out of the tray.
                if !tray::REVEALED.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    let window = webview.window();
                    let _ = window.show();
                    // A window shown after build does not become key on its own everywhere.
                    let _ = window.set_focus();
                }
            }
        })
        .setup(move |app| {
            builder.mount_events(app);

            let reveal = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(REVEAL_FALLBACK).await;
                // Only when the page never got there: on macOS showing an
                // already-visible window raises it over whatever the user switched to
                // meanwhile.
                if !tray::REVEALED.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    if let Some(window) = reveal.get_webview_window("main") {
                        let _ = window.show();
                    }
                }
            });

            // Released Linux builds only: an interface that never loaded is usually
            // WebKit's DMA-BUF renderer losing to the host's graphics driver. Retry once
            // with software rendering — anything that loaded, or already retried, is left
            // alone. Kept out of debug builds because `tauri dev` waits on a dev server,
            // and a slow one starting is not a broken graphics stack.
            #[cfg(all(target_os = "linux", not(debug_assertions)))]
            let heal = app.handle().clone();
            #[cfg(all(target_os = "linux", not(debug_assertions)))]
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(RENDER_HEAL_DEADLINE).await;
                if should_retry_software_rendering(
                    PAGE_LOADED.load(std::sync::atomic::Ordering::Acquire),
                    std::env::var_os(RETRY_MARKER).is_some(),
                    // WebKit's own test: set, and not "0".
                    std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER")
                        .is_some_and(|value| value != "0"),
                ) {
                    eprintln!(
                        "OmnySSH: the interface never loaded — restarting once with software rendering"
                    );
                    // The restarted image claims the single-instance name again, and
                    // finding it still held it would take itself for a second launch
                    // and quit.
                    tauri_plugin_single_instance::destroy(&heal);
                    // `exec` returns only on failure; carry on with this process.
                    let err = exec_software_render_retry();
                    eprintln!("OmnySSH: the restart failed ({err}) — continuing as is");
                }
            });

            let (engine_tx, engine_rx) = tokio::sync::mpsc::channel::<CoreEvent>(256);
            // The additive PTY raw-byte tap (§3.6): the manager mirrors each session's
            // bytes into `raw_tx`; a forwarder demuxes them into per-tab channels.
            let (raw_tx, raw_rx) = tokio::sync::mpsc::channel::<(SessionId, Vec<u8>)>(256);
            let pty = PtyManager::with_raw_output(raw_tx);
            // The bridge hands each PTY exit to that forwarder, behind the session's bytes.
            let (exit_tx, exit_rx) = tokio::sync::mpsc::channel::<SessionId>(64);

            // Pre-load the shared host config so the first `list_hosts` paints
            // immediately. A load failure here is non-fatal — the frontend's
            // `reload_hosts` re-attempts and surfaces the error (tech-gui.md §3.4).
            let gui_state = GuiState::new(engine_tx, pty);
            if let Ok(hosts) = omnyssh_core::config::load_all_hosts() {
                gui_state.set_hosts(hosts);
            }
            app.manage(gui_state);

            // Spawn the forwarders after `manage` so both can reach `GuiState` via
            // `app.state()` (the bridge tracks tunnel statuses; the tap routes raw bytes
            // and ends exited tabs).
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(bridge::forward_core_events(
                handle.clone(),
                engine_rx,
                exit_tx,
            ));
            tauri::async_runtime::spawn(bridge::forward_terminal_output(handle, raw_rx, exit_rx));

            // The pollers and the startup update check are both started by the frontend's
            // first `reload_hosts`, once its event bridge is listening — starting them
            // here would race listener registration and drop `HostStatusChanged` /
            // `UpdateAvailable` before the webview can receive them (§3.4).
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to launch OmnySSH Desktop")
        .run(tray::on_run_event);
}

#[cfg(test)]
mod tests {
    use super::{export_bindings, should_retry_software_rendering, BINDINGS_PATH};

    /// The committed bindings must match a fresh export — fails loudly on drift
    /// without mutating the tracked file (tech-gui.md §0.2 acceptance, §3.3).
    #[test]
    fn committed_bindings_are_in_sync() {
        let tmp = std::env::temp_dir().join(format!("omnyssh-bindings-{}.ts", std::process::id()));
        export_bindings(&tmp);
        let generated = std::fs::read_to_string(&tmp).expect("read fresh bindings");
        let _ = std::fs::remove_file(&tmp);

        let committed = std::fs::read_to_string(BINDINGS_PATH).expect("read committed bindings");
        assert_eq!(
            committed.replace("\r\n", "\n"),
            generated.replace("\r\n", "\n"),
            "ui/src/lib/bindings.ts is out of date — regenerate with `cargo tauri dev`"
        );
    }

    /// The retry exists for one case only: no page, not already retried, no renderer the
    /// user chose. Everything else must launch exactly as it does today.
    #[test]
    fn the_software_render_retry_fires_once_on_a_blank_launch() {
        assert!(should_retry_software_rendering(false, false, false));
        // The page arrived — there is nothing to heal.
        assert!(!should_retry_software_rendering(true, false, false));
        // Already the retry, or the user asked for software rendering himself: a second
        // restart would only loop.
        assert!(!should_retry_software_rendering(false, true, false));
        assert!(!should_retry_software_rendering(false, false, true));
        // Both guards at once, in case one is ever dropped.
        assert!(!should_retry_software_rendering(false, true, true));
    }
}
