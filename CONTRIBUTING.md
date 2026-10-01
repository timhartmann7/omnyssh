# Contributing to OmnySSH

Thank you for your interest in contributing!  This document describes the
development workflow, coding conventions, and review process.

---

## Table of contents

1. [Setting up the development environment](#1-setting-up-the-development-environment)
2. [Running the project](#2-running-the-project)
3. [Running tests](#3-running-tests)
4. [Code conventions](#4-code-conventions)
5. [Commit style](#5-commit-style)
6. [Opening a pull request](#6-opening-a-pull-request)
7. [Reporting bugs](#7-reporting-bugs)

---

## 1. Setting up the development environment

**Prerequisites:**

| Tool | Minimum version | Install |
|------|----------------|---------|
| Rust | stable (1.89+) | `rustup install stable` |
| Git  | any recent     | OS package manager |
| Node.js *(GUI only)* | 20+ | [nodejs.org](https://nodejs.org) or `nvm` |
| Tauri CLI *(GUI only)* | v2 | `npm i -g @tauri-apps/cli@^2` or `cargo install tauri-cli` |

**Clone and build:**

```bash
git clone https://github.com/timhartmann7/omnyssh.git
cd omnyssh
cargo build
```

The first build fetches all dependencies from crates.io and may take a few
minutes.  Subsequent builds are incremental.

**Repository layout:** the repo is a cargo workspace with three crates:

| Crate | Path | Contents |
|-------|------|----------|
| `omnyssh-core` | `crates/omnyssh-core` | SSH engine, configs, metrics, domain events, updater — no UI dependencies |
| `omnyssh` | `crates/omnyssh` | The TUI application (binary `omny`), depends on `omnyssh-core` |
| `omnyssh-gui` | `crates/omnyssh-gui` | The desktop GUI (Tauri 2 + SvelteKit): Rust IPC in `src/`, the web frontend in `ui/` |

Bare `cargo build` / `test` / `install` at the root build **core + TUI only**
(the GUI is excluded via `default-members`), so contributors without the
Node/Tauri toolchain are unaffected.  All `cargo` commands below work from the
repository root; use `-p omnyssh-core` / `-p omnyssh` / `-p omnyssh-gui` to
target one crate.

**Recommended tools:**

```bash
rustup component add clippy rustfmt
cargo install cargo-watch   # optional: auto-rebuild on file changes
```

---

## 2. Running the project

```bash
# Debug build (fast compile, slower runtime)
cargo run

# With a custom config file
cargo run -- --config ./my-config.toml

# Verbose logging to stderr
cargo run -- --verbose

# Release build
cargo build --release
./target/release/omny
```

During development you can use `cargo watch` to rebuild on every save:

```bash
cargo watch -x run
```

### The desktop GUI

The GUI needs the Node/Tauri toolchain (see prerequisites).  From
`crates/omnyssh-gui`:

```bash
cd crates/omnyssh-gui/ui && npm ci && cd ..

# Dev: hot-reloads the frontend and rebuilds the Rust side on change
cargo tauri dev

# Release bundles (.dmg / .AppImage + .deb / .msi|.exe) into target/release/bundle
cargo tauri build
```

---

## 3. Running tests

```bash
# All tests
cargo test

# Only unit tests of one crate (no integration tests)
cargo test -p omnyssh-core --lib
cargo test -p omnyssh --lib

# Only integration tests in crates/omnyssh-core/tests/
cargo test -p omnyssh-core --test metrics_parser
cargo test -p omnyssh-core --test ssh_config_parser

# With output (useful when debugging a failing test)
cargo test -- --nocapture
```

### GUI tests

The GUI has its own Rust and frontend suites (not part of the bare workspace
build):

```bash
cargo test -p omnyssh-gui                       # Rust IPC / DTO tests
cd crates/omnyssh-gui/ui
npm run check                                   # svelte-check + tsc
npm test                                        # vitest unit tests
```

### Linting

All CI checks must pass before a pull request can be merged:

```bash
cargo clippy -- -D warnings   # no warnings allowed
cargo fmt --check             # formatting must match rustfmt defaults
```

---

## 4. Code conventions

These conventions are enforced in code review and by CI.

### Architecture

- **The engine stays UI-free.**  Code in `crates/omnyssh-core` must not depend
  on terminal-rendering, input, or CLI crates; frontend-specific types and
  logic belong in `crates/omnyssh`.
- **Never block the UI thread with SSH operations.**  All network I/O runs in
  background `tokio::spawn` tasks and communicates via `mpsc` channels.
- **Use `Arc<RwLock<T>>` for shared state**, not `Arc<Mutex<T>>`.  The UI
  reads state ~30 times per second; SSH tasks write rarely.
- **One event loop**, many event sources.  Don't create per-screen loops.
- **Separate `AppState` (data) from `ViewState` (UI).**  Background tasks
  only touch `AppState`.

### Error handling

- No `.unwrap()` in production code — use `?`, `anyhow`, or `.expect("reason")`
  only where a panic is provably impossible.
- Every SSH error (timeout, auth failure, host key mismatch) must be shown
  to the user via the status bar or a popup.

### UI

- `render()` functions must never panic.  Use `Option<T>` and show
  `"Loading…"` placeholders when data is not yet available.
- Destructive operations (delete host, delete file) require a confirmation
  popup.
- Every screen has its own `handle_input()` function.

### Cross-platform

- Use `dirs::home_dir()` / `dirs::config_dir()` for all user-directory paths
  — never hardcode `~`.
- Use crossterm for all terminal I/O — no raw ANSI escape codes.
- Parse SSH command output with `.lines()` to handle both `\n` and `\r\n`.

### Dependencies

Before adding a new crate, check whether the feature can be implemented in
~10 lines of Rust.  Every new dependency increases compile time and binary
size.

---

## 5. Commit style

OmnySSH uses [Conventional Commits](https://www.conventionalcommits.org/).

```
<type>(<optional scope>): <short summary>

[optional body]

[optional footer]
```

**Types:**

| Type       | When to use |
|------------|-------------|
| `feat`     | New user-visible feature |
| `fix`      | Bug fix |
| `docs`     | Documentation only |
| `refactor` | Code change with no user-visible effect |
| `test`     | Adding or fixing tests |
| `chore`    | Build system, CI, dependency bumps |
| `perf`     | Performance improvement |

**Examples:**

```
feat(themes): add gruvbox colour scheme

fix(ssh): respect connection timeout when host is unreachable

docs: update README with installation instructions

chore: bump ratatui to 0.29
```

---

## 6. Opening a pull request

1. Fork the repository and create a branch:
   ```bash
   git checkout -b feat/my-feature
   ```
2. Make your changes following the conventions above.
3. Run the full test suite and linter:
   ```bash
   cargo test && cargo clippy -- -D warnings && cargo fmt --check
   ```
4. Push and open a PR against the `main` branch.
5. Fill in the PR template (problem, solution, test plan).
6. A maintainer will review within a few days.

**PR checklist:**

- [ ] All tests pass (`cargo test`)
- [ ] No clippy warnings (`cargo clippy -- -D warnings`)
- [ ] Code is formatted (`cargo fmt`)
- [ ] Relevant tests added (parsers, new features)
- [ ] CHANGELOG entry added
- [ ] README updated if the change is user-visible

---

## 7. Reporting bugs

Please open a GitHub Issue with:

- OmnySSH version (`omny --version`)
- OS and terminal emulator
- Steps to reproduce
- Expected behaviour vs. actual behaviour
- Relevant log output (run with `--verbose 2>omnyssh.log` and attach the log)

For security issues, please **do not** open a public issue.  Email the
maintainers directly.
