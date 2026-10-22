//! The public HTTP API (Technical Plan P3.4; Architectural Design, Read side and API; decision log
//! D-045, D-049). One route table builds the router, and the parity tests read it against the
//! OpenAPI document. Every handler that acts as a player takes [`Player`]: never a player ID from
//! the body, the query or the path. Writes go through the host. Reads project the host's published
//! read model to the request's now, through [`World::view_for`]: never mutating it, never
//! appending, never through the inbox.
//!
//! [`World::view_for`]: voidforge_host::World::view_for

pub mod error;
pub mod time;
pub mod wire;

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::Router;
use axum::extract::{DefaultBodyLimit, FromRef, State};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, RETRY_AFTER};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get, post};
use serde::Deserialize;
use tower_http::cors::{AllowOrigin, CorsLayer};
use voidforge_api_types::command::{CommandAccepted, CommandRequest};
use voidforge_api_types::error::ErrorCode;
use voidforge_api_types::view as api;
use voidforge_host::view::{Query, View, ViewError};
use voidforge_host::{Command, HostHandle, PlanetId, PlayerId, Position};
use voidforge_store::accounts::PgAccounts;
use voidforge_store::log::IdempotencyKey;

use crate::accounts::{Player, Players};
use crate::auth::TokenVerifier;
use crate::codes::game_code;
use crate::limit::Limiter;
use error::{ApiError, ApiJson, ApiPath, ApiQuery, host_error};

/// The most planets one `GET /planets?near=…` answers. More is 400 `invalid_query`, never a
/// truncated list. It bounds the response; the scan is bounded by the universe's size cap, and
/// P3.8's rate limit bounds how often it runs (D-049, T4).
pub const MAX_NEAR_RESULTS: usize = 1_000;

/// The largest request body: a command is a few hundred bytes.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// How long a read waits for the host to publish a due step before answering 503.
pub const STALE_WAIT: Duration = Duration::from_secs(1);

/// Seconds a client waits after a stale projection's 503.
pub const STALE_RETRY_AFTER_SECS: u64 = 1;

/// The header that names a command's idempotency key (D-037).
pub const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

/// The caller's W3C trace context, the only header `api.request` reads to join a trace (D-064).
pub const TRACEPARENT: HeaderName = HeaderName::from_static("traceparent");

/// How long a browser may cache a CORS preflight's answer.
pub const CORS_MAX_AGE: Duration = Duration::from_secs(600);

/// What the handlers share.
#[derive(Clone)]
pub struct AppState {
    /// Token checks (P3.2).
    pub verifier: Arc<TokenVerifier>,
    /// Principals to players (P3.3).
    pub players: Arc<Players<PgAccounts>>,
    /// The engine host.
    pub host: HostHandle,
    /// Now, in Unix milliseconds: the host's clock.
    pub now_unix_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// The universe epoch, Unix milliseconds: the host's.
    pub epoch_unix_ms: i64,
    /// The latest leaderboard build (P3.5): `None` until one succeeds.
    pub leaderboard: crate::leaderboard::Board,
    /// The per-principal rate limits (P3.8), checked by the [`Player`] extractor.
    pub limiter: Arc<Limiter>,
    /// `GET /rules`'s answer (P4.1r): the loaded rules' view as JSON, built once by
    /// [`crate::rules_view::body`].
    pub rules: axum::body::Bytes,
}

impl FromRef<AppState> for Arc<TokenVerifier> {
    fn from_ref(s: &AppState) -> Self {
        s.verifier.clone()
    }
}

impl FromRef<AppState> for Arc<Players<PgAccounts>> {
    fn from_ref(s: &AppState) -> Self {
        s.players.clone()
    }
}

impl FromRef<AppState> for Arc<Limiter> {
    fn from_ref(s: &AppState) -> Self {
        s.limiter.clone()
    }
}

/// One route: its method and path, as the OpenAPI document names them, and its handler.
pub struct Route {
    /// The method.
    pub method: Method,
    /// The path, with `{param}` segments.
    pub path: &'static str,
    /// Whether it needs a token (only `/openapi.json` and `/rules` don't).
    pub authenticated: bool,
    handler: MethodRouter<AppState>,
}

/// Every route: the router is built from this list, and the parity tests read it.
#[must_use]
pub fn routes() -> Vec<Route> {
    let route = |method, path, authenticated, handler| Route {
        method,
        path,
        authenticated,
        handler,
    };
    vec![
        route(Method::POST, "/commands", true, post(commands)),
        route(Method::GET, "/planets/{id}", true, get(planet)),
        route(Method::GET, "/planets", true, get(planets_near)),
        route(Method::GET, "/players/me", true, get(player_me)),
        route(Method::GET, "/players/me/fleets", true, get(fleets)),
        route(Method::GET, "/leaderboard", true, get(leaderboard)),
        route(Method::GET, "/rules", false, get(rules)),
        route(Method::GET, "/openapi.json", false, get(openapi)),
    ]
}

pub fn router(state: AppState, cors_origins: &[HeaderValue]) -> Router {
    layered(
        routes()
            .into_iter()
            .fold(Router::new(), |r, route| r.route(route.path, route.handler)),
        cors_origins,
    )
    .with_state(state)
}

/// Each matched route's request span, `api.request` (P5.1, D-064): the method, the route as
/// declared (`/planets/{id}`, never the path or its query) and the status. The caller's
/// `traceparent` makes it a child in the caller's trace, the web's or a bot's; only that header is
/// read (T3). The command's spans in the host are its children.
async fn traced(
    path: axum::extract::MatchedPath,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let span = tracing::info_span!(
        "api.request",
        http.request.method = %request.method(),
        http.route = path.as_str(),
        http.response.status_code = tracing::field::Empty,
    );
    voidforge_telemetry::set_remote_parent(
        &span,
        request
            .headers()
            .get(TRACEPARENT)
            .and_then(|v| v.to_str().ok()),
    );
    let response = tracing::Instrument::instrument(next.run(request), span.clone()).await;
    span.record("http.response.status_code", response.status().as_u16());
    response
}

/// The layers every route sits behind: `not_found` for an unknown route or method, HEAD
/// included, the body limit, a panic answered 500, and CORS for `cors_origins` outermost, so a
/// preflight is answered before the fallback and every refusal carries the allow headers. Public
/// so a test can put a route behind exactly these.
pub fn layered<S: Clone + Send + Sync + 'static>(
    routes: Router<S>,
    cors_origins: &[HeaderValue],
) -> Router<S> {
    let routes = routes
        .route_layer(axum::middleware::from_fn(traced))
        .fallback(error::not_found)
        .method_not_allowed_fallback(error::not_found)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(axum::middleware::from_fn(error::refuse_head))
        .layer(axum::middleware::from_fn(error::catch_panic));
    if cors_origins.is_empty() {
        return routes;
    }
    routes.layer(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(cors_origins.iter().cloned()))
            .allow_methods([Method::GET, Method::POST])
            .allow_headers([AUTHORIZATION, CONTENT_TYPE, IDEMPOTENCY_KEY, TRACEPARENT])
            .expose_headers([RETRY_AFTER])
            .max_age(CORS_MAX_AGE),
    )
}

/// Parses one `--cors-origin`: exactly an origin as a browser sends it, `http` or `https`, with
/// no path, query or trailing slash.
///
/// # Errors
///
/// A message naming the expected form, for clap to print.
pub fn cors_origin(value: &str) -> Result<HeaderValue, String> {
    let expected = || format!("{value:?} is not an origin such as http://localhost:8081");
    let url = reqwest::Url::parse(value).map_err(|_| expected())?;
    let http = matches!(url.scheme(), "http" | "https");
    if !http || url.origin().ascii_serialization() != value {
        return Err(expected());
    }
    HeaderValue::from_str(value).map_err(|_| expected())
}

// --- POST /commands ---

async fn commands(
    State(s): State<AppState>,
    player: Player,
    headers: HeaderMap,
    ApiJson(request): ApiJson<CommandRequest>,
) -> Result<axum::Json<CommandAccepted>, ApiError> {
    let key = idempotency_key(&headers)?;
    let action = wire::action(request)
        .map_err(|code| ApiError::new(code, "the command names a ship more than once"))?;
    let receipt = s
        .host
        .submit_with_key(
            Command {
                player: player.id,
                action,
            },
            key,
        )
        .await
        .map_err(host_error)?;
    match receipt.outcome {
        Ok(own_events) => {
            let clock = wire::Clock {
                epoch_unix_ms: s.epoch_unix_ms,
            };
            Ok(axum::Json(CommandAccepted {
                seq: receipt.seq,
                game_time: time::timestamp(s.epoch_unix_ms, receipt.t)
                    .map_err(|e| ApiError::internal(&e))?,
                events: clock.events(&own_events),
            }))
        }
        Err(r) => Err(ApiError::new(game_code(r), r.to_string())),
    }
}

/// The `Idempotency-Key` header, by the host's own rule (D-037): exactly one, 1 to 128 bytes of
/// printable ASCII without spaces.
fn idempotency_key(headers: &HeaderMap) -> Result<IdempotencyKey, ApiError> {
    let mut all = headers.get_all(IDEMPOTENCY_KEY).iter();
    let Some(value) = all.next() else {
        return Err(ApiError::new(
            ErrorCode::MissingIdempotencyKey,
            "POST /commands needs an Idempotency-Key header",
        ));
    };
    let invalid = || {
        ApiError::new(
            ErrorCode::InvalidIdempotencyKey,
            "the Idempotency-Key must be one header of 1 to 128 printable ASCII bytes without spaces",
        )
    };
    if all.next().is_some() {
        return Err(invalid());
    }
    let text = value.to_str().map_err(|_| invalid())?;
    IdempotencyKey::new(text).map_err(|_| invalid())
}

// --- The views ---

impl AppState {
    fn clock(&self) -> wire::Clock {
        wire::Clock {
            epoch_unix_ms: self.epoch_unix_ms,
        }
    }

    /// Projects `query` for `viewer` to now. A read model a step behind (the view would cross an
    /// event the host hasn't published yet) waits up to [`STALE_WAIT`] for the next one, then
    /// answers 503: a projection never crosses an event.
    async fn project(&self, query: Query, viewer: PlayerId) -> Result<View, ApiError> {
        let mut host = self.host.clone();
        let deadline = tokio::time::Instant::now() + STALE_WAIT;
        loop {
            let read = host.read();
            let now = (self.now_unix_ms)()
                .checked_sub(self.epoch_unix_ms)
                .ok_or_else(|| {
                    ApiError::internal(&"the clock is before the epoch by more than i64")
                })?;
            let t = now.max(read.world.time_ms);
            match read.world.view_for(&query, viewer, t, &read.rules) {
                Err(ViewError::Stale { .. }) => {}
                other => return other.map_err(view_error),
            }
            match tokio::time::timeout_at(deadline, host.changed()).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(host_error(e)),
                Err(_) => {
                    return Err(ApiError::new(
                        ErrorCode::Unavailable,
                        "the game state is catching up; retry shortly",
                    )
                    .retry_after(STALE_RETRY_AFTER_SECS));
                }
            }
        }
    }
}

fn view_error(e: ViewError) -> ApiError {
    match e {
        ViewError::Unknown => ApiError::new(ErrorCode::UnknownPlanet, "no planet has that ID")
            .with_status(axum::http::StatusCode::NOT_FOUND),
        other => ApiError::internal(&other),
    }
}

fn mapped<T>(r: Result<T, wire::MapError>) -> Result<axum::Json<T>, ApiError> {
    r.map(axum::Json).map_err(|e| ApiError::internal(&e))
}

async fn planet(
    State(s): State<AppState>,
    player: Player,
    ApiPath(id): ApiPath<u64>,
) -> Result<axum::Json<api::PlanetView>, ApiError> {
    match s
        .project(
            Query::Planet {
                planet: PlanetId(id),
            },
            player.id,
        )
        .await?
    {
        View::Planet(v) => mapped(s.clock().planet(v)),
        other => Err(ApiError::internal(&format_args!(
            "a planet query gave {other:?}"
        ))),
    }
}

#[derive(Deserialize)]
struct NearParams {
    near: String,
    radius: i64,
}

/// `x,y,z`: three integers within i64, as the document's pattern says (D-045 left the i64
/// bound to the server).
fn parse_near(near: &str) -> Option<Position> {
    let mut parts = near.split(',');
    let mut next = || -> Option<i64> {
        let s = parts.next()?;
        let digits = s.strip_prefix('-').unwrap_or(s);
        if digits.is_empty() || digits.len() > 19 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    let p = Position {
        x: next()?,
        y: next()?,
        z: next()?,
    };
    parts.next().is_none().then_some(p)
}

async fn planets_near(
    State(s): State<AppState>,
    player: Player,
    ApiQuery(q): ApiQuery<NearParams>,
) -> Result<axum::Json<api::PlanetsNearView>, ApiError> {
    let center = parse_near(&q.near).ok_or_else(|| {
        ApiError::new(
            ErrorCode::InvalidQuery,
            "near is three integers within i64, as x,y,z",
        )
    })?;
    if q.radius < 0 {
        return Err(ApiError::new(
            ErrorCode::InvalidQuery,
            "radius can't be negative",
        ));
    }
    let view = s
        .project(
            Query::PlanetsNear {
                center,
                radius: q.radius,
            },
            player.id,
        )
        .await?;
    let View::PlanetsNear(v) = view else {
        return Err(ApiError::internal(&format_args!(
            "a near query gave {view:?}"
        )));
    };
    if v.planets.len() > MAX_NEAR_RESULTS {
        return Err(ApiError::new(
            ErrorCode::InvalidQuery,
            format!(
                "{} planets are within that radius, more than {MAX_NEAR_RESULTS}; narrow it",
                v.planets.len()
            ),
        ));
    }
    mapped(s.clock().near(v))
}

async fn player_me(
    State(s): State<AppState>,
    player: Player,
) -> Result<axum::Json<api::PlayerView>, ApiError> {
    match s
        .project(Query::Player { player: player.id }, player.id)
        .await
    {
        Ok(View::Player(v)) => mapped(s.clock().player(v)),
        // The extractor joined this player, so an unknown player is a server fault, not a 404.
        Err(e) if e.code == ErrorCode::UnknownPlanet => {
            Err(ApiError::internal(&"a joined player has no player view"))
        }
        Err(e) => Err(e),
        Ok(other) => Err(ApiError::internal(&format_args!(
            "a player query gave {other:?}"
        ))),
    }
}

async fn fleets(
    State(s): State<AppState>,
    player: Player,
) -> Result<axum::Json<api::FleetsView>, ApiError> {
    match s
        .project(Query::Fleets { viewer: player.id }, player.id)
        .await?
    {
        View::Fleets(v) => mapped(s.clock().fleets(v)),
        other => Err(ApiError::internal(&format_args!(
            "a fleets query gave {other:?}"
        ))),
    }
}

/// Seconds a client waits after the 503 for a board not built yet: the job retries every second
/// until the first build succeeds (D-051).
pub const LEADERBOARD_RETRY_AFTER_SECS: u64 = 1;

/// The latest build, with its own `as_of` (up to the job's interval old, by design; a player's own
/// score is current through `/players/me`). Before the first build: 503 with Retry-After.
async fn leaderboard(State(s): State<AppState>, _player: Player) -> Result<Response, ApiError> {
    let latest = s.leaderboard.borrow().clone();
    match latest {
        Some(built) => Ok((
            [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
            built.json.clone(),
        )
            .into_response()),
        None => Err(
            ApiError::new(ErrorCode::Unavailable, "the leaderboard isn't built yet")
                .retry_after(LEADERBOARD_RETRY_AFTER_SECS),
        ),
    }
}

static OPENAPI: LazyLock<String> = LazyLock::new(voidforge_api_types::openapi_json);

/// The contract itself: public, so reading it never needs a token or joins anyone (T2).
async fn openapi() -> Response {
    (
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        OPENAPI.as_str(),
    )
        .into_response()
}

/// The loaded rules (P4.1r, D-058): public like the contract, so reading them never needs a
/// token, joins anyone or spends a rate limit. The bytes were built once at startup.
async fn rules(State(s): State<AppState>) -> Response {
    (
        [
            (CONTENT_TYPE, HeaderValue::from_static("application/json")),
            (CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        s.rules,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_takes_three_integers_within_i64_and_nothing_else() {
        let p = |x, y, z| Some(Position { x, y, z });
        assert_eq!(parse_near("1,-2,3"), p(1, -2, 3));
        assert_eq!(parse_near("0,0,0"), p(0, 0, 0));
        assert_eq!(
            parse_near("9223372036854775807,-9223372036854775808,0"),
            p(i64::MAX, i64::MIN, 0)
        );
        for bad in [
            "9223372036854775808,0,0",
            "0,-9223372036854775809,0",
            "99999999999999999999,0,0",
            "1,2",
            "1,2,3,4",
            "1,2,",
            ",1,2",
            "1.0,2,3",
            "+1,2,3",
            " 1,2,3",
            "1,2,3 ",
            "a,b,c",
            "-,1,2",
            "",
        ] {
            assert_eq!(parse_near(bad), None, "{bad:?}");
        }
    }
}
