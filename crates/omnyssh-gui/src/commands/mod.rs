//! IPC command handlers, one module per domain (tech-gui.md §3.1). Commands are
//! thin: validate input, call the core, return a DTO or an error.

pub mod auth;
pub mod clipboard;
pub mod hosts;
pub mod keysetup;
pub mod sftp;
pub mod snippets;
pub mod terminal;
pub mod tray;
pub mod tunnels;
pub mod update;
