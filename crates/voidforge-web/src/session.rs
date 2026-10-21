//! What the session holds (P4.1, D-057): the login attempts waiting for their callback, and
//! after a login, the ID token and what Home shows. The store is our own bounded in-memory one
//! ([`crate::store`]), so every session ends when the process does. The browser holds only the
//! session cookie.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::config::Secret;
use crate::oidc::{Identity, PendingLogin};

/// The session key of the pending logins.
pub const PENDING_KEY: &str = "pending";
/// The session key of the login.
pub const LOGIN_KEY: &str = "login";
/// The session key marking a login that renews an ended one (§5.6; P4.3, carried by D-059): the
/// web sets it in the fresh session when it ends an expired login, never from a URL, holding
/// the [`Banner`] the first page after the login shows, and the callback takes it.
pub const RENEWING_KEY: &str = "renewing";
/// The session key of the one-shot [`Banner`] the next page shows: set by a renewing login's
/// callback or by a submitted form (P4.4), removed by the page that shows it.
pub const NOTICE_KEY: &str = "notice";
/// The session key of the form keys this session issued (P4.4, phase4-ux-ruling-lead Q3).
pub const FORM_KEYS_KEY: &str = "form_keys";
/// The most form keys one session keeps (P4.4's T3): one per rendered page with forms, so 64
/// renders of open tabs and Back steps. Past it, the oldest goes and its form is "expired".
pub const MAX_FORM_KEYS: usize = 64;
/// A form key's random bytes: 16, as base64url without padding, 22 characters (T2).
pub const FORM_KEY_BYTES: usize = 16;
/// The most login attempts one session keeps: one per open tab, within reason.
pub const MAX_PENDING: usize = 8;
/// How long a login attempt waits for its callback.
pub const PENDING_LIFETIME_SECS: u64 = 600;

/// A session error: the store failed, or a value didn't (de)serialize.
pub type SessionError = tower_sessions::session::Error;

/// A logged-in session.
#[derive(Clone, Serialize, Deserialize)]
pub struct LoggedIn {
    /// The ID token, sent to the API and nowhere else.
    pub id_token: Secret,
    /// The `email` claim, shown to its owner only.
    pub email: Option<String>,
    /// The `name` claim, shown to its owner only.
    pub name: Option<String>,
    /// When the token expires, in Unix seconds. The session outlives it by a grace period, so
    /// the next page can say the login expired.
    pub exp: u64,
}

impl fmt::Debug for LoggedIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoggedIn")
            .field("exp", &self.exp)
            .finish_non_exhaustive()
    }
}

impl From<Identity> for LoggedIn {
    fn from(i: Identity) -> Self {
        Self {
            id_token: i.id_token,
            email: i.email,
            name: i.name,
            exp: i.exp,
        }
    }
}

/// The one-shot line at the top of the next page (phase4-ux §2): what the last action did, or
/// why it wasn't sent. Only the web sets it, in the session; nothing in a URL can (D-060).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Banner {
    /// The first line: `Done: …`, `Not done: …`, or the web's own words.
    pub text: String,
    /// The API's code shown beside it, for a refusal.
    pub code: Option<String>,
    /// The lines after it: a success's events (§6.4), then §5.2's "already submitted".
    pub more: Vec<String>,
}

impl Banner {
    /// A banner of one line with no code.
    #[must_use]
    pub fn line(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            code: None,
            more: Vec::new(),
        }
    }
}

/// A form key the session issued, and whether the host has answered it: a 200 or a 422 came
/// back for it (§5.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FormKey {
    key: String,
    answered: bool,
}

/// What the session knows of a submitted form key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// The session didn't issue it, or dropped it past [`MAX_FORM_KEYS`]: the form has expired.
    Unknown,
    /// Issued, and no answer recorded yet.
    Issued,
    /// Issued, and the host has answered it once already.
    Answered,
}

/// A new form key: [`FORM_KEY_BYTES`] from the CSPRNG, base64url without padding.
fn new_form_key() -> Option<String> {
    let mut bytes = [0u8; FORM_KEY_BYTES];
    aws_lc_rs::rand::fill(&mut bytes).ok()?;
    Some(URL_SAFE_NO_PAD.encode(bytes))
}

/// Issues a fresh form key for a rendered page (P4.4's T1: one per render, shared by its forms)
/// and keeps it, dropping the oldest past [`MAX_FORM_KEYS`]. None when the CSPRNG fails.
///
/// # Errors
/// [`SessionError`] from the store.
pub async fn issue_form_key(session: &Session) -> Result<Option<String>, SessionError> {
    let Some(key) = new_form_key() else {
        return Ok(None);
    };
    let mut keys: Vec<FormKey> = session.get(FORM_KEYS_KEY).await?.unwrap_or_default();
    while keys.len() >= MAX_FORM_KEYS {
        keys.remove(0);
    }
    keys.push(FormKey {
        key: key.clone(),
        answered: false,
    });
    session.insert(FORM_KEYS_KEY, keys).await?;
    Ok(Some(key))
}

fn same(a: &str, b: &str) -> bool {
    aws_lc_rs::constant_time::verify_slices_are_equal(a.as_bytes(), b.as_bytes()).is_ok()
}

/// What this session knows of `key`.
///
/// # Errors
/// [`SessionError`] from the store.
pub async fn form_key_state(session: &Session, key: &str) -> Result<KeyState, SessionError> {
    let keys: Vec<FormKey> = session.get(FORM_KEYS_KEY).await?.unwrap_or_default();
    Ok(match keys.iter().find(|k| same(&k.key, key)) {
        None => KeyState::Unknown,
        Some(k) if k.answered => KeyState::Answered,
        Some(_) => KeyState::Issued,
    })
}

/// Records that the host answered `key` (a 200 or a 422), so a later submit of it says "already
/// submitted" (§5.2). A key the session no longer holds is left alone.
///
/// # Errors
/// [`SessionError`] from the store.
pub async fn mark_answered(session: &Session, key: &str) -> Result<(), SessionError> {
    let mut keys: Vec<FormKey> = session.get(FORM_KEYS_KEY).await?.unwrap_or_default();
    if let Some(k) = keys.iter_mut().find(|k| same(&k.key, key)) {
        k.answered = true;
        session.insert(FORM_KEYS_KEY, keys).await?;
    }
    Ok(())
}

fn fresh(p: &PendingLogin, now: u64) -> bool {
    p.created <= now && now - p.created < PENDING_LIFETIME_SECS
}

/// Adds a login attempt, dropping expired ones and, past [`MAX_PENDING`], the oldest.
///
/// # Errors
/// [`SessionError`] from the store.
pub async fn add_pending(session: &Session, p: PendingLogin, now: u64) -> Result<(), SessionError> {
    let mut list: Vec<PendingLogin> = session.get(PENDING_KEY).await?.unwrap_or_default();
    list.retain(|q| fresh(q, now));
    while list.len() >= MAX_PENDING {
        list.remove(0);
    }
    list.push(p);
    session.insert(PENDING_KEY, list).await
}

/// Takes the attempt whose state matches, if it's still fresh. It's removed either way, so a
/// callback can use it once.
///
/// # Errors
/// [`SessionError`] from the store.
pub async fn take_pending(
    session: &Session,
    state: &str,
    now: u64,
) -> Result<Option<PendingLogin>, SessionError> {
    let mut list: Vec<PendingLogin> = session.get(PENDING_KEY).await?.unwrap_or_default();
    let index = list.iter().position(|p| {
        aws_lc_rs::constant_time::verify_slices_are_equal(
            p.state.expose().as_bytes(),
            state.as_bytes(),
        )
        .is_ok()
    });
    let taken = index.map(|i| list.remove(i));
    list.retain(|q| fresh(q, now));
    if list.is_empty() {
        session.remove::<Vec<PendingLogin>>(PENDING_KEY).await?;
    } else {
        session.insert(PENDING_KEY, list).await?;
    }
    Ok(taken.filter(|p| fresh(p, now)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::store::BoundedStore;

    use super::*;

    fn attempt(state: &str, created: u64) -> PendingLogin {
        PendingLogin {
            state: Secret::new(state.to_owned()),
            nonce: Secret::new("n".to_owned()),
            verifier: Secret::new("v".to_owned()),
            created,
            next: "/".to_owned(),
        }
    }

    fn session() -> Session {
        Session::new(None, Arc::new(BoundedStore::default()), None)
    }

    #[tokio::test]
    async fn an_attempt_is_taken_once() {
        let s = session();
        add_pending(&s, attempt("a", 100), 100).await.expect("add");
        assert!(take_pending(&s, "a", 101).await.expect("take").is_some());
        assert!(take_pending(&s, "a", 101).await.expect("take").is_none());
    }

    #[tokio::test]
    async fn only_the_matching_state_is_taken() {
        let s = session();
        add_pending(&s, attempt("a", 100), 100).await.expect("add");
        add_pending(&s, attempt("b", 100), 100).await.expect("add");
        assert!(take_pending(&s, "c", 101).await.expect("take").is_none());
        assert!(take_pending(&s, "", 101).await.expect("take").is_none());
        let b = take_pending(&s, "b", 101).await.expect("take").expect("b");
        assert_eq!(b.state.expose(), "b");
        assert!(take_pending(&s, "a", 101).await.expect("take").is_some());
    }

    #[tokio::test]
    async fn an_expired_attempt_is_refused_and_dropped() {
        let s = session();
        add_pending(&s, attempt("a", 100), 100).await.expect("add");
        let at_limit = 100 + PENDING_LIFETIME_SECS;
        assert!(
            take_pending(&s, "a", at_limit)
                .await
                .expect("take")
                .is_none()
        );
        // One second earlier it would have been taken.
        add_pending(&s, attempt("b", 100), 100).await.expect("add");
        assert!(
            take_pending(&s, "b", at_limit - 1)
                .await
                .expect("take")
                .is_some()
        );
    }

    #[tokio::test]
    async fn at_most_eight_attempts_are_kept_and_the_oldest_goes() {
        let s = session();
        for i in 0..=MAX_PENDING {
            add_pending(&s, attempt(&format!("s{i}"), 100), 100)
                .await
                .expect("add");
        }
        let list: Vec<PendingLogin> = s.get(PENDING_KEY).await.expect("get").expect("list");
        assert_eq!(list.len(), MAX_PENDING);
        assert!(take_pending(&s, "s0", 101).await.expect("take").is_none());
        assert!(
            take_pending(&s, &format!("s{MAX_PENDING}"), 101)
                .await
                .expect("take")
                .is_some()
        );
    }

    #[tokio::test]
    async fn form_keys_are_22_base64url_characters_and_never_repeat() {
        let s = session();
        let mut seen = Vec::new();
        for _ in 0..MAX_FORM_KEYS {
            let k = issue_form_key(&s).await.expect("issue").expect("rng");
            assert_eq!(k.len(), 22, "{k}");
            assert_eq!(
                URL_SAFE_NO_PAD.decode(&k).expect("base64url").len(),
                FORM_KEY_BYTES
            );
            assert!(
                k.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "{k}"
            );
            seen.push(k);
        }
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), seen.len());
    }

    #[tokio::test]
    async fn a_key_is_issued_answered_and_dropped_past_the_bound() {
        let s = session();
        assert_eq!(
            form_key_state(&s, "nope").await.expect("state"),
            KeyState::Unknown
        );
        let first = issue_form_key(&s).await.expect("issue").expect("rng");
        assert_eq!(
            form_key_state(&s, &first).await.expect("state"),
            KeyState::Issued
        );
        mark_answered(&s, &first).await.expect("mark");
        assert_eq!(
            form_key_state(&s, &first).await.expect("state"),
            KeyState::Answered
        );
        // Marking a key the session never issued adds nothing.
        mark_answered(&s, "stranger").await.expect("mark");
        assert_eq!(
            form_key_state(&s, "stranger").await.expect("state"),
            KeyState::Unknown
        );
        let second = issue_form_key(&s).await.expect("issue").expect("rng");
        for _ in 2..MAX_FORM_KEYS {
            issue_form_key(&s).await.expect("issue");
        }
        // 64 kept: the first is still there; the 65th drops it, and only it.
        assert_eq!(
            form_key_state(&s, &first).await.expect("state"),
            KeyState::Answered
        );
        issue_form_key(&s).await.expect("issue");
        assert_eq!(
            form_key_state(&s, &first).await.expect("state"),
            KeyState::Unknown
        );
        assert_eq!(
            form_key_state(&s, &second).await.expect("state"),
            KeyState::Issued
        );
        let keys: Vec<FormKey> = s.get(FORM_KEYS_KEY).await.expect("get").expect("keys");
        assert_eq!(keys.len(), MAX_FORM_KEYS);
    }

    #[test]
    fn a_login_prints_no_token() {
        let l = LoggedIn {
            id_token: Secret::new("eyJ-token-text".to_owned()),
            email: Some("alice@voidforge.test".to_owned()),
            name: None,
            exp: 1,
        };
        let shown = format!("{l:?}");
        assert!(
            !shown.contains("eyJ") && !shown.contains("alice"),
            "{shown}"
        );
    }
}
