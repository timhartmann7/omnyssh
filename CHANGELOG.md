# Changelog

All notable changes to OmnySSH are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Versions follow [Semantic Versioning](https://semver.org/).

---

## 1.1.2 — 2026-08-22

### Features
- **Hosts imported from `~/.ssh/config` can be edited in the desktop app.** Imported hosts were shown read-only, so changing a port or a username for one meant deleting it from your SSH config and adding it again by hand — while the terminal app had allowed the edit all along. Editing one now saves your own copy, which OmnySSH uses from then on; `~/.ssh/config` is still never written to. The form says so before you save, since later changes you make to that file stop reaching the host once it has been adopted. The bastion and key path the app parsed are carried over even though the form cannot show them, so an adopted `ProxyJump` host keeps connecting through its jump server. Deleting is still offered for your own hosts only — there is nothing of an import to remove — and deleting a copy you adopted brings the imported version back, which the confirmation now tells you.
- **Watch a host by a port check instead of an SSH login.** Firewalls, switches and other appliances answer SSH but have no shell to read `top` or `free` from, so monitoring could only ever fail on them — and log in again every cycle to find that out. A host can now be set to **TCP port check**: OmnySSH opens a connection to the port and closes it, with no login and no commands. Its card shows reachable / unreachable in place of the metric tiles, rather than tiles for numbers nobody collected. Set it in the host form — `tcp` (the host's SSH port) or `tcp:PORT` in the terminal app, a dropdown in the desktop app. Existing hosts are untouched and stay on SSH monitoring. ICMP is not offered yet: unprivileged ping is unavailable on the Linux packaging most people use.

### Bug Fixes
- **The dropdowns in the desktop app look like the rest of the app.** The monitoring picker on the host form and the scope picker on the snippet form were plain `<select>` elements, which every platform draws itself: on macOS that is a light grey popup button with a double arrow, sitting on the dark form as a bright hole a good deal shorter than the fields around it, and no styling on the element had any effect on it. Windows and Linux drew their own versions of the same mismatch. Both now use one control that strips the platform chrome and matches the text fields exactly, in either theme. The list that opens is still the operating system's, so keyboard, screen readers and touch behave as they always did.
- **Nerd Font glyphs render instead of empty boxes.** The desktop terminal asked for a list of system monospace fonts, none of which carry the glyphs that prompts like starship and powerlevel10k, or `eza --icons`, draw from — so those came out as boxes even for people who had a Nerd Font installed. Common Nerd Font families are now named as fallbacks, behind the regular monospace ones so ordinary text keeps the same typeface and spacing. Snippet output and SFTP file previews get the same treatment, since both show text straight from the server. You still need a Nerd Font installed; the app does not ship one.
- **The desktop app reopens at the size and position you left it.** Every launch reset the window to 1100x720 wherever the system chose to put it, so a window sized to your screen — or moved to a second monitor — had to be set up again each time. Size and position now persist. Visibility deliberately does not: the window still starts hidden and appears once the interface has painted, so restoring geometry never brings back the blank frame at launch.
- **macOS: the window buttons no longer sit across the edge of a collapsed sidebar.** The red/amber/green cluster is positioned by the system against the window, not by the app's layout, and it is wider than the collapsed sidebar was — so the sidebar's edge fell on the green button and left it over the content area while the other two stayed on the sidebar. The collapsed sidebar is now wide enough to hold the whole cluster on macOS. Windows and Linux draw their own title bar and keep the narrower one.
- **Hosts split across `Include` files are imported.** A relative pattern — `Include conf.d/*.conf`, the form nearly every split-config guide prints — was looked for in whatever directory the app happened to be launched from, which for the desktop app started from Finder or the application menu is `/`. Nothing matched, so every host defined in `~/.ssh/conf.d` was missing and had to be added by hand. Those patterns now resolve against `~/.ssh`, the way `ssh` itself resolves them. Four things in the same code path were fixed alongside it: full glob patterns work (`?`, `[abc]`, a wildcard in a directory name, and more than one `*` in a file name), several pathnames on one `Include` line are all read instead of none, a quoted path keeps its spaces, and an `Include` written inside a `Host` block no longer swallows that host's remaining settings. An `Include` that matches nothing is written to the log rather than passing in silence.
- **CPU no longer reads ~91–100 % on an idle server whose system language isn't English.** Monitoring reads the idle percentage from `top` and shows the rest as used. On a server set to a language with a comma decimal separator, `top` prints `99,1 id`, and the parser split that line on commas — reading the idle value as `1` and reporting 99 % used on a machine doing nothing. The same servers could show RAM and Disk as N/A, because `free` and `df` translate the labels being looked for, and the process list could come back empty. The monitoring commands now run in a fixed locale, and the CPU parsers understand a decimal comma on their own for hosts where that cannot be set. Process names keep the server's own character set.
- **A host that answers SSH but not shell commands is no longer re-logged-in every 30 seconds.** Network appliances — firewalls, switches — authenticate fine but cannot run `top` or `free`, and a round of failed metric commands was treated as a dead connection. The retry delay was reset on every successful login, so it never grew past its first step: one login every 30 seconds, indefinitely, and as often as every 10 seconds in the desktop app, whose refresh timer also cut the retry delay short. The delay now escalates properly and is left alone by the refresh timer — the trade-off being that a manual refresh no longer cuts a retry delay short, so a host that has been unreachable for a while retries on its own schedule (at most five minutes). Separately, a device that ignores SSH keepalives was being disconnected after 30 seconds even while its commands still worked; connections are now held open, with liveness still bounded by the keepalive limit.
- **`ProxyJump` hosts now connect through their bastion.** A host with `ProxyJump` in `~/.ssh/config` was parsed but never routed: the terminal refused to open it, while metrics, SFTP, snippets and key setup quietly dialled the target address direct — which for an internal host meant every connection failed or, worse, landed somewhere else on that address. Every native SSH path now walks the jump chain the way `ssh -J` does, connecting and authenticating each bastion in turn and tunnelling the next hop over it.
  - The jump alias is resolved against your host list, so `ProxyJump public-proxy` picks up that entry's `HostName`, `User`, `Port` and `IdentityFile`. An alias that matches no entry is used as a literal hostname.
  - Multi-hop values (`ProxyJump first,second`), inline `user@host:port` overrides, IPv6 literals, bastions that are themselves behind a bastion, and the `ProxyJump none` opt-out all behave as OpenSSH does — including its precedence rule that only the first hop of a list contributes bastions of its own. Chains that loop, or run past ten hops, are reported instead of hanging.
  - A `ProxyJump` that cannot be resolved — a malformed hop, an unreadable host list, a cycle — fails the connection. It never falls back to dialling the target address, which is the very thing that made the old behaviour dangerous.
  - Each hop's host key is checked against `known_hosts` under its own name, and each hop authenticates with the usual agent → identity file → default keys → password order. Every hop, including the tunnel opened on a bastion, is bound by the same ten-second budget, so a firewalled target cannot leave a host stuck on "connecting".
  - A bastion you renamed after importing it is still found by the alias other entries name it with.
  - One-click SSH key setup works for hosts behind a bastion: its verification steps now get the time the longer connection needs, and running out of time after password authentication has been disabled rolls the server back instead of reporting a clean failure.
  - Editing an imported host in the TUI no longer drops its `ProxyJump`, nor the name it was imported under: the form has no field for either, so the saved copy used to lose the bastion, and a second edit lost the imported name — which brought the `~/.ssh/config` entry back as a duplicate card and left any other host's `ProxyJump` pointing at an alias nothing answered to. The GUI already preserved both.
- **Windows: the console-window fix from 1.1.1 actually reaches you this time.** The fix was in that release's source, but the Windows installer published under the `v1.1.1` tag was an artifact left over from an earlier build — so the `.exe` it installed was the 1.1.0 one, still linked as a console application, and none of 1.1.1's Windows fixes (the console window, the white launch flash, the `ssh-keygen` console) were in it. The release now discards anything a previous build left behind, publishes only the bundle it just produced for the version being released, and refuses to publish at all if the desktop binary is not linked as a GUI application or if any installer is missing.
- **The update banner links to the release instead of offering an install that cannot run.** Self-update has no endpoints configured yet, so the Install button reported "Self-update is not available yet" on every platform. It now opens the release page in your browser; Install returns when the desktop updater is wired up.
- **A Linux desktop build that opens to an empty window restarts itself once.** On some graphics setups WebKit cannot start its hardware renderer, so the app opened as an empty dark frame and stayed that way, with the error going to a terminal nobody launched it from. If the interface still has not loaded twelve seconds in, OmnySSH now restarts itself once with software rendering. This covers the `.deb` and `.rpm` as well as the `.AppImage`: the failure is WebKit meeting a particular graphics driver rather than anything the AppImage does, and `install.sh` now prefers the `.rpm` on the distributions where it is reported most, so leaving the native packages out would have moved the affected people onto the one build with no answer for it. A launch that loads normally is untouched, a launch you started with `WEBKIT_DISABLE_DMABUF_RENDERER` set yourself is left alone, and the restart can happen only once. macOS and Windows are unaffected.
- **Editing a host no longer drops every other host's monitoring session.** Putting the monitoring mode on the host form meant the terminal app had to restart its polling pool after an edit, since a running poller does not pick up a changed address, port or mode. It restarted the whole pool for any edit at all, so fixing a typo in one host's notes reconnected and re-authenticated every server on the dashboard. The pool is now restarted only when the edit moved something a connection is actually made from — the name, address, user, port, key, password or monitoring mode. Editing tags or notes leaves every session where it was.
- **macOS: `install.sh` no longer installs an older copy of the app.** The installer read the mount point out of `hdiutil`'s output by stopping at the first space. When a volume called OmnySSH was already mounted — because you had opened the `.dmg` in Finder first, or because an earlier run was interrupted — macOS mounts the new one as `OmnySSH 1`, and the truncated path pointed back at the one already there. The script then copied that older app into `/Applications` and reported success.
- **Linux: a `.deb` that could not be installed is no longer reported as installed.** `apt-get install -f` resolves an unsatisfiable dependency by removing the package `dpkg` has just unpacked, and exits successfully having done so — which the installer read as a working install and announced as one, leaving you with the message and nothing else. It now asks `dpkg` what is actually installed, and falls back to the AppImage when the package did not survive, the way the `.rpm` path already did.
- **Linux: installing the `.deb` removes the AppImage it replaces.** The `.rpm` path already did this and the `.deb` path did not. The package installs to `/usr/bin` under its own name while an AppImage from an earlier run sits in `/usr/local/bin`, which comes first on the default `PATH` — so `omnyssh` on the command line kept starting the old build, and the application menu carried two identical entries.
- **ARM64 Linux and Termux install the terminal app instead of nothing.** No desktop build exists for those targets, and `curl … | sh` — the command the README prints — installs the desktop app by default. It warned and exited without installing anything, although the terminal app ships for exactly those targets. It now installs that instead. A desktop build that does exist and fails to install still fails rather than quietly falling back.

### Packaging
- **A native `.rpm` for Fedora and other RPM distributions.** Releases now carry `OmnySSH-x86_64.rpm` alongside the `.AppImage` and `.deb`, and `install.sh` prefers it on `dnf`-based systems — it lands in your application menu and uninstalls with `dnf remove`, no FUSE involved. The package names the WebKitGTK 4.1, JavaScriptCore and GTK 3 libraries it links, so `dnf` resolves them from your own distribution instead of the app carrying a second copy. Distributions that ship no WebKitGTK 4.1 at all, such as RHEL 9 and its rebuilds, will refuse the package; `install.sh` then tries the AppImage there.
- **The Linux packages are named `omny-ssh`.** The bundler derives the package name from the product name, so the `.deb` and `.rpm` register as `omny-ssh` even though the files are called `OmnySSH-x86_64.deb` and `OmnySSH-x86_64.rpm`. Uninstall with `dnf remove omny-ssh` or `apt remove omny-ssh`.

---

## 1.1.1 — 2026-07-28

### Bug Fixes
- **Windows: the desktop app no longer opens a console window next to itself.** The GUI was linked as a console application, so Windows gave it a terminal and kept it on screen for the whole session. Release builds now link as a GUI binary. The same fix would have made one-click SSH key setup flash a console of its own while `ssh-keygen` runs, so that is suppressed too. macOS and Linux were never affected.
- **No more white flash when the desktop app launches.** The window used to appear before the webview had painted anything, so the first frame was blank white before the dark (or light) interface took over. The window is now created hidden and revealed once the page is up, with a fallback that shows it anyway if the interface is slow to load.

---

## 1.1.0 — 2026-07-24

### Features
- **OmnySSH Desktop — a new native GUI app.** A desktop application built on the same engine as the TUI, sharing your `hosts.toml` and `snippets.toml`. It bundles a live metrics dashboard, a multi-session terminal, an SFTP file manager, command snippets, host management, one-click SSH key setup, a command palette, and light/dark themes. Distributed as native installers (macOS `.dmg`, Linux `.AppImage`/`.deb`, Windows `.exe`). The TUI is unchanged and still installs the same ways. macOS builds are unsigned — on first launch right-click the app and choose **Open** (or install via `install.sh`, which sidesteps the Gatekeeper prompt).
- **File manager hidden-file toggle (`.`)**: show or hide dot-prefixed entries (hidden by default). The `..` parent entry is always shown.
- **Nerd Font file icons in the file manager**: per-type glyphs replace the `[DIR]`/`[   ]` markers. Requires a Nerd Font — without one the glyphs render as empty boxes.

### Changed
- **Terminal next-tab moved from `Tab` to `Ctrl+N`**, freeing `Tab` for the shell's own completion inside the embedded terminal.
- **File manager `h`/`Left` now navigates to the parent directory** (previously moved the cursor up), matching `ranger`/`lf`/`nnn`/`vifm`. Returning to a parent places the cursor on the directory just left.

### Bug Fixes
- **The terminal now handles Cyrillic and other multibyte text.** OmnySSH forwards a UTF-8 locale (and the `IUTF8` PTY mode) to the remote shell, mirroring a normal ssh client's `SendEnv LANG LC_*`. Previously the session could fall back to a single-byte locale, so line editing corrupted the prompt on Cyrillic input and editors like `vim` rendered mojibake (plain `cat` was fine). Applies to both the TUI and GUI terminals. Best-effort — servers that don't accept forwarded env vars are unaffected.

### Packaging
- **Release builds now ship the GUI.** CI bundles native desktop installers for macOS (arm64 + x86_64), Linux x86_64 (`.AppImage` + `.deb`), and Windows x86_64, attached to each GitHub Release alongside the TUI archives and covered by `SHA256SUMS`.
- **`install.sh` can install the GUI, the TUI, or both** via `--gui` / `--tui` / `--both` (or `OMNYSSH_INSTALL=…`); it prompts when run interactively and defaults to the GUI (the flagship app) for piped `curl | sh`.

### Documentation
- **CONTRIBUTING** now documents the `omnyssh-gui` crate and how to build/test the GUI with the Node/Tauri toolchain.
- Documented the Auto SSH Key Setup feature (`Shift+K`): added a README Features entry, a Quick Start key reference, and a Help popup shortcut. The feature already existed but was undiscoverable.
- README "Development Roadmap" now lists the current `1.1.0` release (and `1.0.5`); it previously stopped at `1.0.4`.
- Removed the dead `connect` keybinding from the `config.toml` example and the `[keybindings]` config struct. Enter-to-connect was always hard-coded, so the field never had any effect.

### Internal
- **Repository converted to a cargo workspace; the engine now lives in its own crate**: The SSH engine, host/snippet/app configuration, metrics parsers, domain events, and the self-updater moved into the new `omnyssh-core` library crate (`crates/omnyssh-core`), which has no dependency on terminal-UI or CLI crates. The TUI application keeps the `omnyssh` package name and the `omny` binary (`crates/omnyssh`) and consumes the core as a regular dependency. This prepares the architecture for additional frontends (e.g. a GUI) without duplicating the engine.
  - The event bus is split: background tasks emit `CoreEvent` values; the TUI wraps them into its own `AppEvent` stream alongside input events.
  - `threshold_color` was replaced by a UI-agnostic `ThresholdLevel` in the core; the colour mapping moved to the TUI theme module.
  - Keybinding parsing (config strings → key codes) and PTY key-to-bytes translation moved from the engine into the TUI crate; the core PTY API accepts raw bytes only.
  - Removed the unused `nucleo` dependency.
  - No user-facing changes: behavior, appearance, the binary name, and install paths are unchanged.
- **Removed leftover Alerts/Deep Probe remnants**: Deleted the unused `[smart_context]` config (`SmartContextConfig` was parsed but never read) and reworded stale "Deep Probe" comments left after the subsystem was removed in 1.0.5. No user-facing change — existing config files containing a `[smart_context]` section still load, as the section is ignored.

---

## 1.0.5 — 2026-06-07

### Bug Fixes
- **docs.rs documentation build fixed**: The build script wrote the generated man page into the source tree (`doc/omny.1`), which fails on docs.rs because it mounts the sources read-only. The man page is now skipped when building on docs.rs (detected via the `DOCS_RS` environment variable); normal builds still generate and check it in as before.

### Internal
- **Dead-code cleanup**: Removed the unimplemented Alerts/Deep Probe subsystem (backend and UI) and a range of unused items — enum variants, struct fields, helpers, and theme colors. No user-facing behavior changes: METRICS and SERVICES on the detail page, the file manager, and the terminal work as before. The crate-level `#![allow(dead_code)]` was dropped so the compiler now catches dead code on its own.

---

## 1.0.4 — 2026-05-29

### Features
- **Terminal now uses the native russh client (fixes the dead Windows terminal)**: The interactive terminal no longer spawns the system `ssh` binary inside a local pseudo-console (ConPTY on Windows). It now runs over the same pure-Rust russh client as metrics and SFTP, so the terminal works identically on every OS — the blank, unresponsive Terminal tab on Windows is fixed. The terminal reuses the app's existing key, SSH agent, password, and `known_hosts` (trust-on-first-use) authentication, so password-auth hosts still connect without prompting and the separate `SSH_ASKPASS` helper has been removed.
  - ProxyJump (`-J`) is not yet supported in the terminal and is refused with a clear message instead of connecting direct; metrics/SFTP never used it. Tracked as a follow-up.
  - Passphrase-protected keys without an SSH agent cannot be unlocked interactively in the terminal anymore. Use an SSH agent or an unencrypted key. On Windows there is no agent fallback yet, so prefer an unencrypted key there.
  - The terminal no longer reads `~/.ssh/config` directly, so exotic directives such as `ProxyCommand` no longer apply (the parsed HostName/User/Port/IdentityFile/ProxyJump still do). This makes the terminal consistent with the rest of the app.

### Security
- Validate public-key format and reject control characters before embedding keys in remote shell commands.
- Sanitize `sshd_config` directive edits and scope the disabled `Include` to the cloud-init drop-in only.
- Store `config.toml`, `hosts.toml`, and `snippets.toml` with `0600` permissions, and remove temp files left by a failed save.
- Reject `..` traversal and null bytes in SFTP upload/download paths.
- Pin all GitHub Actions to commit SHAs and verify release archives against `SHA256SUMS` in `install.sh` (with a `shasum` fallback on macOS).

### Bug Fixes
- **Log files no longer fill the disk**: On startup OmnySSH now prunes rolling log files older than 7 days from its config directory. The cleanup is best-effort and never blocks startup, works on every platform via the native config path, and only touches `omnyssh.log*` files — `config.toml`, `hosts.toml`, and `snippets.toml` are left untouched.
- **Man page now installs reliably**: `install.sh` failed to install the man page into the system directory (e.g. `/usr/local/share/man` on macOS) because it never elevated with `sudo`, so `man omny` reported "No manual entry". The man page is now downloaded to a temp file first and installed with a `sudo` fallback, mirroring the binary install.
- **Remote command exit status is no longer lost** when it arrives after EOF.
- **Multi-file deletes fixed**: all in-flight remote deletes are tracked before the panel refreshes, and every error from a multi-file local delete is reported.
- **Terminal is always restored on exit**, even if an earlier teardown step fails.
- **Docker view scales to many containers**: `docker inspect` is batched to stay under `ARG_MAX`.

### Documentation
- Fixed inaccurate docs: `--verbose` now correctly states logs are written to a log file (not stderr) in `--help`, the man page, and the README; `install.sh` prints the OS-specific config directory; corrected the horizontal split keybinding in the changelog (`Ctrl+]`, not `Ctrl+-`).
- More inaccurate docs corrected: `--theme` is now documented as persisting your choice to `config.toml` (it always wrote the config back, but the README called it temporary); the `[ui]` config example no longer lists `show_ip`, `show_uptime`, `card_layout`, and `border_style`, which are parsed for forward compatibility but not yet wired to the renderer; and the "static binary" wording was corrected since the x86_64 Linux and macOS builds are not statically linked.

### Other
- Removed the vestigial empty `package.json` and `package-lock.json` (left over from an earlier project name); they served no purpose in this Rust project.
- Removed the unused `config/default.toml` template — it was never read by the app, and the README already carries the canonical config example.
- CI now fails if the committed `doc/omny.1` man page drifts from `src/cli.rs`, so the generated man page can no longer go stale.
- Removed dead code with no effect on runtime behaviour: unused functions with no callers (`Screen::title`, `Host::id`, `key_path_for_host`, the superseded `host_list` render path, and two unused popup renderers), plus three methods that were only reachable from their own tests (`FileManagerPanel::clamp_scroll`, `ProbeOutput::section_names`, `KeySetupMachine::password_disabled`) and those tests.
- Made the `sshd` rollback backup lookup portable so it also works on BusyBox/Alpine hosts.

---

## 1.0.3 — 2026-05-21

### Features
- **Termux / Android support**: Releases now include a statically linked `aarch64-unknown-linux-musl` build that runs on Termux. `install.sh` detects Termux via `$TERMUX_VERSION` / `$PREFIX` and installs the binary into `$PREFIX/bin` (and the man page into `$PREFIX/share/man/man1`) without `sudo`.

---

## 1.0.2 — 2026-05-16

### Features
- **Automatic update checks**: On startup OmnySSH checks GitHub Releases for a newer version and shows a popup when one is available. You can install the update, skip that version, or disable checks entirely. Failed or offline checks are silent and never delay startup.
- **In-app self-update**: For manual / `install.sh` installs on Linux and macOS, an update can be downloaded and installed from within the app — the release archive is verified against its SHA-256 checksum before the binary is replaced. Homebrew, Cargo, and Nix installs instead show the matching upgrade command.
- **Top processes on the detail page**: The server detail view now shows the three busiest processes by CPU usage, along with their CPU and memory percentages.

### Bug Fixes
- **Windows double input fixed**: Each keystroke is now registered once instead of twice (e.g. "j" no longer produced "jj"). Key-release events reported by the Windows console are no longer treated as input.
- **Ubuntu 22.04 compatibility**: Linux release binaries are now built against an older glibc, fixing the `version 'GLIBC_2.39' not found` error when running on Ubuntu 22.04 and similarly aged distributions.
- **Mouse scroll inside full-screen apps fixed**: On the Terminal screen, the mouse wheel now scrolls inside `vim`, `less`, `htop`, and other alternate-screen apps. The wheel is forwarded to the foreground application — as native mouse-wheel events when it enabled mouse reporting, or as cursor-key presses otherwise. The normal screen still scrolls local scrollback.
- **Multi-line paste into the terminal fixed**: Bracketed paste is now implemented. Pasting multi-line text into the Terminal screen no longer drops the first characters, and editors like `vim` insert it verbatim without cascading auto-indent.
- **Top processes exclude OmnySSH's own connection**: The detail-page top-processes panel no longer lists OmnySSH's metric-polling SSH connection. Its `sshd` process chain is filtered out by PID, so an idle server shows real workload while SSH sessions from other users still appear.
- **Bracketed paste restored after system SSH**: Connecting to a host via the system `ssh` binary no longer leaves bracketed paste disabled on return, so multi-line paste into the terminal keeps working.
- **Host keys are now verified**: A server's host key is recorded in `~/.ssh/known_hosts` on first connection (trust on first use). A later key change — or an unreadable `known_hosts` — refuses the connection instead of silently accepting an unverified key.
- **Password authentication reliably disabled**: Auto SSH Key Setup now adds the `PasswordAuthentication`, `UsePAM`, and challenge/keyboard-interactive directives when `sshd_config` omits them, so password login is disabled even on configs that previously relied on compiled-in defaults.
- **System SSH launch failure no longer quits the app**: If the `ssh` binary cannot be started, OmnySSH restores the TUI and reports the error in the status bar instead of exiting.
- **Remote command exit status honoured**: SSH command execution now exposes a non-zero remote exit status; the key-setup sudo check uses it to detect missing sudo access correctly.

### Other
- Release archives are now published alongside a `SHA256SUMS` checksum file.
- **Internal refactor**: The oversized `app.rs` was split into focused submodules under `src/app/` (host, snippets, file manager, terminal, update, input, and action dispatch). This is a pure code reorganization with no change in behaviour.
- **Test coverage**: Added unit tests for previously untested core logic — snippet parameter substitution, host/snippet form validation, the SSH-config/manual host merge, `hosts.toml` serialization, form-field UTF-8 editing, host/snippet filtering and sorting, file-panel selection, and terminal pane state.

---

## 1.0.1 — 2026-04-22

### Bug Fixes
- **TUI display corruption fixed**: Log output no longer bleeds through the TUI interface. All logging is now redirected to a log file (`~/.config/omnyssh/omnyssh.log` on Linux, `~/Library/Application Support/omnyssh/omnyssh.log.*` on macOS) instead of stderr, preventing raw error messages (such as SSH timeout warnings) from corrupting the terminal display during background operations.
- **Error notifications**: Connection failures, discovery timeouts, and snippet execution errors are now displayed as concise notifications in the status bar instead of being silently logged.
- **SFTP connection freeze fixed**: SFTP connections now run in the background with a 30-second timeout, preventing the UI from freezing indefinitely when connecting to slow or unresponsive servers. A "Connecting… (30s timeout)" indicator is displayed during the connection attempt.
- **Terminal scroll fixed**: Two-finger trackpad and mouse-wheel scroll on the Terminal screen now scrolls local scrollback instead of cycling the remote shell's command history. Previously, mouse capture was disabled on the Terminal screen to allow native mouse text selection, which caused host terminal emulators to translate scroll gestures into ArrowUp/ArrowDown keys that bash readline interpreted as history navigation. Mouse capture is now kept on across all screens.
- **Native drag-to-select preserved**: Mouse capture now enables only button and scroll-wheel reporting (`?1000h` + `?1006h`), dropping the aggressive any-motion tracking (`?1002h` / `?1003h`) that crossterm enables by default. In terminals that honor the modifier-bypass for mouse reporting (iTerm2 on macOS, most Linux terminals), hold `Option` (iTerm2) or `Shift` (Linux) while dragging to select and copy text in the Terminal screen without the application intercepting the drag. Note: macOS Terminal.app does not support modifier-bypass for mouse reporting at all — users on Terminal.app should switch to iTerm2 or a similar emulator for in-app text selection.

---

## 1.0.0 — 2026-04-18

First production-ready release of OmnySSH.

### Features

#### Dashboard
- Server cards with live **CPU / RAM / Disk** metrics, uptime, and load average
- Colour-coded thresholds: 🟢 < 60%, 🟡 60–85%, 🔴 > 85%
- Async metrics collection — each host polled independently via SSH
- Cross-platform parsers: Linux (`top`/`free`/`/proc/stat`), macOS (`vm_stat`), Alpine BusyBox
- Configurable poll interval (default 30 s) with exponential backoff on failure
- Sort by name / CPU / RAM / status (`s`)
- Filter by tag (`t`)
- Manual refresh (`r`)
- Connection status indicator: `●` online, `◐` connecting, `✗` failed
- Connection pool: one SSH connection per host, reused for all metrics

#### Host management
- Host list with instant fuzzy search (`/`)
- Automatic import from `~/.ssh/config` (Host, HostName, User, Port, IdentityFile, ProxyJump, Include)
- Add / Edit / Delete hosts via TUI forms
- Tags and notes for each host
- Persistence in `~/.config/omnyssh/hosts.toml` — original `~/.ssh/config` is never modified
- Delete confirmation popup

#### File Manager (SFTP)
- Split-panel browser: local files ↔ remote SFTP
- Directory navigation with `h/j/k/l` and arrow keys
- File operations: upload, download, delete, mkdir, rename
- Progress bar with percentage for transfers
- Multiple file selection with `Space`
- Copy (`c`) / Paste (`p`) across panels
- Plain-text file preview
- Host-picker popup for remote panel

#### Snippets
- Save, edit, and delete global and host-scoped command snippets
- Parameterised snippets with `{{placeholder}}` syntax
- Quick-execute (`x`): run ad-hoc commands from the Dashboard
- Broadcast mode (`b`): execute on multiple hosts in parallel
- Fuzzy search on the Snippets screen
- Persistence in `~/.config/omnyssh/snippets.toml`

#### Multi-session terminal
- PTY-backed terminal with tabs (`Ctrl+T` / `Ctrl+W`)
- Split-view: `Ctrl+\` vertical, `Ctrl+]` horizontal
- Tab navigation with `Ctrl+Right` / `Ctrl+Left`
- Activity indicator on tabs with unseen output
- Full VT100 terminal emulation (`portable-pty` + `vt100`)
- Non-blocking render — terminal never freezes the UI

#### Themes & Configuration
- 4 built-in colour themes: `default`, `dracula`, `nord`, `gruvbox`
- `--theme <THEME>` CLI flag to override theme at runtime: `omny --theme dracula`
- Fully configurable keybindings via `[keybindings]` in config
- `--config <FILE>` flag to load a custom config
- `--help` / `--version` flags

#### General
- Cross-platform: Linux, macOS, Windows (single static binary)
- Panic hook that restores the terminal before printing backtrace
- `russh`-based async SSH client (no external `ssh` binary dependency for metrics)
- CI: GitHub Actions matrix for Ubuntu, macOS, Windows

---

## Development history

| Date | Version | Milestone |
|------|---------|-----------|
| 2026-04-04 | `0.0.1` | Project skeleton — TUI shell, event loop, placeholder screens |
| 2026-04-05 | `0.1.0` | Host list, SSH connect, fuzzy search — first MVP |
| 2026-04-06 | `0.2.0` | Live metrics dashboard with async polling |
| 2026-04-07 | `0.3.0` | Command snippets, quick-execute, broadcast |
| 2026-04-08 | `0.4.0` | SFTP file manager with split-panel UI |
| 2026-04-09 | `0.5.0` | Multi-session PTY tabs and split-view |
| 2026-04-10 | **`1.0.0`** | **Themes, configurable keybindings, production release** |
