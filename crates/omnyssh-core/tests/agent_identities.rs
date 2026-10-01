//! Which agent keys a login offers, against a server that counts them: an agent
//! holding many keys used up the server's `MaxAuthTries` before the host's own
//! key came up.
#![cfg(unix)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::Channel;
use tokio::net::TcpListener;

use omnyssh_core::ssh::client::Host;
use omnyssh_core::ssh::session::SshSession;

/// The agent's keys, in the order it lists them. The server takes only the last.
const KEYS: [&str; 4] = ["work", "github", "old-vps", "app01"];

/// A home of its own, and an agent holding [`KEYS`] whose private files are gone,
/// as with a password manager's agent. Returns the home.
fn setup() -> &'static Path {
    static HOME: OnceLock<(PathBuf, Mutex<Child>)> = OnceLock::new();
    &HOME
        .get_or_init(|| {
            let home = tempfile::tempdir().expect("tempdir").keep();
            std::env::set_var("HOME", &home);
            std::env::set_var("USERPROFILE", &home);
            let socket = home.join("agent.sock");
            // The agent lives as long as its command, which waits on a pipe that
            // closes when this process exits.
            let agent = Command::new("ssh-agent")
                .arg("-a")
                .arg(&socket)
                .args(["sh", "-c", "read -r _"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .expect("these tests need ssh-agent on PATH");
            let started = Instant::now();
            while !socket.exists() {
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "ssh-agent did not start"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            for name in KEYS {
                let key = home.join(name);
                let made = Command::new("ssh-keygen")
                    .args(["-q", "-t", "ed25519", "-N", ""])
                    .arg("-f")
                    .arg(&key)
                    .status()
                    .expect("these tests need ssh-keygen on PATH");
                assert!(made.success());
                let added = Command::new("ssh-add")
                    .arg("-q")
                    .arg(&key)
                    .env("SSH_AUTH_SOCK", &socket)
                    .status()
                    .expect("these tests need ssh-add on PATH");
                assert!(added.success());
            }
            // An unencrypted copy of the app01 key is also the default key, which a
            // login could offer again from its file after the agent's copy.
            let ssh = home.join(".ssh");
            std::fs::create_dir(&ssh).expect("create .ssh");
            std::fs::copy(home.join("app01"), ssh.join("id_ed25519")).expect("copy the key");
            // The app01 key keeps its private file, encrypted, for the test that reads
            // the public half out of it.
            let reencrypted = Command::new("ssh-keygen")
                .args(["-q", "-p", "-P", "", "-N", "secret", "-f"])
                .arg(home.join("app01"))
                .status()
                .expect("ssh-keygen");
            assert!(reencrypted.success());
            for name in &KEYS[..3] {
                std::fs::remove_file(home.join(name)).expect("drop the private key");
            }
            std::env::set_var("SSH_AUTH_SOCK", &socket);
            (home, Mutex::new(agent))
        })
        .0
}

fn public(name: &str) -> PublicKey {
    russh::keys::load_public_key(setup().join(format!("{name}.pub"))).expect("public key")
}

/// Accepts `accepted` and turns every other key down, recording each offer.
#[derive(Clone)]
struct Server {
    accepted: Option<PublicKey>,
    offers: Arc<Mutex<Vec<PublicKey>>>,
}

impl server::Handler for Server {
    type Error = russh::Error;

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        self.offers.lock().unwrap().push(key.clone());
        Ok(self.verdict(key))
    }

    async fn auth_publickey(&mut self, _user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        Ok(self.verdict(key))
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
}

impl Server {
    fn verdict(&self, key: &PublicKey) -> Auth {
        if self.accepted.as_ref() == Some(key) {
            Auth::Accept
        } else {
            Auth::reject()
        }
    }
}

/// Serves on a loopback port; returns the address and the offers it saw.
async fn serve(accepted: Option<PublicKey>) -> (SocketAddr, Arc<Mutex<Vec<PublicKey>>>) {
    let offers = Arc::new(Mutex::new(Vec::new()));
    let server = Server {
        accepted,
        offers: Arc::clone(&offers),
    };
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
    (addr, offers)
}

fn host(name: &str, addr: SocketAddr, identity_file: &str, identities_only: bool) -> Host {
    Host {
        name: name.to_string(),
        hostname: addr.ip().to_string(),
        port: addr.port(),
        user: name.to_string(),
        identity_file: Some(setup().join(identity_file).to_string_lossy().into_owned()),
        identities_only,
        ..Host::default()
    }
}

fn names(offers: &Mutex<Vec<PublicKey>>) -> Vec<&'static str> {
    offers
        .lock()
        .unwrap()
        .iter()
        .map(|key| {
            KEYS.into_iter()
                .find(|name| public(name) == *key)
                .unwrap_or("?")
        })
        .collect()
}

/// The report: `IdentityFile` names the `.pub` of a key only the agent holds.
#[tokio::test]
async fn the_identity_files_agent_key_goes_first() {
    setup();
    let (addr, offers) = serve(Some(public("app01"))).await;
    SshSession::connect(&host("pub-file", addr, "app01.pub", false))
        .await
        .expect("the matching agent key logs in");
    assert_eq!(names(&offers), ["app01"]);
}

/// An encrypted private key with no `.pub` beside it still names its agent key.
#[tokio::test]
async fn an_encrypted_identity_file_is_matched_without_its_passphrase() {
    let home = setup();
    let copy = home.join("app01-copy");
    std::fs::copy(home.join("app01"), &copy).expect("copy the key");
    let (addr, offers) = serve(Some(public("app01"))).await;
    SshSession::connect(&host("encrypted", addr, "app01-copy", false))
        .await
        .expect("the matching agent key logs in");
    assert_eq!(names(&offers), ["app01"]);
}

/// Without IdentitiesOnly the other agent keys still follow, in the agent's order.
#[tokio::test]
async fn other_agent_keys_follow_the_matching_one() {
    setup();
    let (addr, offers) = serve(None).await;
    let _ = SshSession::connect(&host("all-keys", addr, "app01.pub", false)).await;
    assert_eq!(names(&offers), ["app01", "work", "github", "old-vps"]);
}

/// With IdentitiesOnly a refused key is the last one offered: no other agent key,
/// no second go from the identity file, no default key.
#[tokio::test]
async fn identities_only_offers_nothing_else() {
    setup();
    let (addr, offers) = serve(None).await;
    let result = SshSession::connect(&host("only", addr, "app01", true)).await;
    assert!(result.is_err());
    assert_eq!(names(&offers), ["app01"]);
}

/// A key the server turned down through the agent is not offered again from
/// its file, as the identity file or as a default key: each refusal counts
/// towards the server's MaxAuthTries.
#[tokio::test]
async fn a_refused_agent_key_is_not_offered_again_from_its_file() {
    setup();
    let (addr, offers) = serve(None).await;
    let _ = SshSession::connect(&host("once", addr, ".ssh/id_ed25519", false)).await;
    assert_eq!(names(&offers), ["app01", "work", "github", "old-vps"]);
}
