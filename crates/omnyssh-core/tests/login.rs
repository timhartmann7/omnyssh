//! Logging in, end to end, against an in-process SSH server that can switch the
//! password method and keyboard-interactive on and off.
//!
//! On unix every test runs with an SSH agent whose one key the server accepts
//! but the agent refuses to sign with, so each login also goes through the path
//! where a refused signature must not leave the connection stuck.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Response, Session};
use russh::{Channel, ChannelId};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use omnyssh_core::event::CoreEvent;
use omnyssh_core::ssh::client::Host;
use omnyssh_core::ssh::pty::PtyManager;
use omnyssh_core::ssh::session::SshSession;

const PASSWORD: &str = "login-test";

/// Keeps trust-on-first-use off the real `~/.ssh`, and starts the agent.
fn isolate_home() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let home = tempfile::tempdir().expect("tempdir").keep();
        std::env::set_var("HOME", &home);
        std::env::set_var("USERPROFILE", &home);
        std::env::remove_var("SSH_AUTH_SOCK");
        #[cfg(unix)]
        refusing_agent(&home);
    });
}

/// An ssh-agent holding one key it asks confirmation for, with nothing to ask
/// with — so it turns every signature down.
#[cfg(unix)]
fn refusing_agent(home: &std::path::Path) {
    use std::process::Command;
    let socket = home.join("agent.sock");
    let key = home.join("agent_key");
    let started = Command::new("ssh-agent")
        .arg("-a")
        .arg(&socket)
        .env_remove("SSH_ASKPASS")
        .env_remove("DISPLAY")
        .output()
        .expect("these tests need ssh-agent on PATH");
    assert!(started.status.success(), "ssh-agent failed to start");
    let keygen = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", ""])
        .arg("-f")
        .arg(&key)
        .status()
        .expect("these tests need ssh-keygen on PATH");
    assert!(keygen.success());
    let added = Command::new("ssh-add")
        .arg("-q")
        .arg("-c")
        .arg(&key)
        .env("SSH_AUTH_SOCK", &socket)
        .status()
        .expect("these tests need ssh-add on PATH");
    assert!(added.success());
    std::env::set_var("SSH_AUTH_SOCK", &socket);
}

// ---------------------------------------------------------------------------
// SSH server
// ---------------------------------------------------------------------------

/// What the server takes, and what it saw.
#[derive(Clone, Default)]
struct Server {
    password_method: bool,
    /// Keyboard-interactive, with this prompt; `None` for not offered.
    kbd_prompt: Option<&'static str>,
    connections: Arc<AtomicUsize>,
    /// Password-method requests, whether the method is on or not.
    password_tries: Arc<AtomicUsize>,
    /// Every keyboard-interactive answer it was sent.
    kbd_answers: Arc<Mutex<Vec<String>>>,
}

impl server::Handler for Server {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.password_tries.fetch_add(1, Ordering::SeqCst);
        Ok(if self.password_method && password == PASSWORD {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _user: &str,
        _submethods: &str,
        response: Option<Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        let Some(prompt) = self.kbd_prompt else {
            return Ok(Auth::reject());
        };
        let Some(response) = response else {
            return Ok(Auth::Partial {
                name: "".into(),
                instructions: "".into(),
                prompts: vec![(prompt.into(), false)].into(),
            });
        };
        let answers: Vec<String> = response
            .map(|a| String::from_utf8_lossy(&a).into_owned())
            .collect();
        let accepted = answers == [PASSWORD];
        self.kbd_answers.lock().unwrap().extend(answers);
        Ok(if accepted {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    // Every key is turned down (the agent's only after it was asked to sign).
    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::reject())
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, &b"logged-in\r\n"[..])
    }
}

/// A fresh host key for a test server.
fn host_key() -> PrivateKey {
    PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519).expect("host key")
}

/// Serves `server` on a loopback port.
async fn serve(server: Server) -> SocketAddr {
    let config = Arc::new(server::Config {
        keys: vec![host_key()],
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            server.connections.fetch_add(1, Ordering::SeqCst);
            let _ = server::run_stream(Arc::clone(&config), socket, server.clone()).await;
        }
    });
    addr
}

/// Named for the test, which is also its user: what a login learns is kept per
/// user@host:port, and another test's server may get the same port later.
fn host(name: &str, addr: SocketAddr, password: Option<&str>) -> Host {
    Host {
        name: name.to_string(),
        hostname: addr.ip().to_string(),
        port: addr.port(),
        user: name.to_string(),
        password: password.map(str::to_string),
        ..Host::default()
    }
}

// ---------------------------------------------------------------------------
// Saved passwords
// ---------------------------------------------------------------------------

/// A server that turns the password method off (a UniFi console) takes the
/// saved password by keyboard-interactive, and the next login no longer tries
/// the password method first.
#[tokio::test]
async fn a_saved_password_gets_in_by_keyboard_interactive() {
    isolate_home();
    let server = Server {
        kbd_prompt: Some("Password: "),
        ..Server::default()
    };
    let addr = serve(server.clone()).await;
    let udm = host("udm", addr, Some(PASSWORD));

    SshSession::connect(&udm)
        .await
        .expect("first login")
        .disconnect()
        .await;
    assert_eq!(server.password_tries.load(Ordering::SeqCst), 1);

    SshSession::connect(&udm).await.expect("second login");
    assert_eq!(
        server.password_tries.load(Ordering::SeqCst),
        1,
        "the method that works is remembered"
    );
}

/// A refused saved password is sent once per login. Finding out that the
/// server has no keyboard-interactive costs one extra connection, once.
#[tokio::test]
async fn a_refused_password_is_sent_once_per_login() {
    isolate_home();
    let server = Server {
        password_method: true,
        ..Server::default()
    };
    let addr = serve(server.clone()).await;
    let typo = host("typo", addr, Some("wrong"));

    let e = SshSession::connect(&typo).await.err().expect("refused");
    assert!(format!("{e:#}").contains("authentication failed"), "{e:#}");
    assert_eq!(server.password_tries.load(Ordering::SeqCst), 1);
    assert_eq!(server.connections.load(Ordering::SeqCst), 2);

    SshSession::connect(&typo)
        .await
        .err()
        .expect("refused again");
    assert_eq!(server.password_tries.load(Ordering::SeqCst), 2);
    assert_eq!(server.connections.load(Ordering::SeqCst), 3);
}

/// A prompt for a one-time code is not answered with the password.
#[tokio::test]
async fn a_code_prompt_never_gets_the_password() {
    isolate_home();
    let server = Server {
        kbd_prompt: Some("Verification code: "),
        ..Server::default()
    };
    let addr = serve(server.clone()).await;

    let e = SshSession::connect(&host("otp", addr, Some(PASSWORD)))
        .await
        .err()
        .expect("no way in");
    assert!(format!("{e:#}").contains("verification code"), "{e:#}");
    assert!(
        !server
            .kbd_answers
            .lock()
            .unwrap()
            .iter()
            .any(|a| a == PASSWORD),
        "the password went to a code prompt"
    );
}

/// No key and no password: the error says so, with the hint before any ':'
/// (frontends cut there).
#[tokio::test]
async fn a_login_with_nothing_to_try_says_what_is_missing() {
    isolate_home();
    let addr = serve(Server {
        password_method: true,
        ..Server::default()
    })
    .await;

    let e = SshSession::connect(&host("bare", addr, None))
        .await
        .err()
        .expect("no way in");
    let message = e.to_string();
    let head = message.split(':').next().unwrap_or_default();
    assert!(head.contains("no password is saved"), "{message}");
}

/// A server whose only method is one OmnySSH does not have (Kerberos's
/// gssapi-with-mic) says so once it turns a key down, instead of being sent a
/// password no one could get in with.
#[tokio::test]
async fn a_server_with_no_method_omnyssh_has_says_so() {
    isolate_home();
    let config = Arc::new(server::Config {
        keys: vec![host_key()],
        methods: russh::MethodSet::from(&[russh::MethodKind::GssapiWithMic][..]),
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let _ = server::run_stream(Arc::clone(&config), socket, Server::default()).await;
        }
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let key = dir.path().join("id_ed25519");
    host_key()
        .write_openssh_file(&key, russh::keys::ssh_key::LineEnding::LF)
        .expect("write key");

    let host = Host {
        identity_file: Some(key.to_string_lossy().into_owned()),
        ..host("kerberos", addr, Some(PASSWORD))
    };
    let e = SshSession::connect(&host).await.err().expect("no way in");
    assert!(
        e.to_string()
            .contains("offers no login method OmnySSH supports"),
        "{e:#}"
    );
}

// ---------------------------------------------------------------------------
// The terminal asks
// ---------------------------------------------------------------------------

async fn screen_contains(pty: &PtyManager, id: u64, text: &str) -> bool {
    for _ in 0..100 {
        if let Some(parser) = pty.parser_for(id) {
            if parser.lock().unwrap().screen().contents().contains(text) {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// A terminal asks for the password in the tab, says when one is refused, and
/// logs in with the next — on the same keyboard-interactive connection.
#[tokio::test]
async fn a_terminal_asks_for_the_password_in_the_tab() {
    isolate_home();
    let server = Server {
        kbd_prompt: Some("Password: "),
        ..Server::default()
    };
    let addr = serve(server.clone()).await;
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let mut pty = PtyManager::new();
    let id = pty
        .open(&host("term", addr, None), 80, 24, tx)
        .expect("open");
    let login = format!("term@{}'s password:", addr.ip());
    assert!(screen_contains(&pty, id, &login).await, "no prompt");

    pty.write(id, b"wrong\r").expect("write");
    assert!(
        screen_contains(&pty, id, "Permission denied, please try again.").await,
        "no retry"
    );
    pty.write(id, format!("{PASSWORD}\r").as_bytes())
        .expect("write");
    assert!(screen_contains(&pty, id, "logged-in").await, "no shell");

    assert!(
        !pty.parser_for(id)
            .expect("session")
            .lock()
            .unwrap()
            .screen()
            .contents()
            .contains(PASSWORD),
        "the password is never echoed"
    );
    assert_eq!(
        server.connections.load(Ordering::SeqCst),
        2,
        "the keys' connection, and one keyboard-interactive connection for both answers"
    );
}
