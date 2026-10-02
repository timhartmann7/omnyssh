//! Login passwords typed at a prompt.
//!
//! A password is kept in process memory only — never written to disk — and
//! only once a server has accepted it. It is keyed by the login it was typed
//! for: user, host, port and the bastions on the way, so it is never offered to
//! another server that shares an address behind a different bastion.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};

use crate::event::CoreEvent;

#[derive(Default)]
struct State {
    /// Passwords a server accepted, by login key.
    accepted: HashMap<String, String>,
    /// How many times each login's password was remembered.
    generations: HashMap<String, u64>,
    /// How each login's password is sent, once a server has shown it.
    methods: HashMap<String, Method>,
    /// Prompts waiting for an answer, by request id.
    pending: HashMap<u64, oneshot::Sender<Option<String>>>,
    last_request: u64,
}

/// How a server takes the login password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    /// The `password` method.
    Password,
    /// Keyboard-interactive, answering its password prompt.
    KeyboardInteractive,
}

/// How long a prompt waits for the user before the login gives up. Nothing
/// else would end it if the frontend lost the prompt (a reloaded page).
const PROMPT_TIMEOUT: Duration = Duration::from_secs(300);

fn state() -> MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Bumped whenever a password is accepted, so a connection waiting for one can
/// go again.
fn accepted_signal() -> &'static watch::Sender<()> {
    static ACCEPTED: OnceLock<watch::Sender<()>> = OnceLock::new();
    ACCEPTED.get_or_init(|| watch::channel(()).0)
}

/// The password a server accepted for `key` this session.
pub(crate) fn accepted(key: &str) -> Option<String> {
    state().accepted.get(key).cloned()
}

/// Remembers `password` for `key`; call only once the server took it.
pub(crate) fn remember(key: &str, password: &str) {
    {
        let mut state = state();
        state.accepted.insert(key.to_string(), password.to_string());
        *state.generations.entry(key.to_string()).or_default() += 1;
    }
    accepted_signal().send_replace(());
}

fn generation(key: &str) -> u64 {
    state().generations.get(key).copied().unwrap_or_default()
}

/// Forgets `password` for `key` after the server turned it down. A newer one
/// remembered meanwhile stays.
pub(crate) fn forget(key: &str, password: &str) {
    let mut state = state();
    if state.accepted.get(key).map(String::as_str) == Some(password) {
        state.accepted.remove(key);
    }
}

/// How `key`'s server takes a password, if a login has shown it.
pub(crate) fn method(key: &str) -> Option<Method> {
    state().methods.get(key).copied()
}

/// Records how `key`'s server takes a password.
pub(crate) fn learn(key: &str, method: Method) {
    state().methods.insert(key.to_string(), method);
}

/// Resolves once a password is remembered for `key` from now on. One held
/// already did not get the caller in (it was held back from a new host key),
/// so waking on it would only redial straight into the same failure.
pub(crate) async fn remembered(key: &str) {
    let mut rx = accepted_signal().subscribe();
    let seen = generation(key);
    while generation(key) == seen || accepted(key).is_none() {
        // The sender lives in a static and is never dropped.
        let _ = rx.changed().await;
    }
}

/// An answer that has no prompt to go to.
#[derive(Debug, Error)]
pub enum PasswordError {
    /// The request was answered already, or its connection stopped waiting.
    #[error("no login is waiting for this password")]
    NotRequested,
}

/// Answers the prompt `request_id` of a [`CoreEvent::PasswordRequired`]:
/// `Some(password)` to try it, `None` to cancel the login.
///
/// # Errors
/// [`PasswordError::NotRequested`] when no connection waits on that request.
pub fn answer(request_id: u64, password: Option<String>) -> Result<(), PasswordError> {
    let reply = state()
        .pending
        .remove(&request_id)
        .ok_or(PasswordError::NotRequested)?;
    reply
        .send(password)
        .map_err(|_| PasswordError::NotRequested)
}

/// What a password prompt shows.
pub(crate) struct Prompt<'a> {
    /// `user@host` of the server asking.
    pub login: &'a str,
    /// The previous password for it was refused.
    pub retry: bool,
    /// The fingerprint of the server's host key when this connection is the
    /// first to see it, so the user can check it before typing.
    pub new_host_key: Option<&'a str>,
}

/// Why a prompt produced no password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoAnswer {
    Cancelled,
    /// Nobody answered within [`PROMPT_TIMEOUT`].
    TimedOut,
}

/// Asks the user for a login password while a connection authenticates.
#[async_trait]
// async_trait < 0.1.92 marks the future it boxes `#[must_use]` a second time;
// clippy 1.99 flags that, older clippy doesn't know the lint.
#[allow(unknown_lints, clippy::double_must_use)]
pub(crate) trait AskPassword: Send {
    async fn ask(&mut self, prompt: Prompt<'_>) -> Result<String, NoAnswer>;
}

/// Asks through the frontends: sends [`CoreEvent::PasswordRequired`] and waits
/// for [`answer`].
pub struct Prompter {
    tx: mpsc::Sender<CoreEvent>,
    host_name: String,
}

impl Prompter {
    /// A prompter for connections to `host_name`, reporting on `tx`.
    pub fn new(tx: mpsc::Sender<CoreEvent>, host_name: impl Into<String>) -> Self {
        Self {
            tx,
            host_name: host_name.into(),
        }
    }
}

#[async_trait]
impl AskPassword for Prompter {
    async fn ask(&mut self, prompt: Prompt<'_>) -> Result<String, NoAnswer> {
        let (reply, answer) = oneshot::channel();
        let request_id = {
            let mut state = state();
            state.last_request += 1;
            let id = state.last_request;
            state.pending.insert(id, reply);
            id
        };
        let _pending = Pending(request_id);
        let asked = self
            .tx
            .send(CoreEvent::PasswordRequired {
                request_id,
                host_name: self.host_name.clone(),
                login: prompt.login.to_string(),
                retry: prompt.retry,
                new_host_key: prompt.new_host_key.map(str::to_string),
            })
            .await;
        if asked.is_err() {
            return Err(NoAnswer::Cancelled);
        }
        // Answering an expired prompt later fails with NotRequested, which
        // frontends report and close it on.
        match tokio::time::timeout(PROMPT_TIMEOUT, answer).await {
            Ok(Ok(Some(password))) => Ok(password),
            Ok(_) => Err(NoAnswer::Cancelled),
            Err(_) => Err(NoAnswer::TimedOut),
        }
    }
}

/// A prompt waiting for its answer; unlisted once the login stops waiting, so
/// a late answer is refused.
struct Pending(u64);

impl Drop for Pending {
    fn drop(&mut self) {
        state().pending.remove(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(retry: bool) -> Prompt<'static> {
        Prompt {
            login: "root@10.0.0.1",
            retry,
            new_host_key: None,
        }
    }

    async fn asked(rx: &mut mpsc::Receiver<CoreEvent>) -> u64 {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(CoreEvent::PasswordRequired { request_id, .. })) => request_id,
            other => panic!("expected a prompt, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_answer_reaches_the_connection_that_asked() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut prompter = Prompter::new(tx, "web-1");
        let asking = tokio::spawn(async move { prompter.ask(prompt(false)).await });

        let id = asked(&mut rx).await;
        answer(id, Some(String::from("secret"))).expect("answer");
        assert_eq!(asking.await.expect("ran").as_deref(), Ok("secret"));
        assert!(matches!(answer(id, None), Err(PasswordError::NotRequested)));
    }

    #[tokio::test]
    async fn a_cancel_ends_the_login() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut prompter = Prompter::new(tx, "web-1");
        let asking = tokio::spawn(async move { prompter.ask(prompt(true)).await });

        let id = asked(&mut rx).await;
        answer(id, None).expect("cancel");
        assert_eq!(asking.await.expect("ran"), Err(NoAnswer::Cancelled));
    }

    #[tokio::test]
    async fn a_login_that_stops_waiting_refuses_a_late_answer() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut prompter = Prompter::new(tx, "web-1");
        let asking = tokio::spawn(async move { prompter.ask(prompt(false)).await });

        let id = asked(&mut rx).await;
        asking.abort();
        let _ = asking.await;
        assert!(matches!(
            answer(id, Some(String::from("late"))),
            Err(PasswordError::NotRequested)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn an_unanswered_prompt_gives_up() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut prompter = Prompter::new(tx, "web-1");
        let asking = tokio::spawn(async move { prompter.ask(prompt(false)).await });

        let id = asked(&mut rx).await;
        tokio::time::advance(PROMPT_TIMEOUT + Duration::from_secs(1)).await;
        assert_eq!(asking.await.expect("ran"), Err(NoAnswer::TimedOut));
        assert!(matches!(answer(id, None), Err(PasswordError::NotRequested)));
    }

    #[tokio::test]
    async fn a_password_already_held_does_not_wake_a_waiter() {
        let key = "held@10.6.6.6:22";
        remember(key, "secret");
        let waiter = tokio::spawn(async move { remembered(key).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !waiter.is_finished(),
            "only a newly typed password wakes it"
        );
        remember(key, "newer");
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("the waiter wakes")
            .expect("the waiter ran");
    }

    #[tokio::test]
    async fn a_waiter_wakes_when_its_login_is_remembered() {
        let key = "waiter@10.9.9.9:22";
        let waiter = tokio::spawn(async move { remembered(key).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished());

        remember("someone-else@10.9.9.9:22", "x");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "another login does not wake it");

        remember(key, "secret");
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("the waiter wakes")
            .expect("the waiter ran");
    }

    #[test]
    fn the_method_a_login_takes_is_remembered() {
        let key = "method@10.7.7.7:22";
        assert_eq!(method(key), None);
        learn(key, Method::KeyboardInteractive);
        assert_eq!(method(key), Some(Method::KeyboardInteractive));
    }

    #[test]
    fn a_refused_password_is_forgotten_but_a_newer_one_stays() {
        let key = "forget@10.8.8.8:22";
        remember(key, "old");
        forget(key, "old");
        assert_eq!(accepted(key), None);

        remember(key, "new");
        forget(key, "old");
        assert_eq!(accepted(key).as_deref(), Some("new"));
    }
}
