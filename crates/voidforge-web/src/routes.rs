//! The routes (P4.1, D-057): Home, the login, its callback and the logout.
//!
//! - `GET /` shows Home. It reads the public `GET /rules` for the fast-rules line and, logged in,
//!   `GET /players/me` with the ID token, side by side, on every render (P4.2, D-059). An expired login, or
//!   a 401 from the API, ends the session and shows the expired notice; a full universe is its
//!   own page; any other failure is the retry page with the API's status (§5.5).
//! - `GET /login?next=<local path>` starts an attempt and sends the browser to dex.
//! - `GET /callback` takes the attempt matching `state` out of the session (once), exchanges the
//!   code, checks the ID token, then gives the session a **new id** (against fixation) and
//!   returns to `next`. Every refusal is a page and leaves no login.
//! - HEAD is 404 on every path, before any handler runs.
//! - `POST /logout` ends the session. It's a POST, and the cookie is SameSite=Lax, so another
//!   site can't log a user out with a link or a form.
//!
//! Every response says `Cache-Control: no-store` and `Referrer-Policy: no-referrer`: pages
//! show the viewer's email, and the callback's URL holds a code.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::header::{CACHE_CONTROL, LOCATION, REFERRER_POLICY, RETRY_AFTER};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use maud::Markup;
use serde::Deserialize;
use tower_sessions::cookie::SameSite;
use tower_sessions::cookie::time::{Duration as CookieDuration, OffsetDateTime};
use tower_sessions::{Expiry, Session, SessionManagerLayer};

use crate::api::Api;
use crate::messages::{self, Context, Fill, Kind};
use crate::oidc::{self, LoginError, Provider};
use crate::pages::{self, Frame, Notice, Refusal};
use crate::session::{self, LOGIN_KEY, LoggedIn, PENDING_KEY, PENDING_LIFETIME_SECS};
use crate::store::BoundedStore;

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
    /// The API's client.
    pub api: Api,
    /// The bounded session store (R1-1).
    pub sessions: BoundedStore,
}

/// The web app: the routes, the session layer and the headers.
pub fn app(state: AppState, secure_cookie: bool) -> Router {
    // A session nobody logged in lives 10 minutes after its last change, as its login attempts
    // do; a login sets its own expiry (`login_expiry`), and every handler that changes a
    // logged-in session sets it again, since a save recomputes the expiry from this default.
    let sessions = SessionManagerLayer::new(state.sessions.clone())
        .with_expiry(anonymous_expiry())
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
        .layer(axum::middleware::from_fn(refuse_head))
        .layer(axum::middleware::map_response(headers))
        .with_state(state)
}

/// HEAD is refused before the session layer or any handler runs (QA's I1): axum answers HEAD
/// with the GET handler, which would call the API, spend the player's read and could join them
/// (D-047), for a body nobody reads. 404, as the API answers HEAD (D-055).
async fn refuse_head(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if request.method() == axum::http::Method::HEAD {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(request).await
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

fn anonymous_expiry() -> Expiry {
    Expiry::OnInactivity(CookieDuration::seconds(
        i64::try_from(PENDING_LIFETIME_SECS).unwrap_or(i64::MAX),
    ))
}

/// A logged-in session's expiry: the token's `exp` plus the grace, or None past the calendar.
fn login_expiry(exp: u64) -> Option<Expiry> {
    let until = exp.saturating_add(EXPIRED_GRACE_SECS);
    OffsetDateTime::from_unix_timestamp(i64::try_from(until).ok()?)
        .ok()
        .map(Expiry::AtDateTime)
}

/// Sets the expiry a changed session must keep: a logged-in one its login's, any other the
/// anonymous one. A save recomputes the expiry from the layer's default otherwise, which would
/// cut a logged-in session to 10 minutes.
async fn keep_expiry(session: &Session) -> Result<(), session::SessionError> {
    let expiry = match session.get::<LoggedIn>(LOGIN_KEY).await? {
        Some(l) => login_expiry(l.exp).unwrap_or_else(anonymous_expiry),
        None => anonymous_expiry(),
    };
    session.set_expiry(Some(expiry));
    Ok(())
}

/// The 503 page for a new session past the store's cap, logged without a flood (QA's D2).
fn busy(s: &AppState) -> Response {
    s.sessions.refused();
    page(StatusCode::SERVICE_UNAVAILABLE, pages::busy())
}

/// The rules' name for the fast test rules: the frame says so (phase4-ux §2).
pub const FAST_RULES: &str = "v0-fast";

/// Whether the universe runs the fast test rules. A failed read only drops the line (§5.7).
async fn fast_rules(s: &AppState) -> bool {
    s.api.rules().await.is_ok_and(|r| r.name == FAST_RULES)
}

/// Ends the session and shows the expired notice, offering to return to `next`.
async fn expired(session: &Session, next: &str, fast: bool) -> Response {
    if let Err(e) = session.flush().await {
        tracing::error!(error = %e, "ending an expired session failed");
        return internal();
    }
    page(
        StatusCode::OK,
        pages::home_logged_out(Some(Notice::Expired), &login_href(next), fast),
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
    // `/rules` on every page, so the expired and full-universe pages keep the line too (D3). It's
    // read beside `/players/me`, not before it, and has its own short bound, so a hung API holds
    // the page for one call's bound at most (QA's F1).
    let Some(login) = login else {
        let fast = fast_rules(&s).await;
        return page(StatusCode::OK, pages::home_logged_out(None, "/login", fast));
    };
    let Some(now) = oidc::unix_now() else {
        return internal();
    };
    if now >= login.exp {
        tracing::info!(outcome = "expired", "the login expired");
        let fast = fast_rules(&s).await;
        return expired(&session, "/", fast).await;
    }
    let (fast, me) = tokio::join!(fast_rules(&s), s.api.me(&login.id_token));
    let error = match me {
        Ok(me) => return page(StatusCode::OK, pages::home_logged_in(&login, &me, fast)),
        Err(e) => e,
    };
    let m = messages::for_error(&error, Context::Page, Fill::default());
    match m.kind {
        Kind::Login => {
            tracing::info!(outcome = "api_unauthorized", "the API refused the token");
            expired(&session, "/", fast).await
        }
        Kind::UniverseFull => page(StatusCode::OK, pages::universe_full(fast, &m)),
        Kind::Retry {
            status,
            retry_after,
        } => retry(
            status,
            retry_after,
            &pages::retry_page(frame(fast, &login), &m, "/"),
        ),
        // A read the API refused for a reason a page can't act on: the page can't be shown.
        Kind::Banner => retry(
            StatusCode::BAD_GATEWAY.as_u16(),
            None,
            &pages::retry_page(frame(fast, &login), &m, "/"),
        ),
    }
}

fn frame(fast: bool, login: &LoggedIn) -> Frame<'_> {
    Frame {
        fast,
        login: Some(login),
        me: None,
    }
}

/// The retry page with the API's status, and its Retry-After passed on (§5.5).
fn retry(status: u16, retry_after: Option<u64>, markup: &Markup) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = page(status, markup.clone());
    if let Some(secs) = retry_after {
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from(secs));
    }
    response
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
    // A new session past the store's cap is refused (R1-1), never evicting another. This check
    // is the fast path; the save below decides a race at the boundary (QA's D1).
    let new = session.id().is_none();
    if new && !s.sessions.has_room() {
        return busy(&s);
    }
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
    if let Err(e) = keep_expiry(&session).await {
        tracing::error!(error = %e, "reading the session failed");
        return internal();
    }
    // Saved here rather than by the layer after the handler, so a store refused at the cap
    // (another login took the last place since the check) is the busy page, not a bare 500.
    // The store checks and inserts under one lock. A 503 makes the layer skip its own save.
    if new {
        if session.save().await.is_err() {
            return busy(&s);
        }
        s.sessions.admitted();
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

/// A refused callback's page. It never logs anyone in, and it says whether the browser is still
/// logged in from before: a replayed callback keeps the session it found (QA's F1).
async fn refusal(session: &Session, status: StatusCode, why: Refusal) -> Response {
    tracing::warn!(outcome = "refused", reason = ?why, "a login callback was refused");
    let logged_in = matches!(session.get::<LoggedIn>(LOGIN_KEY).await, Ok(Some(_)));
    page(status, pages::refused(why, logged_in))
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
            // Taking it changed the session: keep a login it already had.
            Ok(p) => match keep_expiry(&session).await {
                Ok(()) => p,
                Err(e) => {
                    tracing::error!(error = %e, "reading the session failed");
                    return internal();
                }
            },
            Err(e) => {
                tracing::error!(error = %e, "reading the session failed");
                return internal();
            }
        },
        None => None,
    };
    if q.error.is_some() {
        return refusal(&session, StatusCode::BAD_REQUEST, Refusal::Provider).await;
    }
    let Some(pending) = pending else {
        return refusal(&session, StatusCode::BAD_REQUEST, Refusal::UnknownAttempt).await;
    };
    let Some(code) = q.code.as_deref().filter(|c| !c.is_empty()) else {
        return refusal(&session, StatusCode::BAD_REQUEST, Refusal::NoCode).await;
    };
    let identity = match s.provider.finish(code, &pending).await {
        Ok(i) => i,
        Err(LoginError::KeysUnavailable) => {
            return refusal(
                &session,
                StatusCode::SERVICE_UNAVAILABLE,
                Refusal::Unavailable,
            )
            .await;
        }
        Err(LoginError::Random) => return internal(),
        Err(e) => {
            tracing::warn!(error = %e, "the login's answer failed a check");
            return refusal(&session, StatusCode::BAD_REQUEST, Refusal::Rejected).await;
        }
    };
    let Some(expiry) = login_expiry(identity.exp) else {
        return refusal(&session, StatusCode::BAD_REQUEST, Refusal::Rejected).await;
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
    session.set_expiry(Some(expiry));
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
