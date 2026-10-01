//! Algorithm negotiation, end to end, against an in-process SSH server that
//! offers only what a test names.

use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::{Arc, Once};
use std::time::Duration;

use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{cipher, compression, kex, mac, Channel, ChannelId, Preferred};
use tokio::net::TcpListener;

use omnyssh_core::ssh::client::Host;
use omnyssh_core::ssh::session::SshSession;

const PASSWORD: &str = "algorithms-test";

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

/// Takes the password and answers every command with `ok`.
#[derive(Clone)]
struct Server;

impl server::Handler for Server {
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

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, &b"ok\n"[..])?;
        session.exit_status_request(channel, 0)?;
        session.eof(channel)?;
        session.close(channel)
    }
}

/// Serves on a loopback port, offering only `preferred`.
async fn serve(preferred: Preferred) -> SocketAddr {
    let key = PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519)
        .expect("host key");
    let config = Arc::new(server::Config {
        keys: vec![key],
        preferred,
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let _ = server::run_stream(Arc::clone(&config), socket, Server).await;
        }
    });
    addr
}

fn host(name: &str, addr: SocketAddr) -> Host {
    Host {
        name: name.to_string(),
        hostname: addr.ip().to_string(),
        port: addr.port(),
        user: name.to_string(),
        password: Some(PASSWORD.to_string()),
        ..Host::default()
    }
}

/// Logs in to a server offering only `preferred` and runs a command there.
async fn runs_a_command(name: &str, preferred: Preferred) {
    isolate_home();
    let addr = serve(preferred).await;
    let session = SshSession::connect(&host(name, addr)).await.expect("login");
    let output = session.run_command("true").await.expect("command");
    assert_eq!(output, "ok\n");
}

/// Network gear such as Cisco NX-OS pairs aes-ctr with hmac-sha1 only.
#[tokio::test]
async fn a_server_with_only_hmac_sha1_connects() {
    let preferred = Preferred {
        cipher: Cow::Borrowed(&[cipher::AES_128_CTR]),
        mac: Cow::Borrowed(&[mac::HMAC_SHA1]),
        ..Preferred::DEFAULT
    };
    runs_a_command("hmac-sha1", preferred).await;
}

#[tokio::test]
async fn a_server_that_always_compresses_connects() {
    let preferred = Preferred {
        compression: Cow::Borrowed(&[compression::ZLIB]),
        ..Preferred::DEFAULT
    };
    runs_a_command("zlib", preferred).await;
}

/// Cisco RoomOS takes no key exchange but NIST ECDH, and strict KEX as OpenSSH
/// 9.6 and later do it.
#[tokio::test]
async fn a_server_with_only_nist_ecdh_connects() {
    for curve in [
        kex::ECDH_SHA2_NISTP256,
        kex::ECDH_SHA2_NISTP384,
        kex::ECDH_SHA2_NISTP521,
    ] {
        let preferred = Preferred {
            kex: Cow::Owned(vec![
                curve,
                kex::EXTENSION_SUPPORT_AS_SERVER,
                kex::EXTENSION_OPENSSH_STRICT_KEX_AS_SERVER,
            ]),
            ..Preferred::DEFAULT
        };
        runs_a_command(curve.as_ref(), preferred).await;
    }
}

/// Nor any cipher but aes128-gcm.
#[tokio::test]
async fn a_server_with_only_aes128_gcm_connects() {
    let preferred = Preferred {
        cipher: Cow::Borrowed(&[cipher::AES_128_GCM]),
        ..Preferred::DEFAULT
    };
    runs_a_command("aes128-gcm", preferred).await;
}

/// Logs in to a server offering only `preferred`, which is none of ours, and
/// returns why it failed.
async fn fails(name: &str, preferred: Preferred) -> String {
    isolate_home();
    let addr = serve(preferred).await;
    let e = SshSession::connect(&host(name, addr))
        .await
        .err()
        .expect("no algorithm in common");
    format!("{e:#}")
}

/// A server past OpenSSH 9.6 lists strict KEX markers along with its methods:
/// the error names the methods, not a marker.
#[tokio::test]
async fn a_key_exchange_mismatch_names_the_servers_methods() {
    let preferred = Preferred {
        kex: Cow::Borrowed(&[
            kex::DH_G1_SHA1,
            kex::EXTENSION_SUPPORT_AS_SERVER,
            kex::EXTENSION_OPENSSH_STRICT_KEX_AS_SERVER,
        ]),
        ..Preferred::DEFAULT
    };
    assert_eq!(
        fails("kex-mismatch", preferred).await,
        "SSH connection failed: no common key exchange method; \
         the server offers diffie-hellman-group1-sha1"
    );
}

#[tokio::test]
async fn a_cipher_mismatch_names_the_servers_ciphers() {
    let preferred = Preferred {
        cipher: Cow::Borrowed(&[cipher::AES_128_CBC]),
        ..Preferred::DEFAULT
    };
    assert_eq!(
        fails("cipher-mismatch", preferred).await,
        "SSH connection failed: no common cipher; the server offers aes128-cbc"
    );
}
