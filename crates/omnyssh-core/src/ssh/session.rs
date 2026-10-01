//! Async SSH session management via russh.
//!
//! Provides [`SshSession`] — a thin wrapper around a russh client handle that
//! supports connecting, executing commands, and graceful disconnect.
//! Authentication order: SSH agent → identity file → default keys → password
//! (by the password method or keyboard-interactive) → asking the user, when the
//! caller lets it.
//!
//! Hosts with a `ProxyJump` are reached through their bastions: each hop is
//! connected and authenticated in turn, and the next hop rides a
//! `direct-tcpip` channel opened on the previous one (the `ssh -J` model).
//!
//! Connection and command timeouts are enforced:
//! - Connect timeout: 10 seconds (per hop)
//! - Command timeout: 30 seconds

use std::borrow::Cow;
#[cfg(unix)]
use std::collections::HashSet;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::sync::{MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use anyhow::{anyhow, Context};
use russh::client::{self, Handle};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{cipher, compression, kex, mac, ChannelMsg};
use tokio::sync::watch;
use tokio::time;

use crate::ssh::client::Host;
use crate::ssh::identity::{self, IdentityError};
use crate::ssh::known_hosts::{self, Verdict};
use crate::ssh::password::{self, AskPassword, Method, NoAnswer, Prompt};

// ---------------------------------------------------------------------------
// russh Handler implementation
// ---------------------------------------------------------------------------

/// Shared russh client handler used by every native SSH path (metrics, SFTP,
/// terminal).
///
/// Verifies the server's host key against `~/.ssh/known_hosts` (see
/// [`known_hosts`]). Unknown hosts are recorded on first connection (trust on
/// first use); changed keys are rejected.
pub(crate) struct KnownHostsHandler {
    /// Hostname used for known_hosts lookup.
    host: String,
    /// Port used for known_hosts lookup.
    port: u16,
    /// Set when the server ends the session with a DISCONNECT of its own, as
    /// OpenSSH does after too many failed logins. A link that just dies leaves
    /// it unset.
    hung_up: Arc<AtomicBool>,
    /// Set when the server leaves no login method OmnySSH has: russh's
    /// NoAuthMethod on disconnect, or a refused key that names only others.
    no_method: Arc<AtomicBool>,
    /// What the server's version string says about RSA key signatures.
    rsa_sigs: Arc<Mutex<RsaSigs>>,
    /// The fingerprint of a host key first seen, and recorded, on this connection.
    new_key: Arc<Mutex<Option<String>>>,
    /// Why the host key was turned down, for the user; russh itself only says
    /// "Unknown server key".
    refusal: Arc<Mutex<Option<String>>>,
    /// Whether this connection lends the local agent (`ssh -A`). Only a
    /// terminal's target does; any other gets its agent channels closed.
    lends_agent: bool,
    /// Dropped with the handler when the session ends, which wakes [`Link::ended`]
    /// and any agent channel still being carried. Only unix lends the agent, so
    /// elsewhere it is only ever dropped.
    #[cfg_attr(not(unix), allow(dead_code))]
    ended: watch::Sender<()>,
}

/// What a connection's [`KnownHostsHandler`] reports while it runs.
struct Link {
    hung_up: Arc<AtomicBool>,
    no_method: Arc<AtomicBool>,
    rsa_sigs: Arc<Mutex<RsaSigs>>,
    new_key: Arc<Mutex<Option<String>>>,
    refusal: Arc<Mutex<Option<String>>>,
    /// Changes (to closed) once the session is over.
    ended: watch::Receiver<()>,
}

impl Link {
    fn new_key(&self) -> Option<String> {
        self.new_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The error for a handshake that failed: a turned-down host key says why,
    /// and so does a server with no algorithm in common.
    fn connect_error(&self, e: russh::Error) -> anyhow::Error {
        let refusal = self
            .refusal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match (e, refusal) {
            (russh::Error::UnknownKey, Some(why)) => Refused(why).into(),
            (russh::Error::NoCommonAlgo { kind, theirs, .. }, _) => {
                anyhow!(no_common_algorithm(&kind, &theirs))
            }
            #[cfg(target_os = "macos")]
            (russh::Error::IO(e), _) if e.kind() == std::io::ErrorKind::HostUnreachable => {
                anyhow!("SSH connection failed: {}", dial_error(&e))
            }
            (e, _) => anyhow::Error::new(e).context("SSH connection failed"),
        }
    }
}

/// Why a TCP dial failed. On macOS "No route to host" also comes, at once, from
/// Local Network privacy blocking the app, so it says that may be it; the TUI
/// needs the grant for the terminal it runs in. Short and with no ':', since the
/// TUI shows one cut line.
pub(crate) fn dial_error(e: &std::io::Error) -> String {
    if cfg!(target_os = "macos") && e.kind() == std::io::ErrorKind::HostUnreachable {
        format!(
            "{e}; Local Network privacy may be blocking this app or its terminal \
             (System Settings > Privacy & Security > Local Network)"
        )
    } else {
        e.to_string()
    }
}

/// How much of a server's algorithm list an error quotes.
const OFFER_MAX: usize = 256;

/// Names the kind of algorithm a server has none in common of, and what it
/// offers instead. No ':' after the first, where frontends cut. The list comes
/// from the server, so only the characters algorithm names use are kept, and
/// only so much of it.
fn no_common_algorithm(kind: &russh::AlgorithmKind, theirs: &[String]) -> String {
    let what = match kind {
        russh::AlgorithmKind::Kex => "key exchange method",
        russh::AlgorithmKind::Key => "host key type",
        russh::AlgorithmKind::Cipher => "cipher",
        russh::AlgorithmKind::Mac => "MAC",
        russh::AlgorithmKind::Compression => "compression",
    };
    let mut offer = String::new();
    // The ext-info and strict KEX markers name no method.
    for name in theirs
        .iter()
        .filter(|name| !name.starts_with("ext-info-") && !name.starts_with("kex-strict-"))
    {
        let name: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || "@.-_+".contains(*c))
            .take(OFFER_MAX)
            .collect();
        if name.is_empty() {
            continue;
        }
        if !offer.is_empty() && offer.len() + name.len() > OFFER_MAX {
            offer.push_str(", ...");
            break;
        }
        if !offer.is_empty() {
            offer.push_str(", ");
        }
        offer.push_str(&name);
    }
    if offer.is_empty() {
        format!("SSH connection failed: no common {what}")
    } else {
        format!("SSH connection failed: no common {what}; the server offers {offer}")
    }
}

impl client::Handler for KnownHostsHandler {
    type Error = russh::Error;

    async fn kex_done(
        &mut self,
        _: Option<&[u8]>,
        _: &russh::Names,
        session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        *self
            .rsa_sigs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = rsa_sigs(session.remote_sshid());
        Ok(())
    }

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // No certificate type is asked for, so a server cannot present one.
        let PublicKeyOrCertificate::PublicKey {
            key: server_public_key,
            ..
        } = server_public_key
        else {
            return Ok(false);
        };
        let fingerprint = fingerprint(server_public_key);
        let refusal = match known_hosts::check(&self.host, self.port, server_public_key) {
            Verdict::Known => return Ok(true),
            // Host not seen before — record the key (trust on first use) so a
            // later key change is detected, then accept. Recording is
            // best-effort: a connection must not fail just because
            // known_hosts is unwritable.
            Verdict::Unknown => {
                tracing::warn!(
                    host = %self.host,
                    port = self.port,
                    "Accepting unknown host key for {} (Trust On First Use)", self.host
                );
                match known_hosts::learn(&self.host, self.port, server_public_key) {
                    Ok(()) => tracing::info!(
                        host = %self.host,
                        port = self.port,
                        "recorded new host key in known_hosts"
                    ),
                    Err(e) => tracing::warn!(
                        host = %self.host,
                        error = %e,
                        "could not record host key in known_hosts"
                    ),
                }
                *self
                    .new_key
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(fingerprint);
                return Ok(true);
            }
            // A previously recorded key changed — refuse; possible MITM.
            Verdict::Changed {
                file,
                pinned,
                legacy,
            } => {
                tracing::warn!(
                    host = %self.host,
                    port = self.port,
                    "server key mismatch in known_hosts — possible MITM attack, refusing connection"
                );
                known_hosts::changed_message(
                    &self.host,
                    self.port,
                    server_public_key,
                    &file,
                    &pinned,
                    legacy,
                )
            }
            // Unreadable or corrupt known_hosts — fail closed rather than
            // accept an unverified key.
            Verdict::Unreadable(file, e) => {
                tracing::warn!(
                    host = %self.host,
                    error = %e,
                    "known_hosts check failed; refusing connection"
                );
                known_hosts::unreadable_message(&self.host, self.port, &file, &e)
            }
        };
        *self
            .refusal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(refusal);
        Ok(false)
    }

    async fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        match reason {
            client::DisconnectReason::ReceivedDisconnect(_) => {
                self.hung_up.store(true, Ordering::SeqCst);
                Ok(())
            }
            client::DisconnectReason::Error(e) => {
                if matches!(e, russh::Error::NoAuthMethod) {
                    self.no_method.store(true, Ordering::SeqCst);
                }
                Err(e)
            }
        }
    }

    // Never an Err: that would end the whole connection, terminal and all.
    #[cfg_attr(not(unix), allow(unused_variables))]
    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<client::Msg>,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        if self.lends_agent {
            reply.accept().await;
            // The proxy needs the session loop this callback is holding up.
            #[cfg(unix)]
            tokio::spawn(lend_agent(channel, self.ended.subscribe()));
        } else {
            // ssh(1) refuses these too: a server that asks for an agent nobody
            // offered may be after the keys in it.
            tracing::warn!(host = %self.host, "server opened an agent channel that was not offered; refused it");
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
        }
        Ok(())
    }
}

/// A key's SHA-256 fingerprint, as ssh(1) prints it.
fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// Carries one forwarded agent channel to the local agent, as `ssh -A` does, for
/// no longer than the session lasts: an agent that never answers would otherwise
/// hold the task and its socket until it quits.
///
/// Ends with an EOF, never a close: the server closes once it has seen it.
#[cfg(unix)]
async fn lend_agent(mut channel: russh::Channel<client::Msg>, mut ended: watch::Receiver<()>) {
    let agent = match std::env::var_os("SSH_AUTH_SOCK") {
        Some(path) => tokio::net::UnixStream::connect(path).await,
        None => Err(std::io::ErrorKind::NotFound.into()),
    };
    let carried = match agent {
        Ok(mut agent) => {
            let writer = channel.make_writer();
            let mut remote = tokio::io::join(channel.make_reader(), writer);
            tokio::select! {
                carried = tokio::io::copy_bidirectional(&mut remote, &mut agent) => {
                    carried.map(drop)
                }
                // The session is gone, and the channel with it.
                _ = ended.changed() => return,
            }
        }
        Err(e) => Err(e),
    };
    // A clean copy has already sent its EOF.
    if let Err(e) = carried {
        tracing::debug!(error = %e, "could not lend the SSH agent");
        let _ = channel.eof().await;
    }
}

// ---------------------------------------------------------------------------
// Refused
// ---------------------------------------------------------------------------

/// A connection the server turned away on purpose: it refused every credential,
/// or its host key no longer matches `known_hosts`. A type of its own so a caller
/// that reconnects by itself can stop instead of piling up failed logins.
#[derive(Debug)]
pub(crate) struct Refused(String);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// Whether `e` is, or wraps, a refused connection.
pub(crate) fn is_refused(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        cause.is::<Refused>()
            || matches!(
                cause.downcast_ref::<russh::Error>(),
                Some(russh::Error::UnknownKey)
            )
    })
}

/// Prefixes a hop's error with where it failed. The message is flattened, as
/// before, but a refusal or a locked key stays recognisable through the jump
/// chain.
fn at_hop(e: anyhow::Error, context: String) -> anyhow::Error {
    let message = format!("{context}: {e:#}");
    if let Some(locked) = e
        .chain()
        .find_map(|c| c.downcast_ref::<PassphraseRequired>())
    {
        PassphraseRequired {
            path: locked.path.clone(),
            login: locked.login.clone(),
            message,
        }
        .into()
    } else if let Some(login) = no_password(&e) {
        NoPassword {
            login: login.to_owned(),
            message,
        }
        .into()
    } else if is_refused(&e) {
        Refused(message).into()
    } else {
        anyhow!(message)
    }
}

// ---------------------------------------------------------------------------
// PassphraseRequired
// ---------------------------------------------------------------------------

/// No credential got in, and an encrypted key was skipped for want of its
/// passphrase. Unlike [`Refused`] it is not final: once the key is unlocked
/// ([`crate::ssh::identity::unlock`]) the same login can succeed.
#[derive(Debug)]
pub(crate) struct PassphraseRequired {
    /// Canonical path of the encrypted key.
    path: String,
    /// The login key; a password typed for it helps as much as the passphrase.
    login: String,
    message: String,
}

impl PassphraseRequired {
    fn new(path: String, login: &str) -> Self {
        // The path goes last: frontends cut messages at the first ':', and a
        // Windows path has one.
        let message = format!("SSH key requires a passphrase: {path}");
        Self {
            path,
            login: login.to_string(),
            message,
        }
    }
}

impl fmt::Display for PassphraseRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PassphraseRequired {}

/// The encrypted key a failed connection is waiting on, if that is why it failed.
pub fn passphrase_required(e: &anyhow::Error) -> Option<&str> {
    e.chain()
        .find_map(|cause| cause.downcast_ref::<PassphraseRequired>())
        .map(|locked| locked.path.as_str())
}

// ---------------------------------------------------------------------------
// NoPassword
// ---------------------------------------------------------------------------

/// No key got in and there was no password to try. Not final either: once one
/// is typed for the login elsewhere ([`password::remembered`]) the same
/// connection can go again.
#[derive(Debug)]
pub(crate) struct NoPassword {
    /// The login key the password is remembered under.
    login: String,
    message: String,
}

impl NoPassword {
    fn new(login: &str, host_name: &str) -> Self {
        // No ':' — frontends cut messages there, and the hint must survive.
        let message = format!(
            "SSH authentication failed for {host_name} (no key was accepted and no password is saved; open a terminal to enter it)"
        );
        Self {
            login: login.to_string(),
            message,
        }
    }
}

impl fmt::Display for NoPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for NoPassword {}

/// The login a failed connection needs a password for, if that is why it failed.
pub(crate) fn no_password(e: &anyhow::Error) -> Option<&str> {
    e.chain()
        .find_map(|cause| cause.downcast_ref::<NoPassword>())
        .map(|missing| missing.login.as_str())
}

/// The login key a connection that stopped for a missing credential waits on:
/// a password typed for it elsewhere lets it go again.
pub(crate) fn waiting_login(e: &anyhow::Error) -> Option<&str> {
    e.chain().find_map(|cause| {
        cause
            .downcast_ref::<PassphraseRequired>()
            .map(|locked| locked.login.as_str())
            .or_else(|| cause.downcast_ref::<NoPassword>().map(|m| m.login.as_str()))
    })
}

// ---------------------------------------------------------------------------
// Passwords
// ---------------------------------------------------------------------------

/// Which login passwords a connection may use.
pub(crate) enum Passwords<'a> {
    /// The host's saved password and one typed for the login this session.
    /// Background work: pollers, tunnels that retry, snippets.
    Remembered,
    /// Keys only. Key setup checks a new key this way; any password would let
    /// a broken key pass.
    KeysOnly,
    /// As [`Passwords::Remembered`], then ask the user. A connection the user
    /// started and is watching.
    Ask(&'a mut dyn AskPassword),
}

/// How many passwords the user may type for one login before it fails.
const PASSWORD_PROMPTS: usize = 3;

/// The key a typed password is remembered under: the login plus every bastion
/// on the way, so a private address behind another bastion never gets it.
fn login_key(host: &Host, via: &[Host]) -> String {
    let login = |h: &Host| format!("{}@{}:{}", h.user, h.hostname, h.port);
    std::iter::once(login(host))
        .chain(via.iter().rev().map(login))
        .collect::<Vec<_>>()
        .join(" via ")
}

// ---------------------------------------------------------------------------
// SshConnection
// ---------------------------------------------------------------------------

/// An authenticated russh connection to one host, plus the jump-host
/// connections it is tunnelled through (empty for a direct connection).
///
/// The bastion handles are owned for the whole lifetime of the connection so
/// the chain outlives nothing it carries. Teardown runs the other way: the
/// target's session task holds the `direct-tcpip` stream of the hop below it,
/// so dropping this struct closes the target first and cascades outward.
/// Derefs to the target's [`Handle`], so callers open channels on it exactly as
/// before.
pub(crate) struct SshConnection {
    handle: Handle<KnownHostsHandler>,
    /// Bastions, nearest-first. Never used directly — kept alive by ownership.
    _jumps: Vec<Handle<KnownHostsHandler>>,
}

impl std::ops::Deref for SshConnection {
    type Target = Handle<KnownHostsHandler>;

    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

// ---------------------------------------------------------------------------
// SshSession
// ---------------------------------------------------------------------------

/// An authenticated SSH session ready for command execution.
///
/// Holds the russh client handle for the duration of its lifetime.
/// Drop → the connection is cleaned up by russh's internal tasks.
///
/// Wrapped in Arc to allow sharing across multiple operations (discovery + metrics).
#[derive(Clone)]
pub struct SshSession {
    handle: Arc<SshConnection>,
}

impl SshSession {
    /// Connect and authenticate to `host`.
    ///
    /// Authentication is attempted in order:
    /// 1. SSH agent (unix only, via `SSH_AUTH_SOCK`).
    /// 2. Identity file specified in the host config (`identity_file`).
    /// 3. Default key files (`~/.ssh/id_ed25519`, `id_rsa`, etc.).
    /// 4. Password: one typed for this login earlier in the session, then the
    ///    one in the host config.
    ///
    /// A host with a `ProxyJump` is reached through its bastion chain; each hop
    /// authenticates the same way.
    ///
    /// Returns an error when no method succeeds or the connection times out.
    ///
    /// # Errors
    /// - Connection timeout (> 10 s per hop)
    /// - Authentication failure
    /// - Network error
    /// - An unresolvable `ProxyJump` chain (cycle or too many hops)
    pub async fn connect(host: &Host) -> anyhow::Result<Self> {
        Self::connect_with(host, Passwords::Remembered).await
    }

    /// [`SshSession::connect`] with a say over which passwords it may use.
    pub(crate) async fn connect_with(
        host: &Host,
        passwords: Passwords<'_>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            handle: Arc::new(connect_and_auth(host, passwords).await?),
        })
    }

    /// Execute a shell command on the remote host and return its stdout.
    ///
    /// A new SSH channel is opened for each call so sessions can be
    /// reused across multiple commands. The remote exit status is ignored —
    /// use [`SshSession::run_command_checked`] when it carries the result.
    ///
    /// # Errors
    /// Returns an error on channel failure or if the command times out (30 s).
    pub async fn run_command(&self, cmd: &str) -> anyhow::Result<String> {
        Ok(self.exec(cmd).await?.0)
    }

    /// Like [`SshSession::run_command`] but returns an error when the remote
    /// command exits with a non-zero status. Use for `test`-style probes whose
    /// exit code is the answer (e.g. `sudo -n true`).
    ///
    /// # Errors
    /// As [`SshSession::run_command`], plus a non-zero remote exit status.
    pub async fn run_command_checked(&self, cmd: &str) -> anyhow::Result<String> {
        let (output, status) = self.exec(cmd).await?;
        match status {
            // A missing exit status is treated as success — failing a command
            // that likely worked is worse than missing a rare edge case.
            None | Some(0) => Ok(output),
            Some(code) => Err(anyhow!("remote command exited with status {code}")),
        }
    }

    /// Opens a channel, runs `cmd`, and returns its stdout and exit status.
    async fn exec(&self, cmd: &str) -> anyhow::Result<(String, Option<u32>)> {
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .context("open SSH channel")?;

        channel.exec(true, cmd).await.context("exec SSH command")?;

        time::timeout(Duration::from_secs(30), collect_output(&mut channel))
            .await
            .map_err(|_| anyhow!("command timed out (30 s): {}", cmd))?
            .context("read command output")
    }

    /// Opens a new SSH channel, requests the SFTP subsystem, and returns the
    /// channel as an async stream suitable for [`russh_sftp::client::SftpSession::new`].
    ///
    /// The `SshSession` **must** remain alive for the entire lifetime of the
    /// SFTP session — dropping it closes the underlying TCP connection.
    ///
    /// # Errors
    /// Returns an error if the channel cannot be opened or if the server rejects
    /// the SFTP subsystem request.
    pub async fn open_sftp_channel(
        &self,
    ) -> anyhow::Result<russh::ChannelStream<russh::client::Msg>> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .context("open SFTP session channel")?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .context("request SFTP subsystem")?;
        Ok(channel.into_stream())
    }

    /// Gracefully close the SSH connection.
    pub async fn disconnect(self) {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }
}

// ---------------------------------------------------------------------------
// Connection + authentication
// ---------------------------------------------------------------------------

/// Per-hop budget for the TCP connect and SSH handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Connect to `host`, verify its host key, and authenticate — through the
/// host's `ProxyJump` chain when it has one. Everything but the terminal comes
/// through here: [`SshSession::connect`] (metrics, SFTP, key setup) and tunnels.
///
/// # Errors
/// Connection timeout (> 10 s per hop), host-key rejection, authentication
/// failure, or an unresolvable `ProxyJump` chain.
pub(crate) async fn connect_and_auth(
    host: &Host,
    passwords: Passwords<'_>,
) -> anyhow::Result<SshConnection> {
    connect_chain(host, passwords, false).await
}

/// [`connect_and_auth`] for an interactive shell, whose target lends the local
/// agent when `lends_agent` — decided once by the caller, which also offers it.
/// Bastions never do, as with `ssh -J -A`.
pub(crate) async fn connect_for_shell(
    host: &Host,
    passwords: Passwords<'_>,
    lends_agent: bool,
) -> anyhow::Result<SshConnection> {
    connect_chain(host, passwords, lends_agent).await
}

/// Whether a terminal to `host` lends the local agent. With no agent running there
/// is nothing to lend, and offering one anyway would leave the remote shell an
/// `SSH_AUTH_SOCK` that leads nowhere.
pub(crate) fn forwards_agent(host: &Host) -> bool {
    host.forward_agent && agent_running()
}

/// Whether an agent answers at `SSH_AUTH_SOCK`: the variable outlives an agent that
/// has stopped. A local connect, so it costs nothing to ask.
#[cfg(unix)]
fn agent_running() -> bool {
    std::env::var_os("SSH_AUTH_SOCK")
        .is_some_and(|path| std::os::unix::net::UnixStream::connect(path).is_ok())
}

/// Agent authentication is unix-only, and so is lending the agent.
#[cfg(not(unix))]
fn agent_running() -> bool {
    false
}

/// Both entry points share it, so every native SSH path honors the same keys,
/// agent, passwords, known_hosts policy and bastions.
async fn connect_chain(
    host: &Host,
    mut passwords: Passwords<'_>,
    lends_agent: bool,
) -> anyhow::Result<SshConnection> {
    let chain = jump_chain(host).await?;

    // Walk the bastions outward: the first is reached directly, every later one
    // through its predecessor. The target then rides the last hop.
    let mut jumps: Vec<Handle<KnownHostsHandler>> = Vec::with_capacity(chain.len());
    for (i, hop) in chain.iter().enumerate() {
        let key = login_key(hop, &chain[..i]);
        let handle = match jumps.last() {
            None => connect_direct(hop, &key, &mut passwords, false).await,
            Some(via) => connect_tunnelled(via, hop, &key, &mut passwords, false).await,
        }
        .map_err(|e| at_hop(e, format!("ProxyJump via '{}' failed", hop.name)))?;
        jumps.push(handle);
    }

    let key = login_key(host, &chain);
    let handle = match (jumps.last(), chain.last()) {
        (Some(via), Some(last)) => connect_tunnelled(via, host, &key, &mut passwords, lends_agent)
            .await
            .map_err(|e| at_hop(e, format!("connecting via '{}' failed", last.name)))?,
        _ => connect_direct(host, &key, &mut passwords, lends_agent).await?,
    };

    Ok(SshConnection {
        handle,
        _jumps: jumps,
    })
}

/// Wall-clock budget one [`SshSession::connect`] needs for `host`: the per-hop
/// connect timeout and agent bound once for every bastion in its `ProxyJump`
/// chain, plus the target.
///
/// Callers that wrap the connect in a timeout of their own must scale it by
/// this — a fixed budget trips on a bastion chain before the connection has had
/// the time [`connect_and_auth`] is entitled to.
pub(crate) async fn connect_budget(host: &Host) -> Duration {
    // A chain that fails to resolve costs nothing to connect; the caller's own
    // attempt reports why.
    let hops = jump_chain(host).await.map_or(0, |chain| chain.len());
    // The agent's bound counts too, so a slow but working agent is not taken
    // for a dead host.
    (CONNECT_TIMEOUT + AGENT_BUDGET) * (hops as u32 + 1)
}

// The algorithms offered, spelled out from 1.1.4's (russh 0.46's defaults)
// rather than taken from russh, whose defaults change between releases: later
// ones drop the SHA-1 MACs that some network gear still needs.

const KEX_ORDER: &[kex::Name] = &[
    kex::CURVE25519,
    kex::CURVE25519_PRE_RFC_8731,
    kex::DH_G16_SHA512,
    kex::DH_G14_SHA256,
    // Last, so a server that works without them negotiates as it always did.
    // Some take nothing else (Cisco RoomOS).
    kex::ECDH_SHA2_NISTP256,
    kex::ECDH_SHA2_NISTP384,
    kex::ECDH_SHA2_NISTP521,
    // The one method without SHA-1 that OpenSSH before 5.7 has (RHEL 6).
    kex::DH_GEX_SHA256,
    kex::EXTENSION_SUPPORT_AS_CLIENT,
    kex::EXTENSION_SUPPORT_AS_SERVER,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_SERVER,
];

const CIPHER_ORDER: &[cipher::Name] = &[
    cipher::CHACHA20_POLY1305,
    cipher::AES_256_GCM,
    // All some servers take (Cisco RoomOS).
    cipher::AES_128_GCM,
    cipher::AES_256_CTR,
    cipher::AES_192_CTR,
    cipher::AES_128_CTR,
];

const MAC_ORDER: &[mac::Name] = &[
    mac::HMAC_SHA512_ETM,
    mac::HMAC_SHA256_ETM,
    mac::HMAC_SHA512,
    mac::HMAC_SHA256,
    mac::HMAC_SHA1_ETM,
    mac::HMAC_SHA1,
];

const COMPRESSION_ORDER: &[compression::Name] = &[
    compression::NONE,
    compression::ZLIB,
    compression::ZLIB_LEGACY,
];

/// The russh client configuration for one hop: algorithms, timeouts,
/// keepalives, and the host key types already saved for it first.
fn client_config(host: &Host) -> Arc<client::Config> {
    Arc::new(client::Config {
        preferred: russh::Preferred {
            kex: Cow::Borrowed(KEX_ORDER),
            key: known_hosts::preferred(&host.hostname, host.port),
            cipher: Cow::Borrowed(CIPHER_ORDER),
            mac: Cow::Borrowed(MAC_ORDER),
            compression: Cow::Borrowed(COMPRESSION_ORDER),
            ..russh::Preferred::DEFAULT
        },
        // russh asks for 8192 bits, whose exponentiations stall the connecting
        // task for up to seconds; 4096 is group16's size.
        gex: client::GexParams::new(2048, 4096, 8192).expect("valid group sizes"),
        // No inactivity timeout: russh skips resetting it on the iteration that
        // sends a keepalive, so a peer that never answers `keepalive@openssh.com`
        // (common in appliance SSH stacks) was torn down after 30 s even while
        // its commands still ran. Liveness stays bounded by `keepalive_max`.
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        channel_buffer_size: CHANNEL_QUEUE,
        ..Default::default()
    })
}

/// How many messages russh queues for one channel before the whole connection
/// waits for it to be read: in effect no limit, as in russh 0.46. With russh's
/// 100, a paste echoed back while the terminal waited for window space, or one
/// tunnel client that stopped reading, froze the connection for good.
const CHANNEL_QUEUE: usize = 1 << 24;

/// Resolves `host`'s `ProxyJump` into the hops to connect before it.
///
/// The jump aliases are looked up in the merged host list, so a bastion defined
/// elsewhere in `~/.ssh/config` (or in `hosts.toml`) contributes its own
/// HostName/User/Port/IdentityFile. Loading is skipped entirely for the common
/// no-`ProxyJump` case.
///
/// # Errors
/// An unreadable host list, or a chain that cannot be resolved. Both fail the
/// connection: resolving a bastion alias against nothing would fall back to
/// dialling the alias as a hostname, which is a different machine.
async fn jump_chain(host: &Host) -> anyhow::Result<Vec<Host>> {
    if crate::ssh::jump::jump_value(host).is_none() {
        return Ok(Vec::new());
    }
    // load_all_hosts() is blocking file I/O — keep it off the async worker.
    let known = tokio::task::spawn_blocking(crate::config::load_all_hosts)
        .await
        .context("host list load panicked")?
        // Only the outer error: a parse error quotes the offending line of
        // hosts.toml, which may be a saved password.
        .map_err(|e| anyhow!("could not load hosts for ProxyJump resolution: {e}"))?;

    let chain = crate::ssh::jump::resolve_chain(host, &known)?;
    tracing::debug!(
        host = %host.name,
        via = %chain.iter().map(|h| h.name.as_str()).collect::<Vec<_>>().join(" -> "),
        "resolved ProxyJump chain"
    );
    Ok(chain)
}

/// Opens a TCP connection to `host` and authenticates.
async fn connect_direct(
    host: &Host,
    key: &str,
    passwords: &mut Passwords<'_>,
    lends_agent: bool,
) -> anyhow::Result<Handle<KnownHostsHandler>> {
    let dial = || dial_direct(host, lends_agent);
    finish_auth(dial().await?, host, key, dial, passwords).await
}

/// Reaches `host` through the already-connected bastion `via`: a `direct-tcpip`
/// channel on the bastion carries a second SSH session to the target, which is
/// verified and authenticated in its own right.
async fn connect_tunnelled(
    via: &Handle<KnownHostsHandler>,
    host: &Host,
    key: &str,
    passwords: &mut Passwords<'_>,
    lends_agent: bool,
) -> anyhow::Result<Handle<KnownHostsHandler>> {
    let dial = || dial_tunnelled(via, host, lends_agent);
    finish_auth(dial().await?, host, key, dial, passwords).await
}

/// A connection that has shaken hands and passed the host-key check, not yet
/// authenticated.
struct Dialed {
    handle: Handle<KnownHostsHandler>,
    link: Link,
    /// The session has wound down far enough for `link` to say why it ended.
    settled: bool,
    /// A login request here got no answer in time; nothing more is sent on it.
    broken: bool,
}

impl Dialed {
    /// Whether a login request can still go out on this connection.
    async fn usable(&mut self) -> bool {
        !self.broken && !self.closed().await
    }

    /// Whether the connection is gone. The first time, it waits (briefly) for
    /// the session to finish, so the handler has recorded how it ended.
    async fn closed(&mut self) -> bool {
        if !self.handle.is_closed() {
            return false;
        }
        if !self.settled {
            self.settled = true;
            let _ = time::timeout(Duration::from_secs(1), &mut self.handle).await;
        }
        true
    }
}

/// Opens a TCP connection to `host` and verifies its host key.
async fn dial_direct(host: &Host, lends_agent: bool) -> anyhow::Result<Dialed> {
    let addr = format!("{}:{}", host.hostname, host.port);
    let (handler, link) = known_hosts_handler(host, lends_agent);
    let handle = time::timeout(
        CONNECT_TIMEOUT,
        client::connect(client_config(host), addr, handler),
    )
    .await
    .map_err(|_| anyhow!("SSH connection timed out (10 s)"))?
    .map_err(|e| link.connect_error(e))?;
    Ok(Dialed {
        handle,
        link,
        settled: false,
        broken: false,
    })
}

/// Opens a `direct-tcpip` channel to `host` on the bastion `via` and runs the
/// SSH handshake over it.
async fn dial_tunnelled(
    via: &Handle<KnownHostsHandler>,
    host: &Host,
    lends_agent: bool,
) -> anyhow::Result<Dialed> {
    // The originator address is informational; ssh(1) reports the loopback it
    // forwards from, and servers only log it.
    //
    // Timed out like the handshake it precedes: the bastion answers only once
    // its own connect() to the target resolves, so a firewalled target would
    // otherwise park the caller for the bastion's whole SYN budget.
    let channel = time::timeout(
        CONNECT_TIMEOUT,
        via.channel_open_direct_tcpip(host.hostname.clone(), host.port as u32, "127.0.0.1", 0),
    )
    .await
    .map_err(|_| anyhow!("SSH connection timed out (10 s)"))?
    .with_context(|| format!("open tunnel to {}:{}", host.hostname, host.port))?;

    let (handler, link) = known_hosts_handler(host, lends_agent);
    let handle = time::timeout(
        CONNECT_TIMEOUT,
        client::connect_stream(client_config(host), channel.into_stream(), handler),
    )
    .await
    .map_err(|_| anyhow!("SSH connection timed out (10 s)"))?
    .map_err(|e| link.connect_error(e))?;
    Ok(Dialed {
        handle,
        link,
        settled: false,
        broken: false,
    })
}

/// The host-key verifier for `host`, and what it will report about the
/// connection. The lookup uses the target's own hostname/port even over a
/// tunnel, so `known_hosts` entries match what an `ssh -J` would record.
fn known_hosts_handler(host: &Host, lends_agent: bool) -> (KnownHostsHandler, Link) {
    let (ended_tx, ended) = watch::channel(());
    let link = Link {
        hung_up: Arc::new(AtomicBool::new(false)),
        no_method: Arc::new(AtomicBool::new(false)),
        rsa_sigs: Arc::default(),
        new_key: Arc::new(Mutex::new(None)),
        refusal: Arc::new(Mutex::new(None)),
        ended,
    };
    let handler = KnownHostsHandler {
        host: host.hostname.clone(),
        port: host.port,
        hung_up: Arc::clone(&link.hung_up),
        no_method: Arc::clone(&link.no_method),
        rsa_sigs: Arc::clone(&link.rsa_sigs),
        new_key: Arc::clone(&link.new_key),
        refusal: Arc::clone(&link.refusal),
        lends_agent,
        ended: ended_tx,
    };
    (handler, link)
}

/// Authenticates the `first` connection as `host` (remembered passwords under
/// `key`), converting a refusal into an error. `dial` opens another connection
/// to the same hop, for keyboard-interactive.
async fn finish_auth<F, Fut>(
    first: Dialed,
    host: &Host,
    key: &str,
    dial: F,
    passwords: &mut Passwords<'_>,
) -> anyhow::Result<Handle<KnownHostsHandler>>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<Dialed>>,
{
    let Dialed { handle, link, .. } = first;
    let asking = matches!(passwords, Passwords::Ask(_));
    let rsa_sigs = *link
        .rsa_sigs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (handle, keys) = authenticate(handle, host, asking, rsa_sigs, &link.no_method).await?;
    let encrypted_key = match keys {
        KeyAuth::Accepted => return Ok(handle),
        KeyAuth::Rejected { encrypted_key } => encrypted_key,
    };
    let mut first = Dialed {
        handle,
        link,
        settled: false,
        broken: false,
    };
    let no_method = |first: &Dialed| first.link.no_method.load(Ordering::SeqCst);
    if no_method(&first) || (first.closed().await && no_method(&first)) {
        return Err(Refused(format!(
            "SSH authentication failed for {}: the server offers no login method OmnySSH supports",
            host.name
        ))
        .into());
    }

    // The server login password, never a key passphrase, and tried last: keys
    // are what OmnySSH steers users towards. One typed this session goes first,
    // as it is newer than a saved one — but never to a host key first seen on
    // this connection: that is not the server it was typed for.
    let new_key = first.link.new_key();
    let typed = match passwords {
        Passwords::Remembered | Passwords::Ask(_) if new_key.is_none() => password::accepted(key),
        _ => None,
    };
    let saved = match passwords {
        Passwords::KeysOnly => None,
        _ => host.password.clone(),
    }
    .filter(|p| typed.as_ref() != Some(p));
    let known = typed.is_some() || saved.is_some();
    let mut refused = Vec::new();
    let mut spare = None;
    for (password, was_typed) in typed
        .map(|p| (p, true))
        .into_iter()
        .chain(saved.map(|p| (p, false)))
    {
        match offer(&mut first, &mut spare, &dial, host, key, &password).await {
            Offer::Here => return Ok(password_login(host, first.handle)),
            Offer::There(handle) => return Ok(password_login(host, handle)),
            Offer::Rejected => {
                if was_typed {
                    password::forget(key, &password);
                }
                refused.push(password);
            }
            Offer::Unavailable(e) => return Err(e),
        }
    }

    // A locked identity file is what the host is set up with: its passphrase
    // comes before any password (#97). A locked default key does not hold up a
    // password prompt — it may not be a key this server takes.
    let locked = match encrypted_key {
        Some((path, true)) => return Err(PassphraseRequired::new(path, key).into()),
        Some((path, false)) => Some(path),
        None => None,
    };

    if let Passwords::Ask(ask) = passwords {
        let asked = ask_password(
            &mut **ask,
            &mut first,
            &mut spare,
            &dial,
            host,
            key,
            &refused,
            new_key.as_deref(),
        )
        .await?;
        let refusal = match asked {
            Asked::In(Offer::There(handle)) => return Ok(password_login(host, handle)),
            Asked::In(_) => return Ok(password_login(host, first.handle)),
            Asked::Unanswered(NoAnswer::Cancelled) => {
                format!("SSH login cancelled for {}", host.name)
            }
            Asked::Unanswered(NoAnswer::TimedOut) => format!(
                "SSH login to {} gave up waiting for the password",
                host.name
            ),
            Asked::Refused => format!("SSH authentication failed for {}", host.name),
        };
        // No password got in: the key it skipped still may (#97).
        return Err(match locked {
            Some(path) => PassphraseRequired::new(path, key).into(),
            None => Refused(refusal).into(),
        });
    }

    if let Some(path) = locked {
        return Err(PassphraseRequired::new(path, key).into());
    }
    if !known && !matches!(passwords, Passwords::KeysOnly) {
        return Err(NoPassword::new(key, &host.name).into());
    }

    let message = format!("SSH authentication failed for {}", host.name);
    // Every attempt folds a dropped link into "not accepted". A connection that
    // is gone refused us only if the server hung up itself, as OpenSSH does
    // after too many failed logins.
    if first.closed().await && !first.link.hung_up.load(Ordering::SeqCst) {
        return Err(anyhow!(message));
    }
    Err(Refused(message).into())
}

/// How asking the user went.
enum Asked {
    /// A password got in: [`Offer::Here`] or [`Offer::There`].
    In(Offer),
    /// Every answer was refused.
    Refused,
    Unanswered(NoAnswer),
}

/// Asks the user for the password, up to [`PASSWORD_PROMPTS`] times.
#[allow(clippy::too_many_arguments)]
async fn ask_password<F, Fut>(
    ask: &mut dyn AskPassword,
    first: &mut Dialed,
    spare: &mut Option<Dialed>,
    dial: &F,
    host: &Host,
    key: &str,
    refused: &[String],
    new_key: Option<&str>,
) -> anyhow::Result<Asked>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<Dialed>>,
{
    let login = format!("{}@{}", host.user, host.hostname);
    let mut refused = refused.to_vec();
    let mut retry = false;
    for _ in 0..PASSWORD_PROMPTS {
        let prompt = Prompt {
            login: &login,
            retry,
            new_host_key: new_key,
        };
        let password = match ask.ask(prompt).await {
            Ok(password) => password,
            Err(no_answer) => return Ok(Asked::Unanswered(no_answer)),
        };
        retry = true;
        // Sending it again would only cost another failed login.
        if refused.contains(&password) {
            continue;
        }
        // Only a hang-up this answer caused ends the asking; one from the key
        // attempts before it just moves the answers to fresh connections.
        let first_open = first.usable().await;
        match offer(first, spare, dial, host, key, &password).await {
            Offer::Rejected => refused.push(password),
            Offer::Unavailable(e) => return Err(e),
            accepted => {
                password::remember(key, &password);
                return Ok(Asked::In(accepted));
            }
        }
        // OpenSSH hangs up after too many failed logins; ssh(1) stops there too.
        if first_open && first.closed().await && first.link.hung_up.load(Ordering::SeqCst) {
            break;
        }
    }
    Ok(Asked::Refused)
}

fn password_login(host: &Host, handle: Handle<KnownHostsHandler>) -> Handle<KnownHostsHandler> {
    tracing::info!(
        host = %host.name,
        "Connected via password authentication — consider setting up SSH key"
    );
    handle
}

/// How an offered password went.
enum Offer {
    /// Got in on the first connection.
    Here,
    /// Got in on a fresh connection.
    There(Handle<KnownHostsHandler>),
    /// The server said no.
    Rejected,
    /// No verdict: the connection could not be made or went away.
    Unavailable(anyhow::Error),
}

/// Offers `password` the way ssh(1) does: by the password method, and by
/// keyboard-interactive, which is all some servers take (UniFi consoles turn the
/// password method off). That part runs on a fresh connection, where the keys
/// tried before count against no MaxAuthTries, kept in `spare` for the next
/// answer. Which of the two a login takes is remembered, so a wrong password
/// costs one failed login, not two.
async fn offer<F, Fut>(
    first: &mut Dialed,
    spare: &mut Option<Dialed>,
    dial: &F,
    host: &Host,
    key: &str,
    password: &str,
) -> Offer
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<Dialed>>,
{
    let method = password::method(key);
    let mut tried_password = false;
    if method != Some(Method::KeyboardInteractive) && first.usable().await {
        match password_auth(&mut first.handle, &host.user, password).await {
            Some(true) => {
                password::learn(key, Method::Password);
                return Offer::Here;
            }
            Some(false) => tried_password = true,
            // A server that refuses and hangs up (too many failures) gave its
            // verdict; any other silence is none, and the connection is done.
            None if first.closed().await && first.link.hung_up.load(Ordering::SeqCst) => {
                tried_password = true
            }
            None => first.broken = true,
        }
        if tried_password && method == Some(Method::Password) {
            return Offer::Rejected;
        }
    }

    let fresh = match spare.take() {
        Some(conn) if !conn.handle.is_closed() => conn,
        _ => match dial().await {
            Ok(conn) => conn,
            Err(e) => return Offer::Unavailable(e),
        },
    };
    // With known_hosts unwritable every connection meets the key anew: this one
    // must meet the same key as the first, whose fingerprint the user saw.
    if let (Some(seen), Some(now)) = (first.link.new_key(), fresh.link.new_key()) {
        if seen != now {
            return Offer::Unavailable(
                Refused(format!(
                    "SSH login to {} stopped: the host key changed between connections",
                    host.name
                ))
                .into(),
            );
        }
    }
    let mut fresh = if method == Some(Method::Password) {
        fresh
    } else {
        match kbd_login(fresh, &host.user, password).await {
            (Some(conn), Kbd::Accepted) => {
                password::learn(key, Method::KeyboardInteractive);
                return Offer::There(conn.handle);
            }
            (conn, Kbd::Refused) => {
                password::learn(key, Method::KeyboardInteractive);
                *spare = conn;
                return Offer::Rejected;
            }
            // The password method already turned this password down.
            (_, Kbd::Broken) if tried_password => return Offer::Rejected,
            // A second factor is a dead end whatever the password: say so.
            (_, Kbd::WantsCode) => {
                return Offer::Unavailable(
                    Refused(format!(
                        "SSH login to {} asks for a verification code, which OmnySSH cannot answer",
                        host.name
                    ))
                    .into(),
                )
            }
            (Some(conn), Kbd::NotOffered) => {
                password::learn(key, Method::Password);
                conn
            }
            _ => return Offer::Unavailable(anyhow!("SSH login to {} broke off", host.name)),
        }
    };
    if tried_password {
        return Offer::Rejected;
    }
    match password_auth(&mut fresh.handle, &host.user, password).await {
        Some(true) => {
            password::learn(key, Method::Password);
            Offer::There(fresh.handle)
        }
        Some(false) => Offer::Rejected,
        None => Offer::Unavailable(anyhow!("SSH login to {} broke off", host.name)),
    }
}

// ---------------------------------------------------------------------------
// Authentication helpers
// ---------------------------------------------------------------------------

/// How long one login request may wait for the server's verdict.
const AUTH_TIMEOUT: Duration = Duration::from_secs(30);

enum KeyAuth {
    Accepted,
    /// No key got in. `encrypted_key` is one skipped for want of its
    /// passphrase, and whether it is the host's own identity file.
    Rejected {
        encrypted_key: Option<(String, bool)>,
    },
}

/// Tries the agent, the identity file and the default keys, in that order.
/// Agent keys matching the host's identity go first, as ssh(1) orders them; with
/// `IdentitiesOnly` the agent offers nothing else, and the default keys stay out
/// next to an identity file. `user_started`: a login the user is watching also
/// retries agent keys the user turned down before.
async fn authenticate(
    handle: Handle<KnownHostsHandler>,
    host: &Host,
    user_started: bool,
    rsa_sigs: RsaSigs,
    no_method: &Arc<AtomicBool>,
) -> anyhow::Result<(Handle<KnownHostsHandler>, KeyAuth)> {
    let rsa = RsaHash::new(rsa_sigs);
    let user = host.user.clone();
    let mut encrypted_key: Option<(String, bool)> = None;

    // 1. Try SSH agent first — it handles passphrase-protected keys and is the
    //    most common auth method for non-interactive clients.
    #[cfg(unix)]
    let (handle, turned_down, mut rsa) = {
        let identities = Identities {
            keys: host_identities(host).await,
            only: host.identities_only,
        };
        let (handle, accepted, turned_down, rsa) =
            agent_login(handle, &user, user_started, identities, rsa, no_method).await?;
        if accepted {
            return Ok((handle, KeyAuth::Accepted));
        }
        (handle, turned_down, rsa)
    };
    #[cfg(not(unix))]
    let (turned_down, mut rsa): (Vec<PublicKey>, RsaHash) = {
        let _ = user_started;
        (Vec::new(), rsa)
    };
    let mut handle = handle;

    // 2. Try explicit identity_file from host config. Here and below a key the
    //    server already turned down through the agent is not offered again: every
    //    refusal counts towards its MaxAuthTries.
    if let Some(key_path) = &host.identity_file {
        if !refused_before(key_path, &turned_down).await {
            match try_key_auth(&mut handle, &user, key_path, &mut rsa, no_method).await {
                Ok(true) => return Ok((handle, KeyAuth::Accepted)),
                Ok(false) => {}
                Err(e) => note_encrypted(&mut encrypted_key, e, true)?,
            }
        }
    }

    // 3. Try default key files — mirrors what the `ssh` binary does when no
    //    -i flag is given. Skips files that don't exist. A locked one is worth a
    //    prompt only without an identity file: ssh(1) would not offer it then.
    if host.identities_only && host.identity_file.is_some() {
        return Ok((handle, KeyAuth::Rejected { encrypted_key }));
    }
    for key_path in default_key_paths() {
        if key_path.exists() {
            let path_str = key_path.to_string_lossy().into_owned();
            if refused_before(&path_str, &turned_down).await {
                continue;
            }
            match try_key_auth(&mut handle, &user, &path_str, &mut rsa, no_method).await {
                Ok(true) => return Ok((handle, KeyAuth::Accepted)),
                Ok(false) => {}
                Err(e) if host.identity_file.is_none() => {
                    note_encrypted(&mut encrypted_key, e, false)?
                }
                Err(e) => stalled(e)?,
            }
        }
    }

    Ok((handle, KeyAuth::Rejected { encrypted_key }))
}

/// Whether the key at `path` is one the server turned down through the agent.
/// Never an RSA key: some agents sign with SHA-1 whatever hash is asked for, and
/// the file key then signs as [`RsaHash`] says.
async fn refused_before(path: &str, turned_down: &[PublicKey]) -> bool {
    if turned_down.is_empty() {
        return false;
    }
    let path = path.to_string();
    tokio::task::spawn_blocking(move || identity::public_key(&path))
        .await
        .ok()
        .flatten()
        .is_some_and(|key| !key.algorithm().is_rsa() && turned_down.contains(&key))
}

/// The hash an RSA key signs with on one connection: the best the server lists
/// in `server-sig-algs`, else rsa-sha2-256, which more servers take (Dropbear
/// has no rsa-sha2-512). SHA-1 only for a server that lists nothing else, or is
/// too old to list anything, as PuTTY and ssh(1) before 8.8 sign for it. Asked
/// once: a server that lists nothing makes every ask wait a second.
struct RsaHash {
    sigs: RsaSigs,
    /// Once known; `Some(None)` signs with SHA-1.
    hash: Option<Option<HashAlg>>,
}

impl RsaHash {
    fn new(sigs: RsaSigs) -> Self {
        // A server too old for rsa-sha2 sends no server-sig-algs to wait for.
        let hash = (sigs == RsaSigs::Sha1).then_some(None);
        Self { sigs, hash }
    }

    /// `None` for a key that is not RSA, or one signing with SHA-1.
    async fn for_key(
        &mut self,
        handle: &Handle<KnownHostsHandler>,
        key: &PublicKey,
    ) -> Option<HashAlg> {
        if !key.algorithm().is_rsa() {
            return None;
        }
        if self.hash.is_none() {
            let listed = time::timeout(AUTH_TIMEOUT, handle.best_supported_rsa_hash()).await;
            self.hash = Some(match listed.ok().and_then(Result::ok).flatten() {
                Some(Some(hash)) => Some(hash),
                // ssh-rsa alone.
                Some(None) if self.sigs != RsaSigs::Sha2Unlisted => None,
                _ => Some(HashAlg::Sha256),
            });
        }
        self.hash.flatten()
    }
}

/// What a server's version string says about RSA key signatures.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
enum RsaSigs {
    /// Whatever `server-sig-algs` lists.
    #[default]
    AsListed,
    /// OpenSSH before 7.2 or Dropbear before 2020.79: SHA-1 only.
    Sha1,
    /// OpenSSH 7.4 lists only ssh-rsa yet takes rsa-sha2, as ssh(1) knows.
    Sha2Unlisted,
}

fn rsa_sigs(ident: &[u8]) -> RsaSigs {
    let ident = String::from_utf8_lossy(ident);
    let software = ident.splitn(3, '-').nth(2).unwrap_or_default();
    let version = |rest: &str| -> (u32, u32) {
        let mut parts = rest.split(|c: char| !c.is_ascii_digit());
        let major = parts
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(u32::MAX);
        let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        (major, minor)
    };
    if let Some(rest) = software.strip_prefix("OpenSSH_") {
        match version(rest) {
            v if v < (7, 2) => RsaSigs::Sha1,
            (7, 4) => RsaSigs::Sha2Unlisted,
            _ => RsaSigs::AsListed,
        }
    } else if let Some(rest) = software.strip_prefix("dropbear_") {
        if version(rest) < (2020, 79) {
            RsaSigs::Sha1
        } else {
            RsaSigs::AsListed
        }
    } else {
        RsaSigs::AsListed
    }
}

/// Which agent keys a login offers, and in what order.
#[cfg(unix)]
struct Identities {
    /// The host's own keys, offered first.
    keys: Vec<PublicKey>,
    /// `IdentitiesOnly`: offer no other.
    only: bool,
}

#[cfg(unix)]
impl Identities {
    /// The agent's keys in the order to offer them; the rest keep the agent's.
    fn order(&self, agent: Vec<PublicKey>) -> Vec<PublicKey> {
        let (mut offers, others): (Vec<_>, Vec<_>) =
            agent.into_iter().partition(|key| self.keys.contains(key));
        if !self.only {
            offers.extend(others);
        }
        offers
    }
}

/// The public keys `host` is set up with: its identity file's, or under
/// `IdentitiesOnly` the default keys', as ssh(1) takes them. File reads, so off
/// the async workers.
#[cfg(unix)]
async fn host_identities(host: &Host) -> Vec<PublicKey> {
    let paths: Vec<String> = match (&host.identity_file, host.identities_only) {
        (Some(path), _) => vec![path.clone()],
        (None, true) => default_key_paths()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        (None, false) => return Vec::new(),
    };
    tokio::task::spawn_blocking(move || {
        paths
            .iter()
            .filter_map(|path| identity::public_key(path))
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// A key the server did not answer for in time: the connection is left in an
/// unknown state, so the login stops here.
#[derive(Debug, thiserror::Error)]
#[error("the SSH server did not answer the login in time")]
struct Stalled;

fn stalled(err: anyhow::Error) -> anyhow::Result<()> {
    if err.is::<Stalled>() {
        return Err(err);
    }
    tracing::debug!(error = %err, "public-key authentication attempt failed");
    Ok(())
}

fn note_encrypted(
    encrypted_key: &mut Option<(String, bool)>,
    err: anyhow::Error,
    identity_file: bool,
) -> anyhow::Result<()> {
    match err.downcast_ref::<IdentityError>() {
        Some(IdentityError::Encrypted(path)) => {
            if encrypted_key.is_none() {
                *encrypted_key = Some((path.clone(), identity_file));
            }
            Ok(())
        }
        _ => stalled(err),
    }
}

/// Returns the standard default SSH private key paths in priority order.
/// FIDO (`id_*_sk`) keys are left out: russh cannot sign with them, so a
/// locked one would ask for a passphrase that could never help.
fn default_key_paths() -> Vec<std::path::PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let ssh = home.join(".ssh");
    ["id_ed25519", "id_rsa", "id_ecdsa"]
        .iter()
        .map(|name| ssh.join(name))
        .collect()
}

async fn try_key_auth(
    handle: &mut Handle<KnownHostsHandler>,
    user: &str,
    key_path: &str,
    rsa: &mut RsaHash,
    no_method: &AtomicBool,
) -> anyhow::Result<bool> {
    // load_secret_key is synchronous (file I/O) — offload to blocking pool.
    let path = key_path.to_string();
    let key_pair = tokio::task::spawn_blocking(move || identity::load_key_pair(&path))
        .await
        .context("spawn_blocking panicked")??;
    let hash = rsa.for_key(handle, key_pair.public_key()).await;

    // russh signs file keys itself, so giving up on the wait leaves it free.
    let result = time::timeout(
        AUTH_TIMEOUT,
        handle.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key_pair), hash)),
    )
    .await
    .map_err(|_| Stalled)?
    .context("authenticate_publickey")?;
    note_methods(&result, no_method);
    Ok(result.success())
}

/// Notes a login the server turned down while naming only methods OmnySSH does
/// not have (such as gssapi-with-mic): no password or key can get in then.
fn note_methods(result: &client::AuthResult, no_method: &AtomicBool) {
    use russh::MethodKind::{KeyboardInteractive, Password, PublicKey};
    if let client::AuthResult::Failure {
        remaining_methods, ..
    } = result
    {
        let usable =
            |m: &russh::MethodKind| matches!(m, PublicKey | Password | KeyboardInteractive);
        if !remaining_methods.is_empty() && !remaining_methods.iter().any(usable) {
            no_method.store(true, Ordering::SeqCst);
        }
    }
}

/// How long the SSH agent gets to answer the connect and the key listing. An
/// agent that accepts and never replies must not stall the whole login.
#[cfg(unix)]
const AGENT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long one agent signature may take. It can wait on the user (a confirm
/// dialog, a PIN, Touch ID).
#[cfg(unix)]
const SIGN_TIMEOUT: Duration = Duration::from_secs(60);

/// Upper bound on the agent's share of one hop's login, for callers that time
/// the whole connect (so a slow but working agent is not taken for a dead host).
const AGENT_BUDGET: Duration = Duration::from_secs(70);

/// Agent keys whose signature was turned down or timed out, by fingerprint.
/// Background logins skip them, so a confirm dialog the user said no to does
/// not come back every time a poller reconnects.
#[cfg(unix)]
fn refused_agent_keys() -> MutexGuard<'static, HashSet<String>> {
    static REFUSED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    REFUSED
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// A key's entry in [`refused_agent_keys`]. Some agents never sign with SHA-1,
/// which only old servers ask for, so that refusal is kept apart: other hosts
/// still get the key.
#[cfg(unix)]
fn refusal_entry(key: &PublicKey, hash: Option<HashAlg>) -> String {
    let fingerprint = fingerprint(key);
    if key.algorithm().is_rsa() && hash.is_none() {
        format!("{fingerprint} ssh-rsa")
    } else {
        fingerprint
    }
}

/// Runs the agent's keys in a task that owns the connection. A caller that gives
/// up mid-signature (a poller restarted, a tunnel stopped) then only detaches;
/// the task itself ends within the signing bound.
///
/// Also returns the keys the server turned down, and the RSA hash if it asked.
#[cfg(unix)]
async fn agent_login(
    handle: Handle<KnownHostsHandler>,
    user: &str,
    user_started: bool,
    identities: Identities,
    mut rsa: RsaHash,
    no_method: &Arc<AtomicBool>,
) -> anyhow::Result<(Handle<KnownHostsHandler>, bool, Vec<PublicKey>, RsaHash)> {
    let user = user.to_string();
    let no_method = Arc::clone(no_method);
    tokio::spawn(async move {
        let mut handle = handle;
        let mut turned_down = Vec::new();
        let accepted = try_agent_auth(
            &mut handle,
            &user,
            user_started,
            &identities,
            &mut turned_down,
            &mut rsa,
            &no_method,
        )
        .await
        .unwrap_or(false);
        (handle, accepted, turned_down, rsa)
    })
    .await
    .context("SSH agent login failed")
}

#[cfg(unix)]
async fn try_agent_auth(
    handle: &mut Handle<KnownHostsHandler>,
    user: &str,
    user_started: bool,
    identities: &Identities,
    turned_down: &mut Vec<PublicKey>,
    rsa: &mut RsaHash,
    no_method: &AtomicBool,
) -> anyhow::Result<bool> {
    use russh::keys::agent::AgentIdentity;

    let (agent, listed) = time::timeout(AGENT_TIMEOUT, async {
        let mut agent = connect_agent().await?;
        let listed = agent
            .request_identities()
            .await
            .context("request agent identities")?;
        Ok::<_, anyhow::Error>((agent, listed))
    })
    .await
    .map_err(|_| anyhow!("SSH agent did not answer"))??;
    // Plain keys only: OmnySSH does not log in with certificates.
    let listed = listed
        .into_iter()
        .filter_map(|identity| match identity {
            AgentIdentity::PublicKey { key, .. } => Some(key),
            AgentIdentity::Certificate { .. } => None,
        })
        .collect();

    let stalled = Arc::new(AtomicBool::new(false));
    let mut signer = AgentSigner {
        agent: Some(agent),
        failed: Arc::new(tokio::sync::Notify::new()),
        refused: false,
        stalled: Arc::clone(&stalled),
    };
    for pubkey in identities.order(listed) {
        let hash = rsa.for_key(handle, &pubkey).await;
        if !user_started && refused_agent_keys().contains(&refusal_entry(&pubkey, hash)) {
            continue;
        }
        // Fresh per key: a refusal must not cut the next key's attempt short.
        let failed = Arc::new(tokio::sync::Notify::new());
        signer.failed = Arc::clone(&failed);
        signer.refused = false;
        let result = {
            let attempt =
                handle.authenticate_publickey_with(user, pubkey.clone(), hash, &mut signer);
            tokio::pin!(attempt);
            tokio::select! {
                biased;
                done = &mut attempt => Some(done),
                () = failed.notified() => {
                    // russh got the buffer back unsigned, sent nothing and now
                    // waits for a reply that will not come. Let it finish handing
                    // the buffer over.
                    let _ = time::timeout(Duration::from_millis(100), &mut attempt).await;
                    None
                }
            }
        };
        match result {
            Some(Ok(client::AuthResult::Success)) => return Ok(true),
            // The agent would not sign: the server never turned this key down.
            Some(Ok(result @ client::AuthResult::Failure { .. })) if !signer.refused => {
                note_methods(&result, no_method);
                turned_down.push(pubkey)
            }
            Some(Ok(client::AuthResult::Failure { .. })) => {}
            Some(Err(_)) => {}
            None if stalled.load(Ordering::SeqCst) => {
                tracing::debug!("SSH agent stopped answering; trying other methods");
                return Ok(false);
            }
            // Turned down: the agent is fine, and a later key may sign.
            None => {}
        }
    }
    Ok(false)
}

#[cfg(unix)]
async fn connect_agent(
) -> anyhow::Result<russh::keys::agent::client::AgentClient<tokio::net::UnixStream>> {
    russh::keys::agent::client::AgentClient::connect_env()
        .await
        .context("connect to SSH agent")
}

/// Signs through the SSH agent without ever leaving russh waiting.
///
/// russh treats a signer error as final for the connection: it keeps waiting
/// for the signature and swallows every later auth request, so one refused or
/// stalled signature hung the login. Handing the buffer back unchanged makes
/// russh send nothing and carry on, and [`try_agent_auth`] moves on to the
/// other methods.
#[cfg(unix)]
struct AgentSigner {
    agent: Option<russh::keys::agent::client::AgentClient<tokio::net::UnixStream>>,
    failed: Arc<tokio::sync::Notify>,
    /// The agent did not sign for the current key.
    refused: bool,
    /// Set when a signature timed out: the agent is hung, not just unwilling.
    stalled: Arc<AtomicBool>,
}

#[cfg(unix)]
impl russh::Signer for AgentSigner {
    type Error = russh::AgentAuthError;

    async fn auth_sign(
        &mut self,
        key: &russh::keys::agent::AgentIdentity,
        hash_alg: Option<HashAlg>,
        to_sign: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let mut signed = None;
        if let Some(agent) = self.agent.as_mut() {
            let request = agent.sign_request(key, hash_alg, to_sign.clone());
            match time::timeout(SIGN_TIMEOUT, request).await {
                Ok(result) => signed = result.ok(),
                // A timed-out request leaves the agent connection mid-reply.
                Err(_) => {
                    self.agent = None;
                    self.stalled.store(true, Ordering::SeqCst);
                }
            }
        }
        let entry = refusal_entry(&key.public_key(), hash_alg);
        match signed {
            Some(data) => {
                refused_agent_keys().remove(&entry);
                Ok(data)
            }
            None => {
                refused_agent_keys().insert(entry);
                self.refused = true;
                self.failed.notify_one();
                Ok(to_sign)
            }
        }
    }
}

/// Password-method login: `Some(verdict)`, or `None` when there is none — the
/// connection went away or the server took too long, and it is not used again.
async fn password_auth(
    handle: &mut Handle<KnownHostsHandler>,
    user: &str,
    password: &str,
) -> Option<bool> {
    // No reply for us to owe here, so giving up on the wait is safe.
    match time::timeout(AUTH_TIMEOUT, handle.authenticate_password(user, password)).await {
        Ok(Ok(result)) if result.success() => Some(true),
        Ok(Ok(_)) if !handle.is_closed() => Some(false),
        _ => None,
    }
}

/// Rounds of server prompts one keyboard-interactive login may take.
const KBD_ROUNDS: usize = 4;

/// How long each keyboard-interactive reply may take; a wrong password makes
/// PAM stall for a few seconds.
const KBD_TIMEOUT: Duration = Duration::from_secs(20);

/// How a keyboard-interactive login went.
enum Kbd {
    Accepted,
    /// The server was sent the password and did not take it.
    Refused,
    /// The server does not do keyboard-interactive, or not in a way the
    /// password can answer.
    NotOffered,
    /// The server asked for a one-time code, not a password.
    WantsCode,
    /// No clean end (timeout, the session died): the connection is not reused.
    Broken,
}

/// Keyboard-interactive login answering the password prompt with `password`,
/// on a connection where it is the first method. Runs in a task that owns the
/// connection, so a caller that gives up only detaches. Hands the connection
/// back unless it broke.
async fn kbd_login(conn: Dialed, user: &str, password: &str) -> (Option<Dialed>, Kbd) {
    let (user, password) = (user.to_string(), password.to_string());
    tokio::spawn(async move {
        let mut conn = conn;
        match kbd_exchange(&mut conn, &user, &password).await {
            Kbd::Broken => {
                release(conn.handle).await;
                (None, Kbd::Broken)
            }
            outcome => (Some(conn), outcome),
        }
    })
    .await
    .unwrap_or((None, Kbd::Broken))
}

async fn kbd_exchange(conn: &mut Dialed, user: &str, password: &str) -> Kbd {
    use russh::client::KeyboardInteractiveAuthResponse as Reply;

    let Dialed { handle, link, .. } = conn;
    let mut password = Some(password);
    let mut wants_code = false;
    let mut reply = kbd_wait(
        &mut link.ended,
        handle.authenticate_keyboard_interactive_start(user, None),
    )
    .await;
    for round in 0.. {
        let prompts = match reply {
            Some(Ok(Reply::Success)) => return Kbd::Accepted,
            Some(Ok(Reply::Failure { .. })) if wants_code => return Kbd::WantsCode,
            // Refused only if the password went out; prompts it could not
            // answer (a user name, several fields) are as good as no offer.
            Some(Ok(Reply::Failure { .. })) if password.is_none() => return Kbd::Refused,
            Some(Ok(Reply::Failure { .. })) => return Kbd::NotOffered,
            Some(Ok(Reply::InfoRequest { prompts, .. })) if round <= KBD_ROUNDS => prompts,
            _ => return Kbd::Broken,
        };
        // Always answered: russh waits for the answer and swallows everything
        // else meanwhile. Past the round cap, no answers at all, which makes the
        // server give up.
        let answers = if round < KBD_ROUNDS {
            kbd_answers(&prompts, &mut password, &mut wants_code)
        } else {
            Vec::new()
        };
        reply = kbd_wait(
            &mut link.ended,
            handle.authenticate_keyboard_interactive_respond(answers),
        )
        .await;
    }
    Kbd::Broken
}

/// A keyboard-interactive step, given up after [`KBD_TIMEOUT`] or as soon as the
/// session ends.
async fn kbd_wait<T>(
    ended: &mut watch::Receiver<()>,
    step: impl Future<Output = Result<T, russh::Error>>,
) -> Option<Result<T, russh::Error>> {
    tokio::select! {
        reply = time::timeout(KBD_TIMEOUT, step) => reply.ok(),
        _ = ended.changed() => None,
    }
}

/// Drops a connection that may have a server prompt waiting for us, answering
/// it blank first; a prompt arriving later finds nobody to hand it to and ends
/// the session.
async fn release(mut handle: Handle<KnownHostsHandler>) {
    let _ = time::timeout(
        Duration::ZERO,
        handle.authenticate_keyboard_interactive_respond(Vec::new()),
    )
    .await;
}

/// Answers to one round of keyboard-interactive prompts: the password goes, once,
/// to a lone hidden prompt; anything else (a visible question, several fields)
/// gets blanks the server will refuse. A prompt for a one-time code gets a blank
/// too — the password must not end up at a 2FA service — and is noted in
/// `wants_code`.
fn kbd_answers(
    prompts: &[russh::client::Prompt],
    password: &mut Option<&str>,
    wants_code: &mut bool,
) -> Vec<String> {
    match prompts {
        [only] if !only.echo && asks_for_code(&only.prompt) => {
            *wants_code = true;
            vec![String::new()]
        }
        [only] if !only.echo => vec![password.take().unwrap_or_default().to_string()],
        _ => vec![String::new(); prompts.len()],
    }
}

/// Whether a prompt asks for a one-time code rather than the password. A deny
/// list, so "Password:" in any language still gets the password. Whole words
/// only, and never from a `user@host` (OpenSSH puts one in front): a user or
/// host name is no hint.
fn asks_for_code(prompt: &str) -> bool {
    let prompt = prompt.to_lowercase();
    let words: Vec<&str> = prompt
        .split_whitespace()
        .filter(|chunk| !chunk.contains('@'))
        .flat_map(|chunk| chunk.split(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();
    words.windows(2).any(|pair| pair == ["one", "time"])
        || words.iter().any(|word| {
            matches!(
                *word,
                "code" | "token" | "otp" | "passcode" | "2fa" | "yubikey" | "duo" | "pin"
            ) || word.starts_with("verif")
        })
}

// ---------------------------------------------------------------------------
// Output collection
// ---------------------------------------------------------------------------

async fn collect_output(
    channel: &mut russh::Channel<russh::client::Msg>,
) -> anyhow::Result<(String, Option<u32>)> {
    let mut buf = Vec::new();
    let mut exit_status = None;
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Data { ref data }) => {
                buf.extend_from_slice(data);
            }
            Some(ChannelMsg::ExtendedData { .. }) => {
                // stderr — discard to avoid corrupting stdout-only parser input
            }
            Some(ChannelMsg::ExitStatus { exit_status: code }) => {
                // Record it but keep reading: trailing stdout may still arrive
                // before the channel is closed.
                exit_status = Some(code);
            }
            Some(ChannelMsg::Eof) => {
                // Continue reading — ExitStatus may arrive after Eof.
            }
            Some(ChannelMsg::Close) | None => break,
            _ => {}
        }
    }
    // Use .lines() semantics: replace \r\n → \n for cross-platform safety.
    let raw = String::from_utf8_lossy(&buf);
    let normalised: String = raw.lines().flat_map(|l| [l, "\n"]).collect();
    Ok((normalised, exit_status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locked_key_stays_recognisable_through_a_jump_host() {
        let locked = anyhow::Error::from(PassphraseRequired::new(
            String::from("/k/id"),
            "root@10.0.0.5:22",
        ));
        let hop = at_hop(locked, String::from("ProxyJump via 'bastion' failed"));
        assert_eq!(passphrase_required(&hop), Some("/k/id"));
        assert_eq!(waiting_login(&hop), Some("root@10.0.0.5:22"));
        assert!(!is_refused(&hop));
        assert_eq!(
            hop.to_string(),
            "ProxyJump via 'bastion' failed: SSH key requires a passphrase: /k/id"
        );
    }

    #[test]
    fn a_missing_password_stays_recognisable_through_a_jump_host() {
        let missing = anyhow::Error::from(NoPassword::new("root@10.0.0.5:22", "db"));
        let hop = at_hop(missing, String::from("connecting via 'bastion' failed"));
        assert_eq!(no_password(&hop), Some("root@10.0.0.5:22"));
        assert_eq!(waiting_login(&hop), Some("root@10.0.0.5:22"));
        assert!(!is_refused(&hop));
        // Frontends cut at the first ':'; the reason must come before it.
        assert!(!NoPassword::new("k", "db").to_string().contains(':'));
    }

    #[test]
    fn a_password_is_remembered_per_login_and_bastion() {
        let host = |name: &str, user: &str, hostname: &str| Host {
            name: name.to_string(),
            user: user.to_string(),
            hostname: hostname.to_string(),
            ..Host::default()
        };
        let db = host("db", "root", "10.0.0.5");
        let via_a = [host("a", "ops", "a.example.com")];
        let via_b = [host("b", "ops", "b.example.com")];
        assert_ne!(login_key(&db, &via_a), login_key(&db, &via_b));
        assert_ne!(login_key(&db, &[]), login_key(&db, &via_a));
        assert_ne!(
            login_key(&db, &[]),
            login_key(&host("db", "admin", "10.0.0.5"), &[])
        );
    }

    fn prompt(text: &str, echo: bool) -> russh::client::Prompt {
        russh::client::Prompt {
            prompt: text.to_string(),
            echo,
        }
    }

    #[test]
    fn the_password_answers_one_hidden_prompt_only() {
        let (mut password, mut code) = (Some("secret"), false);
        let hidden = [prompt("Password: ", false)];
        assert_eq!(kbd_answers(&hidden, &mut password, &mut code), ["secret"]);
        // A second ask (a retry) must not get it again.
        assert_eq!(kbd_answers(&hidden, &mut password, &mut code), [""]);
        assert!(!code);
    }

    #[test]
    fn other_prompts_are_answered_blank() {
        let (mut password, mut code) = (Some("secret"), false);
        assert!(kbd_answers(&[], &mut password, &mut code).is_empty());
        assert_eq!(
            kbd_answers(&[prompt("Username: ", true)], &mut password, &mut code),
            [""]
        );
        let two = [prompt("Password: ", false), prompt("Code: ", false)];
        assert_eq!(kbd_answers(&two, &mut password, &mut code), ["", ""]);
        assert_eq!(
            password,
            Some("secret"),
            "never spent on a prompt it did not answer"
        );
    }

    #[test]
    fn a_code_prompt_never_gets_the_password() {
        for text in [
            "Verification code: ",
            "Enter PASSCODE:",
            "One-time password (OATH) for `root':",
            "Duo two-factor login",
            "PIN: ",
        ] {
            let (mut password, mut code) = (Some("secret"), false);
            assert_eq!(
                kbd_answers(&[prompt(text, false)], &mut password, &mut code),
                [""],
                "{text}"
            );
            assert!(code, "{text}");
        }
        // Localised password prompts still get it.
        for text in [
            "Password: ",
            "Passwort: ",
            "Contraseña: ",
            "(root@udm) Password:",
            "(vscode@gitcode) Password:",
            "Password for duo-admin@host:",
        ] {
            let (mut password, mut code) = (Some("secret"), false);
            assert_eq!(
                kbd_answers(&[prompt(text, false)], &mut password, &mut code),
                ["secret"],
                "{text}"
            );
        }
    }

    #[test]
    fn a_locked_key_is_found_under_added_context() {
        let e = anyhow::Error::from(PassphraseRequired::new(String::from("/k/id"), "k"))
            .context("SFTP SSH connect");
        assert_eq!(passphrase_required(&e), Some("/k/id"));
    }

    // Some agents sign with SHA-1 whatever is asked for, so an RSA key the server
    // turned down through the agent is still worth offering from its file.
    #[tokio::test]
    async fn only_a_non_rsa_key_refused_through_the_agent_is_skipped() {
        use russh::keys::ssh_key::LineEnding;
        let dir = tempfile::tempdir().expect("tempdir");
        let mut rng = russh::keys::key::safe_rng();
        let rsa = russh::keys::ssh_key::private::RsaKeypair::random(&mut rng, 2048).expect("rsa");
        let keys = [
            ("id_rsa", russh::keys::PrivateKey::from(rsa)),
            (
                "id_ed25519",
                russh::keys::PrivateKey::random(&mut rng, russh::keys::Algorithm::Ed25519)
                    .expect("ed25519"),
            ),
        ];
        for (name, key) in &keys {
            let path = dir.path().join(name);
            key.write_openssh_file(&path, LineEnding::LF)
                .expect("write key");
            let path = path.to_string_lossy().into_owned();
            let turned_down = [key.public_key().clone()];
            let skipped = refused_before(&path, &turned_down).await;
            assert_eq!(skipped, *name == "id_ed25519", "{name}");
        }
    }

    #[test]
    fn only_an_unreachable_host_hints_at_local_network_access() {
        use std::io::{Error, ErrorKind};

        let others = [
            Error::from(ErrorKind::ConnectionRefused),
            Error::from(ErrorKind::TimedOut),
            Error::from(ErrorKind::NetworkUnreachable),
        ];
        for e in others {
            assert_eq!(dial_error(&e), e.to_string());
        }
        let (_, link) = known_hosts_handler(&Host::default(), false);
        let refused = link.connect_error(russh::Error::IO(ErrorKind::ConnectionRefused.into()));
        assert_eq!(
            format!("{refused:#}"),
            format!(
                "SSH connection failed: {}",
                Error::from(ErrorKind::ConnectionRefused)
            )
        );

        #[cfg(target_os = "macos")]
        {
            // EHOSTUNREACH, what a Local Network refusal comes back as.
            let e = Error::from_raw_os_error(65);
            let message = dial_error(&e);
            assert!(
                message.starts_with("No route to host (os error 65); "),
                "{message}"
            );
            assert!(
                message.contains("Privacy & Security > Local Network"),
                "{message}"
            );
            assert!(message.contains("this app or its terminal"), "{message}");
            assert!(!message.contains(':'), "{message}");
            // EHOSTDOWN is a host that is off.
            let down = Error::from_raw_os_error(64);
            assert_eq!(dial_error(&down), down.to_string());
            // The dial of an SSH connection says it too, after the reason.
            let e = link.connect_error(russh::Error::IO(e));
            assert_eq!(
                format!("{e:#}"),
                format!("SSH connection failed: {message}")
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            let e = Error::from(ErrorKind::HostUnreachable);
            assert_eq!(dial_error(&e), e.to_string());
        }
    }

    #[test]
    fn a_missing_algorithm_is_named_with_the_offer() {
        let offer = [String::from("aes128-gcm@openssh.com")];
        assert_eq!(
            no_common_algorithm(&russh::AlgorithmKind::Cipher, &offer),
            "SSH connection failed: no common cipher; the server offers aes128-gcm@openssh.com"
        );
        assert_eq!(
            no_common_algorithm(&russh::AlgorithmKind::Mac, &[]),
            "SSH connection failed: no common MAC"
        );
    }

    #[test]
    fn only_servers_before_rsa_sha2_sign_with_sha1() {
        for old in [
            "SSH-2.0-OpenSSH_5.3",
            "SSH-2.0-OpenSSH_6.6.1p1 Ubuntu-2ubuntu2",
            "SSH-2.0-OpenSSH_7.1",
            "SSH-2.0-dropbear_2019.78",
        ] {
            assert_eq!(rsa_sigs(old.as_bytes()), RsaSigs::Sha1, "{old}");
        }
        for new in [
            "SSH-2.0-OpenSSH_7.2",
            "SSH-2.0-OpenSSH_7.5",
            "SSH-2.0-OpenSSH_10.2p1 Ubuntu-2ubuntu3.6",
            "SSH-2.0-dropbear_2020.79",
            "SSH-2.0-Go",
            "SSH-2.0-OpenSSH_for_Windows_9.5",
            "garbage",
        ] {
            assert_eq!(rsa_sigs(new.as_bytes()), RsaSigs::AsListed, "{new}");
        }
        assert_eq!(
            rsa_sigs(b"SSH-2.0-OpenSSH_7.4p1 Debian-10+deb9u7"),
            RsaSigs::Sha2Unlisted
        );
    }

    #[test]
    fn the_quoted_offer_is_cleaned_and_bounded() {
        let offer = [
            "ecdh-sha2-nistp256",
            "ext-info-s",
            "kex-strict-s-v00@openssh.com",
            "odd\u{1b}[2J: name\n",
        ]
        .map(String::from);
        let message = no_common_algorithm(&russh::AlgorithmKind::Kex, &offer);
        assert_eq!(
            message,
            "SSH connection failed: no common key exchange method; \
             the server offers ecdh-sha2-nistp256, odd2Jname"
        );

        let long = vec!["x".repeat(100); 10];
        let message = no_common_algorithm(&russh::AlgorithmKind::Key, &long);
        assert!(message.len() < 100 + OFFER_MAX, "{message}");
        assert!(message.ends_with(", ..."), "{message}");
        assert_eq!(message.matches(':').count(), 1, "{message}");

        let huge = [String::from("y").repeat(10_000)];
        let message = no_common_algorithm(&russh::AlgorithmKind::Key, &huge);
        assert!(message.len() < 100 + OFFER_MAX, "{message}");
    }
}
