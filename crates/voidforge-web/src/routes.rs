//! The routes (P4.1, D-057): Home, the login, its callback and the logout.
//!
//! - `GET /` shows Home. Logged in, it calls `GET /players/me` with the ID token on every render;
//!   an expired login, or a 401 from the API, ends the session and shows the expired notice.
//! - `GET /login?next=<local path>` starts an attempt and sends the browser to dex.
//! - `GET /callback` takes the attempt matching `state` out of the session (once), exchanges the
//!   code, checks the ID token, then gives the session a **new id** (against fixation) and
//!   returns to `next`. Every refusal is a page and leaves no login.
//! - `POST /logout` ends the session. It's a POST, and the cookie is SameSite=Lax, so another
//!   site can't log a user out with a link or a form.
//!
//! Every response says `Cache-Control: no-store` and `Referrer-Policy: no-referrer`: pages
//! show the viewer's email, and the callback's URL holds a code.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::header::{CACHE_CONTROL, LOCATION, REFERRER_POLICY};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use maud::Markup;
use serde::Deserialize;
use tower_sessions::cookie::SameSite;
use tower_sessions::cookie::time::{Duration as CookieDuration, OffsetDateTime};
use tower_sessions::{Expiry, MemoryStore, Session, SessionManagerLayer};

use crate::api::{self, MeAnswer};
use crate::oidc::{self, LoginError, Provider};
use crate::pages::{self, Notice, Refusal};
use crate::session::{self, LOGIN_KEY, LoggedIn, PENDING_KEY, PENDING_LIFETIME_SECS};

/// The session cookie's name.
pub const COOKIE_NAME: &str = "voidforge_session";
/// The longest `next` path kept.
pub const MAX_NEXT_BYTES: usize = 512;
/// How long a session outlives its token. Within it, the next page sees the expired login and
/// says so (P4.1 A4); a session ending exactly at `exp` would vanish first, cookie and all, and
/// the user would only see a plain logged-out page. The token itself is never used past `exp`.
pub const EXPIRED_GRACE_SECS: u64 = 24 * 60 * 60;

/// What the handlers share.
#[derive(Clone)]
pub struct AppState {
    /// The issuer and its keys.
    pub provider: Arc<Provider>,
    /// The API's HTTP client.
    pub api: reqwest::Client,
    /// The API's base URL.
    pub api_url: String,
}

/// The web app: the routes, the session layer and the headers.
pub fn app(state: AppState, secure_cookie: bool) -> Router {
    let sessions = SessionManagerLayer::new(MemoryStore::default())
        .with_name(COOKIE_NAME)
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(secure_cookie)
        .with_path("/");
    Router::new()
        .route("/", get(home))
        .route("/login", get(login))
        .route("/callback", get(callback))
        .route("/logout", post(logout))
        .layer(sessions)
        .layer(axum::middleware::map_response(headers))
        .with_state(state)
}

async fn headers(mut response: Response) -> Response {
    let h = response.headers_mut();
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}

fn see_other(to: &str) -> Response {
    match HeaderValue::from_str(to) {
        Ok(location) => (StatusCode::SEE_OTHER, [(LOCATION, location)]).into_response(),
        Err(_) => internal(),
    }
}

fn page(status: StatusCode, markup: Markup) -> Response {
    (status, markup).into_response()
}

fn internal() -> Response {
    page(StatusCode::INTERNAL_SERVER_ERROR, pages::internal())
}

/// A local path to return to after logging in, or None: it must start with one `/`, and hold
/// no backslash or control character, so it can't name another site.
#[must_use]
pub fn local_path(next: &str) -> Option<&str> {
    let ok = next.starts_with('/')
        && !next.starts_with("//")
        && !next.contains('\\')
        && !next.chars().any(char::is_control)
        && next.len() <= MAX_NEXT_BYTES;
    ok.then_some(next)
}

fn login_href(next: &str) -> String {
    if next == "/" {
        "/login".to_owned()
    } else {
        reqwest::Url::parse_with_params("http://x/login", [("next", next)]).map_or_else(
            |_| "/login".to_owned(),
            |u| format!("/login?{}", u.query().unwrap_or("")),
        )
    }
}

/// Ends the session and shows the expired notice, offering to return to `next`.
async fn expired(session: &Session, next: &str) -> Response {
    if let Err(e) = session.flush().await {
        tracing::error!(error = %e, "ending an expired session failed");
        return internal();
    }
    page(
        StatusCode::OK,
        pages::home_logged_out(Some(Notice::Expired), &login_href(next)),
    )
}

#[tracing::instrument(name = "web.home", skip_all)]
async fn home(State(s): State<AppState>, session: Session) -> Response {
    let login = match session.get::<LoggedIn>(LOGIN_KEY).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, "reading the session failed");
            return internal();
        }
    };
    let Some(login) = login else {
        return page(StatusCode::OK, pages::home_logged_out(None, "/login"));
    };
    let Some(now) = oidc::unix_now() else {
        return internal();
    };
    if now >= login.exp {
        tracing::info!(outcome = "expired", "the login expired");
        return expired(&session, "/").await;
    }
    let me = api::player_me(&s.api, &s.api_url, &login.id_token).await;
    if matches!(me, MeAnswer::Unauthorized) {
        tracing::info!(outcome = "api_unauthorized", "the API refused the token");
        return expired(&session, "/").await;
    }
    page(StatusCode::OK, pages::home_logged_in(&login, &me))
}

#[derive(Deserialize)]
struct LoginQuery {
    next: Option<String>,
}

#[tracing::instrument(name = "web.login", skip_all)]
async fn login(
    State(s): State<AppState>,
    session: Session,
    Query(q): Query<LoginQuery>,
) -> Response {
    let next = q
        .next
        .as_deref()
        .and_then(local_path)
        .unwrap_or("/")
        .to_owned();
    let Some(now) = oidc::unix_now() else {
        return internal();
    };
    let (url, pending) = match s.provider.begin(next, now) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "starting a login failed");
            return internal();
        }
    };
    if let Err(e) = session::add_pending(&session, pending, now).await {
        tracing::error!(error = %e, "storing the login attempt failed");
        return internal();
    }
    // A session that isn't logged in lives as long as its attempts; a logged-in one keeps the
    // expiry its token gave it.
    match session.get::<LoggedIn>(LOGIN_KEY).await {
        Ok(None) => session.set_expiry(Some(Expiry::OnInactivity(CookieDuration::seconds(
            i64::try_from(PENDING_LIFETIME_SECS).unwrap_or(i64::MAX),
        )))),
        Ok(Some(_)) => {}
        Err(e) => {
            tracing::error!(error = %e, "reading the session failed");
            return internal();
        }
    }
    tracing::info!(outcome = "redirected", "a login started");
    see_other(&url)
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

fn refusal(status: StatusCode, why: Refusal) -> Response {
    tracing::warn!(outcome = "refused", reason = ?why, "a login callback was refused");
    page(status, pages::refused(why))
}

#[tracing::instrument(name = "web.callback", skip_all)]
async fn callback(
    State(s): State<AppState>,
    session: Session,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let Some(now) = oidc::unix_now() else {
        return internal();
    };
    // The attempt is taken out first, whatever else the callback says, so it can't be used
    // twice.
    let pending = match q.state.as_deref() {
        Some(state) => match session::take_pending(&session, state, now).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "reading the session failed");
                return internal();
            }
        },
        None => None,
    };
    if q.error.is_some() {
        return refusal(StatusCode::BAD_REQUEST, Refusal::Provider);
    }
    let Some(pending) = pending else {
        return refusal(StatusCode::BAD_REQUEST, Refusal::UnknownAttempt);
    };
    let Some(code) = q.code.as_deref().filter(|c| !c.is_empty()) else {
        return refusal(StatusCode::BAD_REQUEST, Refusal::NoCode);
    };
    let identity = match s.provider.finish(code, &pending).await {
        Ok(i) => i,
        Err(LoginError::KeysUnavailable) => {
            return refusal(StatusCode::SERVICE_UNAVAILABLE, Refusal::Unavailable);
        }
        Err(LoginError::Random) => return internal(),
        Err(e) => {
            tracing::warn!(error = %e, "the login's answer failed a check");
            return refusal(StatusCode::BAD_REQUEST, Refusal::Rejected);
        }
    };
    let until = identity.exp.saturating_add(EXPIRED_GRACE_SECS);
    let Ok(session_end) =
        OffsetDateTime::from_unix_timestamp(i64::try_from(until).unwrap_or(i64::MAX))
    else {
        return refusal(StatusCode::BAD_REQUEST, Refusal::Rejected);
    };
    // A new session id at login, so an id planted before it is worthless (fixation).
    let stored = async {
        session.cycle_id().await?;
        session.remove::<serde_json::Value>(PENDING_KEY).await?;
        session.insert(LOGIN_KEY, LoggedIn::from(identity)).await
    };
    if let Err(e) = stored.await {
        tracing::error!(error = %e, "storing the login failed");
        return internal();
    }
    session.set_expiry(Some(Expiry::AtDateTime(session_end)));
    tracing::info!(outcome = "logged_in", "a login completed");
    see_other(&pending.next)
}

#[tracing::instrument(name = "web.logout", skip_all)]
async fn logout(session: Session) -> Response {
    if let Err(e) = session.flush().await {
        tracing::error!(error = %e, "ending the session failed");
        return internal();
    }
    tracing::info!(outcome = "logged_out", "a logout");
    see_other("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_paths_are_returned_to() {
        for good in ["/", "/planets/3", "/fleets?x=1"] {
            assert_eq!(local_path(good), Some(good));
        }
        let long = format!("/{}", "a".repeat(MAX_NEXT_BYTES));
        for bad in [
            "",
            "//evil.example",
            "/\\evil.example",
            "https://evil.example",
            "evil",
            "/a\nb",
            long.as_str(),
        ] {
            assert_eq!(local_path(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_login_link_carries_next() {
        assert_eq!(login_href("/"), "/login");
        assert_eq!(login_href("/fleets?x=1"), "/login?next=%2Ffleets%3Fx%3D1");
    }
}
