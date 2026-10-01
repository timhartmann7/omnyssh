//! The terminal's byte stream end to end, against an in-process SSH server that
//! takes input through a small window and echoes it back in small writes, the
//! way a tty does.

use std::net::SocketAddr;
use std::sync::{Arc, Once};
use std::time::Duration;

use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;

use omnyssh_core::event::CoreEvent;
use omnyssh_core::ssh::client::Host;
use omnyssh_core::ssh::pty::PtyManager;

const PASSWORD: &str = "terminal-test";

/// Keeps trust-on-first-use off the real `~/.ssh`, and the local agent's keys
/// out of the login.
fn isolate_home() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let home = tempfile::tempdir().expect("tempdir").keep();
        std::env::set_var("HOME", &home);
        std::env::set_var("USERPROFILE", &home);
        std::env::remove_var("SSH_AUTH_SOCK");
    });
}

#[derive(Clone)]
struct Echo;

impl server::Handler for Echo {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == PASSWORD {
            Auth::Accept
        } else {
            Auth::reject()
        })
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

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        for piece in data.chunks(64) {
            session.data(channel, piece.to_vec())?;
        }
        Ok(())
    }
}

/// Serves on a loopback port with a Dropbear-sized receive window.
async fn serve() -> SocketAddr {
    let key = PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519)
        .expect("host key");
    let config = Arc::new(server::Config {
        keys: vec![key],
        window_size: 24 * 1024,
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let _ = server::run_stream(Arc::clone(&config), socket, Echo).await;
        }
    });
    addr
}

/// A paste far larger than the server's window comes back whole while the
/// terminal is still writing it, and the tab can still be closed. The pump
/// waits for window space on each write; were the echo queued behind it, the
/// window update would never be read.
#[tokio::test]
async fn a_paste_larger_than_the_window_is_echoed_back() {
    isolate_home();
    let addr = serve().await;
    let host = Host {
        name: String::from("echo"),
        hostname: addr.ip().to_string(),
        port: addr.port(),
        user: String::from("tester"),
        password: Some(PASSWORD.to_string()),
        ..Host::default()
    };

    let (raw_tx, mut raw_rx) = mpsc::channel(256);
    let (tx, mut events) = mpsc::channel(64);
    let (exited_tx, mut exited) = mpsc::unbounded_channel();
    // Drain the output nudges the way a frontend does, keeping the exits.
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if let CoreEvent::PtyExited(id) = event {
                let _ = exited_tx.send(id);
            }
        }
    });

    let mut ptys = PtyManager::with_raw_output(raw_tx);
    let id = ptys.open(&host, 80, 24, tx).expect("open");
    // Both ways a paste arrives: the desktop app's 8 KiB pieces, and one write.
    let paste = vec![b'p'; 256 * 1024];
    for piece in paste.chunks(8 * 1024) {
        ptys.write(id, piece).expect("write");
    }
    ptys.write(id, &paste).expect("write");

    let mut echoed = 0;
    while echoed < 2 * paste.len() {
        let (_, bytes) = timeout(Duration::from_secs(20), raw_rx.recv())
            .await
            .unwrap_or_else(|_| panic!("echo stalled at {echoed} bytes"))
            .expect("the session is alive");
        echoed += bytes.iter().filter(|&&b| b == b'p').count();
    }

    ptys.close(id);
    let ended = timeout(Duration::from_secs(10), exited.recv())
        .await
        .expect("the session ends when closed");
    assert_eq!(ended, Some(id));
}
