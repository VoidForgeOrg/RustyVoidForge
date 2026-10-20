//! What the session holds (P4.1, D-057): the login attempts waiting for their callback, and
//! after a login, the ID token and what Home shows. The store is tower-sessions' in-memory one,
//! so every session ends when the process does. The browser holds only the session cookie.

use std::fmt;

use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::config::Secret;
use crate::oidc::{Identity, PendingLogin};

/// The session key of the pending logins.
pub const PENDING_KEY: &str = "pending";
/// The session key of the login.
pub const LOGIN_KEY: &str = "login";
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

    use tower_sessions::MemoryStore;

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
        Session::new(None, Arc::new(MemoryStore::default()), None)
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
