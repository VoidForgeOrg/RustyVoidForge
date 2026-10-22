//! A player's typed client for the public API (D-045, D-049): every request and answer is a
//! `voidforge-api-types` value, sent with the player's own token. The bots reach the server over
//! HTTP alone.
//!
//! A 429 `rate_limited` is retried after its Retry-After, at most [`RATE_LIMIT_RETRIES`] times
//! and never for a Retry-After over [`MAX_RETRY_AFTER`] (D-054). Each wait is logged, and
//! counted: the e2e scenario fails if it met any, since the defaults must leave the bots margin.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::RETRY_AFTER;
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

/// How many times one request is retried after a 429.
pub const RATE_LIMIT_RETRIES: u32 = 3;

/// The longest Retry-After the client waits; a longer one is answered as the refusal it is.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// One player's API client.
#[derive(Clone)]
pub struct Player {
    /// The name the scenario's log uses.
    pub name: &'static str,
    http: reqwest::Client,
    api: String,
    token: String,
    /// The 429s this player met, retried or not.
    limited: Arc<AtomicU64>,
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
            limited: Arc::new(AtomicU64::new(0)),
        }
    }

    /// How many 429s this player has met.
    #[must_use]
    pub fn rate_limited(&self) -> u64 {
        self.limited.load(Ordering::Relaxed)
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
        let request = request.bearer_auth(&self.token);
        // The bot's trace continues in the API (P5.1, D-064).
        let request = match voidforge_telemetry::traceparent(&tracing::Span::current()) {
            Some(traceparent) => request.header("traceparent", traceparent),
            None => request,
        };
        let mut tries = 0;
        let response = loop {
            let attempt = request
                .try_clone()
                .ok_or_else(|| http(&"the request can't be repeated"))?;
            let response = attempt.send().await.map_err(|e| http(&e))?;
            if response.status() != StatusCode::TOO_MANY_REQUESTS {
                break response;
            }
            self.limited.fetch_add(1, Ordering::Relaxed);
            let wait = retry_after(&response).filter(|w| *w <= MAX_RETRY_AFTER);
            match wait {
                Some(wait) if tries < RATE_LIMIT_RETRIES => {
                    tries += 1;
                    tracing::warn!(
                        player = self.name,
                        request = what,
                        secs = wait.as_secs(),
                        "rate limited (429); waiting its Retry-After, then retrying"
                    );
                    tokio::time::sleep(wait).await;
                }
                _ => break response,
            }
        };
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

/// A response's Retry-After in whole seconds, if it has a valid one.
fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    let secs: u64 = response
        .headers()
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
    Some(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Instant;

    use super::*;

    /// P5.1 (D-064): a bot's request carries its span in `traceparent`, so an e2e trace starts
    /// in the bot and continues in the API.
    #[tokio::test]
    async fn a_request_carries_the_bot_s_trace() {
        use std::io::{BufRead as _, BufReader};
        voidforge_telemetry::testing::capture();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let got = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut traceparent = None;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("traceparent:") {
                    traceparent = Some(v.trim().to_owned());
                }
            }
            let reply = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
            std::io::Write::write_all(&mut stream, reply.as_bytes()).unwrap();
            traceparent
        });
        let span = tracing::info_span!("bot.request");
        let trace = voidforge_telemetry::trace_id(&span).expect("exported");
        let alice = Player::new("alice", reqwest::Client::new(), &api, "t".to_owned());
        let answered = tracing::Instrument::instrument(alice.get_json("/x"), span)
            .await
            .unwrap();
        assert!(answered.is_ok());
        let traceparent = got
            .join()
            .unwrap()
            .expect("the request carries a traceparent");
        assert!(traceparent.contains(&trace), "{traceparent} names {trace}");
    }

    /// Each request's idempotency key and arrival time.
    type Seen = Arc<Mutex<Vec<(Option<String>, Instant)>>>;

    /// An API stand-in: answers each request with the next of `replies` (status, Retry-After,
    /// body), recording each request's idempotency key and arrival time.
    struct StandIn {
        api: String,
        requests: Seen,
    }

    impl StandIn {
        fn start(replies: Vec<(u16, Option<&'static str>, &'static str)>) -> Self {
            use std::io::{BufRead as _, BufReader, Read as _};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let api = format!("http://{}", listener.local_addr().unwrap());
            let requests: Seen = Arc::new(Mutex::new(Vec::new()));
            let seen = requests.clone();
            std::thread::spawn(move || {
                let mut replies = replies.into_iter();
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { return };
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let (mut length, mut key) = (0, None);
                    loop {
                        let mut header = String::new();
                        reader.read_line(&mut header).unwrap();
                        if header == "\r\n" || header.is_empty() {
                            break;
                        }
                        let lower = header.to_ascii_lowercase();
                        if let Some(v) = lower.strip_prefix("content-length:") {
                            length = v.trim().parse().unwrap();
                        }
                        if let Some(v) = lower.strip_prefix("idempotency-key:") {
                            key = Some(v.trim().to_owned());
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    seen.lock().unwrap().push((key, Instant::now()));
                    let (status, retry, reply) = replies.next().unwrap_or((500, None, "{}"));
                    let retry = retry.map_or(String::new(), |r| format!("retry-after: {r}\r\n"));
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n{retry}content-length: {}\r\nconnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
                }
            });
            Self { api, requests }
        }

        fn player(&self) -> Player {
            Player::new("alice", reqwest::Client::new(), &self.api, "t".to_owned())
        }

        fn requests(&self) -> Vec<(Option<String>, Instant)> {
            self.requests.lock().unwrap().clone()
        }
    }

    const LIMITED: &str =
        r#"{"code":"rate_limited","message":"too many requests; retry after 1 s"}"#;
    const BOARD: &str = r#"{"as_of":"2026-10-20T00:00:00.000Z","entries":[]}"#;

    /// The longest a stand-in run may take: a client that ignores its bounds fails here, fast,
    /// instead of hanging the suite.
    const RUN_WITHIN: Duration = Duration::from_secs(20);

    async fn bounded<T>(f: impl Future<Output = T>) -> T {
        tokio::time::timeout(RUN_WITHIN, f)
            .await
            .unwrap_or_else(|_| panic!("the client didn't finish within {RUN_WITHIN:?}"))
    }

    /// A 429 waits its Retry-After, then the same request again succeeds; the wait is counted.
    #[tokio::test]
    async fn a_429_waits_its_retry_after_then_retries() {
        let s = StandIn::start(vec![(429, Some("1"), LIMITED), (200, None, BOARD)]);
        let alice = s.player();
        let reply = bounded(alice.leaderboard()).await.unwrap();
        assert!(reply.is_ok(), "{reply:?}");
        assert_eq!(alice.rate_limited(), 1);
        let seen = s.requests();
        assert_eq!(seen.len(), 2);
        let waited = seen[1].1 - seen[0].1;
        assert!(waited >= Duration::from_secs(1), "retried after {waited:?}");
        assert!(waited < Duration::from_secs(2), "retried after {waited:?}");
    }

    /// At most three retries: a fourth 429 is answered as the refusal it is, every one counted.
    #[tokio::test]
    async fn retries_are_bounded() {
        let s = StandIn::start(vec![(429, Some("0"), LIMITED); 5]);
        let alice = s.player();
        let reply = bounded(alice.leaderboard()).await.unwrap();
        let Err(refused) = reply else {
            panic!("a 429 after three retries is a refusal")
        };
        assert_eq!(
            (refused.status, refused.body.code),
            (429, ErrorCode::RateLimited)
        );
        assert_eq!(s.requests().len(), 4, "the request and three retries");
        assert_eq!(alice.rate_limited(), 4);
    }

    /// A Retry-After that's missing, malformed or over a minute isn't waited: the 429 is answered
    /// at once.
    #[tokio::test]
    async fn an_unusable_retry_after_is_not_waited() {
        for retry in [None, Some("soon"), Some("61"), Some("-1")] {
            let s = StandIn::start(vec![(429, retry, LIMITED), (200, None, BOARD)]);
            let alice = s.player();
            let started = Instant::now();
            let reply = bounded(alice.leaderboard()).await.unwrap();
            assert!(
                matches!(reply, Err(Refused { status: 429, .. })),
                "{retry:?}: {reply:?}"
            );
            assert_eq!(s.requests().len(), 1, "{retry:?}");
            assert!(started.elapsed() < Duration::from_secs(1), "{retry:?}");
        }
    }

    /// A retried command keeps its idempotency key: a 429 never consumed it (D-054).
    #[tokio::test]
    async fn a_retried_command_keeps_its_key() {
        let accepted = r#"{"seq":1,"game_time":"2026-10-20T00:00:00.000Z","events":[]}"#;
        let s = StandIn::start(vec![(429, Some("0"), LIMITED), (200, None, accepted)]);
        let alice = s.player();
        let command: CommandRequest =
            serde_json::from_str(r#"{"type":"disband_fleet","payload":{"fleet_id":1}}"#).unwrap();
        let reply = bounded(alice.command(&command, "the-key")).await.unwrap();
        assert!(reply.is_ok(), "{reply:?}");
        let keys: Vec<_> = s.requests().into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            keys,
            [Some("the-key".to_owned()), Some("the-key".to_owned())]
        );
    }
}
