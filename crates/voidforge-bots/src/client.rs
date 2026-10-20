//! A player's typed client for the public API (D-045, D-049): every request and answer is a
//! `voidforge-api-types` value, sent with the player's own token. The bots reach the server over
//! HTTP alone.

use serde::de::DeserializeOwned;
use voidforge_api_types::command::{CommandAccepted, CommandRequest};
use voidforge_api_types::error::{ErrorBody, ErrorCode};
use voidforge_api_types::view::{FleetsView, Leaderboard, PlanetView, PlanetsNearView, PlayerView};
use voidforge_api_types::wire::{PlanetId, Position};

use crate::Failure;

/// A refusal: the status, and D-045's error body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// The HTTP status.
    pub status: u16,
    /// The error body.
    pub body: ErrorBody,
}

/// What a call returned: the answer, or a refusal the scenario may expect.
pub type Reply<T> = Result<T, Refused>;

/// One player's API client.
#[derive(Clone)]
pub struct Player {
    /// The name the scenario's log uses.
    pub name: &'static str,
    http: reqwest::Client,
    api: String,
    token: String,
}

impl Player {
    /// A client for `api` (for example `http://127.0.0.1:8080`) with `token`.
    #[must_use]
    pub fn new(name: &'static str, http: reqwest::Client, api: &str, token: String) -> Self {
        Self {
            name,
            http,
            api: api.trim_end_matches('/').to_owned(),
            token,
        }
    }

    /// `GET /players/me`; the first call joins (D-047).
    ///
    /// # Errors
    /// See [`Player::get`].
    pub async fn me(&self) -> Result<Reply<PlayerView>, Failure> {
        self.get("/players/me").await
    }

    /// `GET /players/me/fleets`.
    ///
    /// # Errors
    /// See [`Player::get`].
    pub async fn fleets(&self) -> Result<Reply<FleetsView>, Failure> {
        self.get("/players/me/fleets").await
    }

    /// `GET /planets/{id}`.
    ///
    /// # Errors
    /// See [`Player::get`].
    pub async fn planet(&self, id: PlanetId) -> Result<Reply<PlanetView>, Failure> {
        self.get(&format!("/planets/{}", id.0)).await
    }

    /// `GET /planets?near=x,y,z&radius=r`.
    ///
    /// # Errors
    /// See [`Player::get`].
    pub async fn near(&self, at: Position, radius: i64) -> Result<Reply<PlanetsNearView>, Failure> {
        self.get(&format!(
            "/planets?near={},{},{}&radius={radius}",
            at.x, at.y, at.z
        ))
        .await
    }

    /// `GET /leaderboard`.
    ///
    /// # Errors
    /// See [`Player::get`].
    pub async fn leaderboard(&self) -> Result<Reply<Leaderboard>, Failure> {
        self.get("/leaderboard").await
    }

    /// A `GET` of any API `path` (one starting with `/`), as JSON: the terminal's `get` (D-053).
    ///
    /// # Errors
    /// See [`Player::get`].
    pub async fn get_json(&self, path: &str) -> Result<Reply<serde_json::Value>, Failure> {
        self.get(path).await
    }

    /// `POST /commands` with `key` as the `Idempotency-Key`.
    ///
    /// # Errors
    /// See [`Player::get`].
    pub async fn command(
        &self,
        command: &CommandRequest,
        key: &str,
    ) -> Result<Reply<CommandAccepted>, Failure> {
        let request = self
            .http
            .post(format!("{}/commands", self.api))
            .header("idempotency-key", key)
            .json(command);
        self.send(request, "POST /commands").await
    }

    /// A `GET` of `path`.
    ///
    /// # Errors
    /// [`Failure::NotReady`] for a 503 `unavailable` (dex's keys not loaded yet),
    /// [`Failure::Http`] when the server can't be reached or answers with something that is
    /// neither the expected type nor an [`ErrorBody`].
    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<Reply<T>, Failure> {
        let request = self.http.get(format!("{}{path}", self.api));
        self.send(request, &format!("GET {path}")).await
    }

    async fn send<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        what: &str,
    ) -> Result<Reply<T>, Failure> {
        let http = |e: &dyn std::fmt::Display| Failure::Http(format!("{}: {what}: {e}", self.name));
        let response = request
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| http(&e))?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(|e| http(&e))?;
        if (200..300).contains(&status) {
            return serde_json::from_str(&text)
                .map(Ok)
                .map_err(|e| http(&format!("HTTP {status}, an unexpected body ({e}): {text}")));
        }
        let body: ErrorBody = serde_json::from_str(&text)
            .map_err(|e| http(&format!("HTTP {status}, not an error body ({e}): {text}")))?;
        if status == 503 && body.code == ErrorCode::Unavailable {
            return Err(Failure::NotReady(format!(
                "{}: {what}: {}",
                self.name, body.message
            )));
        }
        Ok(Err(Refused { status, body }))
    }
}
