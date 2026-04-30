<div align="center">

# OmnySSH

### TUI SSH dashboard & server manager — manage all your servers from a single terminal window

[![Crates.io](https://img.shields.io/crates/v/omnyssh.svg)](https://crates.io/crates/omnyssh)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/crates/d/omnyssh.svg)](https://crates.io/crates/omnyssh)
[![Build Status](https://img.shields.io/github/actions/workflow/status/timhartmann7/omnyssh/ci.yml?branch=main)](https://github.com/timhartmann7/omnyssh/actions)

![Demo](assets/demo.gif)

**[Features](#features)** •
**[Installation](#installation)** •
**[Quick Start](#quick-start)** •
**[Documentation](#documentation)** •
**[Contributing](#contributing)**

</div>

---

## Why OmnySSH?

Managing multiple SSH servers shouldn't require juggling terminal tabs, remembering IP addresses, or running the same commands over and over. OmnySSH brings **dashboard-style monitoring**, **visual file management**, and **command automation** into a single, lightweight TUI.

**Stop switching between tools. Start managing smarter.**

| Traditional Workflow | With OmnySSH |
|---------------------|--------------|
| Open 10 terminal tabs for 10 servers | Single dashboard with all servers visible |
| `ssh user@192.168.1.10` → `top` → note CPU | Live CPU/RAM/Disk metrics on cards |
| `scp -r local/ user@host:/remote/` | Drag-and-drop file manager (local ↔ remote) |
| Paste the same deploy command everywhere | Save as snippet, broadcast to all hosts |
| `tmux` for multi-session SSH | Built-in tabs + split-view terminal |

---

## Features

### 📊 **Live Metrics Dashboard**
Server cards with real-time CPU, RAM, and disk usage. Color-coded thresholds (green → yellow → red) make it easy to spot issues at a glance.

### 📁 **Visual File Manager**
Split-panel SFTP browser (local ↔ remote) with progress bars, multi-selection, and intuitive keyboard shortcuts. No more memorizing `scp` syntax.

### ⚡ **Command Snippets**
Save frequently-used commands and execute them on any server with one keypress. Broadcast a command to multiple hosts simultaneously.

### 🖥️ **Multi-Session Terminal**
PTY tabs and split-view for working on several servers at once. Switch between hosts without leaving the app.

### 🔍 **Fuzzy Search**
Find any host or snippet instantly. Type a few letters, get instant results.

### 🎨 **4 Built-in Themes**
Choose from **Default**, **Dracula**, **Nord**, or **Gruvbox**. Switch themes on the fly with `--theme`.

### ⌨️ **Configurable Keybindings**
Remap global shortcuts in one TOML file. Make OmnySSH work the way you work.

### 🌍 **Cross-Platform**
Linux, macOS, Windows. Single static binary, no runtime dependencies.

---

## Comparison

| Feature | OmnySSH | plain SSH | Termius | tmux + ssh |
|---------|---------|-----------|---------|------------|
| **TUI interface** | ✅ | ❌ | ✅ (GUI) | ✅ |
| **Live metrics dashboard** | ✅ | ❌ | ✅ | ❌ |
| **Visual file manager (SFTP)** | ✅ | ❌ | ✅ | ❌ |
| **Command snippets** | ✅ | ❌ | ✅ | ❌ |
| **Multi-session tabs** | ✅ | ❌ | ✅ | ✅ |
| **Fuzzy search** | ✅ | ❌ | ✅ | ❌ |
| **Configurable themes** | ✅ | ❌ | ✅ | ⚠️ |
| **Open source** | ✅ | ✅ | ❌ | ✅ |
| **Free** | ✅ | ✅ | 💰 | ✅ |
| **Runs in terminal** | ✅ | ✅ | ❌ | ✅ |
| **Single binary** | ✅ | ✅ | ❌ | ❌ |

---

## Installation

### ⚡ Quick Install (Recommended)

**One command to install on Linux/macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/timhartmann7/omnyssh/main/install.sh | sh
```

This script auto-detects your OS and architecture, downloads the latest release, and installs it to your PATH.

---

### 🍺 Homebrew (macOS/Linux)

```bash
brew install timhartmann7/tap/omnyssh
```

---

### 📦 Pre-built Binaries

Download from the [**Releases**](https://github.com/timhartmann7/omnyssh/releases) page:

| Platform | Archive |
|----------|---------|
| Linux x86_64 | `omny-x86_64-unknown-linux-gnu.tar.gz` |
| Linux ARM64 | `omny-aarch64-unknown-linux-gnu.tar.gz` |
| macOS Intel | `omny-x86_64-apple-darwin.tar.gz` |
| macOS Apple Silicon | `omny-aarch64-apple-darwin.tar.gz` |
| Windows x86_64 | `omny-x86_64-pc-windows-msvc.zip` |

Extract and move the binary to your PATH:

```bash
tar -xzf omny-*.tar.gz
sudo mv omny /usr/local/bin/
```

---

### 🦀 Cargo (from crates.io)

```bash
cargo install omnyssh
```

---

### 🔨 From Source

```bash
git clone https://github.com/timhartmann7/omnyssh.git
cd omnyssh
cargo build --release
# Binary at: ./target/release/omny
```

---

### ❄️ Nix (Flakes)

A `flake.nix` is provided for [Nix](https://nixos.org/) users. Requires flakes
enabled (`experimental-features = nix-command flakes` in `~/.config/nix/nix.conf`).

**Run without installing:**

```bash
nix run github:timhartmann7/omnyssh
nix run github:timhartmann7/omnyssh -- --theme dracula
```

**Build a local checkout:**

```bash
git clone https://github.com/timhartmann7/omnyssh.git
cd omnyssh
nix build              # binary at ./result/bin/omny
./result/bin/omny --version
```

**Install into your user profile:**

```bash
nix profile install github:timhartmann7/omnyssh
```

**Develop with a pinned toolchain:**

```bash
nix develop            # drops you into a shell with rustc, cargo, clippy,
                       # rustfmt, rust-analyzer, and all build inputs ready
cargo build
```

The flake exposes `packages.default` (the `omny` binary plus man page),
`apps.default` (for `nix run`), and `devShells.default`. It evaluates
cleanly across `x86_64-linux`, `aarch64-linux`, `x86_64-darwin`, and
`aarch64-darwin`.

---

## Quick Start

1. **Install OmnySSH** (see above)

2. **Run the app:**

   ```bash
   omny
   ```

3. **Add your first server:**
   - Press `a` in the dashboard
   - Enter hostname, user, and SSH key path
   - Press `Enter` to connect

4. **Try different themes:**

   ```bash
   omny --theme dracula
   omny --theme nord
   omny --theme gruvbox
   ```

5. **View full documentation:**

   ```bash
   man omny      # Man page with all options
   omny --help   # Quick reference
   ```

6. **Explore features:**
   - `1` — Dashboard (live metrics)
   - `2` — File Manager (SFTP browser)
   - `3` — Snippets (saved commands)
   - `4` — Terminal (multi-session)
   - `/` — Fuzzy search
   - `?` — Help popup

---

## Documentation

### Man Page (Linux/macOS)

```bash
man omny
```

### Usage

```
omny [OPTIONS]

Options:
  -c, --config <FILE>   Path to a custom config file
  -t, --theme <THEME>   Override the color theme (default | dracula | nord | gruvbox)
  -v, --verbose         Enable debug logging (written to stderr)
  -h, --help            Print help
  -V, --version         Print version
```

### Configuration

Config files live in:
- **Linux/macOS:** `~/.config/omnyssh/`
- **Windows:** `%APPDATA%\omnyssh\`

| File | Purpose |
|------|---------|
| `config.toml` | App settings, theme, keybindings |
| `hosts.toml` | Managed host list |
| `snippets.toml` | Saved commands |

The original `~/.ssh/config` is **never modified** — hosts are imported read-only at startup.

#### Example: config.toml

```toml
[general]
refresh_interval = 30          # seconds between metric refreshes
default_shell = "/bin/bash"
ssh_command = "ssh"            # path to system SSH binary
max_concurrent_connections = 10

[ui]
theme = "default"              # default | dracula | nord | gruvbox
show_ip = true
show_uptime = true
card_layout = "grid"           # grid | list
border_style = "rounded"       # rounded | plain | double

[keybindings]
quit         = "q"
search       = "/"
connect      = "Enter"
dashboard    = "1"
file_manager = "2"
snippets     = "3"
```

#### Example: hosts.toml

```toml
[[hosts]]
name = "web-prod-1"
hostname = "192.168.1.10"
user = "deploy"
port = 22
identity_file = "~/.ssh/id_ed25519"
tags = ["production", "web"]
notes = "Main web server. Nginx + Node.js"

[[hosts]]
name = "db-master"
hostname = "10.0.0.50"
user = "admin"
port = 2222
tags = ["production", "database"]
notes = "PostgreSQL 16. Don't restart without warning #backend"
```

#### Example: snippets.toml

```toml
[[snippets]]
name = "Docker: restart all"
command = "cd /opt/app && docker compose down && docker compose up -d"
scope = "global"
tags = ["docker"]

[[snippets]]
name = "Restart service"
command = "sudo systemctl restart {{service_name}}"
scope = "global"
params = ["service_name"]
```

### Themes

| Theme | Description |
|-------|-------------|
| `default` | Neutral blue/cyan — works with any terminal palette |
| `dracula` | Purple, pink, green — [Dracula](https://draculatheme.com/) |
| `nord` | Arctic blues and teals — [Nord](https://www.nordtheme.com/) |
| `gruvbox` | Warm amber and orange — [Gruvbox](https://github.com/morhetz/gruvbox) |

Set the theme permanently in `config.toml` or temporarily via the `--theme` flag.

---

## Development Roadmap

| Version | Stage | Description |
|---------|-------|-------------|
| `0.0.1` | 0 | Project skeleton — TUI shell with placeholder screens |
| `0.1.0` | 1 | Host list, SSH connect, fuzzy search — MVP |
| `0.2.0` | 2 | Live metrics dashboard |
| `0.3.0` | 3 | Snippets & quick-execute |
| `0.4.0` | 4 | SFTP file manager |
| `0.5.0` | 5 | Multi-session tabs & split-view |
| **`1.0.0`** | **6** | **Polish, themes, configurable keybindings — current** ✅ |

---

## Contributing

Contributions are welcome! Please read [**CONTRIBUTING.md**](CONTRIBUTING.md) for development setup, code conventions, and the PR checklist.

---

## License

Apache 2.0 — see [**LICENSE**](LICENSE).

---

<div align="center">

### ⭐ Star this repo if you find it useful!

[Report Bug](https://github.com/timhartmann7/omnyssh/issues) •
[Request Feature](https://github.com/timhartmann7/omnyssh/issues) •
[Discussions](https://github.com/timhartmann7/omnyssh/discussions)

</div>
