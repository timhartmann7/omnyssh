# Changelog

All notable changes to OmnySSH are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Versions follow [Semantic Versioning](https://semver.org/).

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
- Split-view: `Ctrl+\` vertical, `Ctrl+-` horizontal
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
