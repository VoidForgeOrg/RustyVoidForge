//! The routes: Home, the login, its callback and the logout (P4.1, D-057), and the game's pages
//! (P4.3, D-060).
//!
//! - Every logged-in page starts the same way ([`begin`]): the login, or the logged-out Home at
//!   that URL with Log in returning there (§4.6, E4), or the expired notice. Then it makes all
//!   its reads **at once** (T1): the public `GET /rules` for the fast-rules line, `GET
//!   /players/me` (the read that joins, D-047) and the page's own, each within its bound, so a
//!   hung API holds a page for one bound. The Universe's centre needs a read before its search.
//! - The first failing read decides the page ([`failed`]), `/players/me` first: an expired login
//!   or a 401 ends the session and shows the expired notice; a full universe is its own page;
//!   any other failure is the retry page with the API's status (§5.5). A failed `/rules` only
//!   drops its line (§5.7).
//! - `GET /` is Home (§4.1); `/planets/{id}` (§4.2), `/fleets` (§4.3), `/universe` (§4.4) and
//!   `/leaderboard` (§4.5) are the game's pages. A planet number or a search the web can't use
//!   is refused before any call (T5). Any other path is §6.2's Not found.
//! - `GET /login?next=<local path>` starts an attempt and sends the browser to dex.
//! - `GET /callback` takes the attempt matching `state` out of the session (once), exchanges the
//!   code, checks the ID token, then gives the session a **new id** (against fixation) and
//!   returns to `next`. Every refusal is a page and leaves no login.
//! - HEAD is 404 on every path, before any handler runs.
//! - `POST /logout` ends the session and returns to `/?logged_out`, whose logged-out Home says
//!   so (§4.1, E9). It's a POST, and the cookie is SameSite=Lax, so another site can't log a user
//!   out with a link or a form.
//!
//! - `POST /act` is every form's target (P4.4, D-061; phase4-ux §5.1): the session's form-key
//!   check (Q3, also the CSRF check), the web's own input check (§6.3), then exactly one `POST
//!   /commands` with the form key as its `Idempotency-Key`, and a 303 back with the outcome in the
//!   one-shot banner; a retryable failure is §5.5's retry page, re-posting the same key. Nothing
//!   is ever sent twice by the web, and a 401 or an expired login sends nothing again by itself.
//! - `GET /planets/{p}/cancel-construction/{b}`, `/planets/{p}/demolish/{b}`,
//!   `/planets/{p}/cancel-build/{n}` and `/fleets/{f}/turn-back` are §5.4's confirm pages: they
//!   only read and render the real form with their own key; the link to them carries none.
//!
//! Every response says `Cache-Control: no-store` and `Referrer-Policy: no-referrer`: pages
//! show the viewer's email, and the callback's URL holds a code.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::header::{CACHE_CONTROL, LOCATION, REFERRER_POLICY, RETRY_AFTER};
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use maud::Markup;
use serde::Deserialize;
use tower_sessions::cookie::SameSite;
use tower_sessions::cookie::time::{Duration as CookieDuration, OffsetDateTime};
use tower_sessions::{Expiry, Session, SessionManagerLayer};

use voidforge_api_types::command::IdempotencyKey;
use voidforge_api_types::error::ErrorCode;
use voidforge_api_types::rules::RulesView;
use voidforge_api_types::wire::{PlanetId, Position};

use crate::api::{Api, ApiError};
use crate::format;
use crate::forms::{self, Posted};
use crate::messages::{self, Context, Fill, Kind, Message};
use crate::oidc::{self, LoginError, Provider};
use crate::pages::{self, Found, Frame, Nav, Notice, Refusal, SearchForm};
use crate::session::{
    self, Banner, KeyState, LOGIN_KEY, LoggedIn, NOTICE_KEY, PENDING_KEY, PENDING_LIFETIME_SECS,
    RENEWING_KEY,
};
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
        .route("/planets/{id}", get(planet))
        .route("/fleets", get(fleets))
        .route("/universe", get(universe))
        .route("/leaderboard", get(leaderboard))
        .route("/act", post(act))
        .route(
            "/planets/{id}/cancel-construction/{building}",
            get(confirm_cancel_construction),
        )
        .route("/planets/{id}/demolish/{building}", get(confirm_demolish))
        .route(
            "/planets/{id}/cancel-build/{build}",
            get(confirm_cancel_build),
        )
        .route("/fleets/{id}/turn-back", get(confirm_turn_back))
        .fallback(not_found)
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

/// The public rules, or None when the read failed: a page then only drops what it would have
/// shown from them (§5.7).
async fn rules(s: &AppState) -> Option<RulesView> {
    s.api.rules().await.ok()
}

/// Whether these rules are the fast test rules (§2).
fn is_fast(rules: Option<&RulesView>) -> bool {
    rules.is_some_and(|r| r.name == FAST_RULES)
}

/// Whether the universe runs the fast test rules. A failed read only drops the line (§5.7).
async fn fast_rules(s: &AppState) -> bool {
    is_fast(rules(s).await.as_ref())
}

/// The Universe page's default radius: 100 u around the homeworld, a UI default (Q5).
pub const DEFAULT_RADIUS_DU: i64 = 100_000;

/// Ends the session. The fresh session it leaves is marked [`RENEWING_KEY`] with `renewing`, the
/// banner the first page after logging in again shows once (§5.6, §5.1); it lives as long as a
/// login attempt. With the store full, it's left unmarked: the line is a courtesy, never a reason
/// to fail the page. Err is the page for a store that failed.
async fn end_login(s: &AppState, session: &Session, renewing: Banner) -> Result<(), Box<Response>> {
    if let Err(e) = session.flush().await {
        tracing::error!(error = %e, "ending an expired session failed");
        return Err(Box::new(internal()));
    }
    if s.sessions.has_room() {
        let marked = async {
            session.insert(RENEWING_KEY, renewing).await?;
            session.set_expiry(Some(anonymous_expiry()));
            session.save().await
        };
        if marked.await.is_ok() {
            s.sessions.admitted();
        } else if let Err(e) = session.flush().await {
            tracing::error!(error = %e, "ending an expired session failed");
            return Err(Box::new(internal()));
        }
    }
    Ok(())
}

/// Ends the session and shows `notice` on the logged-out Home, offering to return to `next`;
/// logging in again from there says so once (§5.6).
async fn expired_with(
    s: &AppState,
    session: &Session,
    next: &str,
    fast: bool,
    notice: Notice,
) -> Response {
    if let Err(r) = end_login(s, session, Banner::line(pages::RENEWED)).await {
        return *r;
    }
    let next = local_path(next).unwrap_or("/");
    page(
        StatusCode::OK,
        pages::home_logged_out(Some(notice), &login_href(next), fast),
    )
}

/// Ends an expired login and says so (P4.1 A4).
async fn expired(s: &AppState, session: &Session, next: &str, fast: bool) -> Response {
    expired_with(s, session, next, fast, Notice::Expired).await
}

/// The page a request is on, path and query: Refresh and Try again link to it, and a login
/// returns to it.
fn here(uri: &Uri) -> String {
    uri.path_and_query().map_or("/", |p| p.as_str()).to_owned()
}

/// A logged-in viewer: the login, and the one-shot banner this page shows.
struct Viewer {
    login: LoggedIn,
    banner: Option<Banner>,
}

/// A logged-in page's start: the viewer, or the response for one without a usable login. A
/// logged-out viewer gets the logged-out Home at this URL, with `notice` and Log in returning
/// here (§4.6, E4); an expired login ends the session and says so. Logged in, it calls nothing,
/// and takes the session's one-shot notice (§5.6), changing the session only when there is one.
async fn begin(
    s: &AppState,
    session: &Session,
    here: &str,
    notice: Option<Notice>,
) -> Result<Viewer, Box<Response>> {
    let login = match session.get::<LoggedIn>(LOGIN_KEY).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, "reading the session failed");
            return Err(Box::new(internal()));
        }
    };
    // `/rules` for the logged-out pages too, so they keep the fast-rules line (D3).
    let Some(login) = login else {
        let fast = fast_rules(s).await;
        let next = local_path(here).unwrap_or("/");
        return Err(Box::new(page(
            StatusCode::OK,
            pages::home_logged_out(notice, &login_href(next), fast),
        )));
    };
    let Some(now) = oidc::unix_now() else {
        return Err(Box::new(internal()));
    };
    if now >= login.exp {
        tracing::info!(outcome = "expired", "the login expired");
        let fast = fast_rules(s).await;
        return Err(Box::new(expired(s, session, here, fast).await));
    }
    let banner = match session.get::<Banner>(NOTICE_KEY).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "reading the session failed");
            return Err(Box::new(internal()));
        }
    };
    if banner.is_some() {
        let taken = async {
            session.remove::<Banner>(NOTICE_KEY).await?;
            keep_expiry(session).await
        };
        if let Err(e) = taken.await {
            tracing::error!(error = %e, "taking the notice failed");
            return Err(Box::new(internal()));
        }
    }
    Ok(Viewer { login, banner })
}

/// A failed read, as its page (§5.5, §6): the expired notice for a refused login, the full
/// universe, or the retry page with the API's status. `fill` names the page's planet.
async fn failed(
    s: &AppState,
    session: &Session,
    frame: Frame<'_>,
    error: &ApiError,
    fill: Fill,
    here: &str,
) -> Response {
    let m = messages::for_error(error, Context::Page, fill);
    match m.kind {
        Kind::Login => {
            tracing::info!(outcome = "api_unauthorized", "the API refused the token");
            expired(s, session, here, frame.fast).await
        }
        Kind::UniverseFull => page(StatusCode::OK, pages::universe_full(frame.fast, &m)),
        Kind::Retry {
            status,
            retry_after,
        } => retry(status, retry_after, &pages::retry_page(frame, &m, here)),
        // A read the API refused for a reason a page can't act on: the page can't be shown.
        Kind::Banner => retry(
            StatusCode::BAD_GATEWAY.as_u16(),
            None,
            &pages::retry_page(frame, &m, here),
        ),
    }
}

fn frame(fast: bool, viewer: &Viewer, nav: Option<Nav>) -> Frame<'_> {
    Frame {
        fast,
        login: Some(&viewer.login),
        me: None,
        nav,
        banner: viewer.banner.as_ref(),
    }
}

/// A fresh form key for the page being rendered (P4.4's T1), kept in the session (§5.1.1); Err
/// is the page for a store or CSPRNG that failed.
async fn form_key(session: &Session) -> Result<String, Box<Response>> {
    let issued = async {
        let key = session::issue_form_key(session).await?;
        keep_expiry(session).await?;
        Ok::<_, session::SessionError>(key)
    };
    match issued.await {
        Ok(Some(key)) => Ok(key),
        Ok(None) => {
            tracing::error!("the CSPRNG failed to make a form key");
            Err(Box::new(internal()))
        }
        Err(e) => {
            tracing::error!(error = %e, "keeping a form key failed");
            Err(Box::new(internal()))
        }
    }
}

/// A planet number as the web takes it: digits only, within u64 (T5).
fn planet_number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

#[tracing::instrument(name = "web.home", skip_all)]
async fn home(State(s): State<AppState>, session: Session, uri: Uri) -> Response {
    let notice = (uri.query() == Some("logged_out")).then_some(Notice::LoggedOut);
    let viewer = match begin(&s, &session, "/", notice).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let (fast, me) = tokio::join!(fast_rules(&s), s.api.me(&viewer.login.id_token));
    match me {
        Ok(me) => page(
            StatusCode::OK,
            pages::home_logged_in(&viewer.login, &me, fast, viewer.banner.as_ref()),
        ),
        Err(e) => {
            let frame = frame(fast, &viewer, Some(Nav::Home));
            failed(&s, &session, frame, &e, Fill::default(), "/").await
        }
    }
}

#[tracing::instrument(name = "web.planet", skip_all)]
async fn planet(State(s): State<AppState>, session: Session, uri: Uri) -> Response {
    let here = here(&uri);
    let viewer = match begin(&s, &session, &here, None).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    // Checked before any game call (T5); the public `/rules` still keeps the frame whole (R3).
    let raw = uri.path().strip_prefix("/planets/").unwrap_or("");
    let Some(id) = planet_number(raw) else {
        let fast = fast_rules(&s).await;
        let m = Message {
            copy: pages::NOT_A_PLANET_NUMBER.to_owned(),
            code: Some(ErrorCode::InvalidQuery.as_str()),
            kind: Kind::Banner,
        };
        return page(
            StatusCode::BAD_REQUEST,
            pages::no_such_planet(frame(fast, &viewer, None), &m),
        );
    };
    let token = &viewer.login.id_token;
    let (rules, me, planet, fleets) = tokio::join!(
        rules(&s),
        s.api.me(token),
        s.api.planet(token, PlanetId(id)),
        s.api.my_fleets(token)
    );
    let fast = is_fast(rules.as_ref());
    let fill = Fill {
        planet: Some(id),
        target: None,
    };
    let frame = frame(fast, &viewer, None);
    let me = match me {
        Ok(me) => me,
        Err(e) => return failed(&s, &session, frame, &e, fill, &here).await,
    };
    let frame = Frame {
        me: Some(&me),
        ..frame
    };
    let planet = match planet {
        Ok(p) => p,
        Err(
            e @ ApiError::Refused {
                code: ErrorCode::UnknownPlanet,
                ..
            },
        ) => {
            let m = messages::for_error(&e, Context::Page, fill);
            return page(StatusCode::NOT_FOUND, pages::no_such_planet(frame, &m));
        }
        Err(e) => return failed(&s, &session, frame, &e, fill, &here).await,
    };
    let fleets = match fleets {
        Ok(f) => f,
        Err(e) => return failed(&s, &session, frame, &e, fill, &here).await,
    };
    let key = match form_key(&session).await {
        Ok(k) => k,
        Err(r) => return *r,
    };
    page(
        StatusCode::OK,
        pages::planet(frame, &here, &me, &planet, &fleets, rules.as_ref(), &key),
    )
}

#[tracing::instrument(name = "web.fleets", skip_all)]
async fn fleets(State(s): State<AppState>, session: Session, uri: Uri) -> Response {
    let here = here(&uri);
    let viewer = match begin(&s, &session, &here, None).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let token = &viewer.login.id_token;
    let (fast, me, fleets) = tokio::join!(fast_rules(&s), s.api.me(token), s.api.my_fleets(token));
    let frame = frame(fast, &viewer, Some(Nav::Fleets));
    let me = match me {
        Ok(me) => me,
        Err(e) => return failed(&s, &session, frame, &e, Fill::default(), &here).await,
    };
    let frame = Frame {
        me: Some(&me),
        ..frame
    };
    let fleets = match fleets {
        Ok(f) => f,
        Err(e) => return failed(&s, &session, frame, &e, Fill::default(), &here).await,
    };
    let key = match form_key(&session).await {
        Ok(k) => k,
        Err(r) => return *r,
    };
    // §4.3: `?target=` pre-fills every Send form; anything but a planet number is ignored.
    let target = query_pairs(uri.query())
        .get("target")
        .and_then(|t| planet_number(t));
    page(
        StatusCode::OK,
        pages::fleets(frame, &here, &me, &fleets, &key, target),
    )
}

#[tracing::instrument(name = "web.leaderboard", skip_all)]
async fn leaderboard(State(s): State<AppState>, session: Session, uri: Uri) -> Response {
    let here = here(&uri);
    let viewer = match begin(&s, &session, &here, None).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let token = &viewer.login.id_token;
    let (fast, me, board) = tokio::join!(fast_rules(&s), s.api.me(token), s.api.leaderboard(token));
    let frame = frame(fast, &viewer, Some(Nav::Leaderboard));
    let me = match me {
        Ok(me) => me,
        Err(e) => return failed(&s, &session, frame, &e, Fill::default(), &here).await,
    };
    let frame = Frame {
        me: Some(&me),
        ..frame
    };
    match board {
        Ok(board) => page(
            StatusCode::OK,
            pages::leaderboard(frame, &here, &me, &board),
        ),
        // Before the board's first build: §6.1's leaderboard line, as the retry page.
        Err(ApiError::Refused {
            code: ErrorCode::Unavailable,
            status,
            retry_after,
        }) => {
            let m = Message {
                copy: messages::with_wait(pages::LEADERBOARD_NOT_READY.to_owned(), retry_after),
                code: Some(ErrorCode::Unavailable.as_str()),
                kind: Kind::Retry {
                    status,
                    retry_after,
                },
            };
            retry(status, retry_after, &pages::retry_page(frame, &m, &here))
        }
        Err(e) => failed(&s, &session, frame, &e, Fill::default(), &here).await,
    }
}

/// A query string's fields, decoded; the last of a repeated one wins.
fn query_pairs(query: Option<&str>) -> std::collections::HashMap<String, String> {
    reqwest::Url::parse(&format!("http://x/?{}", query.unwrap_or("")))
        .map(|u| u.query_pairs().into_owned().collect())
        .unwrap_or_default()
}

/// Where a Universe search is centred.
enum Centre {
    /// The viewer's homeworld, at the default radius (Q5).
    Home,
    /// A planet, at the default radius (`?around=`).
    Around(u64),
    /// A point and a radius, in distance units (`?x=&y=&z=&radius=`).
    At(Position, i64),
}

/// The search a Universe query asks for, and the form as typed; Err when the web can't use it
/// (§4.4, §6.3). Any of x, y, z or radius makes it an explicit search, which needs all four.
fn search(query: Option<&str>) -> (Result<Centre, ()>, SearchForm) {
    let pairs = query_pairs(query);
    let field = |k: &str| pairs.get(k).cloned().unwrap_or_default();
    let typed = SearchForm {
        x: field("x"),
        y: field("y"),
        z: field("z"),
        radius: field("radius"),
    };
    let explicit = ["x", "y", "z", "radius"]
        .iter()
        .any(|k| pairs.contains_key(*k));
    let centre = if explicit {
        let radius = format::parse_units(&typed.radius).filter(|r| *r >= 0);
        match (
            format::parse_units(&typed.x),
            format::parse_units(&typed.y),
            format::parse_units(&typed.z),
            radius,
        ) {
            (Some(x), Some(y), Some(z), Some(r)) => Ok(Centre::At(Position { x, y, z }, r)),
            _ => Err(()),
        }
    } else if let Some(around) = pairs.get("around") {
        planet_number(around).map(Centre::Around).ok_or(())
    } else {
        Ok(Centre::Home)
    };
    (centre, typed)
}

#[tracing::instrument(name = "web.universe", skip_all)]
async fn universe(State(s): State<AppState>, session: Session, uri: Uri) -> Response {
    let here = here(&uri);
    let viewer = match begin(&s, &session, &here, None).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let (centre, typed) = search(uri.query());
    // Checked before any game call (T5); the public `/rules` still keeps the frame whole (R3).
    let Ok(centre) = centre else {
        let fast = fast_rules(&s).await;
        let found = Found::Refused(pages::UNIVERSE_INPUT, None);
        return page(
            StatusCode::BAD_REQUEST,
            pages::universe(
                frame(fast, &viewer, Some(Nav::Universe)),
                &here,
                &typed,
                found,
            ),
        );
    };
    let token = &viewer.login.id_token;
    // The centre's read goes beside `/players/me` when the query names it; the default needs
    // `/players/me` first, for the homeworld.
    let around = match centre {
        Centre::Around(id) => Some(id),
        Centre::Home | Centre::At(..) => None,
    };
    let centre_read = async {
        match around {
            Some(id) => Some(s.api.planet(token, PlanetId(id)).await),
            None => None,
        }
    };
    let (fast, me, around_view) = tokio::join!(fast_rules(&s), s.api.me(token), centre_read);
    let frame = frame(fast, &viewer, Some(Nav::Universe));
    let me = match me {
        Ok(me) => me,
        Err(e) => return failed(&s, &session, frame, &e, Fill::default(), &here).await,
    };
    let frame = Frame {
        me: Some(&me),
        ..frame
    };
    let centre_view = match (centre, around_view) {
        (Centre::At(position, radius), _) => Ok((position, radius, None)),
        (Centre::Around(id), Some(view)) => view.map(|p| (p.position, DEFAULT_RADIUS_DU, Some(id))),
        (Centre::Home | Centre::Around(_), _) => {
            let id = me.homeworld_id.0;
            s.api
                .planet(token, me.homeworld_id)
                .await
                .map(|p| (p.position, DEFAULT_RADIUS_DU, Some(id)))
        }
    };
    let (position, radius, around) = match centre_view {
        Ok(c) => c,
        Err(e) => {
            let fill = Fill {
                planet: around,
                target: None,
            };
            if let ApiError::Refused {
                code: ErrorCode::UnknownPlanet,
                ..
            } = e
            {
                let m = messages::for_error(&e, Context::Page, fill);
                return page(StatusCode::NOT_FOUND, pages::no_such_planet(frame, &m));
            }
            return failed(&s, &session, frame, &e, fill, &here).await;
        }
    };
    let form = SearchForm::filled(&position, radius);
    let radius_du = u64::try_from(radius).unwrap_or(0);
    match s.api.planets_near(token, position, radius_du).await {
        Ok(near) => {
            let found = Found::Planets {
                centre: &position,
                around,
                radius_du: radius,
                near: &near,
                me: &me,
            };
            page(StatusCode::OK, pages::universe(frame, &here, &form, found))
        }
        // The numbers were checked, so `invalid_query` here means too many planets (§4.4).
        Err(ApiError::Refused {
            code: ErrorCode::InvalidQuery,
            ..
        }) => {
            let found = Found::Refused(
                pages::TOO_MANY_PLANETS,
                Some(ErrorCode::InvalidQuery.as_str()),
            );
            page(
                StatusCode::BAD_REQUEST,
                pages::universe(frame, &here, &form, found),
            )
        }
        Err(e) => failed(&s, &session, frame, &e, Fill::default(), &here).await,
    }
}

/// Puts `banner` in the session for the next page, and sends the browser there (§5.1's 303).
async fn banner_back(session: &Session, to: &str, banner: Banner) -> Response {
    let stored = async {
        session.insert(NOTICE_KEY, banner).await?;
        keep_expiry(session).await
    };
    if let Err(e) = stored.await {
        tracing::error!(error = %e, "keeping the banner failed");
        return internal();
    }
    see_other(to)
}

/// Records that the host answered `key` (a 200 or a 422) and keeps the session's expiry.
async fn answered(session: &Session, key: &str) -> Result<(), session::SessionError> {
    session::mark_answered(session, key).await?;
    keep_expiry(session).await
}

/// `POST /act`: every form's target (§5.1). In order: the login (none, or expired: nothing is
/// sent, and the player is told so); the form key, which this session must have issued (Q3: an
/// old tab's or another site's form is "expired", with no call); the web's own checks (§6.3);
/// then one `POST /commands` with that key. The outcome:
/// - 200: 303 to the page (Fleets after Assemble and Send) with `Done: …` and the events;
/// - a refusal: 303 back with `Not done: {copy} ({code})`; the next render has a new key;
/// - 429, 503, 500, unreachable or unexpected: §5.5's retry page, re-posting the same key;
/// - 401: the session ends, and the login returns to the page with §5.1's line. Never resent.
///
/// Only a 200 or a 422 marks the key answered: those are the outcomes the host recorded
/// (D-037). A key answered before gets its first outcome again with §5.2's line.
#[tracing::instrument(name = "web.act", skip_all)]
async fn act(State(s): State<AppState>, session: Session, body: Bytes) -> Response {
    let posted = Posted::parse(&body);
    let return_to = posted.return_to().to_owned();
    let login = match session.get::<LoggedIn>(LOGIN_KEY).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, "reading the session failed");
            return internal();
        }
    };
    let Some(login) = login else {
        tracing::info!(outcome = "not_logged_in", "a form without a login");
        let fast = fast_rules(&s).await;
        return page(
            StatusCode::OK,
            pages::home_logged_out(Some(Notice::NotLoggedIn), &login_href(&return_to), fast),
        );
    };
    let Some(now) = oidc::unix_now() else {
        return internal();
    };
    if now >= login.exp {
        tracing::info!(outcome = "expired", "a form on an expired login");
        let fast = fast_rules(&s).await;
        return expired_with(&s, &session, &return_to, fast, Notice::ActionExpired).await;
    }
    let state = match posted.key() {
        Some(k) => match session::form_key_state(&session, k).await {
            Ok(state) => state,
            Err(e) => {
                tracing::error!(error = %e, "reading the session failed");
                return internal();
            }
        },
        None => KeyState::Unknown,
    };
    let (Some(key), KeyState::Issued | KeyState::Answered) = (posted.key(), state) else {
        tracing::info!(
            outcome = "expired_form",
            "a form key this session didn't issue"
        );
        return banner_back(&session, &return_to, Banner::line(forms::EXPIRED_FORM)).await;
    };
    let order = match forms::order(&posted) {
        Ok(o) => o,
        Err(check) => {
            tracing::info!(outcome = "checked", check = ?check, "the web refused a form");
            let (copy, code) = check.copy();
            let banner = Banner {
                text: copy.to_owned(),
                code: code.map(str::to_owned),
                more: Vec::new(),
            };
            return banner_back(&session, &return_to, banner).await;
        }
    };
    // Issued keys are 22 base64url characters, always a valid key.
    let Ok(idempotency) = IdempotencyKey::parse(Some(key)) else {
        return internal();
    };
    let again = state == KeyState::Answered;
    let already = || {
        if again {
            vec![forms::ALREADY_SUBMITTED.to_owned()]
        } else {
            Vec::new()
        }
    };
    let error = match s
        .api
        .command(&login.id_token, &idempotency, &order.command)
        .await
    {
        Ok(accepted) => {
            tracing::info!(outcome = "done", "a command was accepted");
            if let Err(e) = answered(&session, key).await {
                tracing::error!(error = %e, "keeping the answer failed");
                return internal();
            }
            let (text, mut more) = forms::done_lines(order.done, &accepted);
            more.extend(already());
            let banner = Banner {
                text,
                code: None,
                more,
            };
            return banner_back(&session, order.after_success(&return_to), banner).await;
        }
        Err(e) => e,
    };
    let m = messages::for_error(&error, Context::Command, order.fill);
    match m.kind {
        Kind::Login => {
            tracing::info!(
                outcome = "api_unauthorized",
                "the API refused the token on a command"
            );
            if let Err(r) = end_login(&s, &session, Banner::line(forms::ORDER_NOT_SENT)).await {
                return *r;
            }
            see_other(&login_href(&return_to))
        }
        Kind::Retry {
            status,
            retry_after,
        } => {
            tracing::info!(outcome = "retry", status, "a command wasn't answered");
            let fast = fast_rules(&s).await;
            let frame = Frame {
                fast,
                login: Some(&login),
                me: None,
                nav: None,
                banner: None,
            };
            retry(
                status,
                retry_after,
                &pages::command_retry(frame, &m, &posted, &return_to),
            )
        }
        Kind::Banner | Kind::UniverseFull => {
            tracing::info!(outcome = "refused", code = m.code, "a command was refused");
            if matches!(error, ApiError::Refused { status: 422, .. })
                && let Err(e) = answered(&session, key).await
            {
                tracing::error!(error = %e, "keeping the answer failed");
                return internal();
            }
            let banner = Banner {
                text: format!("Not done: {}", m.copy),
                code: m.code.map(str::to_owned),
                more: already(),
            };
            banner_back(&session, &return_to, banner).await
        }
    }
}

/// Which planet confirm page (§5.4).
#[derive(Clone, Copy)]
enum PlanetConfirm {
    CancelConstruction,
    Demolish,
    CancelBuild,
}

async fn confirm_cancel_construction(
    State(s): State<AppState>,
    session: Session,
    uri: Uri,
    Path((p, b)): Path<(String, String)>,
) -> Response {
    planet_confirm(s, session, uri, &p, &b, PlanetConfirm::CancelConstruction).await
}

async fn confirm_demolish(
    State(s): State<AppState>,
    session: Session,
    uri: Uri,
    Path((p, b)): Path<(String, String)>,
) -> Response {
    planet_confirm(s, session, uri, &p, &b, PlanetConfirm::Demolish).await
}

async fn confirm_cancel_build(
    State(s): State<AppState>,
    session: Session,
    uri: Uri,
    Path((p, n)): Path<(String, String)>,
) -> Response {
    planet_confirm(s, session, uri, &p, &n, PlanetConfirm::CancelBuild).await
}

/// A planet's confirm page: the login, then `/rules`, `/players/me` and the planet at once, so
/// the amounts are current (§5.4), then the page with its own key. A path that isn't two numbers
/// is §6.2's Not found, with no call.
#[tracing::instrument(name = "web.confirm", skip_all)]
async fn planet_confirm(
    s: AppState,
    session: Session,
    uri: Uri,
    p: &str,
    thing: &str,
    which: PlanetConfirm,
) -> Response {
    let here = here(&uri);
    let viewer = match begin(&s, &session, &here, None).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let (Some(id), Some(thing)) = (planet_number(p), planet_number(thing)) else {
        return not_found().await;
    };
    let token = &viewer.login.id_token;
    let (rules, me, planet) = tokio::join!(
        rules(&s),
        s.api.me(token),
        s.api.planet(token, PlanetId(id))
    );
    let fill = Fill {
        planet: Some(id),
        target: None,
    };
    let frame = frame(is_fast(rules.as_ref()), &viewer, None);
    let me = match me {
        Ok(me) => me,
        Err(e) => return failed(&s, &session, frame, &e, fill, &here).await,
    };
    let frame = Frame {
        me: Some(&me),
        ..frame
    };
    let planet = match planet {
        Ok(p) => p,
        Err(
            e @ ApiError::Refused {
                code: ErrorCode::UnknownPlanet,
                ..
            },
        ) => {
            let m = messages::for_error(&e, Context::Page, fill);
            return page(StatusCode::NOT_FOUND, pages::no_such_planet(frame, &m));
        }
        Err(e) => return failed(&s, &session, frame, &e, fill, &here).await,
    };
    let key = match form_key(&session).await {
        Ok(k) => k,
        Err(r) => return *r,
    };
    let markup = match which {
        PlanetConfirm::CancelConstruction => {
            pages::confirm_cancel_construction(frame, &me, &planet, thing, &key)
        }
        PlanetConfirm::Demolish => {
            pages::confirm_demolish(frame, &me, &planet, rules.as_ref(), thing, &key)
        }
        PlanetConfirm::CancelBuild => pages::confirm_cancel_build(frame, &me, &planet, thing, &key),
    };
    page(StatusCode::OK, markup)
}

/// Turn back's confirm page (§5.4): `/players/me/fleets` for the time flown, beside `/rules` and
/// `/players/me`, then the page with its own key.
#[tracing::instrument(name = "web.confirm", skip_all)]
async fn confirm_turn_back(
    State(s): State<AppState>,
    session: Session,
    uri: Uri,
    Path(f): Path<String>,
) -> Response {
    let here = here(&uri);
    let viewer = match begin(&s, &session, &here, None).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Some(f) = planet_number(&f) else {
        return not_found().await;
    };
    let token = &viewer.login.id_token;
    let (fast, me, fleets) = tokio::join!(fast_rules(&s), s.api.me(token), s.api.my_fleets(token));
    let frame = frame(fast, &viewer, None);
    let me = match me {
        Ok(me) => me,
        Err(e) => return failed(&s, &session, frame, &e, Fill::default(), &here).await,
    };
    let frame = Frame {
        me: Some(&me),
        ..frame
    };
    let fleets = match fleets {
        Ok(f) => f,
        Err(e) => return failed(&s, &session, frame, &e, Fill::default(), &here).await,
    };
    let key = match form_key(&session).await {
        Ok(k) => k,
        Err(r) => return *r,
    };
    page(
        StatusCode::OK,
        pages::confirm_turn_back(frame, &fleets, f, &key),
    )
}

/// §6.2: any other path. It reads nothing, so it has no frame.
async fn not_found() -> Response {
    page(StatusCode::NOT_FOUND, pages::not_found())
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
    // A login renewing an ended session leaves the next page its one-shot notice (§5.6).
    let stored = async {
        session.cycle_id().await?;
        session.remove::<serde_json::Value>(PENDING_KEY).await?;
        if let Some(banner) = session.remove::<Banner>(RENEWING_KEY).await? {
            session.insert(NOTICE_KEY, banner).await?;
        }
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
    see_other("/?logged_out")
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
