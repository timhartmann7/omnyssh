//! DTOs crossing the IPC boundary (tech-gui.md §4.1). Every type derives serde +
//! `specta::Type` so `bindings.ts` is generated, never hand-written. Secret
//! fields (`password`, key material) never appear here.

use serde::{Deserialize, Serialize};

use omnyssh_core::config::app_config::UpdateConfig;
use omnyssh_core::config::snippets::{Snippet, SnippetScope};
use omnyssh_core::event::{
    DetectedService, MetricValue, Metrics, ProcessInfo, ServiceKind, ServiceMetric,
};
use omnyssh_core::ssh::client::{ConnectionStatus, Host, HostSource, MonitorMode};
use omnyssh_core::ssh::key_setup::KeySetupStep;
use omnyssh_core::ssh::sftp::FileEntry;
use omnyssh_core::ssh::tunnel::{LocalForward, TunnelStatus};
use omnyssh_core::update::UpdateInfo;

/// Host origin, mirrors `omnyssh_core::ssh::client::HostSource`.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum HostSourceDto {
    SshConfig,
    Manual,
}

/// How a host is watched, mirrors `omnyssh_core::ssh::client::MonitorMode`
/// (tech-gui.md §4.1). `tcpPort` means reachability only — no login, no metrics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum MonitorModeDto {
    Ssh,
    TcpPort,
}

impl From<MonitorMode> for MonitorModeDto {
    fn from(mode: MonitorMode) -> Self {
        match mode {
            MonitorMode::Ssh => Self::Ssh,
            MonitorMode::TcpPort => Self::TcpPort,
        }
    }
}

impl From<MonitorModeDto> for MonitorMode {
    fn from(mode: MonitorModeDto) -> Self {
        match mode {
            MonitorModeDto::Ssh => Self::Ssh,
            MonitorModeDto::TcpPort => Self::TcpPort,
        }
    }
}

/// A host as the frontend sees it — password and private-key material omitted
/// (tech-gui.md §3.4). `hasKey` reports whether an identity file is configured;
/// the key path itself never crosses the boundary.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HostDto {
    pub name: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub source: HostSourceDto,
    pub has_key: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_auth_disabled: Option<bool>,
    pub monitoring: MonitorModeDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_port: Option<u16>,
    pub local_forwards: Vec<LocalForwardDto>,
    pub tunnel_autostart: bool,
    pub forward_agent: bool,
}

/// One `ssh -L` rule (tech-gui.md §4.1): listen on `bindAddress:bindPort` here and
/// reach `remoteHost:remotePort` as the host resolves it. No `bindAddress` means the
/// loopback, as with ssh.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LocalForwardDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_address: Option<String>,
    pub bind_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

/// Where a host's tunnel stands (tech-gui.md §4.1). Internally tagged on `kind`,
/// like `ConnectionStatusDto`.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TunnelStatusDto {
    Connecting,
    Up,
    Retrying { message: String },
    Failed { message: String },
    Stopped,
}

/// Inbound host form payload for `save_host` (tech-gui.md §4.1, Stage 4.1). Always
/// builds a **manual** `Host`: editing an SSH-config import saves a copy that shadows
/// it, and `~/.ssh/config` itself is never written.
/// `password`/`identityFile` arrive here (the create/edit form owns them) but never
/// travel back out: the outbound `HostDto` omits both (§3.4). Inbound only, so it
/// derives `Deserialize` (not `Serialize`).
#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HostInputDto {
    pub name: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    #[serde(default)]
    pub identity_file: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub proxy_jump: Option<String>,
    // `tags[]` is required on the wire (tech-gui.md §4.1); the form always sends an
    // array, so no `serde(default)` — that would emit an optional `tags?` and drift.
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub monitoring: Option<MonitorModeDto>,
    #[serde(default)]
    pub monitor_port: Option<u16>,
    pub local_forwards: Vec<LocalForwardDto>,
    pub tunnel_autostart: bool,
    pub forward_agent: bool,
}

/// What this desktop allows the tray (tech-gui.md §4.2 `set_tray_behavior`): an icon
/// at all, and hiding a minimized window into it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TraySupportDto {
    pub available: bool,
    pub minimize: bool,
}

/// Live connection state for a host (tech-gui.md §4.1). Internally tagged so the
/// frontend consumes a discriminated union keyed on `kind`.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ConnectionStatusDto {
    Unknown,
    Connecting,
    Connected,
    Failed { message: String },
}

/// A single process in the "top processes" panel (tech-gui.md §4.1).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProcessDto {
    pub name: String,
    pub cpu_percent: f64,
    pub mem_percent: f64,
}

/// A metrics snapshot for a host (tech-gui.md §4.1). The core's `Instant` is
/// flattened to `ageSeconds` (seconds since the sample) so it can serialise.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MetricsDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_avg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_info: Option<String>,
    pub top_processes: Vec<ProcessDto>,
    pub age_seconds: u64,
}

/// A service kind detected on a host, mirrors `omnyssh_core::event::ServiceKind`.
/// Wire names are lowercase (`docker`, `nginx`, `postgresql`, `redis`, `nodejs`);
/// if the core adds a kind, extend this enum so it is never silently dropped
/// (tech-gui.md §4.1).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKindDto {
    Docker,
    Nginx,
    PostgreSQL,
    Redis,
    NodeJS,
}

/// One quick-scan metric for a detected service (tech-gui.md §4.1). `MetricValue`
/// is integer-only today; widen this if the core adds a non-integral variant.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ServiceMetricDto {
    pub name: String,
    pub value: i64,
}

/// A service detected on a host with its quick-scan metrics (tech-gui.md §4.1).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDto {
    pub kind: ServiceKindDto,
    pub metrics: Vec<ServiceMetricDto>,
}

/// Snippet scope, mirrors `omnyssh_core::config::snippets::SnippetScope`. Wire
/// names are lowercase (`global`, `host`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum SnippetScopeDto {
    Global,
    Host,
}

/// A saved command snippet as the frontend sees it (tech-gui.md §4.1). Crosses the
/// boundary both ways — outbound for `list_snippets`, inbound for `save_snippet` —
/// so it derives `Deserialize` too. Optional fields are omitted when absent, matching
/// the sparse `snippets.toml` the TUI writes.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SnippetDto {
    pub name: String,
    pub command: String,
    pub scope: SnippetScopeDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<String>>,
}

/// A file or directory in an SFTP panel listing (tech-gui.md §4.1). Maps from the
/// core `FileEntry`; `path` is the absolute path the frontend marks entries by.
/// `modified` is Unix seconds, `null` when unknown.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FileEntryDto {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    pub modified: Option<i64>,
}

/// Live progress for one SFTP upload/download (tech-gui.md §4.1). The GUI allocates
/// `transferId` when it issues the transfer and resolves its owning `sessionId` via
/// `transfer_owner` (§3.4); `done`/`total` are byte counts (`total` is `0` when the
/// remote size could not be determined).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgressDto {
    pub session_id: u64,
    pub transfer_id: u64,
    pub done: u64,
    pub total: u64,
}

/// A newer release the app can offer (tech-gui.md §4.1). `version` is the latest
/// version (no leading `v`); `url` is the release page; `canSelfUpdate` mirrors the
/// core's self-update eligibility. The core `UpdateInfo` has no release-notes field, so
/// none is invented (§4.1).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfoDto {
    pub version: String,
    pub url: String,
    pub tag: String,
    pub can_self_update: bool,
}

/// Update-checker preferences, mirrors core `UpdateConfig` (tech-gui.md §4.3). Crosses
/// both ways: outbound for the settings screen, inbound for `save_update_config`.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConfigDto {
    pub check_on_startup: bool,
    pub skip_version: String,
}

/// One step of the auto key-setup flow, for the progress view (tech-gui.md §4.2/§4.3).
/// `index` is 1-based (`1..=total`); `description` is the core's human-readable label.
/// Maps from the core `KeySetupStep`.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct KeySetupStepDto {
    pub index: u8,
    pub total: u8,
    pub description: String,
}

/// Raw PTY output bytes for a terminal session's per-session `Channel` (tech-gui.md
/// §3.3/§3.6). Deliberately **not** `Serialize`: that dodges the blanket
/// `Serialize -> IpcResponse` mapping (which would JSON-encode to a slow `number[]`),
/// so the bytes ride the channel as a raw `ArrayBuffer` that xterm writes directly.
/// Only ever sent, never received — the sole non-DTO on the boundary.
#[derive(specta::Type)]
#[specta(transparent)]
pub struct TerminalBytes(pub Vec<u8>);

impl tauri::ipc::IpcResponse for TerminalBytes {
    fn body(self) -> tauri::Result<tauri::ipc::InvokeResponseBody> {
        Ok(tauri::ipc::InvokeResponseBody::Raw(self.0))
    }
}

impl From<&HostSource> for HostSourceDto {
    fn from(source: &HostSource) -> Self {
        match source {
            HostSource::SshConfig => Self::SshConfig,
            HostSource::Manual => Self::Manual,
        }
    }
}

impl From<&Host> for HostDto {
    fn from(host: &Host) -> Self {
        Self {
            name: host.name.clone(),
            hostname: host.hostname.clone(),
            user: host.user.clone(),
            port: host.port,
            tags: host.tags.clone(),
            notes: host.notes.clone(),
            source: (&host.source).into(),
            has_key: host.identity_file.is_some(),
            password_auth_disabled: host.password_auth_disabled,
            monitoring: host.monitoring.into(),
            monitor_port: host.monitor_port,
            local_forwards: host.local_forwards.iter().map(Into::into).collect(),
            tunnel_autostart: host.tunnel_autostart,
            forward_agent: host.forward_agent,
        }
    }
}

impl From<&LocalForward> for LocalForwardDto {
    fn from(forward: &LocalForward) -> Self {
        Self {
            bind_address: forward.bind_address.clone(),
            bind_port: forward.bind_port,
            remote_host: forward.remote_host.clone(),
            remote_port: forward.remote_port,
        }
    }
}

impl From<LocalForwardDto> for LocalForward {
    fn from(dto: LocalForwardDto) -> Self {
        Self {
            bind_address: dto.bind_address,
            bind_port: dto.bind_port,
            remote_host: dto.remote_host,
            remote_port: dto.remote_port,
        }
    }
}

/// Rejects a rule `hosts.toml` could not read back. Rules are stored in the `ssh -L`
/// notation, so one that does not survive it — a zero port, an empty host — would be
/// saved, then dropped with a warning on the next load.
pub fn check_forwards(forwards: &[LocalForwardDto]) -> Result<(), String> {
    for dto in forwards {
        let forward = LocalForward::from(dto.clone());
        if forward.to_string().parse::<LocalForward>().as_ref() != Ok(&forward) {
            return Err(format!("'{forward}' is not a valid port forward"));
        }
    }
    Ok(())
}

impl From<&TunnelStatus> for TunnelStatusDto {
    fn from(status: &TunnelStatus) -> Self {
        match status {
            TunnelStatus::Connecting => Self::Connecting,
            TunnelStatus::Up => Self::Up,
            TunnelStatus::Retrying(message) => Self::Retrying {
                message: message.clone(),
            },
            TunnelStatus::Failed(message) => Self::Failed {
                message: message.clone(),
            },
            TunnelStatus::Stopped => Self::Stopped,
        }
    }
}

impl From<HostInputDto> for Host {
    /// Build a **manual** host from the form payload (tech-gui.md §4.1, Stage 4.1).
    /// `source` is forced to `Manual` (the form only ever authors manual entries);
    /// blank optional fields collapse to `None` so an empty identity path never reads
    /// as `hasKey` and an empty password is not persisted. Key-setup metadata
    /// (`password_auth_disabled`, `key_setup_date`) and the SSH-config rename origin
    /// are not the form's to set — `save_host` carries them over across an edit.
    fn from(dto: HostInputDto) -> Self {
        // The frontend already trims; collapse an exact-empty string to `None` as a
        // last guard. Password is not trimmed — its bytes are preserved verbatim.
        let non_empty = |s: Option<String>| s.filter(|v| !v.is_empty());
        let monitoring: MonitorMode = dto.monitoring.map(Into::into).unwrap_or_default();
        Host {
            name: dto.name,
            hostname: dto.hostname,
            user: dto.user,
            port: dto.port,
            identity_file: non_empty(dto.identity_file),
            // Read from `~/.ssh/config` only; `save_host` carries it over.
            identities_only: false,
            password: non_empty(dto.password),
            proxy_jump: non_empty(dto.proxy_jump),
            tags: dto.tags,
            notes: non_empty(dto.notes),
            source: HostSource::Manual,
            original_ssh_host: None,
            monitoring,
            // Port 0 is not dialable, and an SSH host has nothing to probe: drop
            // both, so a later mode switch cannot inherit a stale target.
            monitor_port: dto
                .monitor_port
                .filter(|&p| p != 0 && monitoring == MonitorMode::TcpPort),
            local_forwards: dto.local_forwards.into_iter().map(Into::into).collect(),
            tunnel_autostart: dto.tunnel_autostart,
            forward_agent: dto.forward_agent,
            key_setup_date: None,
            password_auth_disabled: None,
        }
    }
}

impl From<&ConnectionStatus> for ConnectionStatusDto {
    fn from(status: &ConnectionStatus) -> Self {
        match status {
            ConnectionStatus::Unknown => Self::Unknown,
            ConnectionStatus::Connecting => Self::Connecting,
            ConnectionStatus::Connected => Self::Connected,
            ConnectionStatus::Failed(message) => Self::Failed {
                message: message.clone(),
            },
        }
    }
}

impl From<&ProcessInfo> for ProcessDto {
    fn from(process: &ProcessInfo) -> Self {
        Self {
            name: process.name.clone(),
            cpu_percent: process.cpu_percent,
            mem_percent: process.mem_percent,
        }
    }
}

impl From<&Metrics> for MetricsDto {
    fn from(metrics: &Metrics) -> Self {
        Self {
            cpu_percent: metrics.cpu_percent,
            ram_percent: metrics.ram_percent,
            disk_percent: metrics.disk_percent,
            uptime: metrics.uptime.clone(),
            load_avg: metrics.load_avg.clone(),
            os_info: metrics.os_info.clone(),
            top_processes: metrics
                .top_processes
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(ProcessDto::from)
                .collect(),
            age_seconds: metrics.last_updated.elapsed().as_secs(),
        }
    }
}

impl From<&ServiceKind> for ServiceKindDto {
    fn from(kind: &ServiceKind) -> Self {
        match kind {
            ServiceKind::Docker => Self::Docker,
            ServiceKind::Nginx => Self::Nginx,
            ServiceKind::PostgreSQL => Self::PostgreSQL,
            ServiceKind::Redis => Self::Redis,
            ServiceKind::NodeJS => Self::NodeJS,
        }
    }
}

impl From<&ServiceMetric> for ServiceMetricDto {
    fn from(metric: &ServiceMetric) -> Self {
        let MetricValue::Integer(value) = metric.value;
        Self {
            name: metric.name.clone(),
            value,
        }
    }
}

impl From<&DetectedService> for ServiceDto {
    fn from(service: &DetectedService) -> Self {
        Self {
            kind: (&service.kind).into(),
            metrics: service.metrics.iter().map(ServiceMetricDto::from).collect(),
        }
    }
}

impl From<&SnippetScope> for SnippetScopeDto {
    fn from(scope: &SnippetScope) -> Self {
        match scope {
            SnippetScope::Global => Self::Global,
            SnippetScope::Host => Self::Host,
        }
    }
}

impl From<&SnippetScopeDto> for SnippetScope {
    fn from(scope: &SnippetScopeDto) -> Self {
        match scope {
            SnippetScopeDto::Global => Self::Global,
            SnippetScopeDto::Host => Self::Host,
        }
    }
}

impl From<&Snippet> for SnippetDto {
    fn from(snippet: &Snippet) -> Self {
        Self {
            name: snippet.name.clone(),
            command: snippet.command.clone(),
            scope: (&snippet.scope).into(),
            host: snippet.host.clone(),
            tags: snippet.tags.clone(),
            params: snippet.params.clone(),
        }
    }
}

impl From<SnippetDto> for Snippet {
    fn from(dto: SnippetDto) -> Self {
        Self {
            name: dto.name,
            command: dto.command,
            scope: (&dto.scope).into(),
            host: dto.host,
            tags: dto.tags,
            params: dto.params,
        }
    }
}

impl From<&FileEntry> for FileEntryDto {
    fn from(entry: &FileEntry) -> Self {
        Self {
            name: entry.name.clone(),
            path: entry.path.clone(),
            size: entry.size,
            is_dir: entry.is_dir,
            modified: entry.modified,
        }
    }
}

impl From<&UpdateInfo> for UpdateInfoDto {
    fn from(info: &UpdateInfo) -> Self {
        Self {
            version: info.latest.clone(),
            url: info.release_url(),
            tag: info.tag.clone(),
            can_self_update: info.can_self_update,
        }
    }
}

impl From<&UpdateConfig> for UpdateConfigDto {
    fn from(config: &UpdateConfig) -> Self {
        Self {
            check_on_startup: config.check_on_startup,
            skip_version: config.skip_version.clone(),
        }
    }
}

impl From<UpdateConfigDto> for UpdateConfig {
    fn from(dto: UpdateConfigDto) -> Self {
        Self {
            check_on_startup: dto.check_on_startup,
            skip_version: dto.skip_version,
        }
    }
}

impl From<KeySetupStep> for KeySetupStepDto {
    fn from(step: KeySetupStep) -> Self {
        Self {
            // The core enum is a plain C-like discriminant (`GenerateKey = 1 ..=
            // FinalCheck = 6`), so the cast is the 1-based position directly.
            index: step as u8,
            total: KeySetupStep::all_steps().len() as u8,
            description: step.description().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_with_secret() -> Host {
        Host {
            name: "web-prod-1".to_string(),
            hostname: "10.0.0.1".to_string(),
            user: "deploy".to_string(),
            port: 2222,
            identity_file: Some("/home/me/.ssh/id_ed25519".to_string()),
            password: Some("s3cr3t-p4ss".to_string()),
            tags: vec!["prod".to_string()],
            notes: Some("primary".to_string()),
            source: HostSource::Manual,
            password_auth_disabled: Some(true),
            ..Host::default()
        }
    }

    #[test]
    fn host_dto_never_serialises_a_password_or_key() {
        let dto = HostDto::from(&host_with_secret());
        let json = serde_json::to_string(&dto).expect("serialise HostDto");
        // The wire form must carry neither the secret field nor its value.
        // (`passwordAuthDisabled` is a public boolean flag, not the password.)
        assert!(
            !json.contains(r#""password""#),
            "password field leaked: {json}"
        );
        assert!(!json.contains("s3cr3t"), "password value leaked: {json}");
        assert!(!json.contains("identityFile"), "key field leaked: {json}");
        assert!(!json.contains("id_ed25519"), "key path leaked: {json}");
    }

    #[test]
    fn host_dto_maps_public_fields() {
        let dto = HostDto::from(&host_with_secret());
        assert_eq!(dto.name, "web-prod-1");
        assert_eq!(dto.hostname, "10.0.0.1");
        assert_eq!(dto.user, "deploy");
        assert_eq!(dto.port, 2222);
        assert_eq!(dto.tags, vec!["prod".to_string()]);
        assert_eq!(dto.notes.as_deref(), Some("primary"));
        assert!(matches!(dto.source, HostSourceDto::Manual));
        // `hasKey` is derived from the identity file, which itself stays backend-side.
        assert!(dto.has_key);
        assert_eq!(dto.password_auth_disabled, Some(true));
    }

    #[test]
    fn host_dto_has_key_is_false_without_identity_file() {
        let host = Host {
            identity_file: None,
            ..Host::default()
        };
        assert!(!HostDto::from(&host).has_key);
    }

    fn full_input() -> HostInputDto {
        HostInputDto {
            name: "web-prod-1".to_string(),
            hostname: "10.0.0.1".to_string(),
            user: "deploy".to_string(),
            port: 2222,
            identity_file: Some("/home/me/.ssh/id_ed25519".to_string()),
            password: Some("s3cr3t-p4ss".to_string()),
            proxy_jump: Some("bastion".to_string()),
            tags: vec!["prod".to_string()],
            notes: Some("primary".to_string()),
            monitoring: None,
            monitor_port: None,
            local_forwards: vec![],
            tunnel_autostart: false,
            forward_agent: false,
        }
    }

    #[test]
    fn host_input_maps_to_a_manual_host() {
        // The form only ever authors manual entries — editing an import produces a
        // manual copy (tech-gui.md §4.1) — so `source` is forced regardless of input.
        let host = Host::from(full_input());
        assert_eq!(host.name, "web-prod-1");
        assert_eq!(host.hostname, "10.0.0.1");
        assert_eq!(host.user, "deploy");
        assert_eq!(host.port, 2222);
        assert_eq!(
            host.identity_file.as_deref(),
            Some("/home/me/.ssh/id_ed25519")
        );
        assert_eq!(host.proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(host.tags, vec!["prod".to_string()]);
        assert_eq!(host.notes.as_deref(), Some("primary"));
        assert_eq!(host.source, HostSource::Manual);
        // Key-setup metadata + rename origin are never the form's to set.
        assert!(host.original_ssh_host.is_none());
        assert!(host.key_setup_date.is_none());
        assert!(host.password_auth_disabled.is_none());
    }

    #[test]
    fn host_input_keeps_the_password_backend_side() {
        // The password rides inbound into the backend `Host`, then the outbound
        // `HostDto` must drop it: it never reaches the webview (§3.4).
        let host = Host::from(full_input());
        assert_eq!(host.password.as_deref(), Some("s3cr3t-p4ss"));
        let json = serde_json::to_string(&HostDto::from(&host)).expect("serialise HostDto");
        assert!(!json.contains("password"), "password leaked: {json}");
        assert!(!json.contains("s3cr3t"), "password value leaked: {json}");
    }

    #[test]
    fn host_input_collapses_blank_optionals_to_none() {
        // An empty identity path must not read as `hasKey`; an empty password/proxy/
        // notes must not persist an empty string.
        let host = Host::from(HostInputDto {
            name: "h".to_string(),
            hostname: "example.com".to_string(),
            user: "root".to_string(),
            port: 22,
            identity_file: Some(String::new()),
            password: Some(String::new()),
            proxy_jump: Some(String::new()),
            tags: vec![],
            notes: Some(String::new()),
            monitoring: None,
            monitor_port: None,
            local_forwards: vec![],
            tunnel_autostart: false,
            forward_agent: false,
        });
        assert!(host.identity_file.is_none());
        assert!(host.password.is_none());
        assert!(host.proxy_jump.is_none());
        assert!(host.notes.is_none());
        assert!(!HostDto::from(&host).has_key);
    }

    #[test]
    fn connection_status_dto_maps_every_variant() {
        assert!(matches!(
            ConnectionStatusDto::from(&ConnectionStatus::Unknown),
            ConnectionStatusDto::Unknown
        ));
        assert!(matches!(
            ConnectionStatusDto::from(&ConnectionStatus::Connecting),
            ConnectionStatusDto::Connecting
        ));
        assert!(matches!(
            ConnectionStatusDto::from(&ConnectionStatus::Connected),
            ConnectionStatusDto::Connected
        ));
        let failed = ConnectionStatusDto::from(&ConnectionStatus::Failed("boom".to_string()));
        match failed {
            ConnectionStatusDto::Failed { message } => assert_eq!(message, "boom"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn connection_status_dto_tags_on_kind() {
        let json = serde_json::to_string(&ConnectionStatusDto::from(&ConnectionStatus::Failed(
            "down".to_string(),
        )))
        .expect("serialise status");
        assert_eq!(json, r#"{"kind":"failed","message":"down"}"#);
        let connected = serde_json::to_string(&ConnectionStatusDto::Connected).unwrap();
        assert_eq!(connected, r#"{"kind":"connected"}"#);
    }

    #[test]
    fn metrics_dto_passes_none_through() {
        let dto = MetricsDto::from(&Metrics::default());
        assert!(dto.cpu_percent.is_none());
        assert!(dto.ram_percent.is_none());
        assert!(dto.disk_percent.is_none());
        assert!(dto.uptime.is_none());
        assert!(dto.load_avg.is_none());
        assert!(dto.os_info.is_none());
        assert!(dto.top_processes.is_empty());
        // A freshly stamped sample is age zero.
        assert_eq!(dto.age_seconds, 0);
    }

    #[test]
    fn metrics_dto_maps_populated_fields() {
        let metrics = Metrics {
            cpu_percent: Some(42.5),
            ram_percent: Some(70.0),
            disk_percent: Some(12.0),
            uptime: Some("3 days".to_string()),
            load_avg: Some("0.5 0.4 0.3".to_string()),
            os_info: Some("Ubuntu 22.04".to_string()),
            top_processes: Some(vec![ProcessInfo {
                name: "postgres".to_string(),
                cpu_percent: 30.0,
                mem_percent: 15.0,
            }]),
            ..Metrics::default()
        };
        let dto = MetricsDto::from(&metrics);
        assert_eq!(dto.cpu_percent, Some(42.5));
        assert_eq!(dto.ram_percent, Some(70.0));
        assert_eq!(dto.disk_percent, Some(12.0));
        assert_eq!(dto.uptime.as_deref(), Some("3 days"));
        assert_eq!(dto.os_info.as_deref(), Some("Ubuntu 22.04"));
        assert_eq!(dto.top_processes.len(), 1);
        assert_eq!(dto.top_processes[0].name, "postgres");
        assert_eq!(dto.top_processes[0].cpu_percent, 30.0);
        assert_eq!(dto.top_processes[0].mem_percent, 15.0);
    }

    fn metric(name: &str, value: i64) -> ServiceMetric {
        ServiceMetric {
            name: name.to_string(),
            value: MetricValue::Integer(value),
        }
    }

    #[test]
    fn service_kind_dto_uses_lowercase_wire_names() {
        // The frontend switches on these exact strings (tech-gui.md §4.1).
        let names = [
            (ServiceKind::Docker, r#""docker""#),
            (ServiceKind::Nginx, r#""nginx""#),
            (ServiceKind::PostgreSQL, r#""postgresql""#),
            (ServiceKind::Redis, r#""redis""#),
            (ServiceKind::NodeJS, r#""nodejs""#),
        ];
        for (kind, wire) in names {
            let json = serde_json::to_string(&ServiceKindDto::from(&kind)).expect("serialise kind");
            assert_eq!(json, wire, "kind {kind:?} must map to {wire}");
        }
    }

    #[test]
    fn service_dto_maps_kind_and_integer_metrics() {
        let service = DetectedService {
            kind: ServiceKind::Docker,
            metrics: vec![
                metric("containers_running", 4),
                metric("containers_stopped", 1),
            ],
        };
        let dto = ServiceDto::from(&service);
        assert!(matches!(dto.kind, ServiceKindDto::Docker));
        assert_eq!(dto.metrics.len(), 2);
        assert_eq!(dto.metrics[0].name, "containers_running");
        assert_eq!(dto.metrics[0].value, 4);
        assert_eq!(dto.metrics[1].name, "containers_stopped");
        assert_eq!(dto.metrics[1].value, 1);
    }

    #[test]
    fn service_dto_keeps_an_empty_metric_list() {
        let dto = ServiceDto::from(&DetectedService {
            kind: ServiceKind::Nginx,
            metrics: vec![],
        });
        assert!(matches!(dto.kind, ServiceKindDto::Nginx));
        assert!(dto.metrics.is_empty());
    }

    fn full_snippet() -> Snippet {
        Snippet {
            name: "restart-svc".to_string(),
            command: "systemctl restart {{service}}".to_string(),
            scope: SnippetScope::Host,
            host: Some("web-1".to_string()),
            tags: Some(vec!["ops".to_string()]),
            params: Some(vec!["service".to_string()]),
        }
    }

    #[test]
    fn snippet_dto_maps_every_field() {
        let dto = SnippetDto::from(&full_snippet());
        assert_eq!(dto.name, "restart-svc");
        assert_eq!(dto.command, "systemctl restart {{service}}");
        assert_eq!(dto.scope, SnippetScopeDto::Host);
        assert_eq!(dto.host.as_deref(), Some("web-1"));
        assert_eq!(dto.tags, Some(vec!["ops".to_string()]));
        assert_eq!(dto.params, Some(vec!["service".to_string()]));
    }

    #[test]
    fn snippet_scope_dto_uses_lowercase_wire_names() {
        // The frontend switches on these exact strings (tech-gui.md §4.1).
        let global = serde_json::to_string(&SnippetScopeDto::Global).unwrap();
        assert_eq!(global, r#""global""#);
        let host = serde_json::to_string(&SnippetScopeDto::Host).unwrap();
        assert_eq!(host, r#""host""#);
    }

    #[test]
    fn snippet_dto_omits_absent_optionals_on_the_wire() {
        let dto = SnippetDto::from(&Snippet {
            name: "ls".to_string(),
            command: "ls -la".to_string(),
            scope: SnippetScope::Global,
            host: None,
            tags: None,
            params: None,
        });
        let json = serde_json::to_string(&dto).unwrap();
        assert_eq!(json, r#"{"name":"ls","command":"ls -la","scope":"global"}"#);
    }

    #[test]
    fn snippet_dto_round_trips_through_snippet() {
        let original = full_snippet();
        let back: Snippet = SnippetDto::from(&original).into();
        assert_eq!(back.name, original.name);
        assert_eq!(back.command, original.command);
        assert_eq!(back.scope, original.scope);
        assert_eq!(back.host, original.host);
        assert_eq!(back.tags, original.tags);
        assert_eq!(back.params, original.params);
    }

    #[test]
    fn snippet_dto_deserialises_a_sparse_inbound_payload() {
        // A minimal save_snippet payload: optionals absent -> None (tech-gui.md §4.1).
        let dto: SnippetDto =
            serde_json::from_str(r#"{"name":"pwd","command":"pwd","scope":"global"}"#).unwrap();
        assert_eq!(dto.scope, SnippetScopeDto::Global);
        assert!(dto.host.is_none());
        assert!(dto.tags.is_none());
        assert!(dto.params.is_none());
    }

    #[test]
    fn file_entry_dto_maps_a_file_and_a_directory() {
        let file = FileEntry {
            name: "config.toml".to_string(),
            path: "/etc/omnyssh/config.toml".to_string(),
            size: 4096,
            is_dir: false,
            modified: Some(1_700_000_000),
        };
        let dto = FileEntryDto::from(&file);
        assert_eq!(dto.name, "config.toml");
        assert_eq!(dto.path, "/etc/omnyssh/config.toml");
        assert_eq!(dto.size, 4096);
        assert!(!dto.is_dir);
        assert_eq!(dto.modified, Some(1_700_000_000));

        let dir = FileEntry {
            name: "..".to_string(),
            path: "/etc".to_string(),
            size: 0,
            is_dir: true,
            modified: None,
        };
        let dto = FileEntryDto::from(&dir);
        assert!(dto.is_dir);
        assert_eq!(dto.size, 0);
        assert!(dto.modified.is_none());
    }

    #[test]
    fn file_entry_dto_uses_camel_case_is_dir_on_the_wire() {
        // The frontend reads `isDir` (tech-gui.md §4.1); a snake-case leak would
        // silently render every entry as a file.
        let json = serde_json::to_string(&FileEntryDto::from(&FileEntry {
            name: "srv".to_string(),
            path: "/srv".to_string(),
            size: 0,
            is_dir: true,
            modified: Some(1_700_000_000),
        }))
        .expect("serialise FileEntryDto");
        assert_eq!(
            json,
            r#"{"name":"srv","path":"/srv","size":0,"isDir":true,"modified":1700000000}"#
        );
    }

    #[test]
    fn update_info_dto_maps_from_core_and_omits_notes() {
        use omnyssh_core::update::InstallMethod;
        let info = UpdateInfo {
            current: "1.0.0".to_string(),
            latest: "1.2.0".to_string(),
            tag: "v1.2.0".to_string(),
            method: InstallMethod::Manual,
            can_self_update: true,
        };
        let dto = UpdateInfoDto::from(&info);
        // `version` is the latest release, `url` the core's release page (tech-gui.md §4.1).
        assert_eq!(dto.version, "1.2.0");
        assert_eq!(dto.tag, "v1.2.0");
        assert_eq!(dto.url, info.release_url());
        assert!(dto.can_self_update);
        // No release-notes field is invented.
        let json = serde_json::to_string(&dto).expect("serialise UpdateInfoDto");
        assert!(!json.contains("notes"), "invented a notes field: {json}");
        assert!(
            json.contains(r#""canSelfUpdate":true"#),
            "wire name drift: {json}"
        );
    }

    #[test]
    fn update_config_dto_round_trips_through_core() {
        // The settings screen edits these and `save_update_config` persists them; the
        // round-trip must be lossless (tech-gui.md §4.3 test obligation).
        let core = UpdateConfig {
            check_on_startup: false,
            skip_version: "1.2.3".to_string(),
        };
        let dto = UpdateConfigDto::from(&core);
        assert!(!dto.check_on_startup);
        assert_eq!(dto.skip_version, "1.2.3");
        let json = serde_json::to_string(&dto).expect("serialise UpdateConfigDto");
        assert_eq!(json, r#"{"checkOnStartup":false,"skipVersion":"1.2.3"}"#);

        let back: UpdateConfig = dto.into();
        assert!(!back.check_on_startup);
        assert_eq!(back.skip_version, "1.2.3");
    }

    #[test]
    fn key_setup_step_dto_maps_index_total_and_label() {
        // The progress view reads a 1-based `index` out of `total` plus the core's
        // label (tech-gui.md §4.2). The first/last steps pin the discriminant range.
        let dto = KeySetupStepDto::from(KeySetupStep::VerifyKeyAuth);
        assert_eq!(dto.index, 3);
        assert_eq!(dto.total, 6);
        assert_eq!(dto.description, "Verifying key authentication");
        assert_eq!(KeySetupStepDto::from(KeySetupStep::GenerateKey).index, 1);
        assert_eq!(KeySetupStepDto::from(KeySetupStep::FinalCheck).index, 6);
    }

    #[test]
    fn key_setup_step_dto_uses_camel_case_wire_names() {
        // The frontend reads `index`/`total`/`description` (tech-gui.md §4.2).
        let json = serde_json::to_string(&KeySetupStepDto::from(KeySetupStep::CopyPublicKey))
            .expect("serialise KeySetupStepDto");
        assert_eq!(
            json,
            r#"{"index":2,"total":6,"description":"Copying public key to server"}"#
        );
    }

    #[test]
    fn transfer_progress_dto_carries_session_transfer_and_byte_counts() {
        let json = serde_json::to_string(&TransferProgressDto {
            session_id: 3,
            transfer_id: 7,
            done: 512,
            total: 2048,
        })
        .expect("serialise TransferProgressDto");
        assert_eq!(
            json,
            r#"{"sessionId":3,"transferId":7,"done":512,"total":2048}"#
        );
    }

    fn rule(bind: Option<&str>, port: u16, host: &str, hostport: u16) -> LocalForwardDto {
        LocalForwardDto {
            bind_address: bind.map(str::to_string),
            bind_port: port,
            remote_host: host.to_string(),
            remote_port: hostport,
        }
    }

    #[test]
    fn host_dto_carries_forwards_and_autostart_but_still_no_secret() {
        let mut host = host_with_secret();
        host.local_forwards = vec![
            "9443:127.0.0.1:9443".parse().expect("rule"),
            "[::1]:8080:db:5432".parse().expect("rule"),
        ];
        host.tunnel_autostart = true;
        let json = serde_json::to_value(HostDto::from(&host)).expect("serialise HostDto");
        assert_eq!(
            json["localForwards"],
            serde_json::json!([
                {"bindPort": 9443, "remoteHost": "127.0.0.1", "remotePort": 9443},
                {"bindAddress": "::1", "bindPort": 8080, "remoteHost": "db", "remotePort": 5432},
            ])
        );
        assert_eq!(json["tunnelAutostart"], true);
        let text = json.to_string();
        assert!(
            !text.contains("s3cr3t") && !text.contains("id_ed25519"),
            "{text}"
        );
    }

    #[test]
    fn host_input_forwards_reach_the_saved_host() {
        let mut input = full_input();
        input.local_forwards = vec![rule(None, 5432, "localhost", 5432)];
        input.tunnel_autostart = true;
        let host = Host::from(input);
        assert_eq!(host.local_forwards[0].to_string(), "5432:localhost:5432");
        assert!(host.tunnel_autostart);
    }

    #[test]
    fn forward_agent_crosses_both_ways() {
        let mut input = full_input();
        input.forward_agent = true;
        let host = Host::from(input);
        assert!(host.forward_agent);
        let json = serde_json::to_value(HostDto::from(&host)).expect("serialise HostDto");
        assert_eq!(json["forwardAgent"], true);
    }

    #[test]
    fn check_forwards_keeps_only_rules_hosts_toml_reads_back() {
        assert!(check_forwards(&[
            rule(None, 9443, "127.0.0.1", 9443),
            rule(Some("::1"), 8080, "fe80::1", 80),
            rule(Some("*"), 8080, "web", 80),
            rule(Some("0.0.0.0"), 8080, "web", 80),
        ])
        .is_ok());
        for bad in [
            rule(None, 0, "web", 80),
            rule(None, 8080, "web", 0),
            rule(None, 8080, "", 80),
            rule(None, 8080, "[web", 80),
        ] {
            assert!(
                check_forwards(std::slice::from_ref(&bad)).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn tunnel_status_is_tagged_on_kind() {
        let wire = |status: TunnelStatus| {
            serde_json::to_value(TunnelStatusDto::from(&status)).expect("serialise")
        };
        assert_eq!(wire(TunnelStatus::Up), serde_json::json!({"kind": "up"}));
        assert_eq!(
            wire(TunnelStatus::Connecting),
            serde_json::json!({"kind": "connecting"})
        );
        assert_eq!(
            wire(TunnelStatus::Stopped),
            serde_json::json!({"kind": "stopped"})
        );
        assert_eq!(
            wire(TunnelStatus::Retrying(String::from("connection lost"))),
            serde_json::json!({"kind": "retrying", "message": "connection lost"})
        );
        assert_eq!(
            wire(TunnelStatus::Failed(String::from("port 80 in use"))),
            serde_json::json!({"kind": "failed", "message": "port 80 in use"})
        );
    }
}
