//! Loading SSH identity files, including passphrase-protected keys.
//!
//! Passphrases are cached in process memory only — never written to disk —
//! and keyed by the canonical path of the private key so every host that
//! shares a key unlocks it once per session.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use russh::keys::ssh_key::private::KeypairData;
use russh::keys::{PrivateKey, PublicKey};
use thiserror::Error;
use tokio::sync::{mpsc, watch};

use crate::event::CoreEvent;

#[derive(Default)]
struct Cache {
    /// Passphrases that decrypted their key.
    passphrases: HashMap<String, String>,
    /// Keys found encrypted. `unlock` takes no other path, so a frontend cannot
    /// use it to probe arbitrary files.
    encrypted: HashSet<String>,
    /// Keys a retrying connection has asked about since they were last unlocked.
    asked: HashSet<String>,
}

fn cache() -> MutexGuard<'static, Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Bumped on every unlock, so a connection parked on a key can go again.
fn unlocks() -> &'static watch::Sender<()> {
    static UNLOCKS: OnceLock<watch::Sender<()>> = OnceLock::new();
    UNLOCKS.get_or_init(|| watch::channel(()).0)
}

/// Failure to load or unlock a private key file.
#[derive(Debug, Error)]
pub enum IdentityError {
    /// The key is encrypted and no usable passphrase is cached.
    #[error("SSH key requires a passphrase: {0}")]
    Encrypted(String),
    /// The passphrase did not decrypt the key.
    #[error("wrong passphrase")]
    WrongPassphrase,
    /// No connection has asked for this key's passphrase.
    #[error("no passphrase was asked for SSH key {0}")]
    NotRequested(String),
    /// The file could not be read or used for a reason other than encryption.
    #[error("could not load SSH key {path}: {source}")]
    Load {
        path: String,
        #[source]
        source: anyhow::Error,
    },
}

/// Expand `~/` and, when the file exists, resolve it to a canonical path so
/// `~/.ssh/id_ed25519` and `/home/me/.ssh/id_ed25519` share one cache slot.
pub(crate) fn normalize_key_path(path: &str) -> String {
    let expanded = expand_tilde(path);
    std::fs::canonicalize(&expanded)
        .map(|p| without_verbatim_prefix(p.to_string_lossy().into_owned()))
        .unwrap_or(expanded)
}

/// Windows canonicalizes to `\\?\C:\…`; the path is shown to the user, so keep
/// the drive form they know.
fn without_verbatim_prefix(path: String) -> String {
    match path.strip_prefix(r"\\?\") {
        Some(rest) if rest.get(1..2) == Some(":") => rest.to_string(),
        _ => path,
    }
}

fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") || path == "~" {
        if let Some(home) = dirs::home_dir() {
            return path.replacen('~', &home.to_string_lossy(), 1);
        }
    }
    path.to_string()
}

/// Whether a passphrase is cached for `path` (a canonical key path, as carried
/// by [`crate::event::CoreEvent::KeyPassphraseRequired`]).
pub(crate) fn is_unlocked(path: &str) -> bool {
    cache().passphrases.contains_key(path)
}

/// Asks the frontends for `path`'s passphrase on behalf of `host_name`, for a
/// connection the user started. Skipped when the key was unlocked while that
/// connection was still under way.
pub async fn ask_passphrase(tx: &mpsc::Sender<CoreEvent>, host_name: &str, path: &str) {
    if !is_unlocked(path) {
        send_prompt(tx, host_name, path).await;
    }
}

/// [`ask_passphrase`] for a connection that retries on its own (a poller, a
/// tunnel): each key is asked about once until it is unlocked, so a dismissed
/// prompt does not come back by itself.
pub(crate) async fn ask_passphrase_once(tx: &mpsc::Sender<CoreEvent>, host_name: &str, path: &str) {
    let first = {
        let mut cache = cache();
        !cache.passphrases.contains_key(path) && cache.asked.insert(path.to_string())
    };
    if first {
        send_prompt(tx, host_name, path).await;
    }
}

async fn send_prompt(tx: &mpsc::Sender<CoreEvent>, host_name: &str, path: &str) {
    let _ = tx
        .send(CoreEvent::KeyPassphraseRequired {
            host_name: host_name.to_string(),
            key_path: path.to_string(),
        })
        .await;
}

/// Resolves once a passphrase is cached for `path`.
pub(crate) async fn unlocked(path: &str) {
    // Subscribe before checking, so an unlock in between is not missed.
    let mut rx = unlocks().subscribe();
    while !is_unlocked(path) {
        // The sender lives in a static and is never dropped.
        let _ = rx.changed().await;
    }
}

/// Decrypt `path` with `passphrase` and remember it for the rest of the process.
///
/// # Errors
/// [`IdentityError::NotRequested`] for a key no connection found encrypted,
/// [`IdentityError::WrongPassphrase`], or a key that cannot be read or used.
pub fn unlock(path: &str, passphrase: &str) -> Result<(), IdentityError> {
    let key_path = normalize_key_path(path);
    if !cache().encrypted.contains(&key_path) {
        return Err(IdentityError::NotRequested(key_path));
    }
    // Plain first: the passphrase may have been removed on disk since the
    // prompt, and then the key needs none.
    let loaded = match load_secret_key(&key_path, None) {
        Err(russh::keys::Error::KeyIsEncrypted) => load_secret_key(&key_path, Some(passphrase)),
        plain => plain,
    };
    match loaded {
        Ok(_) => {
            {
                let mut cache = cache();
                cache
                    .passphrases
                    .insert(key_path.clone(), passphrase.to_string());
                cache.asked.remove(&key_path);
            }
            unlocks().send_replace(());
            Ok(())
        }
        // Unreadable, or decrypted into a key russh cannot sign with (FIDO).
        Err(e @ (russh::keys::Error::IO(_) | russh::keys::Error::UnsupportedKeyType { .. })) => {
            Err(IdentityError::Load {
                path: key_path,
                source: e.into(),
            })
        }
        // The key is known to be encrypted, so any other decode failure is the
        // passphrase (russh reports it as a cipher or parse error).
        Err(_) => Err(IdentityError::WrongPassphrase),
    }
}

/// The public half of the key at `path`, found as ssh(1) finds it and never
/// decrypted: the file itself when it is a public key (an `IdentityFile` may name
/// the `.pub` of a key that only an agent holds), else `<path>.pub`, else the
/// copy an OpenSSH private key carries in the clear, else an unencrypted PEM key's.
///
/// Without its comment, so it compares equal to the same key from an agent.
pub(crate) fn public_key(path: &str) -> Option<PublicKey> {
    let path = expand_tilde(path);
    let key = russh::keys::load_public_key(&path)
        .or_else(|_| russh::keys::load_public_key(format!("{path}.pub")))
        .ok()
        .or_else(|| {
            PrivateKey::read_openssh_file(&path)
                .ok()
                .map(|key| key.public_key().clone())
        })
        .or_else(|| Some(load_secret_key(&path, None).ok()?.public_key().clone()))?;
    Some(PublicKey::from(key.key_data().clone()))
}

/// [`russh::keys::load_secret_key`], turning down a key russh cannot sign with
/// (FIDO, DSA): signing with one would end the whole connection.
fn load_secret_key(path: &str, passphrase: Option<&str>) -> Result<PrivateKey, russh::keys::Error> {
    let key = russh::keys::load_secret_key(path, passphrase)?;
    match key.key_data() {
        KeypairData::Ed25519(_) | KeypairData::Ecdsa(_) | KeypairData::Rsa(_) => Ok(key),
        _ => Err(russh::keys::Error::UnsupportedKeyType {
            key_type_string: key.algorithm().to_string(),
            key_type_raw: Vec::new(),
        }),
    }
}

/// Load a private key, using a cached passphrase when the file is encrypted.
///
/// # Errors
/// [`IdentityError::Encrypted`] when the key needs a passphrase that has not
/// been unlocked yet; [`IdentityError::Load`] for I/O or parse failures.
pub(crate) fn load_key_pair(path: &str) -> Result<PrivateKey, IdentityError> {
    let key_path = normalize_key_path(path);
    // Plain first: a key whose passphrase was removed on disk must not be fed
    // a cached one.
    match load_secret_key(&key_path, None) {
        Ok(key) => return Ok(key),
        Err(russh::keys::Error::KeyIsEncrypted) => {}
        Err(e) => {
            return Err(IdentityError::Load {
                path: key_path,
                source: e.into(),
            })
        }
    }

    let passphrase = {
        let mut cache = cache();
        cache.encrypted.insert(key_path.clone());
        cache.passphrases.get(&key_path).cloned()
    };
    let Some(passphrase) = passphrase else {
        return Err(IdentityError::Encrypted(key_path));
    };
    load_secret_key(&key_path, Some(&passphrase)).map_err(|_| {
        // Re-encrypted since it was unlocked: ask again. Only this passphrase
        // goes; a newer one unlocked meanwhile stays.
        let mut cache = cache();
        if cache.passphrases.get(&key_path) == Some(&passphrase) {
            cache.passphrases.remove(&key_path);
            cache.asked.remove(&key_path);
        }
        IdentityError::Encrypted(key_path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    fn ssh_keygen(args: &[&str]) {
        let status = Command::new("ssh-keygen")
            .args(args)
            .status()
            .expect("these tests need ssh-keygen on PATH");
        assert!(status.success(), "ssh-keygen {args:?} failed: {status}");
    }

    fn write_key(dir: &Path, passphrase: &str) -> String {
        let path: PathBuf = dir.join("id_ed25519");
        let path = path.to_str().expect("utf-8 path").to_string();
        ssh_keygen(&["-q", "-t", "ed25519", "-f", &path, "-N", passphrase]);
        path
    }

    #[test]
    fn an_unencrypted_key_loads_without_a_passphrase() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_key(dir.path(), "");
        load_key_pair(&path).expect("unencrypted key should load");
    }

    #[test]
    fn an_encrypted_key_reports_that_a_passphrase_is_required() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_key(dir.path(), "correct horse");
        match load_key_pair(&path) {
            Err(IdentityError::Encrypted(reported)) => {
                assert_eq!(reported, normalize_key_path(&path))
            }
            other => panic!("expected Encrypted, got {other:?}"),
        }
    }

    #[test]
    fn unlocking_caches_the_passphrase_for_later_loads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_key(dir.path(), "correct horse");
        assert!(load_key_pair(&path).is_err());

        unlock(&path, "correct horse").expect("unlock");
        assert!(is_unlocked(&normalize_key_path(&path)));
        load_key_pair(&path).expect("cached passphrase should decrypt the key");
    }

    #[test]
    fn a_wrong_passphrase_is_rejected_and_not_cached() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_key(dir.path(), "correct horse");
        assert!(load_key_pair(&path).is_err());

        assert!(matches!(
            unlock(&path, "wrong"),
            Err(IdentityError::WrongPassphrase)
        ));
        assert!(matches!(
            load_key_pair(&path),
            Err(IdentityError::Encrypted(_))
        ));
    }

    #[test]
    fn only_a_key_found_encrypted_can_be_unlocked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_key(dir.path(), "correct horse");
        assert!(matches!(
            unlock(&path, "correct horse"),
            Err(IdentityError::NotRequested(_))
        ));
    }

    #[test]
    fn a_key_changed_on_disk_after_unlocking_is_read_as_it_is_now() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_key(dir.path(), "correct horse");
        assert!(load_key_pair(&path).is_err());
        unlock(&path, "correct horse").expect("unlock");

        ssh_keygen(&["-q", "-p", "-f", &path, "-P", "correct horse", "-N", "new"]);
        assert!(
            matches!(load_key_pair(&path), Err(IdentityError::Encrypted(_))),
            "a stale passphrase must lead to a new prompt"
        );
        assert!(!is_unlocked(&normalize_key_path(&path)));

        ssh_keygen(&["-q", "-p", "-f", &path, "-P", "new", "-N", ""]);
        load_key_pair(&path).expect("a key without a passphrase loads as is");
    }

    #[test]
    fn a_passphrase_removed_on_disk_after_the_prompt_still_unlocks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_key(dir.path(), "correct horse");
        assert!(load_key_pair(&path).is_err());

        ssh_keygen(&["-q", "-p", "-f", &path, "-P", "correct horse", "-N", ""]);
        unlock(&path, "correct horse").expect("a plain key needs no passphrase");
        assert!(is_unlocked(&normalize_key_path(&path)));
    }

    #[test]
    fn a_windows_verbatim_path_is_shown_in_its_drive_form() {
        let verbatim = String::from(r"\\?\C:\Users\me\.ssh\id_ed25519");
        assert_eq!(
            without_verbatim_prefix(verbatim),
            r"C:\Users\me\.ssh\id_ed25519"
        );
        let unc = String::from(r"\\?\UNC\server\share\key");
        assert_eq!(without_verbatim_prefix(unc.clone()), unc);
        assert_eq!(without_verbatim_prefix(String::from("/k/id")), "/k/id");
    }

    #[tokio::test]
    async fn retrying_connections_ask_once_per_key_and_users_every_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_key(dir.path(), "correct horse");
        assert!(load_key_pair(&path).is_err());
        let key = normalize_key_path(&path);
        let (tx, mut rx) = mpsc::channel(8);
        let mut asked_for = move || {
            std::iter::from_fn(|| rx.try_recv().ok())
                .map(|event| match event {
                    CoreEvent::KeyPassphraseRequired { host_name, .. } => host_name,
                    other => panic!("unexpected {other:?}"),
                })
                .collect::<Vec<_>>()
        };

        ask_passphrase_once(&tx, "poller-a", &key).await;
        ask_passphrase_once(&tx, "poller-b", &key).await;
        ask_passphrase(&tx, "terminal", &key).await;
        assert_eq!(asked_for(), ["poller-a", "terminal"]);

        unlock(&path, "correct horse").expect("unlock");
        ask_passphrase(&tx, "terminal", &key).await;
        assert!(asked_for().is_empty(), "an unlocked key is not asked for");

        // Re-encrypted on disk: that is worth one new prompt.
        ssh_keygen(&["-q", "-p", "-f", &path, "-P", "correct horse", "-N", "new"]);
        assert!(load_key_pair(&path).is_err());
        ask_passphrase_once(&tx, "poller-a", &key).await;
        ask_passphrase_once(&tx, "poller-b", &key).await;
        assert_eq!(asked_for(), ["poller-a"]);
    }

    #[tokio::test]
    async fn a_waiter_wakes_when_its_key_is_unlocked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_key(dir.path(), "correct horse");
        assert!(load_key_pair(&path).is_err());
        let key = normalize_key_path(&path);

        let waiter = tokio::spawn(async move { unlocked(&key).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished());

        unlock(&path, "correct horse").expect("unlock");
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("the waiter wakes")
            .expect("the waiter ran");
    }
}
