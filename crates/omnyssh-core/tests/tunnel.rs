//! Port forwarding end to end, against an in-process SSH server.
//!
//! The server takes one password and opens `direct-tcpip` channels to loopback
//! targets, so every tunnel here goes through a real handshake, authentication
//! and channel traffic without a system `sshd`. A relay between the tunnel and
//! the server stands in for the network, so a test can cut it.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::Channel;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};

use omnyssh_core::event::CoreEvent;
use omnyssh_core::ssh::client::Host;
use omnyssh_core::ssh::tunnel::{LocalForward, TunnelManager, TunnelStatus};

const PASSWORD: &str = "tunnel-test";
/// A password the server answers by dropping the connection mid-login.
const DROPS_THE_LINE: &str = "drop-the-line";

/// Keeps trust-on-first-use off the real `~/.ssh/known_hosts`, and the local
/// agent's keys out of the login.
fn isolate_home() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let home = tempfile::tempdir().expect("tempdir").keep();
        std::env::set_var("HOME", &home);
        std::env::set_var("USERPROFILE", &home);
        std::env::remove_var("SSH_AUTH_SOCK");
    });
}

// ---------------------------------------------------------------------------
// SSH server
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct TestServer {
    /// Its own session once running, to hang up on a client the way OpenSSH does.
    session: Arc<Mutex<Option<server::Handle>>>,
}

impl server::Handler for TestServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        if password == DROPS_THE_LINE {
            return Err(russh::Error::Disconnect);
        }
        Ok(if password == PASSWORD {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    /// Every key is turned down and the server then hangs up, like OpenSSH past
    /// its MaxAuthTries.
    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        if let Some(session) = self.session.lock().unwrap().clone() {
            tokio::spawn(async move {
                let reason = russh::Disconnect::ProtocolError;
                let text = String::from("Too many authentication failures");
                let _ = session.disconnect(reason, text, String::new()).await;
            });
        }
        Ok(Auth::reject())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Like sshd, refuse the channel when the target does not answer.
        let target = format!("{host_to_connect}:{port_to_connect}");
        let Ok(mut socket) = TcpStream::connect(target).await else {
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
        });
        Ok(())
    }
}

/// A fresh Ed25519 key.
fn ed25519() -> PrivateKey {
    PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519).expect("ed25519 key")
}

/// An SSH server on a loopback port, and its host key.
async fn ssh_server() -> (SocketAddr, PrivateKey) {
    let key = ed25519();
    let config = Arc::new(server::Config {
        keys: vec![key.clone()],
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let handler = TestServer::default();
            if let Ok(running) =
                server::run_stream(Arc::clone(&config), socket, handler.clone()).await
            {
                *handler.session.lock().unwrap() = Some(running.handle());
            }
        }
    });
    (addr, key)
}

/// A target service that echoes back whatever it is sent, prefixed with `tag`.
async fn echo_service(tag: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 256];
                while let Ok(n) = socket.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let reply = [tag.as_bytes(), &buf[..n]].concat();
                    if socket.write_all(&reply).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

/// A target service that sends `len` bytes to whoever connects, then waits.
async fn bulk_service(len: usize) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let _ = socket.write_all(&vec![b'x'; len]).await;
                let _ = socket.read(&mut [0u8; 1]).await;
            });
        }
    });
    port
}

// ---------------------------------------------------------------------------
// The network in between
// ---------------------------------------------------------------------------

/// A TCP relay to the SSH server that a test can take down and bring back on
/// the same port, the way a network drop looks to the client.
struct Link {
    port: u16,
    server: SocketAddr,
    dials: Arc<AtomicUsize>,
    relay: Mutex<Option<JoinHandle<()>>>,
}

impl Link {
    async fn to(server: SocketAddr) -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let link = Arc::new(Self {
            port: listener.local_addr().expect("addr").port(),
            server,
            dials: Arc::new(AtomicUsize::new(0)),
            relay: Mutex::new(None),
        });
        link.serve(listener);
        link
    }

    fn serve(&self, listener: TcpListener) {
        let (server, dials) = (self.server, Arc::clone(&self.dials));
        let relay = tokio::spawn(async move {
            // Dropped with the relay, so a cut also severs the live connections.
            let mut pipes = JoinSet::new();
            while let Ok((mut client, _)) = listener.accept().await {
                dials.fetch_add(1, Ordering::SeqCst);
                pipes.spawn(async move {
                    if let Ok(mut upstream) = TcpStream::connect(server).await {
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                    }
                });
            }
        });
        *self.relay.lock().unwrap() = Some(relay);
    }

    /// Severs every connection and stops answering.
    fn cut(&self) {
        if let Some(relay) = self.relay.lock().unwrap().take() {
            relay.abort();
        }
    }

    /// Answers again, on the same port.
    async fn restore(&self) {
        let listener = TcpListener::bind(("127.0.0.1", self.port))
            .await
            .expect("rebind the link");
        self.serve(listener);
    }

    fn dials(&self) -> usize {
        self.dials.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A loopback port nothing listens on right now.
async fn free_port() -> u16 {
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    probe.local_addr().expect("addr").port()
}

fn forward(local: u16, target: u16) -> LocalForward {
    format!("{local}:127.0.0.1:{target}")
        .parse()
        .expect("valid rule")
}

fn host(name: &str, port: u16, password: &str, forwards: Vec<LocalForward>) -> Host {
    Host {
        name: name.to_string(),
        hostname: String::from("127.0.0.1"),
        user: String::from("tester"),
        port,
        password: Some(password.to_string()),
        local_forwards: forwards,
        ..Host::default()
    }
}

/// Tunnel statuses as the manager reports them, kept apart per host so waiting
/// on one never swallows another's; errors are kept too.
struct Statuses {
    rx: mpsc::Receiver<CoreEvent>,
    held: Vec<(String, TunnelStatus)>,
    errors: Vec<String>,
}

impl Statuses {
    /// A manager and the statuses it reports.
    fn manager() -> (TunnelManager, Self) {
        let (tx, rx) = mpsc::channel(64);
        (
            TunnelManager::new(tx),
            Self {
                rx,
                held: Vec::new(),
                errors: Vec::new(),
            },
        )
    }

    /// The next status `name`'s tunnel reports.
    async fn next(&mut self, name: &str) -> TunnelStatus {
        if let Some(i) = self.held.iter().position(|(host, _)| host == name) {
            return self.held.remove(i).1;
        }
        loop {
            match self.recv().await {
                CoreEvent::TunnelStatusChanged(host, status) if host == name => return status,
                CoreEvent::TunnelStatusChanged(host, status) => self.held.push((host, status)),
                CoreEvent::Error(message) => self.errors.push(message),
                _ => {}
            }
        }
    }

    /// The next error the manager reports.
    async fn error(&mut self) -> String {
        loop {
            if !self.errors.is_empty() {
                return self.errors.remove(0);
            }
            match self.recv().await {
                CoreEvent::TunnelStatusChanged(host, status) => self.held.push((host, status)),
                CoreEvent::Error(message) => self.errors.push(message),
                _ => {}
            }
        }
    }

    async fn recv(&mut self) -> CoreEvent {
        tokio::time::timeout(Duration::from_secs(30), self.rx.recv())
            .await
            .expect("an event within 30s")
            .expect("the manager is alive")
    }

    /// Skips `name`'s statuses until one matches, and returns it.
    async fn until(&mut self, name: &str, wanted: impl Fn(&TunnelStatus) -> bool) -> TunnelStatus {
        loop {
            let status = self.next(name).await;
            if wanted(&status) {
                return status;
            }
        }
    }
}

/// Sends `message` through the local port and returns the reply.
async fn round_trip(port: u16, message: &str) -> String {
    let mut socket = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    socket.write_all(message.as_bytes()).await.expect("write");
    let mut buf = vec![0u8; 256];
    let n = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut buf))
        .await
        .expect("a reply within 10s")
        .expect("read");
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// Whether something listens on the loopback `port`. Binds the way the tunnel
/// does, so connections still in TIME_WAIT on a released port do not count.
async fn is_bound(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).await.is_err()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Several hosts at once, each with several ports — the shape asked for in #89.
#[tokio::test]
async fn several_hosts_each_forward_several_ports() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let (web, db, cache) = (
        echo_service("web:").await,
        echo_service("db:").await,
        echo_service("cache:").await,
    );
    let (web_local, db_local, cache_local) =
        (free_port().await, free_port().await, free_port().await);

    let (mut tunnels, mut statuses) = Statuses::manager();
    tunnels.start(host(
        "app",
        server.port(),
        PASSWORD,
        vec![forward(web_local, web), forward(db_local, db)],
    ));
    tunnels.start(host(
        "cache",
        server.port(),
        PASSWORD,
        vec![forward(cache_local, cache)],
    ));

    assert_eq!(statuses.next("app").await, TunnelStatus::Connecting);
    assert_eq!(statuses.next("app").await, TunnelStatus::Up);
    statuses.until("cache", |s| *s == TunnelStatus::Up).await;

    let (a, b, c) = tokio::join!(
        round_trip(web_local, "one"),
        round_trip(db_local, "two"),
        round_trip(cache_local, "three"),
    );
    assert_eq!(
        (a.as_str(), b.as_str(), c.as_str()),
        ("web:one", "db:two", "cache:three")
    );

    // Parallel connections on one port each get a channel of their own.
    let parallel: Vec<_> = (0..8)
        .map(|i| tokio::spawn(round_trip(web_local, if i % 2 == 0 { "x" } else { "y" })))
        .collect();
    for (i, reply) in parallel.into_iter().enumerate() {
        let expected = if i % 2 == 0 { "web:x" } else { "web:y" };
        assert_eq!(reply.await.expect("join"), expected);
    }
    assert!(tunnels.is_running("app") && tunnels.is_running("cache"));
}

/// A dropped connection is dialled again on its own, and the local port is
/// held the whole time so nothing else can take it.
#[tokio::test]
async fn a_dropped_connection_comes_back_with_its_ports() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let link = Link::to(server).await;
    let target = echo_service("").await;
    let local = free_port().await;

    let (mut tunnels, mut statuses) = Statuses::manager();
    tunnels.start(host(
        "flaky",
        link.port,
        PASSWORD,
        vec![forward(local, target)],
    ));
    statuses.until("flaky", |s| *s == TunnelStatus::Up).await;
    assert_eq!(round_trip(local, "before").await, "before");

    link.cut();
    let status = statuses
        .until("flaky", |s| matches!(s, TunnelStatus::Retrying(_)))
        .await;
    assert_eq!(
        status,
        TunnelStatus::Retrying(String::from("connection lost"))
    );
    assert!(
        is_bound(local).await,
        "the port is held while the link is down"
    );

    // At least one redial fails against the dead link before it comes back.
    statuses
        .until(
            "flaky",
            |s| matches!(s, TunnelStatus::Retrying(r) if r != "connection lost"),
        )
        .await;
    link.restore().await;

    statuses.until("flaky", |s| *s == TunnelStatus::Up).await;
    assert_eq!(round_trip(local, "after").await, "after");
}

/// A refused login ends the tunnel instead of being retried.
#[tokio::test]
async fn a_rejected_password_fails_without_a_retry() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let link = Link::to(server).await;
    let local = free_port().await;

    let (mut tunnels, mut statuses) = Statuses::manager();
    tunnels.start(host("typo", link.port, "wrong", vec![forward(local, 9)]));

    assert_eq!(statuses.next("typo").await, TunnelStatus::Connecting);
    match statuses.next("typo").await {
        TunnelStatus::Failed(reason) => {
            assert!(reason.contains("authentication failed"), "{reason}")
        }
        other => panic!("expected Failed, got {other:?}"),
    }

    assert_no_redial(&link).await;
    assert!(!tunnels.is_running("typo"));
    assert!(
        !is_bound(local).await,
        "a failed tunnel lets go of its ports"
    );
}

/// A link that dies during the login is a network fault, not a refusal: the
/// tunnel keeps dialling.
#[tokio::test]
async fn a_connection_lost_during_login_is_retried() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let link = Link::to(server).await;

    let (mut tunnels, mut statuses) = Statuses::manager();
    tunnels.start(host(
        "cut",
        link.port,
        DROPS_THE_LINE,
        vec![forward(free_port().await, 9)],
    ));

    assert_eq!(statuses.next("cut").await, TunnelStatus::Connecting);
    for _ in 0..2 {
        let status = statuses.next("cut").await;
        assert!(matches!(status, TunnelStatus::Retrying(_)), "{status:?}");
    }
    assert!(link.dials() >= 2);
    assert!(tunnels.is_running("cut"));
}

/// A server that turns the login down and hangs up is refusing it, not losing the
/// link — retrying is exactly what gets a client banned.
#[tokio::test]
async fn a_server_that_hangs_up_after_a_rejection_is_not_redialled() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let link = Link::to(server).await;
    let key = tempfile::NamedTempFile::new().expect("key file");
    russh::keys::encode_pkcs8_pem(&ed25519(), key.as_file()).expect("write key");

    let (mut tunnels, mut statuses) = Statuses::manager();
    let mut maxed = host(
        "maxed",
        link.port,
        "wrong",
        vec![forward(free_port().await, 9)],
    );
    maxed.identity_file = Some(key.path().to_string_lossy().into_owned());
    tunnels.start(maxed);

    assert_eq!(statuses.next("maxed").await, TunnelStatus::Connecting);
    let status = statuses.next("maxed").await;
    assert!(matches!(status, TunnelStatus::Failed(_)), "{status:?}");
    assert_no_redial(&link).await;
}

/// One login is at most two connections — the refused one, and one that finds
/// out whether keyboard-interactive takes the password — and nothing follows.
async fn assert_no_redial(link: &Link) {
    let dials = link.dials();
    assert!(dials <= 2, "{dials} connections for one login");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(link.dials(), dials, "a refused login must not be retried");
}

/// A locked key is not a refusal: the tunnel asks for the passphrase once and
/// holds its ports without redialling until the key is unlocked.
#[tokio::test]
async fn a_locked_key_waits_for_its_passphrase() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let link = Link::to(server).await;
    let local = free_port().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let key = dir.path().join("locked");
    let key_path = key.to_str().expect("utf-8 path").to_string();
    let keygen = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-f", &key_path, "-N", "sesame"])
        .status()
        .expect("this test needs ssh-keygen on PATH");
    assert!(keygen.success());

    let (mut tunnels, mut statuses) = Statuses::manager();
    let mut locked = host("locked", link.port, "wrong", vec![forward(local, 9)]);
    locked.identity_file = Some(key_path.clone());
    tunnels.start(locked);

    assert_eq!(statuses.next("locked").await, TunnelStatus::Connecting);
    match statuses.next("locked").await {
        TunnelStatus::Retrying(reason) => {
            assert!(reason.contains("requires a passphrase"), "{reason}")
        }
        other => panic!("expected Retrying, got {other:?}"),
    }
    let dials = link.dials();
    assert!(dials <= 2, "{dials} connections for one login");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(link.dials(), dials, "a locked key must not be redialled");
    assert!(
        is_bound(local).await,
        "the tunnel keeps its ports while it waits"
    );
    // Reported canonical, but in the drive form on Windows (no `\\?\` prefix).
    let canonical = std::fs::canonicalize(&key).expect("canonical path");
    let canonical = canonical.to_string_lossy();
    let expected = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
    match statuses.rx.try_recv() {
        Ok(CoreEvent::KeyPassphraseRequired {
            host_name,
            key_path,
        }) => {
            assert_eq!(host_name, "locked");
            assert_eq!(key_path, expected);
        }
        other => panic!("expected a passphrase prompt, got {other:?}"),
    }
    assert!(statuses.rx.try_recv().is_err(), "the prompt is sent once");

    omnyssh_core::ssh::identity::unlock(&key_path, "sesame").expect("unlock");
    // The server turns every key down and hangs up, so this dial is refused —
    // after one more connection for the saved password, which the hang-up cut off.
    statuses
        .until("locked", |s| matches!(s, TunnelStatus::Failed(_)))
        .await;
    let redials = link.dials() - dials;
    assert!(
        (1..=2).contains(&redials),
        "the unlock redials at once: {redials}"
    );
}

/// A host key that no longer matches `known_hosts` is refused for good.
#[tokio::test]
async fn a_changed_host_key_fails_without_a_retry() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let impostor = ed25519().public_key().clone();
    let known_hosts = std::env::home_dir()
        .expect("home")
        .join(".ssh")
        .join("known_hosts");
    russh::keys::known_hosts::learn_known_hosts_path(
        "127.0.0.1",
        server.port(),
        &impostor,
        &known_hosts,
    )
    .expect("record the old key");

    let (mut tunnels, mut statuses) = Statuses::manager();
    tunnels.start(host(
        "moved",
        server.port(),
        PASSWORD,
        vec![forward(free_port().await, 9)],
    ));

    match statuses
        .until("moved", |s| !matches!(s, TunnelStatus::Connecting))
        .await
    {
        TunnelStatus::Failed(reason) => assert!(
            reason.contains("has changed") && reason.contains(&*known_hosts.to_string_lossy()),
            "{reason}"
        ),
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// A port already taken fails the tunnel before any dial.
#[tokio::test]
async fn a_busy_port_fails_before_dialling() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let link = Link::to(server).await;
    let squatter = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let busy = squatter.local_addr().expect("addr").port();

    let (mut tunnels, mut statuses) = Statuses::manager();
    tunnels.start(host("clash", link.port, PASSWORD, vec![forward(busy, 9)]));

    match statuses.next("clash").await {
        TunnelStatus::Failed(reason) => assert!(reason.contains(&busy.to_string()), "{reason}"),
        other => panic!("expected Failed, got {other:?}"),
    }
    assert_eq!(link.dials(), 0);
}

/// Stopping reports it and frees the ports at once.
#[tokio::test]
async fn stopping_releases_the_ports() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let target = echo_service("").await;
    let local = free_port().await;

    let (mut tunnels, mut statuses) = Statuses::manager();
    tunnels.start(host(
        "once",
        server.port(),
        PASSWORD,
        vec![forward(local, target)],
    ));
    statuses.until("once", |s| *s == TunnelStatus::Up).await;

    tunnels.stop("once");
    assert_eq!(statuses.next("once").await, TunnelStatus::Stopped);
    assert!(!is_bound(local).await);
    assert!(!tunnels.is_running("once"));
}

/// The forwards of a host share one SSH connection, so a local client that stops
/// reading must not hold up the others (a paused download in a browser).
#[tokio::test]
async fn a_client_that_stops_reading_holds_up_no_other_forward() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let bulk = bulk_service(32 << 20).await;
    let echo = echo_service("echo:").await;
    let (bulk_local, echo_local) = (free_port().await, free_port().await);

    let (mut tunnels, mut statuses) = Statuses::manager();
    tunnels.start(host(
        "busy",
        server.port(),
        PASSWORD,
        vec![forward(bulk_local, bulk), forward(echo_local, echo)],
    ));
    statuses.until("busy", |s| *s == TunnelStatus::Up).await;

    let _stalled = TcpStream::connect(("127.0.0.1", bulk_local))
        .await
        .expect("connect");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(round_trip(echo_local, "ping").await, "echo:ping");
    tunnels.stop("busy");
}

/// A connection the server cannot open is closed and reported, while the tunnel
/// itself stays up for the forwards that work.
#[tokio::test]
async fn a_forward_the_server_cannot_open_is_reported() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let (working, nothing_there) = (echo_service("").await, free_port().await);
    let (good, bad) = (free_port().await, free_port().await);

    let (mut tunnels, mut statuses) = Statuses::manager();
    tunnels.start(host(
        "half",
        server.port(),
        PASSWORD,
        vec![forward(good, working), forward(bad, nothing_there)],
    ));
    statuses.until("half", |s| *s == TunnelStatus::Up).await;

    let mut socket = TcpStream::connect(("127.0.0.1", bad))
        .await
        .expect("connect");
    let mut buf = [0u8; 16];
    let read = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut buf)).await;
    assert!(
        matches!(read, Ok(Ok(0)) | Ok(Err(_))),
        "the local socket is closed"
    );
    let error = statuses.error().await;
    assert!(
        error.contains("'half'") && error.contains(&bad.to_string()),
        "{error}"
    );

    assert_eq!(round_trip(good, "still").await, "still");
    assert!(tunnels.is_running("half"));
}

/// A start right after a stop takes the ports over from the stopping run instead
/// of racing it for them, and the statuses arrive in order.
#[tokio::test]
async fn a_start_right_after_a_stop_takes_the_ports_over() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let target = echo_service("").await;
    let local = free_port().await;

    let (mut tunnels, mut statuses) = Statuses::manager();
    let rule = host(
        "again",
        server.port(),
        PASSWORD,
        vec![forward(local, target)],
    );
    tunnels.start(rule.clone());
    statuses.until("again", |s| *s == TunnelStatus::Up).await;

    tunnels.stop("again");
    assert!(!tunnels.is_running("again"));
    tunnels.start(rule);
    assert_eq!(statuses.next("again").await, TunnelStatus::Stopped);
    assert_eq!(statuses.next("again").await, TunnelStatus::Connecting);
    assert_eq!(statuses.next("again").await, TunnelStatus::Up);
    assert_eq!(round_trip(local, "back").await, "back");
}

/// An edit restarts a running tunnel on the new rules; cosmetic edits and hosts
/// that were never started are left alone.
#[tokio::test]
async fn an_edit_restarts_only_the_tunnel_it_touches() {
    isolate_home();
    let (server, _) = ssh_server().await;
    let (old_target, new_target) = (echo_service("old:").await, echo_service("new:").await);
    let (old_local, new_local) = (free_port().await, free_port().await);

    let (mut tunnels, mut statuses) = Statuses::manager();
    let before = host(
        "svc",
        server.port(),
        PASSWORD,
        vec![forward(old_local, old_target)],
    );
    tunnels.start(before.clone());
    statuses.until("svc", |s| *s == TunnelStatus::Up).await;

    let mut cosmetic = before.clone();
    cosmetic.notes = Some(String::from("renamed the notes only"));
    let idle = host(
        "idle",
        server.port(),
        PASSWORD,
        vec![forward(free_port().await, 9)],
    );
    tunnels.sync(&[cosmetic, idle]);
    assert!(!tunnels.is_running("idle"), "sync never starts a tunnel");
    assert_eq!(round_trip(old_local, "a").await, "old:a");

    let mut edited = before.clone();
    edited.local_forwards = vec![forward(new_local, new_target)];
    tunnels.sync(&[edited]);
    assert_eq!(statuses.next("svc").await, TunnelStatus::Stopped);
    assert_eq!(statuses.next("svc").await, TunnelStatus::Connecting);
    assert_eq!(statuses.next("svc").await, TunnelStatus::Up);
    assert_eq!(round_trip(new_local, "b").await, "new:b");
    assert!(
        !is_bound(old_local).await,
        "the dropped rule's port is free"
    );

    // A host that is gone takes its tunnel with it.
    tunnels.sync(&[]);
    assert_eq!(statuses.next("svc").await, TunnelStatus::Stopped);
}
