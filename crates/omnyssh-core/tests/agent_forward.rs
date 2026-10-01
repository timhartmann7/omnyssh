//! Agent forwarding, end to end, against an in-process SSH server that opens an
//! agent channel whether or not the client offered one — the way a server after
//! the keys would.

#![cfg(unix)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId, ChannelMsg};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use omnyssh_core::event::CoreEvent;
use omnyssh_core::ssh::client::Host;
use omnyssh_core::ssh::pty::PtyManager;
use omnyssh_core::ssh::session::SshSession;

const PASSWORD: &str = "agent-test";

/// Keeps trust-on-first-use off the real `~/.ssh`, and starts an agent holding
/// one key for the server to ask about.
fn isolate_home() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        use std::process::Command;
        let home = tempfile::tempdir().expect("tempdir").keep();
        std::env::set_var("HOME", &home);
        let socket = home.join("agent.sock");
        let key = home.join("agent_key");
        let started = Command::new("ssh-agent")
            .arg("-a")
            .arg(&socket)
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
            .arg(&key)
            .env("SSH_AUTH_SOCK", &socket)
            .status()
            .expect("these tests need ssh-add on PATH");
        assert!(added.success());
        std::env::set_var("SSH_AUTH_SOCK", &socket);
    });
}

// ---------------------------------------------------------------------------
// SSH server
// ---------------------------------------------------------------------------

/// What became of the server's "list your keys" on an agent channel.
#[derive(Debug, PartialEq)]
enum Reply {
    /// An agent answered: its message type and key count.
    Answered(u8, u32),
    /// The client closed the channel without an answer.
    Closed,
    /// Nothing came back at all — neither refused nor answered.
    Silent,
}

#[derive(Clone)]
struct Server {
    /// Whether the client offered its agent.
    offered: Arc<AtomicBool>,
    replies: mpsc::UnboundedSender<Reply>,
}

impl server::Handler for Server {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == PASSWORD {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    // The agent's key is not the way in; the password is.
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

    async fn agent_request(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.offered.store(true, Ordering::SeqCst);
        Ok(true)
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, &b"logged-in\r\n"[..])?;
        self.ask_agent(session);
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let mut echo = b"echo:".to_vec();
        echo.extend_from_slice(data);
        session.data(channel, echo)
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.ask_agent(session);
        session.exit_status_request(channel, 0)?;
        session.eof(channel)?;
        session.close(channel)
    }
}

impl Server {
    /// Opens an agent channel back to the client and asks it for its keys.
    /// Spawned: the session is busy running this handler until it returns.
    fn ask_agent(&self, session: &Session) {
        let handle = session.handle();
        let replies = self.replies.clone();
        tokio::spawn(async move {
            let Ok(mut channel) = handle.channel_open_agent().await else {
                let _ = replies.send(Reply::Closed);
                return;
            };
            // SSH_AGENTC_REQUEST_IDENTITIES, length-prefixed. A channel closed
            // under it may already turn this down.
            let _ = channel.data(&[0u8, 0, 0, 1, 11][..]).await;
            let mut answer = Vec::new();
            let reply = loop {
                if answer.len() >= 9 {
                    let count = u32::from_be_bytes([answer[5], answer[6], answer[7], answer[8]]);
                    break Reply::Answered(answer[4], count);
                }
                match tokio::time::timeout(Duration::from_secs(5), channel.wait()).await {
                    Ok(Some(ChannelMsg::Data { data })) => answer.extend_from_slice(&data),
                    Ok(Some(ChannelMsg::Eof | ChannelMsg::Close) | None) => break Reply::Closed,
                    Ok(Some(_)) => {}
                    Err(_) => break Reply::Silent,
                }
            };
            let _ = replies.send(reply);
        });
    }
}

/// Serves a fresh server on a loopback port; returns where, whether the client
/// offered its agent, and what each agent channel got back.
async fn serve() -> (SocketAddr, Arc<AtomicBool>, mpsc::UnboundedReceiver<Reply>) {
    let (replies, received) = mpsc::unbounded_channel();
    let server = Server {
        offered: Arc::new(AtomicBool::new(false)),
        replies,
    };
    let offered = Arc::clone(&server.offered);
    let config = Arc::new(server::Config {
        keys: vec![
            PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519)
                .expect("host key"),
        ],
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let _ = server::run_stream(Arc::clone(&config), socket, server.clone()).await;
        }
    });
    (addr, offered, received)
}

fn host(name: &str, addr: SocketAddr, forward_agent: bool) -> Host {
    Host {
        name: name.to_string(),
        hostname: addr.ip().to_string(),
        port: addr.port(),
        user: name.to_string(),
        password: Some(PASSWORD.to_string()),
        forward_agent,
        ..Host::default()
    }
}

async fn next_reply(received: &mut mpsc::UnboundedReceiver<Reply>) -> Reply {
    tokio::time::timeout(Duration::from_secs(20), received.recv())
        .await
        .expect("the server never asked the agent")
        .expect("server gone")
}

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

/// Opens a terminal to `host`; the manager is returned so the session lives until
/// the test is done with it.
async fn terminal(host: &Host) -> (PtyManager, u64) {
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let mut pty = PtyManager::new();
    let id = pty.open(host, 80, 24, tx).expect("open");
    assert!(screen_contains(&pty, id, "logged-in").await, "no shell");
    (pty, id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A host set to forward the agent gets it in a terminal: the server can list
/// the local agent's key through the channel it opens.
#[tokio::test]
async fn a_terminal_lends_the_agent_to_a_host_set_to_forward_it() {
    isolate_home();
    let (addr, offered, mut replies) = serve().await;

    let _pty = terminal(&host("lend", addr, true)).await;

    // SSH_AGENT_IDENTITIES_ANSWER, with the agent's one key.
    assert_eq!(next_reply(&mut replies).await, Reply::Answered(12, 1));
    assert!(offered.load(Ordering::SeqCst), "the terminal never offered");
}

/// A host not set to forward it is neither offered the agent nor let into it
/// when it opens an agent channel anyway — and the refusal costs it nothing else:
/// the terminal carries on.
#[tokio::test]
async fn a_terminal_keeps_the_agent_from_any_other_host() {
    isolate_home();
    let (addr, offered, mut replies) = serve().await;

    let (mut pty, id) = terminal(&host("keep", addr, false)).await;

    assert_eq!(next_reply(&mut replies).await, Reply::Closed);
    assert!(!offered.load(Ordering::SeqCst));
    // The server writes to the shell after every write it gets back.
    pty.write(id, b"still-here").expect("write");
    assert!(
        screen_contains(&pty, id, "echo:still-here").await,
        "the refusal took the terminal down"
    );
}

/// Only terminals lend the agent: the monitoring connection to a host set to
/// forward it refuses an agent channel all the same.
#[tokio::test]
async fn a_monitoring_connection_never_lends_the_agent() {
    isolate_home();
    let (addr, offered, mut replies) = serve().await;

    let session = SshSession::connect(&host("poll", addr, true))
        .await
        .expect("login");
    session.run_command("true").await.expect("command");

    assert_eq!(next_reply(&mut replies).await, Reply::Closed);
    assert!(!offered.load(Ordering::SeqCst));
    session
        .run_command("true")
        .await
        .expect("the refusal took the connection down");
    assert_eq!(next_reply(&mut replies).await, Reply::Closed);
}
