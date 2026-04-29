use anyhow::Context;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

/// Main application configuration, loaded from
/// `~/.config/omnyssh/config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub general: GeneralConfig,
    pub ui: UiConfig,
    pub keybindings: KeybindingsConfig,
    pub smart_context: SmartContextConfig,
    pub auto_key_setup: AutoKeySetupConfig,
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
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            refresh_interval: 30,
            default_shell: String::from("/bin/bash"),
            ssh_command: String::from("ssh"),
            max_concurrent_connections: 10,
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
    /// These names correspond to themes defined in [`crate::ui::theme::Theme`].
    pub fn available_themes() -> &'static [&'static str] {
        &["default", "dracula", "nord", "gruvbox"]
    }

    /// Checks if the given theme name is valid.
    ///
    /// # Examples
    /// ```
    /// # use omnyssh::config::app_config::UiConfig;
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
    pub connect: String,
    pub dashboard: String,
    pub file_manager: String,
    pub snippets: String,
    /// Key to cycle to the next app screen (dashboard → files → snippets →
    /// terminal).  Also used to switch panels in File Manager.
    /// Default: `"Tab"`.
    pub next_screen: String,
    /// Key to cycle terminal tabs / split panes.
    /// Default: `"Tab"`.
    pub next_tab: String,
}

/// Smart Server Context configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartContextConfig {
    /// Enable automatic service discovery and monitoring.
    pub enabled: bool,
    /// Seconds between deep probe scans (set to 0 to disable periodic scans).
    pub scan_interval: u64,
}

impl Default for SmartContextConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            scan_interval: 300, // 5 minutes
        }
    }
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

impl Default for KeybindingsConfig {
    fn default() -> Self {
        Self {
            quit: String::from("q"),
            search: String::from("/"),
            connect: String::from("Enter"),
            dashboard: String::from("F1"),
            file_manager: String::from("F2"),
            snippets: String::from("F3"),
            next_screen: String::from("Tab"),
            next_tab: String::from("Tab"),
        }
    }
}

// ---------------------------------------------------------------------------
// Parsed keybindings — config strings resolved to crossterm KeyCodes
// ---------------------------------------------------------------------------

/// A resolved key binding that may optionally require the `Ctrl` modifier.
#[derive(Debug, Clone, Copy)]
pub struct KeyBind {
    pub code: KeyCode,
    /// If true, the `Ctrl` modifier must be pressed for this binding to match.
    pub ctrl: bool,
}

impl KeyBind {
    pub fn matches(&self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        key.code == self.code && ctrl == self.ctrl
    }
}

/// Keybindings resolved from [`KeybindingsConfig`] into concrete
/// [`crossterm::event::KeyCode`] values used by the event loop.
#[derive(Debug, Clone)]
pub struct ParsedKeybindings {
    /// Key that exits the application (default: `q`).
    pub quit: KeyCode,
    /// Key that activates fuzzy search (default: `/`).
    pub search: KeyCode,
    /// Key that confirms / connects (default: `Enter`).
    pub connect: KeyCode,
    /// Key that switches to the Dashboard screen (default: `F1`).
    pub dashboard: KeyCode,
    /// Key that switches to the File Manager screen (default: `F2`).
    pub file_manager: KeyCode,
    /// Key that switches to the Snippets screen (default: `F3`).
    pub snippets: KeyCode,
    /// Key that cycles to the next screen / switches FM panels (default: `Tab`).
    pub next_screen: KeyBind,
    /// Key that cycles terminal tabs / split panes (default: `Tab`).
    pub next_tab: KeyBind,
}

impl ParsedKeybindings {
    /// Parses a [`KeybindingsConfig`] into concrete key codes.
    ///
    /// Unknown key names fall back to the default binding so the application
    /// never becomes unusable due to a misconfiguration.
    pub fn from_config(cfg: &KeybindingsConfig) -> Self {
        let defaults = KeybindingsConfig::default();
        Self {
            quit: parse_keycode(&cfg.quit)
                .unwrap_or_else(|| parse_keycode(&defaults.quit).expect("default quit")),
            search: parse_keycode(&cfg.search)
                .unwrap_or_else(|| parse_keycode(&defaults.search).expect("default search")),
            connect: parse_keycode(&cfg.connect)
                .unwrap_or_else(|| parse_keycode(&defaults.connect).expect("default connect")),
            dashboard: parse_keycode(&cfg.dashboard)
                .unwrap_or_else(|| parse_keycode(&defaults.dashboard).expect("default dashboard")),
            file_manager: parse_keycode(&cfg.file_manager).unwrap_or_else(|| {
                parse_keycode(&defaults.file_manager).expect("default file_manager")
            }),
            snippets: parse_keycode(&cfg.snippets)
                .unwrap_or_else(|| parse_keycode(&defaults.snippets).expect("default snippets")),
            next_screen: parse_keybind(&cfg.next_screen)
                .unwrap_or_else(|| parse_keybind(&defaults.next_screen).expect("default next_screen")),
            next_tab: parse_keybind(&cfg.next_tab)
                .unwrap_or_else(|| parse_keybind(&defaults.next_tab).expect("default next_tab")),
        }
    }
}

impl Default for ParsedKeybindings {
    fn default() -> Self {
        Self::from_config(&KeybindingsConfig::default())
    }
}

/// Parses a key name string (from config TOML) into a [`KeyCode`].
///
/// Supported formats:
/// - Single printable character: `"q"`, `"/"`, `" "` → `KeyCode::Char(_)`
/// - `"Enter"` → `KeyCode::Enter`
/// - `"Esc"` / `"Escape"` → `KeyCode::Esc`
/// - `"Tab"` → `KeyCode::Tab`
/// - `"Backspace"` / `"BS"` → `KeyCode::Backspace`
/// - `"F1"` … `"F12"` → `KeyCode::F(_)`
/// - `"Up"`, `"Down"`, `"Left"`, `"Right"` → directional keys
///
/// Returns `None` for unrecognised strings.
pub fn parse_keycode(s: &str) -> Option<KeyCode> {
    match s {
        "Enter" => Some(KeyCode::Enter),
        "Esc" | "Escape" => Some(KeyCode::Esc),
        "Tab" => Some(KeyCode::Tab),
        "Backtab" | "BackTab" | "ShiftTab" => Some(KeyCode::BackTab),
        "Backspace" | "BS" => Some(KeyCode::Backspace),
        "Delete" | "Del" => Some(KeyCode::Delete),
        "Up" => Some(KeyCode::Up),
        "Down" => Some(KeyCode::Down),
        "Left" => Some(KeyCode::Left),
        "Right" => Some(KeyCode::Right),
        "Home" => Some(KeyCode::Home),
        "End" => Some(KeyCode::End),
        "PageUp" => Some(KeyCode::PageUp),
        "PageDown" => Some(KeyCode::PageDown),
        f if f.starts_with('F') || f.starts_with('f') => f[1..].parse::<u8>().ok().map(KeyCode::F),
        c if c.chars().count() == 1 => c.chars().next().map(KeyCode::Char),
        _ => None,
    }
}

/// Parses a key binding string into a [`KeyBind`].
///
/// Supports two formats:
/// - `"Ctrl+<key>"` — requires the `Ctrl` modifier (e.g. `"Ctrl+T"`, `"Ctrl+W"`).
///   For printable characters the key portion is lower-cased automatically.
/// - Plain key names — passed through to [`parse_keycode`] with `ctrl: false`.
///
/// # Examples
/// ```
/// # use omnyssh::config::app_config::parse_keybind;
/// // Ctrl+T for screen cycling, freeing Tab for shell completion.
/// let kb = parse_keybind("Ctrl+T").unwrap();
/// assert!(kb.ctrl);
/// ```
pub fn parse_keybind(s: &str) -> Option<KeyBind> {
    // "Ctrl+<key>" format (case-insensitive prefix).
    if let Some(rest) = s
        .strip_prefix("Ctrl+")
        .or_else(|| s.strip_prefix("ctrl+"))
        .or_else(|| s.strip_prefix("CTRL+"))
    {
        // Ctrl+<char>: always lower-case so "Ctrl+T" and "Ctrl+t" both work.
        if rest.chars().count() == 1 {
            return rest.chars().next().map(|c| KeyBind {
                code: KeyCode::Char(c.to_ascii_lowercase()),
                ctrl: true,
            });
        }
        // Named key e.g. "Ctrl+Enter", "Ctrl+Tab".
        if let Some(code) = parse_keycode(rest) {
            return Some(KeyBind { code, ctrl: true });
        }
        return None;
    }
    // Plain key name — no modifier required.
    parse_keycode(s).map(|code| KeyBind { code, ctrl: false })
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

/// Saves the theme selection to the config file.
///
/// If the config file doesn't exist, it creates a new one with the theme setting.
/// If it exists, it updates the `[ui]` section's `theme` field.
///
/// # Errors
/// Returns an error if the config file cannot be written or parsed.
pub fn save_theme_to_config(theme_name: &str) -> anyhow::Result<()> {
    use crate::utils::platform;

    let config_path = match platform::app_config_path() {
        Some(p) => p,
        None => anyhow::bail!("Cannot determine config path for this platform"),
    };

    // Ensure config directory exists
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    // Load existing config or create default
    let mut config = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config: {}", config_path.display()))?;
        toml::from_str::<AppConfig>(&content)
            .with_context(|| format!("Failed to parse config: {}", config_path.display()))?
    } else {
        AppConfig::default()
    };

    // Update theme
    config.ui.theme = theme_name.to_string();

    // Serialize and write back
    let content = toml::to_string_pretty(&config).context("Failed to serialize config")?;

    std::fs::write(&config_path, content)
        .with_context(|| format!("Failed to write config: {}", config_path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that every hard-coded default key string parses successfully.
    /// This converts a potential runtime panic in `.expect("default …")` into a
    /// compile-time-visible test failure.
    #[test]
    fn default_keybindings_parse() {
        let _kb = ParsedKeybindings::default();
    }

    #[test]
    fn parse_ctrl_combo() {
        let kb = parse_keybind("Ctrl+T").unwrap();
        assert!(kb.ctrl);
        assert_eq!(kb.code, KeyCode::Char('t'));

        let kb = parse_keybind("ctrl+w").unwrap();
        assert!(kb.ctrl);
        assert_eq!(kb.code, KeyCode::Char('w'));

        let kb = parse_keybind("CTRL+q").unwrap();
        assert!(kb.ctrl);
        assert_eq!(kb.code, KeyCode::Char('q'));
    }

    #[test]
    fn parse_plain_key() {
        let kb = parse_keybind("Tab").unwrap();
        assert!(!kb.ctrl);
        assert_eq!(kb.code, KeyCode::Tab);

        let kb = parse_keybind("F5").unwrap();
        assert!(!kb.ctrl);
    }
}
