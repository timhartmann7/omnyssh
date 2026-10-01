//! Host key verification against `known_hosts`, as ssh(1) does it.
//!
//! Keys live in `~/.ssh/known_hosts` on every platform, shared with OpenSSH.
//! Up to 1.1.3 the Windows build kept them in `%USERPROFILE%\ssh\known_hosts`
//! (no dot, russh-keys' own choice there); keys pinned in that file are still
//! honoured, but it is never written again.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use russh::keys::known_hosts::{known_host_keys_path, learn_known_hosts_path};
use russh::keys::{Algorithm, EcdsaCurve, HashAlg, PublicKey};

/// What `known_hosts` says about the key a server offered.
pub(crate) enum Verdict {
    /// A saved key matches.
    Known,
    /// No key is saved for the host.
    Unknown,
    /// Keys are saved for the host and the offered one is not among them.
    Changed {
        file: PathBuf,
        /// The types saved, when none is the offered key's.
        pinned: Vec<Algorithm>,
        /// The file is the one OmnySSH kept up to 1.1.3.
        legacy: bool,
    },
    /// The file could not be read or parsed, or there is no home to find it in.
    Unreadable(PathBuf, russh::keys::Error),
}

/// `~/.ssh/known_hosts`, the file new keys are saved to.
pub(crate) fn path() -> Option<PathBuf> {
    home().map(|home| home.join(".ssh").join("known_hosts"))
}

// `USERPROFILE` first on Windows, as russh-keys resolves its own file.
fn home() -> Option<PathBuf> {
    std::env::home_dir()
}

#[cfg(windows)]
fn legacy_path() -> Option<PathBuf> {
    home().map(|home| home.join("ssh").join("known_hosts"))
}

#[cfg(not(windows))]
fn legacy_path() -> Option<PathBuf> {
    None
}

/// The files to consult, in order.
fn files() -> Vec<PathBuf> {
    path().into_iter().chain(legacy_path()).collect()
}

/// Checks `key`, offered by `host:port`, against the saved keys.
pub(crate) fn check(host: &str, port: u16, key: &PublicKey) -> Verdict {
    check_in(&files(), host, port, key)
}

/// The first file holding a key for the host decides. Any matching line is
/// enough, as in ssh(1): a stale line next to the right one refuses nothing. A
/// key of a type nobody saved is refused too, as ssh(1) does: else a server
/// that shows only another type, as a man in the middle can, would pass for a
/// new host. A file that cannot be opened counts as empty, as in ssh(1).
fn check_in(files: &[PathBuf], host: &str, port: u16, key: &PublicKey) -> Verdict {
    // No home: nothing to check against, and nowhere a key could be pinned.
    if files.is_empty() {
        return Verdict::Unreadable(
            PathBuf::from("~/.ssh/known_hosts"),
            russh::keys::Error::NoHomeDir,
        );
    }
    for file in files {
        let saved = match saved_keys(file, host, port) {
            Ok(saved) => saved,
            Err(e) => return Verdict::Unreadable(file.clone(), e),
        };
        if saved.iter().any(|k| k.key_data() == key.key_data()) {
            return Verdict::Known;
        }
        if !saved.is_empty() {
            let mut pinned = Vec::new();
            if !saved.iter().any(|k| same_type(k, key)) {
                for algorithm in saved.iter().map(PublicKey::algorithm) {
                    if !pinned.contains(&algorithm) {
                        pinned.push(algorithm);
                    }
                }
            }
            return Verdict::Changed {
                file: file.clone(),
                pinned,
                legacy: legacy_path().as_ref() == Some(file),
            };
        }
    }
    Verdict::Unknown
}

/// The keys `file` holds for `host:port`. ssh(1) writes names in lower case and
/// matches them regardless of case, so a mixed-case name is looked up both ways.
fn saved_keys(file: &Path, host: &str, port: u16) -> Result<Vec<PublicKey>, russh::keys::Error> {
    let lower = host.to_ascii_lowercase();
    let mut names = vec![host];
    if lower != host {
        names.push(&lower);
    }
    let mut keys = Vec::new();
    for name in names {
        keys.extend(
            known_host_keys_path(name, port, file)?
                .into_iter()
                .map(|(_, key)| key),
        );
    }
    Ok(keys)
}

/// Saves a key first seen on this connection (trust on first use).
pub(crate) fn learn(host: &str, port: u16, key: &PublicKey) -> Result<(), russh::keys::Error> {
    let path = path().ok_or(russh::keys::Error::NoHomeDir)?;
    learn_known_hosts_path(host, port, key, path)
}

/// The host key algorithms asked for when nothing is pinned: russh 0.46's list,
/// which OmnySSH shipped with, then P-384, last so that no server that worked
/// shows another key; some devices have no other (Cisco RoomOS set to ECDSA).
/// `ssh-rsa` (SHA-1) only after all of them, for servers before OpenSSH 7.2
/// that have nothing else (RHEL 6).
const KEY_ORDER: &[Algorithm] = &[
    Algorithm::Ed25519,
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP256,
    },
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP521,
    },
    Algorithm::Rsa {
        hash: Some(HashAlg::Sha256),
    },
    Algorithm::Rsa {
        hash: Some(HashAlg::Sha512),
    },
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP384,
    },
    Algorithm::Rsa { hash: None },
];

/// Host key algorithms for `host:port`, those of the keys saved for it first,
/// as ssh(1) orders them: a server with several keys then shows the pinned one
/// rather than one of another type that would be taken as new.
pub(crate) fn preferred(host: &str, port: u16) -> Cow<'static, [Algorithm]> {
    preferred_in(&files(), host, port)
}

fn preferred_in(files: &[PathBuf], host: &str, port: u16) -> Cow<'static, [Algorithm]> {
    let Some(saved) = files
        .iter()
        .filter_map(|file| saved_keys(file, host, port).ok())
        .find(|saved| !saved.is_empty())
    else {
        return Cow::Borrowed(KEY_ORDER);
    };
    let (mut order, rest): (Vec<Algorithm>, Vec<Algorithm>) = KEY_ORDER
        .iter()
        .cloned()
        // A saved RSA key brings ssh-rsa along, after both SHA-2 forms: an
        // OpenSSH before 7.2 shows that key no other way.
        .partition(|algo| saved.iter().any(|k| signs_with(k, algo)));
    order.extend(rest);
    Cow::Owned(order)
}

/// Types compare by key: an RSA key is one type whatever hash it signs with.
fn same_type(a: &PublicKey, b: &PublicKey) -> bool {
    a.algorithm() == b.algorithm()
}

fn signs_with(key: &PublicKey, algo: &Algorithm) -> bool {
    match algo {
        Algorithm::Rsa { .. } => key.algorithm().is_rsa(),
        _ => key.algorithm() == *algo,
    }
}

/// The host as a refusal names it.
fn who(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("{host} port {port}")
    }
}

/// The key type as `ssh-keygen -l` names it, with the curve, since a server can
/// change curves, and the name OpenSSH gives the server's file for it.
fn key_type(algorithm: &Algorithm) -> (String, Option<&'static str>) {
    match algorithm {
        Algorithm::Ed25519 => ("ED25519".into(), Some("ed25519")),
        Algorithm::Ecdsa { curve } => {
            let bits = match curve {
                EcdsaCurve::NistP256 => 256,
                EcdsaCurve::NistP384 => 384,
                EcdsaCurve::NistP521 => 521,
            };
            (format!("ECDSA P-{bits}"), Some("ecdsa"))
        }
        Algorithm::Rsa { .. } => ("RSA".into(), Some("rsa")),
        other => (other.to_string(), None),
    }
}

/// Shown when a saved key no longer matches. The first ':' closes the headline,
/// so a frontend that cuts there (the TUI status bar) keeps just that.
///
/// Names the offered key's type: a server has one key per type, and checking
/// another type's key on the server shows a mismatch that is not there. A
/// refusal from the legacy file says it is OmnySSH's own list, or a user whose
/// PuTTY and `ssh` connect has no reason to believe it.
pub(crate) fn changed_message(
    host: &str,
    port: u16,
    key: &PublicKey,
    file: &Path,
    pinned: &[Algorithm],
    legacy: bool,
) -> String {
    // `ssh-keygen -R` wants `[host]:port` off port 22; quoted, or zsh takes it
    // for a glob.
    let target = if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    };
    let (kind, server_file) = key_type(&key.algorithm());
    let saved = if pinned.is_empty() {
        "the one".to_string()
    } else {
        let types: Vec<String> = pinned.iter().map(|a| key_type(a).0).collect();
        format!("the {} key", types.join(" or "))
    };
    let whose = if legacy {
        ", the list OmnySSH kept up to 1.1.3, which PuTTY and OpenSSH never read"
    } else {
        ""
    };
    let how = server_file
        .map(|name| format!(", e.g. with ssh-keygen -lf /etc/ssh/ssh_host_{name}_key.pub"))
        .unwrap_or_default();
    format!(
        "Host key of {who} has changed: its {kind} key {fingerprint} does not match \
         {saved} saved in {file}{whose}. If the server was reinstalled or replaced, \
         check that key on the server{how}, then remove the old one with \
         ssh-keygen -R \"{target}\" -f \"{file}\". \
         Otherwise someone may be intercepting the connection.",
        who = who(host, port),
        fingerprint = key.fingerprint(HashAlg::Sha256),
        file = file.display(),
    )
}

/// Shown when a `known_hosts` file cannot be used: the connection is refused
/// rather than let an unchecked key in.
pub(crate) fn unreadable_message(
    host: &str,
    port: u16,
    file: &Path,
    error: &russh::keys::Error,
) -> String {
    format!(
        "Host key of {} could not be checked: {} is unreadable ({error})",
        who(host, port),
        file.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::private::RsaKeypair;
    use russh::keys::PrivateKey;
    use std::io::Write;

    fn ed25519() -> PublicKey {
        let mut rng = russh::keys::key::safe_rng();
        PrivateKey::random(&mut rng, Algorithm::Ed25519)
            .expect("ed25519 key")
            .public_key()
            .clone()
    }

    fn write(file: &Path, lines: &[String]) {
        let mut f = std::fs::File::create(file).expect("create");
        for line in lines {
            writeln!(f, "{line}").expect("write");
        }
    }

    fn line(host: &str, key: &PublicKey) -> String {
        format!("{host} {}", key.to_openssh().expect("openssh line"))
    }

    #[test]
    fn a_saved_key_is_known_and_a_different_one_changed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("known_hosts");
        let (server, other) = (ed25519(), ed25519());
        write(&file, &[line("10.0.0.5", &server)]);
        let files = [file.clone()];

        assert!(matches!(
            check_in(&files, "10.0.0.5", 22, &server),
            Verdict::Known
        ));
        match check_in(&files, "10.0.0.5", 22, &other) {
            Verdict::Changed {
                file: path,
                pinned,
                legacy,
            } => {
                assert_eq!(path, file);
                // Same type: nothing to name.
                assert!(pinned.is_empty());
                assert!(!legacy);
            }
            _ => panic!("expected a changed key"),
        }
        assert!(matches!(
            check_in(&files, "10.0.0.6", 22, &other),
            Verdict::Unknown
        ));
    }

    #[test]
    fn a_stale_line_next_to_the_right_one_is_no_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("known_hosts");
        let (old, new) = (ed25519(), ed25519());
        write(&file, &[line("vm", &old), line("vm", &new)]);
        assert!(matches!(check_in(&[file], "vm", 22, &new), Verdict::Known));
    }

    #[test]
    fn the_first_file_with_a_key_for_the_host_decides() {
        let dir = tempfile::tempdir().unwrap();
        let (primary, legacy) = (dir.path().join("a"), dir.path().join("b"));
        let (server, stale) = (ed25519(), ed25519());
        let files = [primary.clone(), legacy.clone()];

        // The legacy pin still refuses a key nobody saved elsewhere...
        write(&legacy, &[line("vm", &stale)]);
        match check_in(&files, "vm", 22, &server) {
            Verdict::Changed { file, .. } => assert_eq!(file, legacy),
            _ => panic!("expected the legacy pin to refuse"),
        }
        // ...until the key is saved in the primary file, which is read first.
        write(&primary, &[line("vm", &server)]);
        assert!(matches!(
            check_in(&files, "vm", 22, &server),
            Verdict::Known
        ));
        // A stale primary pin is never overruled by the legacy file.
        write(&primary, &[line("vm", &stale)]);
        write(&legacy, &[line("vm", &server)]);
        match check_in(&files, "vm", 22, &server) {
            Verdict::Changed { file, .. } => assert_eq!(file, primary),
            _ => panic!("expected the primary pin to refuse"),
        }
    }

    #[test]
    fn a_key_of_a_type_nobody_saved_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("known_hosts");
        write(&file, &[line("vm", &ed25519())]);
        // What a man in the middle shows when it has no Ed25519 key to offer.
        let p384 = russh::keys::parse_public_key_base64(
            "AAAAE2VjZHNhLXNoYTItbmlzdHAzODQAAAAIbmlzdHAzODQAAABhBPsWebQPfTKmztyvWSqgE1HXWtAJwl6Y\
             YUx43JswHMefMvUBiOAnCS20o697vnbFr6WtNWGsTt48NyDfBtwezmZ4wyhOqDnd7kJL8MUsWd3S7E4xe5RBd\
             U39kfoUDZ2WOQ==",
        )
        .expect("p384 key");
        match check_in(std::slice::from_ref(&file), "vm", 22, &p384) {
            Verdict::Changed {
                file: path, pinned, ..
            } => {
                assert_eq!(path, file);
                assert_eq!(pinned, [Algorithm::Ed25519]);
            }
            _ => panic!("a key of another type passed for a new host"),
        }
        // A host with nothing saved still trusts its first key.
        assert!(matches!(
            check_in(&[file], "other", 22, &p384),
            Verdict::Unknown
        ));
    }

    #[test]
    fn no_home_refuses_rather_than_trust_anything() {
        assert!(matches!(
            check_in(&[], "vm", 22, &ed25519()),
            Verdict::Unreadable(..)
        ));
    }

    #[test]
    fn a_mixed_case_name_finds_the_pin_ssh_wrote_in_lower_case() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("known_hosts");
        let (server, other) = (ed25519(), ed25519());
        write(&file, &[line("build.corp.lan", &server)]);
        let files = [file.clone()];
        assert!(matches!(
            check_in(&files, "Build.Corp.lan", 22, &server),
            Verdict::Known
        ));
        assert!(matches!(
            check_in(&files, "Build.Corp.lan", 22, &other),
            Verdict::Changed { .. }
        ));
    }

    #[test]
    fn saved_types_are_preferred() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("known_hosts");
        write(&file, &[]);
        let files = [file.clone()];
        assert_eq!(preferred_in(&files, "vm", 22), KEY_ORDER);

        // ssh(1) before 8.5 pinned ECDSA, which then has to come before Ed25519.
        let ecdsa = russh::keys::parse_public_key_base64(
            "AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBAdX7uLfmKNNWdDCmvSEIf+RcVQX7pM+\
             X+JsRGPG88ZBnYMJCOypWfiNliHIPyo8fNivzpE4a6ZynYc8KHiEz+4=",
        )
        .expect("ecdsa key");
        write(&file, &[line("vm", &ecdsa)]);
        let order = preferred_in(&files, "vm", 22);
        let p256 = Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        };
        assert_eq!(order.first(), Some(&p256));
        assert_eq!(order.len(), KEY_ORDER.len());
    }

    #[test]
    fn a_p384_pin_is_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("known_hosts");
        let p384 = russh::keys::parse_public_key_base64(
            "AAAAE2VjZHNhLXNoYTItbmlzdHAzODQAAAAIbmlzdHAzODQAAABhBPsWebQPfTKmztyvWSqgE1HXWtAJwl6Y\
             YUx43JswHMefMvUBiOAnCS20o697vnbFr6WtNWGsTt48NyDfBtwezmZ4wyhOqDnd7kJL8MUsWd3S7E4xe5RBd\
             U39kfoUDZ2WOQ==",
        )
        .expect("p384 key");
        write(&file, &[line("vm", &p384)]);
        let order = preferred_in(&[file], "vm", 22);
        let p384 = Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP384,
        };
        assert_eq!(order.first(), Some(&p384));
        assert_eq!(order.len(), KEY_ORDER.len());
    }

    #[test]
    fn nothing_pinned_asks_for_p384_then_sha1_last() {
        let p384 = Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP384,
        };
        assert_eq!(
            KEY_ORDER[KEY_ORDER.len() - 2..],
            [p384, Algorithm::Rsa { hash: None }]
        );
    }

    #[test]
    fn an_rsa_pin_asks_for_sha2_then_sha1() {
        let mut rng = russh::keys::key::safe_rng();
        let pair = RsaKeypair::random(&mut rng, 2048).expect("rsa key");
        let pinned = PrivateKey::from(pair).public_key().clone();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("known_hosts");
        write(&file, &[line("vm", &pinned)]);
        let order = preferred_in(&[file], "vm", 22);
        assert_eq!(
            order[..3],
            [
                Algorithm::Rsa {
                    hash: Some(HashAlg::Sha256)
                },
                Algorithm::Rsa {
                    hash: Some(HashAlg::Sha512)
                },
                Algorithm::Rsa { hash: None },
            ]
        );
        assert_eq!(order[3], Algorithm::Ed25519);
    }

    #[test]
    fn an_rsa_pin_holds_whatever_hash_was_negotiated() {
        let rsa = || {
            let mut rng = russh::keys::key::safe_rng();
            let pair = RsaKeypair::random(&mut rng, 2048).expect("rsa key");
            PrivateKey::from(pair).public_key().clone()
        };
        let pinned = rsa();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("known_hosts");
        write(&file, &[line("vm", &pinned)]);
        let files = [file.clone()];
        assert!(matches!(
            check_in(&files, "vm", 22, &pinned),
            Verdict::Known
        ));
        // Another RSA key is a changed one, not a new type to trust.
        match check_in(&files, "vm", 22, &rsa()) {
            Verdict::Changed {
                file: path, pinned, ..
            } => assert_eq!((path, pinned), (file, vec![])),
            _ => panic!("an RSA key under another hash name slipped past the pin"),
        }
    }

    #[test]
    fn the_refusal_names_the_key_the_file_and_the_remedy() {
        let file = Path::new("/home/me/.ssh/known_hosts");
        let key = ed25519();
        let message = changed_message("10.0.0.5", 2222, &key, file, &[], false);
        // The TUI status bar keeps what comes before the first ':'.
        assert_eq!(
            message.split(':').next(),
            Some("Host key of 10.0.0.5 port 2222 has changed")
        );
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        assert!(message.contains(&format!(
            "its ED25519 key {fingerprint} does not match the one saved in /home/me/.ssh/known_hosts."
        )));
        assert!(message.contains("ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub"));
        assert!(
            message.contains("ssh-keygen -R \"[10.0.0.5]:2222\" -f \"/home/me/.ssh/known_hosts\"")
        );
        assert!(!message.contains("PuTTY"));
    }

    #[test]
    fn a_refusal_names_the_saved_type_when_another_is_offered() {
        let file = Path::new("/home/me/.ssh/known_hosts");
        let p256 = Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        };
        let message = changed_message(
            "vm",
            22,
            &ed25519(),
            file,
            std::slice::from_ref(&p256),
            false,
        );
        assert!(message
            .contains("does not match the ECDSA P-256 key saved in /home/me/.ssh/known_hosts."));
        let rsa = Algorithm::Rsa { hash: None };
        let message = changed_message("vm", 22, &ed25519(), file, &[p256, rsa], false);
        assert!(message.contains("does not match the ECDSA P-256 or RSA key saved in"));
    }

    #[test]
    fn a_legacy_refusal_says_the_file_is_omnysshs_own() {
        let file = Path::new(r"C:\Users\me\ssh\known_hosts");
        let message = changed_message("192.168.1.25", 22, &ed25519(), file, &[], true);
        assert_eq!(
            message.split(':').next(),
            Some("Host key of 192.168.1.25 has changed")
        );
        assert!(message.contains(
            r"saved in C:\Users\me\ssh\known_hosts, the list OmnySSH kept up to 1.1.3, which PuTTY and OpenSSH never read."
        ));
        assert!(
            message.contains(r#"ssh-keygen -R "192.168.1.25" -f "C:\Users\me\ssh\known_hosts""#)
        );
    }

    /// The old file, where russh-keys pinned keys on Windows, is still read.
    #[cfg(windows)]
    #[test]
    fn the_legacy_file_is_still_read() {
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("USERPROFILE", home.path());
        let key = ed25519();
        let legacy = legacy_path().expect("legacy path");
        assert_eq!(legacy, home.path().join("ssh").join("known_hosts"));
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        write(&legacy, &[line("vm", &key)]);
        assert_eq!(path(), Some(home.path().join(".ssh").join("known_hosts")));
        assert!(matches!(check("vm", 22, &key), Verdict::Known));
        // A refusal from it says it is the old file.
        assert!(matches!(
            check("vm", 22, &ed25519()),
            Verdict::Changed { legacy: true, .. }
        ));
    }
}
