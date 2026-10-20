//! The typed client for the game's public API (P4.2, D-059): one method per route the web uses,
//! returning api-types values, and one total error.
//!
//! - The authenticated methods take the session's ID token and send it as Bearer; `rules` takes
//!   none, so a public call can't carry it.
//! - Every call ends in a view or an [`ApiError`]: the contract's error body at the status its
//!   code is sent with, no answer, a status the route never sends, or a body that isn't the
//!   contract's. Nothing on the path panics, and a body is read only up to [`MAX_BODY_BYTES`].
//! - The token never reaches a log, an error or a `Debug`. Each failure is logged once, with
//!   the route, the status and the web span's trace ID; the API's `message` is logged, never
//!   shown (D-045).

use std::fmt;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::RETRY_AFTER;
use serde::de::DeserializeOwned;
use voidforge_api_types::command::{CommandAccepted, CommandRequest, IdempotencyKey};
use voidforge_api_types::error::{ErrorBody, ErrorCode};
use voidforge_api_types::rules::RulesView;
use voidforge_api_types::view::{FleetsView, Leaderboard, PlanetView, PlanetsNearView, PlayerView};
use voidforge_api_types::wire::{PlanetId, Position};

use crate::config::Secret;

/// How long one API call may take in all. A token signed with a new key makes the server fetch
/// dex's keys (up to 5 s) before a stale view's wait (up to 1 s), so this leaves room for the
/// slowest legitimate call (T4).
pub const API_TIMEOUT: Duration = Duration::from_secs(10);
/// How long connecting may take, so a dead API is reported in seconds, not after the total.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// The largest body read. The largest view, 1,000 planets near a point, is far below it.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
/// The longest Retry-After kept: the server's own maximum (`MAX_RETRY_SECS`).
pub const MAX_RETRY_AFTER_SECS: u64 = 30;

/// How a call ended without its view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// The contract's error body, at the status its code is sent with.
    Refused {
        /// The code.
        code: ErrorCode,
        /// The status it came with.
        status: u16,
        /// Retry-After's whole seconds, on a 429 or 503 that sent a valid one (at most
        /// [`MAX_RETRY_AFTER_SECS`]).
        retry_after: Option<u64>,
    },
    /// No answer: the connection failed, or the time bound passed.
    Unreachable(Unreachable),
    /// A status the route never sends: a redirect (none is followed), a success other than
    /// 200, or an error body whose code isn't sent with that status.
    UnexpectedStatus(u16),
    /// The status was expected, but the body isn't the contract's, or is too large.
    Undecodable {
        /// The status it came with.
        status: u16,
    },
}

/// Why there was no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreachable {
    /// Connecting failed: nothing listening, a refused or reset connection, a bad address.
    Connect,
    /// The time bound passed, connecting or waiting for the answer.
    Timeout,
    /// The request failed another way after connecting.
    Other,
}

/// The routes the web calls (T1: not `/openapi.json`), for the logs and the one status
/// exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Me,
    MyFleets,
    Planet,
    PlanetsNear,
    Leaderboard,
    Command,
    Rules,
}

impl Route {
    fn name(self) -> &'static str {
        match self {
            Self::Me => "GET /players/me",
            Self::MyFleets => "GET /players/me/fleets",
            Self::Planet => "GET /planets/{id}",
            Self::PlanetsNear => "GET /planets",
            Self::Leaderboard => "GET /leaderboard",
            Self::Command => "POST /commands",
            Self::Rules => "GET /rules",
        }
    }

    /// Whether `code` arriving with `status` is the contract's: the code's own status, or
    /// `unknown_planet` at 404 from `GET /planets/{id}` (T2).
    fn sends(self, code: ErrorCode, status: u16) -> bool {
        code.status() == status
            || (self == Self::Planet && code == ErrorCode::UnknownPlanet && status == 404)
    }
}

/// The API client: a base URL and an HTTP client that follows no redirects.
#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    base: String,
}

impl fmt::Debug for Api {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Api").field("base", &self.base).finish()
    }
}

impl Api {
    /// A client for the API at `base` (no trailing slash), with the standard time bounds.
    ///
    /// # Errors
    /// reqwest's, if the client can't be built.
    pub fn new(base: String) -> Result<Self, reqwest::Error> {
        Self::with_timeouts(base, API_TIMEOUT, CONNECT_TIMEOUT)
    }

    /// A client with other time bounds, for tests that can't wait the standard ones.
    ///
    /// # Errors
    /// reqwest's, if the client can't be built.
    pub fn with_timeouts(
        base: String,
        total: Duration,
        connect: Duration,
    ) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .timeout(total)
            .connect_timeout(connect)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { http, base })
    }

    /// `GET /players/me`: the player, joining on first use (D-047).
    ///
    /// # Errors
    /// See [`ApiError`].
    pub async fn me(&self, token: &Secret) -> Result<PlayerView, ApiError> {
        let req = self.http.get(format!("{}/players/me", self.base));
        self.send(Route::Me, req.bearer_auth(token.expose())).await
    }

    /// `GET /players/me/fleets`.
    ///
    /// # Errors
    /// See [`ApiError`].
    pub async fn my_fleets(&self, token: &Secret) -> Result<FleetsView, ApiError> {
        let req = self.http.get(format!("{}/players/me/fleets", self.base));
        self.send(Route::MyFleets, req.bearer_auth(token.expose()))
            .await
    }

    /// `GET /planets/{id}`.
    ///
    /// # Errors
    /// See [`ApiError`]; an unknown planet is `unknown_planet` at 404.
    pub async fn planet(&self, token: &Secret, id: PlanetId) -> Result<PlanetView, ApiError> {
        let req = self.http.get(format!("{}/planets/{}", self.base, id.0));
        self.send(Route::Planet, req.bearer_auth(token.expose()))
            .await
    }

    /// `GET /planets?near=x,y,z&radius=r`, in distance units.
    ///
    /// # Errors
    /// See [`ApiError`].
    pub async fn planets_near(
        &self,
        token: &Secret,
        near: Position,
        radius_du: u64,
    ) -> Result<PlanetsNearView, ApiError> {
        // Digits, minus signs and commas only: nothing in the query needs escaping.
        let req = self.http.get(format!(
            "{}/planets?near={},{},{}&radius={radius_du}",
            self.base, near.x, near.y, near.z
        ));
        self.send(Route::PlanetsNear, req.bearer_auth(token.expose()))
            .await
    }

    /// `GET /leaderboard`.
    ///
    /// # Errors
    /// See [`ApiError`].
    pub async fn leaderboard(&self, token: &Secret) -> Result<Leaderboard, ApiError> {
        let req = self.http.get(format!("{}/leaderboard", self.base));
        self.send(Route::Leaderboard, req.bearer_auth(token.expose()))
            .await
    }

    /// `POST /commands` with the form's key. A repeat of the key returns its first answer
    /// (D-037), so retrying after a failure without an answer runs the command at most once.
    ///
    /// # Errors
    /// See [`ApiError`].
    pub async fn command(
        &self,
        token: &Secret,
        key: &IdempotencyKey,
        command: &CommandRequest,
    ) -> Result<CommandAccepted, ApiError> {
        let req = self
            .http
            .post(format!("{}/commands", self.base))
            .header("Idempotency-Key", key.as_str())
            .json(command);
        self.send(Route::Command, req.bearer_auth(token.expose()))
            .await
    }

    /// `GET /rules`: public, so it takes no token and sends none (D-058).
    ///
    /// # Errors
    /// See [`ApiError`].
    pub async fn rules(&self) -> Result<RulesView, ApiError> {
        let req = self.http.get(format!("{}/rules", self.base));
        self.send(Route::Rules, req).await
    }

    async fn send<T: DeserializeOwned>(
        &self,
        route: Route,
        req: reqwest::RequestBuilder,
    ) -> Result<T, ApiError> {
        let result = exchange(route, req).await;
        if let Err(e) = &result {
            log(route, e);
        }
        result
    }
}

/// One call: send, classify the status, read the body within its cap, decode.
async fn exchange<T: DeserializeOwned>(
    route: Route,
    req: reqwest::RequestBuilder,
) -> Result<T, ApiError> {
    let mut response = req.send().await.map_err(unreachable)?;
    let status = response.status();
    if status != StatusCode::OK && !(status.is_client_error() || status.is_server_error()) {
        return Err(ApiError::UnexpectedStatus(status.as_u16()));
    }
    let retry_after = retry_after(&response);
    let body = read_capped(&mut response).await?;
    let undecodable = ApiError::Undecodable {
        status: status.as_u16(),
    };
    if status == StatusCode::OK {
        return serde_json::from_slice(&body).map_err(|_| undecodable);
    }
    let error: ErrorBody = serde_json::from_slice(&body).map_err(|_| undecodable)?;
    let status = status.as_u16();
    if !route.sends(error.code, status) {
        tracing::warn!(
            route = route.name(),
            status,
            code = error.code.as_str(),
            "the API sent a code with a status the contract doesn't give it"
        );
        return Err(ApiError::UnexpectedStatus(status));
    }
    tracing::debug!(
        route = route.name(),
        code = error.code.as_str(),
        message = %error.message,
        "the API's message"
    );
    Err(ApiError::Refused {
        code: error.code,
        status,
        retry_after: retry_after.filter(|_| matches!(status, 429 | 503)),
    })
}

fn unreachable(e: reqwest::Error) -> ApiError {
    ApiError::Unreachable(if e.is_timeout() {
        Unreachable::Timeout
    } else if e.is_connect() {
        Unreachable::Connect
    } else {
        Unreachable::Other
    })
}

/// Retry-After as whole seconds, capped; None when it's missing or not a number.
fn retry_after(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|s| s.min(MAX_RETRY_AFTER_SECS))
}

/// The body, refused as undecodable past [`MAX_BODY_BYTES`] without reading the rest. A failure
/// while reading is a failure to answer.
async fn read_capped(response: &mut reqwest::Response) -> Result<Vec<u8>, ApiError> {
    let too_large = ApiError::Undecodable {
        status: response.status().as_u16(),
    };
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY_BYTES as u64)
    {
        return Err(too_large);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
        if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            return Err(too_large);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Logs a failed call once: a refusal at INFO, anything else at WARN, with the route, the
/// status and the trace ID (T5), so a failure here can be found in the server's log.
fn log(route: Route, e: &ApiError) {
    let trace_id = voidforge_telemetry::trace_id(&tracing::Span::current()).unwrap_or_default();
    match e {
        ApiError::Refused { code, status, .. } => tracing::info!(
            route = route.name(),
            status,
            code = code.as_str(),
            trace_id,
            "the API refused"
        ),
        ApiError::Unreachable(why) => tracing::warn!(
            route = route.name(),
            why = ?why,
            trace_id,
            "the API can't be reached"
        ),
        ApiError::UnexpectedStatus(status) => tracing::warn!(
            route = route.name(),
            status,
            trace_id,
            "the API answered a status this route never sends"
        ),
        ApiError::Undecodable { status } => tracing::warn!(
            route = route.name(),
            status,
            trace_id,
            "the API's answer isn't the contract's"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_planet_at_404_is_the_contract_only_for_one_planet() {
        assert!(Route::Planet.sends(ErrorCode::UnknownPlanet, 404));
        assert!(Route::Planet.sends(ErrorCode::UnknownPlanet, 422));
        assert!(!Route::Command.sends(ErrorCode::UnknownPlanet, 404));
        assert!(!Route::Me.sends(ErrorCode::UnknownPlanet, 404));
        assert!(!Route::Planet.sends(ErrorCode::NotFound, 422));
        assert!(!Route::Planet.sends(ErrorCode::Busy, 404));
    }

    #[test]
    fn debug_shows_no_more_than_the_base() {
        let api = Api::new("http://localhost:8080".to_owned()).unwrap();
        assert_eq!(format!("{api:?}"), r#"Api { base: "http://localhost:8080" }"#);
    }
}
