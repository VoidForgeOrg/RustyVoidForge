//! Token checks (Technical Plan P3.2; decision log D-046): a request's `Authorization: Bearer
//! <dex ID token>` becomes a verified [`Principal`] (issuer and subject), or a 401, before
//! anything reaches the host. Nothing in the engine knows about auth.
//!
//! The order is fixed, and every failure is a typed refusal, never an accept or a sentinel:
//! 1. the header, before any key state: none is 401 `missing_token`; anything but exactly one
//!    well-formed `Bearer <token>` of at most 8 KiB is 401 `invalid_token`;
//! 2. the token's header: `alg` must be RS256 (never `none`, HS* or another family), no `crit`
//!    (RFC 7515 4.1.11), and a `kid` of 1 to 128 printable ASCII bytes is required;
//! 3. the key, from dex's keys cached by `kid` ([`TokenVerifier`]); when none can be loaded, or
//!    an unknown `kid`'s refresh fails, 503 `unavailable` with `Retry-After`: the token may be
//!    valid, and it can't be checked now;
//! 4. the signature (RS256 only), then the claims, checked here rather than by the library:
//!    `iss` exactly, every `aud` in the allowlist, `exp` (required) and `nbf` (if present) with
//!    the leeway, `iat` (required) not in the future, and a `sub` of 1 to 255 bytes.
//!
//! 401 bodies are D-045's `ErrorBody`, with `WWW-Authenticate: Bearer` (RFC 6750), and never
//! say which check failed; the check is logged at debug level, never with the token's text.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{FromRef, FromRequestParts};
use axum::http::header::{AUTHORIZATION, RETRY_AFTER, WWW_AUTHENTICATE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use voidforge_api_types::error::{ErrorBody, ErrorCode};

/// The local stack's issuer (D-015).
pub const DEFAULT_ISSUER: &str = "http://localhost:5556/dex";
/// The most bytes an `Authorization` header may have.
pub const MAX_HEADER_BYTES: usize = 8 * 1024;
/// The most bytes a discovery document or key set may have.
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024;
/// How long one fetch from dex may take.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// The longest `Retry-After` and the longest wait between startup attempts.
pub const MAX_RETRY_SECS: u64 = 30;
/// The largest leeway the configuration accepts: more would quietly weaken expiry.
pub const MAX_LEEWAY_SECS: u64 = 300;

// --- Configuration ---

/// The token checks' settings, from flags or the environment. The defaults match the local
/// stack; P3.4's `serve` takes them.
#[derive(Debug, Clone, clap::Args)]
pub struct AuthArgs {
    /// The issuer: dex's URL, matched exactly as given (a trailing slash included), and the base
    /// of its discovery document.
    #[arg(long, env = "VOIDFORGE_ISSUER", default_value = DEFAULT_ISSUER)]
    pub issuer: String,
    /// The audiences a token may name; every one it names must be here.
    #[arg(
        long = "audience",
        env = "VOIDFORGE_AUDIENCES",
        value_delimiter = ',',
        default_values_t = [String::from("voidforge-web"), String::from("voidforge-cli")]
    )]
    pub audiences: Vec<String>,
    /// Clock skew allowed on `exp`, `nbf` and `iat`, in seconds (at most 300).
    #[arg(long, env = "VOIDFORGE_TOKEN_LEEWAY_SECS", default_value_t = 30)]
    pub token_leeway_secs: u64,
    /// How often dex's keys are fetched again, in seconds (60 to 86,400).
    #[arg(long, env = "VOIDFORGE_JWKS_REFRESH_SECS", default_value_t = 900)]
    pub jwks_refresh_secs: u64,
    /// The least time between two fetches, in seconds (1 to 3,600, and at most the refresh).
    #[arg(long, env = "VOIDFORGE_JWKS_MIN_REFRESH_SECS", default_value_t = 10)]
    pub jwks_min_refresh_secs: u64,
    /// How long keys stay usable without a successful fetch, in seconds (from the refresh to
    /// seven days).
    #[arg(long, env = "VOIDFORGE_JWKS_MAX_STALE_SECS", default_value_t = 86_400)]
    pub jwks_max_stale_secs: u64,
}

/// Checked settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    /// The issuer, exactly.
    pub issuer: String,
    /// The audience allowlist.
    pub audiences: Vec<String>,
    /// The leeway.
    pub leeway: Duration,
    /// The periodic refresh.
    pub refresh: Duration,
    /// The least time between fetches.
    pub min_refresh: Duration,
    /// How long keys stay usable without a successful fetch.
    pub max_stale: Duration,
}

/// A setting out of range: the server refuses to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid token-check setting {setting}: {reason}")]
pub struct ConfigError {
    /// The setting.
    pub setting: &'static str,
    /// Why.
    pub reason: String,
}

impl AuthArgs {
    /// The settings, checked.
    ///
    /// # Errors
    /// [`ConfigError`] for an empty or non-HTTP issuer, no or an empty audience, a leeway over
    /// 300 s, or a refresh setting that is zero, absurd, or inconsistent with the others.
    pub fn config(&self) -> Result<AuthConfig, ConfigError> {
        let bad = |setting, reason: &str| ConfigError {
            setting,
            reason: reason.to_owned(),
        };
        // Taken verbatim: the match with the token's `iss` and discovery's `issuer` is exact, so
        // a trailing slash here fails there, loudly, rather than being trimmed silently.
        let issuer = self.issuer.as_str();
        if !(issuer.starts_with("http://") || issuer.starts_with("https://"))
            || issuer.len() <= "https://".len()
        {
            return Err(bad("issuer", "must be an http(s) URL"));
        }
        if self.audiences.is_empty() || self.audiences.iter().any(String::is_empty) {
            return Err(bad("audience", "at least one, none empty"));
        }
        if self.token_leeway_secs > MAX_LEEWAY_SECS {
            return Err(bad("token-leeway-secs", "at most 300"));
        }
        if !(60..=86_400).contains(&self.jwks_refresh_secs) {
            return Err(bad("jwks-refresh-secs", "60 to 86400"));
        }
        if !(1..=3_600).contains(&self.jwks_min_refresh_secs)
            || self.jwks_min_refresh_secs > self.jwks_refresh_secs
        {
            return Err(bad(
                "jwks-min-refresh-secs",
                "1 to 3600, and at most the refresh",
            ));
        }
        if !(self.jwks_refresh_secs..=7 * 86_400).contains(&self.jwks_max_stale_secs) {
            return Err(bad("jwks-max-stale-secs", "from the refresh to 604800"));
        }
        Ok(AuthConfig {
            issuer: issuer.to_owned(),
            audiences: self.audiences.clone(),
            leeway: Duration::from_secs(self.token_leeway_secs),
            refresh: Duration::from_secs(self.jwks_refresh_secs),
            min_refresh: Duration::from_secs(self.jwks_min_refresh_secs),
            max_stale: Duration::from_secs(self.jwks_max_stale_secs),
        })
    }
}

// --- Results ---

/// A verified token's issuer and subject. P3.3 maps them to a player. `Debug` hides the
/// subject, which is personal data, so a logged principal never leaks it (D-047).
#[derive(Clone, PartialEq, Eq)]
pub struct Principal {
    /// The issuer, as configured.
    pub issuer: String,
    /// The subject: dex's stable ID for the user, 1 to 255 bytes.
    pub subject: String,
}

impl fmt::Debug for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Principal")
            .field("issuer", &self.issuer)
            .field("subject", &format_args!("<{} bytes>", self.subject.len()))
            .finish()
    }
}

/// Why a request has no principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No `Authorization` header: 401 `missing_token`.
    Missing,
    /// Anything else that fails: 401 `invalid_token`. The check is logged, never shown.
    Invalid(Check),
    /// The keys can't be loaded now: 503 `unavailable`, retry after this many seconds.
    Unavailable { retry_after_secs: u64 },
}

/// Which check refused a token: for the debug log and the tests, never for the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Several `Authorization` headers.
    SeveralHeaders,
    /// The header is over 8 KiB.
    TooLarge,
    /// Not `Bearer <token>`, or the token isn't a JWT's characters.
    Malformed,
    /// The token's header doesn't decode.
    Header,
    /// `alg` isn't RS256.
    Algorithm,
    /// No `kid`, or not 1 to 128 printable ASCII bytes.
    KeyId,
    /// A `crit` header: no critical extension is understood (RFC 7515 4.1.11).
    Critical,
    /// No key has that `kid`, even after a refresh.
    UnknownKey,
    /// The signature doesn't verify, or the claims don't decode.
    Signature,
    /// `iss` isn't the issuer.
    Issuer,
    /// An `aud` value isn't in the allowlist, or there's none.
    Audience,
    /// Past `exp` plus the leeway.
    Expired,
    /// Before `nbf` minus the leeway.
    NotYetValid,
    /// `iat` is in the future beyond the leeway.
    IssuedInFuture,
    /// `sub` is empty or over 255 bytes.
    Subject,
}

// --- Keys ---

/// dex's keys, cached by `kid`, and the token checks over them. Share it as `Arc`; run
/// [`TokenVerifier::refresh_forever`] beside the server.
pub struct TokenVerifier {
    config: AuthConfig,
    http: reqwest::Client,
    validation: Validation,
    keys: RwLock<KeySet>,
    /// Single flight: one fetch at a time, and how the last one ended.
    fetch: tokio::sync::Mutex<Option<LastFetch>>,
}

/// When the last fetch finished, and whether it succeeded.
struct LastFetch {
    at: Instant,
    ok: bool,
}

#[derive(Default)]
struct KeySet {
    keys: HashMap<String, DecodingKey>,
    /// When the keys were last fetched successfully.
    fetched_at: Option<Instant>,
}

/// What a fetch from dex can fail with. Each keeps the last good keys.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// The request failed or timed out.
    #[error("fetching {what}: {error}")]
    Http {
        /// The discovery document or the key set.
        what: &'static str,
        /// What happened.
        error: String,
    },
    /// The document is over 64 KiB or not the JSON it should be.
    #[error("{what} is malformed: {reason}")]
    Malformed {
        /// The discovery document or the key set.
        what: &'static str,
        /// Why.
        reason: String,
    },
    /// The discovery document names another issuer: the configuration is wrong.
    #[error("the discovery document's issuer {found:?} isn't the configured {expected:?}")]
    IssuerMismatch {
        /// Configured.
        expected: String,
        /// Found.
        found: String,
    },
    /// The key set has no usable RS256 key.
    #[error("the key set has no usable RS256 key")]
    NoKeys,
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<serde_json::Value>,
}

enum Lookup {
    Found(DecodingKey),
    Unknown,
    Unavailable,
}

impl TokenVerifier {
    /// A verifier with no keys yet: they load on the first request or the first refresh.
    ///
    /// # Errors
    /// [`FetchError::Http`] if the HTTP client can't be built.
    pub fn new(config: AuthConfig) -> Result<Self, FetchError> {
        let http = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| FetchError::Http {
                what: "the HTTP client",
                error: e.to_string(),
            })?;
        // The library checks the signature and the algorithm only; every claim is checked in
        // `claims`, where its edges are explicit.
        let mut validation = Validation::new(Algorithm::RS256);
        validation.algorithms = vec![Algorithm::RS256];
        validation.required_spec_claims.clear();
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        validation.leeway = 0;
        Ok(Self {
            config,
            http,
            validation,
            keys: RwLock::new(KeySet::default()),
            fetch: tokio::sync::Mutex::new(None),
        })
    }

    /// The settings.
    #[must_use]
    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    /// Verifies a request's headers.
    ///
    /// # Errors
    /// [`Refusal`]: missing, invalid (with the check), or unavailable.
    pub async fn verify(&self, headers: &HeaderMap) -> Result<Principal, Refusal> {
        let token = bearer(headers)?;
        let header = jsonwebtoken::decode_header(token).map_err(|_| refuse(Check::Header))?;
        if header.alg != Algorithm::RS256 {
            return Err(refuse(Check::Algorithm));
        }
        // RFC 7515 4.1.11: a critical extension this verifier doesn't know must be refused, and
        // it knows none (dex never sets `crit`).
        if header.crit.is_some() {
            return Err(refuse(Check::Critical));
        }
        let kid = header
            .kid
            .filter(|k| well_formed_kid(k))
            .ok_or(refuse(Check::KeyId))?;
        let key = match self.key(&kid).await {
            Lookup::Found(key) => key,
            Lookup::Unknown => return Err(refuse(Check::UnknownKey)),
            Lookup::Unavailable => {
                return Err(Refusal::Unavailable {
                    retry_after_secs: self.retry_after().await,
                });
            }
        };
        let data = jsonwebtoken::decode::<Claims>(token, &key, &self.validation)
            .map_err(|_| refuse(Check::Signature))?;
        // The time is read after the key lookup, which may wait on the lock and on fetches:
        // `exp`, `nbf` and `iat` are judged as of now, not as of the request's arrival (repair
        // round 2, dev-qa's F1). A clock before 1970 is an error, never a time of 0.
        let Some(now) = unix_secs(SystemTime::now()) else {
            tracing::error!("the system clock is before 1970: tokens can't be checked");
            return Err(Refusal::Unavailable {
                retry_after_secs: MAX_RETRY_SECS,
            });
        };
        self.claims(data.claims, now).map_err(refuse)
    }

    fn claims(&self, c: Claims, now: u64) -> Result<Principal, Check> {
        let leeway = self.config.leeway.as_secs();
        if c.iss != self.config.issuer {
            return Err(Check::Issuer);
        }
        let audiences = match &c.aud {
            Audience::One(a) => std::slice::from_ref(a),
            Audience::Many(a) => a.as_slice(),
        };
        if audiences.is_empty() || !audiences.iter().all(|a| self.config.audiences.contains(a)) {
            return Err(Check::Audience);
        }
        if now > c.exp.saturating_add(leeway) {
            return Err(Check::Expired);
        }
        if c.nbf.is_some_and(|nbf| now.saturating_add(leeway) < nbf) {
            return Err(Check::NotYetValid);
        }
        if c.iat > now.saturating_add(leeway) {
            return Err(Check::IssuedInFuture);
        }
        if c.sub.is_empty() || c.sub.len() > 255 {
            return Err(Check::Subject);
        }
        Ok(Principal {
            issuer: c.iss,
            subject: c.sub,
        })
    }

    /// The cached key for `kid`, while the cache is fresh enough.
    fn cached(&self, kid: &str) -> Option<DecodingKey> {
        let set = self.keys.read().unwrap_or_else(|e| e.into_inner());
        if self.usable(&set) {
            set.keys.get(kid).cloned()
        } else {
            None
        }
    }

    fn usable(&self, set: &KeySet) -> bool {
        set.fetched_at
            .is_some_and(|at| at.elapsed() <= self.config.max_stale)
    }

    /// The key for `kid`: from the cache, or after a refresh. Fetches run one at a time, and at
    /// most once per `min_refresh`, counted from when the last one **finished**, so a slow dex
    /// can't let queued requests each fetch (repair round 1, review-r1's R1-1). A request that
    /// waited for another's fetch looks at the cache again before fetching itself: past the floor
    /// it would otherwise fetch a key that's already cached (dev-qa's schedule, repair round 2).
    async fn key(&self, kid: &str) -> Lookup {
        if let Some(key) = self.cached(kid) {
            return Lookup::Found(key);
        }
        let mut last = self.fetch.lock().await;
        if let Some(key) = self.cached(kid) {
            return Lookup::Found(key);
        }
        let due = last
            .as_ref()
            .is_none_or(|l| l.at.elapsed() >= self.config.min_refresh);
        let ok = if due {
            let ok = self.fetch_and_store().await.is_ok();
            *last = Some(LastFetch {
                at: Instant::now(),
                ok,
            });
            ok
        } else {
            last.as_ref().is_some_and(|l| l.ok)
        };
        drop(last);
        if let Some(key) = self.cached(kid) {
            return Lookup::Found(key);
        }
        let usable = self.usable(&self.keys.read().unwrap_or_else(|e| e.into_inner()));
        // The last fetch succeeded and lacks the kid: it's unknown. It failed: it may be a key
        // dex just rotated in, so it can't be judged now.
        if ok && usable {
            Lookup::Unknown
        } else {
            Lookup::Unavailable
        }
    }

    /// Seconds until the next fetch may run, from 1 to 30.
    async fn retry_after(&self) -> u64 {
        let wait = self
            .fetch
            .lock()
            .await
            .as_ref()
            .map_or(Duration::ZERO, |l| {
                self.config.min_refresh.saturating_sub(l.at.elapsed())
            });
        wait.as_secs().saturating_add(1).clamp(1, MAX_RETRY_SECS)
    }

    /// Fetches the keys now, through discovery, and replaces the cache on success. The periodic
    /// refresh calls it; requests go through the floor instead. Either way the floor counts
    /// from when it finishes.
    ///
    /// # Errors
    /// [`FetchError`]; the cache keeps its last good keys.
    pub async fn refresh(&self) -> Result<usize, FetchError> {
        let mut last = self.fetch.lock().await;
        let result = self.fetch_and_store().await;
        *last = Some(LastFetch {
            at: Instant::now(),
            ok: result.is_ok(),
        });
        result
    }

    /// One fetch; the cache is replaced only on success. Failures are logged.
    async fn fetch_and_store(&self) -> Result<usize, FetchError> {
        match self.fetch_keys().await {
            Ok(keys) => {
                let n = keys.len();
                let mut set = self.keys.write().unwrap_or_else(|e| e.into_inner());
                *set = KeySet {
                    keys,
                    fetched_at: Some(Instant::now()),
                };
                Ok(n)
            }
            Err(e) => {
                tracing::error!(error = %e, "fetching dex's keys failed; keeping the last good keys");
                Err(e)
            }
        }
    }

    /// Keeps the keys fresh for the server's life: until a first set loads, retries with a
    /// backoff from 1 s doubling to 30 s; then refreshes every `refresh`. Failures are logged
    /// and keep the last good keys.
    pub async fn refresh_forever(self: Arc<Self>) {
        let mut backoff = Duration::from_secs(1);
        loop {
            let loaded = match self.refresh().await {
                Ok(n) => {
                    tracing::info!(keys = n, "dex's keys loaded");
                    true
                }
                Err(e) => {
                    tracing::error!(error = %e, "fetching dex's keys failed");
                    false
                }
            };
            let has_keys = self
                .keys
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .fetched_at
                .is_some();
            let wait = if loaded || has_keys {
                self.config.refresh
            } else {
                let wait = backoff;
                backoff = backoff
                    .saturating_mul(2)
                    .min(Duration::from_secs(MAX_RETRY_SECS));
                wait
            };
            tokio::time::sleep(wait).await;
        }
    }

    async fn fetch_keys(&self) -> Result<HashMap<String, DecodingKey>, FetchError> {
        let url = format!("{}/.well-known/openid-configuration", self.config.issuer);
        let discovery: Discovery = self.get_json("the discovery document", &url).await?;
        if discovery.issuer != self.config.issuer {
            return Err(FetchError::IssuerMismatch {
                expected: self.config.issuer.clone(),
                found: discovery.issuer,
            });
        }
        let jwks: Jwks = self.get_json("the key set", &discovery.jwks_uri).await?;
        let mut keys = HashMap::new();
        for jwk in &jwks.keys {
            match rsa_key(jwk) {
                Some((kid, key)) => {
                    keys.entry(kid).or_insert(key);
                }
                None => tracing::debug!("skipped a key that isn't a usable RS256 signing key"),
            }
        }
        if keys.is_empty() {
            return Err(FetchError::NoKeys);
        }
        Ok(keys)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        what: &'static str,
        url: &str,
    ) -> Result<T, FetchError> {
        let http = |error: String| FetchError::Http { what, error };
        let mut response = self
            .http
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| http(e.to_string()))?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| http(e.to_string()))? {
            if body.len().saturating_add(chunk.len()) > MAX_DOCUMENT_BYTES {
                return Err(FetchError::Malformed {
                    what,
                    reason: "over 64 KiB".to_owned(),
                });
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|e| FetchError::Malformed {
            what,
            reason: e.to_string(),
        })
    }
}

/// An RS256 signing key from a JWK: `kty` RSA, `alg` RS256 or absent, `use` sig or absent, a
/// well-formed `kid`, and an `n` and `e` that make a key.
fn rsa_key(jwk: &serde_json::Value) -> Option<(String, DecodingKey)> {
    let field = |name: &str| jwk.get(name).and_then(serde_json::Value::as_str);
    let absent_or = |name: &str, want: &str| jwk.get(name).is_none() || field(name) == Some(want);
    if field("kty") != Some("RSA") || !absent_or("alg", "RS256") || !absent_or("use", "sig") {
        return None;
    }
    let kid = field("kid").filter(|k| well_formed_kid(k))?;
    let key = DecodingKey::from_rsa_components(field("n")?, field("e")?).ok()?;
    Some((kid.to_owned(), key))
}

/// Seconds since the Unix epoch, or None for a clock before it: never a sentinel 0.
fn unix_secs(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn well_formed_kid(kid: &str) -> bool {
    (1..=128).contains(&kid.len()) && kid.bytes().all(|b| b.is_ascii_graphic())
}

fn refuse(check: Check) -> Refusal {
    tracing::debug!(?check, "token refused");
    Refusal::Invalid(check)
}

/// The token from exactly one `Authorization: Bearer <token>` header.
fn bearer(headers: &HeaderMap) -> Result<&str, Refusal> {
    let mut all = headers.get_all(AUTHORIZATION).iter();
    let value = all.next().ok_or(Refusal::Missing)?;
    if all.next().is_some() {
        return Err(refuse(Check::SeveralHeaders));
    }
    if value.len() > MAX_HEADER_BYTES {
        return Err(refuse(Check::TooLarge));
    }
    let text = value.to_str().map_err(|_| refuse(Check::Malformed))?;
    let (scheme, token) = text.split_once(' ').ok_or(refuse(Check::Malformed))?;
    let jwt_chars = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.');
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() || !token.bytes().all(jwt_chars) {
        return Err(refuse(Check::Malformed));
    }
    Ok(token)
}

/// The claims checked; anything else in the token is ignored. Each numeric claim is a
/// non-negative integer within u64, or the token doesn't decode.
#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Audience,
    exp: u64,
    iat: u64,
    #[serde(default)]
    nbf: Option<u64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

// --- The extractor ---

impl<S> FromRequestParts<S> for Principal
where
    Arc<TokenVerifier>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Refusal;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Refusal> {
        Arc::<TokenVerifier>::from_ref(state)
            .verify(&parts.headers)
            .await
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Missing => (
                StatusCode::UNAUTHORIZED,
                ErrorCode::MissingToken,
                "an Authorization: Bearer token is required",
            ),
            Self::Invalid(_) => (
                StatusCode::UNAUTHORIZED,
                ErrorCode::InvalidToken,
                "the token isn't valid",
            ),
            Self::Unavailable { .. } => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::Unavailable,
                "tokens can't be checked right now; retry later",
            ),
        };
        let body = ErrorBody {
            code,
            message: message.to_owned(),
        };
        let mut response = (status, axum::Json(body)).into_response();
        let headers = response.headers_mut();
        match self {
            Self::Missing => {
                headers.insert(
                    WWW_AUTHENTICATE,
                    HeaderValue::from_static("Bearer realm=\"voidforge\""),
                );
            }
            Self::Invalid(_) => {
                headers.insert(
                    WWW_AUTHENTICATE,
                    HeaderValue::from_static("Bearer realm=\"voidforge\", error=\"invalid_token\""),
                );
            }
            Self::Unavailable { retry_after_secs } => {
                headers.insert(RETRY_AFTER, HeaderValue::from(retry_after_secs));
            }
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Repair round 1, R1-2: a clock before 1970 is no time at all, never 0.
    #[test]
    fn a_clock_before_the_epoch_is_no_time() {
        assert_eq!(unix_secs(UNIX_EPOCH), Some(0));
        assert_eq!(unix_secs(UNIX_EPOCH + Duration::from_secs(42)), Some(42));
        assert_eq!(unix_secs(UNIX_EPOCH - Duration::from_secs(1)), None);
    }
}
