use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Main application configuration, loaded from
/// `~/.config/omnyssh/config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub general: GeneralConfig,
    pub ui: UiConfig,
    pub keybindings: KeybindingsConfig,
    pub auto_key_setup: AutoKeySetupConfig,
    pub update: UpdateConfig,
}

/// General / runtime settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Seconds between automatic metric refreshes.
    pub refresh_interval: u64,

    pub default_shell: String,

    /// Path to the system SSH binary.
    pub ssh_command: String,

    pub max_concurrent_connections: usize,

    /// External editor command.
    /// Examples:
    /// "zed"
    /// "code --wait"
    /// "vim"
    /// Empty = use $VISUAL → $EDITOR → vim
    pub external_editor: String,

    /// Files larger than this open externally.
    pub large_file_mb: u64,

    /// Ask before downloading a large remote file.
    pub prompt_large_files: bool,

    /// Upload remote temp files automatically after editing.
    pub auto_upload_external: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            refresh_interval: 30,
            default_shell: String::from("/bin/bash"),
            ssh_command: String::from("ssh"),
            max_concurrent_connections: 10,

            external_editor: String::new(),
            large_file_mb: 5,
            prompt_large_files: true,
            auto_upload_external: true,
        }
    }
}

/// Visual / theme settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// One of: `default`, `dracula`, `nord`, `gruvbox`.
    pub theme: String,
    // TODO(future-stage): these fields are parsed from user config but not yet
    // wired up to the renderer.  They are kept in the struct so existing config
    // files are accepted without error; the renderer will consume them once the
    // corresponding UI features land.
    pub show_ip: bool,
    pub show_uptime: bool,
    /// One of: `grid`, `list`.
    pub card_layout: String,
    /// One of: `rounded`, `plain`, `double`.
    pub border_style: String,
}

impl UiConfig {
    /// Returns the list of all available built-in theme names.
    ///
    /// These names correspond to the built-in themes of the TUI frontend.
    pub fn available_themes() -> &'static [&'static str] {
        &["default", "dracula", "nord", "gruvbox"]
    }

    /// Checks if the given theme name is valid.
    ///
    /// # Examples
    /// ```
    /// # use omnyssh_core::config::app_config::UiConfig;
    /// assert!(UiConfig::is_valid_theme("dracula"));
    /// assert!(!UiConfig::is_valid_theme("unknown"));
    /// ```
    pub fn is_valid_theme(name: &str) -> bool {
        Self::available_themes().contains(&name)
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: String::from("default"),
            show_ip: true,
            show_uptime: true,
            card_layout: String::from("grid"),
            border_style: String::from("rounded"),
        }
    }
}

/// Keyboard shortcut overrides (all values are key name strings).
///
/// Supports plain key names (`"Tab"`, `"q"`, `"F1"`) and `"Ctrl+<char>"` format
/// (e.g. `"Ctrl+T"`, `"Ctrl+W"`) for modifiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindingsConfig {
    pub quit: String,
    pub search: String,
    pub dashboard: String,
    pub file_manager: String,
    pub snippets: String,
    /// Key to cycle to the next app screen (dashboard → files → snippets →
    /// terminal).  Also used to switch panels in File Manager.
    /// Default: `"Tab"`.
    pub next_screen: String,
    /// Key to cycle terminal tabs / split panes.
    /// Default: `"Ctrl+N"`.
    pub next_tab: String,
}

/// Auto SSH Key Setup configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoKeySetupConfig {
    /// Enable the auto key setup feature.
    pub enabled: bool,
    /// Show suggestion banner when password authentication is detected.
    pub suggest_on_password_auth: bool,
    /// Automatically disable password authentication after key setup (requires sudo).
    pub disable_password_auth: bool,
    /// SSH key type to generate (ed25519 | rsa-4096).
    pub key_type: String,
    /// Directory where SSH keys are stored (default: ~/.ssh).
    pub key_directory: String,
    /// Always create a backup of sshd_config before modification.
    pub backup_sshd_config: bool,
    /// Ask for confirmation before disabling password authentication.
    pub confirm_before_disable: bool,
}

impl Default for AutoKeySetupConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            suggest_on_password_auth: true,
            disable_password_auth: true,
            key_type: String::from("ed25519"),
            key_directory: String::from("~/.ssh"),
            backup_sshd_config: true,
            confirm_before_disable: true,
        }
    }
}

/// Update checker configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// Check GitHub Releases for a newer version on startup.
    pub check_on_startup: bool,
    /// A version the user chose to skip; it is never offered again.
    pub skip_version: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            check_on_startup: true,
            skip_version: String::new(),
        }
    }
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        Self {
            quit: String::from("q"),
            search: String::from("/"),
            dashboard: String::from("F1"),
            file_manager: String::from("F2"),
            snippets: String::from("F3"),
            next_screen: String::from("Tab"),
            next_tab: String::from("Ctrl+N"),
        }
    }
}

// ---------------------------------------------------------------------------
// Config file loading
// ---------------------------------------------------------------------------

/// Loads the application config from `path`, or from the default location
/// (`~/.config/omnyssh/config.toml`) when `path` is `None`.
///
/// A missing config file is silently ignored and [`AppConfig::default`] is
/// returned.  Parse errors are propagated so the user sees them at startup.
///
/// # Errors
/// Returns an error only if the file exists but cannot be read or parsed.
pub fn load_app_config(path: Option<&std::path::Path>) -> anyhow::Result<AppConfig> {
    use crate::utils::platform;

    let config_path = match path {
        Some(p) => p.to_path_buf(),
        None => match platform::app_config_path() {
            Some(p) => p,
            None => return Ok(AppConfig::default()),
        },
    };

    if !config_path.exists() {
        return Ok(AppConfig::default());
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config: {}", config_path.display()))?;

    let config: AppConfig = toml::from_str(&content)
        .with_context(|| format!("Failed to parse config: {}", config_path.display()))?;

    Ok(config)
}

/// Loads the on-disk config (or a default), applies `mutator`, and writes it
/// back. Reading fresh from disk avoids clobbering unrelated edits.
///
/// # Errors
/// Returns an error if the config file cannot be read, parsed, or written.
fn persist_config<F: FnOnce(&mut AppConfig)>(mutator: F) -> anyhow::Result<()> {
    use crate::utils::platform;

    let config_path = match platform::app_config_path() {
        Some(p) => p,
        None => anyhow::bail!("Cannot determine config path for this platform"),
    };

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    let mut config = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config: {}", config_path.display()))?;
        toml::from_str::<AppConfig>(&content)
            .with_context(|| format!("Failed to parse config: {}", config_path.display()))?
    } else {
        AppConfig::default()
    };

    mutator(&mut config);

    let content = toml::to_string_pretty(&config).context("Failed to serialize config")?;
    std::fs::write(&config_path, content)
        .with_context(|| format!("Failed to write config: {}", config_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&config_path, perms);
    }

    Ok(())
}

/// Saves the theme selection to the config file's `[ui]` section.
///
/// # Errors
/// Returns an error if the config file cannot be written or parsed.
pub fn save_theme_to_config(theme_name: &str) -> anyhow::Result<()> {
    persist_config(|config| config.ui.theme = theme_name.to_string())
}

/// Saves the entire `[general]` section to the config file.
/// Shared by both the TUI and GUI so editor preferences stay synchronized.
/// # Errors
/// Returns an error if the config file cannot be written or parsed.
pub fn save_general_config(general: &GeneralConfig) -> anyhow::Result<()> {
    let general = general.clone();
    persist_config(move |config| config.general = general)
}

/// Saves the update-checker preferences to the config file's `[update]`
/// section.
///
/// # Errors
/// Returns an error if the config file cannot be written or parsed.
pub fn save_update_config(update: &UpdateConfig) -> anyhow::Result<()> {
    let update = update.clone();
    persist_config(move |config| config.update = update)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_config_defaults_to_enabled() {
        let cfg = UpdateConfig::default();
        assert!(cfg.check_on_startup);
        assert!(cfg.skip_version.is_empty());
    }

    /// A config file written by an older release (no `[update]` section)
    /// must still parse, falling back to the default update settings.
    #[test]
    fn config_without_update_section_parses() {
        let cfg: AppConfig = toml::from_str("[ui]\ntheme = \"nord\"\n").unwrap();
        assert_eq!(cfg.ui.theme, "nord");
        assert!(cfg.update.check_on_startup);
    }

    #[test]
    fn update_config_round_trips_through_toml() {
        let mut cfg = AppConfig::default();
        cfg.update.check_on_startup = false;
        cfg.update.skip_version = "1.2.3".to_string();

        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let parsed: AppConfig = toml::from_str(&serialized).unwrap();
        assert!(!parsed.update.check_on_startup);
        assert_eq!(parsed.update.skip_version, "1.2.3");
    }
}
