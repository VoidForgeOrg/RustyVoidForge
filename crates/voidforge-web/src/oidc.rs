//! The OIDC client (P4.1, D-057): the authorization code flow with PKCE, against dex.
//!
//! Our own, on jsonwebtoken and aws-lc-rs rather than the openidconnect crate, which would have
//! brought 90 crates, ten duplicate versions and an unpatched advisory (D-057). It's security
//! code, so every rule is explicit here:
//! - **Endpoints come only from discovery**, whose `issuer` must equal the configured one
//!   exactly, and the authorization, token and key endpoints must share the issuer's origin.
//!   The HTTP client follows no redirects, so the token POST can't be sent anywhere else.
//! - **state, nonce and the PKCE verifier** are 32 bytes each from aws-lc-rs's CSPRNG,
//!   base64url without padding. The challenge is S256 (RFC 7636 4.2).
//! - **The token response** must say `token_type` Bearer (in any case) and carry an `id_token`.
//! - **The ID token check is at least as strict as the server's (D-046):** RS256 only, no
//!   `crit`, a well-formed `kid`; the key from dex's set, refetched on an unknown `kid` one
//!   fetch at a time and at most once per [`MIN_REFRESH`], counted from when the last fetch
//!   finished; the clock read after the key lookup; then `iss` exactly, our client ID in `aud`,
//!   `azp` equal to our client ID whenever `aud` has several values or `azp` is present, `exp`
//!   (required) and `nbf` with a bounded leeway, `iat` (required) not in the future, a `sub` of
//!   1 to 255 bytes, and `nonce` equal to the login attempt's own, compared in constant time.
//!
//! Nothing here logs or prints a token, a code, the verifier, state, nonce or the secret.

use std::collections::HashMap;
use std::fmt;
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

use crate::config::{Config, Secret};

/// The most bytes a discovery document, key set or token response may have.
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024;
/// How long one request to dex may take.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// The shortest time between two key fetches, counted from when the last one finished.
pub const MIN_REFRESH: Duration = Duration::from_secs(10);
/// The leeway on `exp`, `nbf` and `iat`: the server's default (D-046).
pub const LEEWAY_SECS: u64 = 30;
/// The scopes asked for: `email` and `profile` give Home the viewer's email or name.
pub const SCOPES: &str = "openid email profile";

/// A login attempt, kept in the session until its callback, which uses it once.
#[derive(Clone, Serialize, Deserialize)]
pub struct PendingLogin {
    /// The `state` sent to dex.
    pub state: Secret,
    /// The `nonce` sent to dex.
    pub nonce: Secret,
    /// The PKCE verifier; only its S256 challenge was sent.
    pub verifier: Secret,
    /// When the attempt began, in Unix seconds.
    pub created: u64,
    /// Where to go after logging in: a local path.
    pub next: String,
}

impl fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingLogin")
            .field("created", &self.created)
            .field("next", &self.next)
            .finish_non_exhaustive()
    }
}

/// A verified login.
#[derive(Debug, Clone)]
pub struct Identity {
    /// The ID token, for the API.
    pub id_token: Secret,
    /// The `email` claim, if any.
    pub email: Option<String>,
    /// The `name` claim, if any.
    pub name: Option<String>,
    /// The token's `exp`, in Unix seconds.
    pub exp: u64,
}

/// Which ID token check failed: for the log and the tests. The page names the class only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// The token's header doesn't decode.
    Header,
    /// `alg` isn't RS256.
    Algorithm,
    /// A `crit` header (RFC 7515 4.1.11).
    Critical,
    /// No `kid`, or not 1 to 128 printable ASCII bytes.
    KeyId,
    /// No key has that `kid`, even after a refetch.
    UnknownKey,
    /// The signature doesn't verify, or the claims don't decode.
    Signature,
    /// `iss` isn't the issuer.
    Issuer,
    /// Our client ID isn't in `aud`.
    Audience,
    /// `aud` has several values and `azp` isn't our client ID, or `azp` names another client.
    AuthorizedParty,
    /// No `exp`, or past it plus the leeway.
    Expired,
    /// Before `nbf` minus the leeway.
    NotYetValid,
    /// No `iat`, or in the future beyond the leeway.
    IssuedAt,
    /// `sub` is empty or over 255 bytes.
    Subject,
    /// No `nonce`, or not this attempt's.
    Nonce,
}

/// Why a callback didn't log anyone in.
#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    /// The token endpoint failed or answered something other than 200 with JSON.
    #[error("the code exchange failed: {0}")]
    Exchange(String),
    /// `token_type` isn't Bearer.
    #[error("the token response's token_type isn't Bearer")]
    TokenType,
    /// The token response has no `id_token`.
    #[error("the token response has no id_token")]
    NoIdToken,
    /// The ID token failed a check.
    #[error("the ID token failed the {0:?} check")]
    Token(Check),
    /// dex's keys can't be loaded now.
    #[error("dex's keys can't be loaded")]
    KeysUnavailable,
    /// The CSPRNG failed.
    #[error("the system's random number generator failed")]
    Random,
}

/// Why the web app can't use the issuer.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// The request failed or timed out.
    #[error("fetching {what}: {error}")]
    Http {
        /// What was fetched.
        what: &'static str,
        /// What happened.
        error: String,
    },
    /// Over 64 KiB, or not the JSON it should be.
    #[error("{what} is malformed: {reason}")]
    Malformed {
        /// What was fetched.
        what: &'static str,
        /// Why.
        reason: String,
    },
    /// The document names another issuer.
    #[error("the discovery document's issuer {found:?} isn't the configured {expected:?}")]
    IssuerMismatch {
        /// Configured.
        expected: String,
        /// Found.
        found: String,
    },
    /// An endpoint isn't on the issuer's origin.
    #[error("the {endpoint} {url:?} isn't on the issuer's origin {origin:?}")]
    CrossOrigin {
        /// Which endpoint.
        endpoint: &'static str,
        /// Its URL.
        url: String,
        /// The issuer's origin.
        origin: String,
    },
    /// The issuer doesn't offer PKCE's S256.
    #[error("the issuer doesn't support PKCE with S256")]
    NoS256,
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct TokenResponse {
    token_type: String,
    id_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Audience,
    azp: Option<String>,
    exp: Option<u64>,
    iat: Option<u64>,
    nbf: Option<u64>,
    nonce: Option<String>,
    email: Option<String>,
    name: Option<String>,
}

/// When the last key fetch finished, and whether it succeeded.
#[derive(Clone, Copy)]
struct LastFetch {
    at: Instant,
    ok: bool,
}

enum Lookup {
    Found(DecodingKey),
    Unknown,
    Unavailable,
}

/// The issuer, its endpoints from discovery, and dex's keys.
pub struct Provider {
    issuer: String,
    client_id: String,
    client_secret: Secret,
    redirect_url: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    http: reqwest::Client,
    validation: Validation,
    keys: RwLock<HashMap<String, DecodingKey>>,
    /// Single flight: one fetch at a time, and when the last one finished.
    fetch: tokio::sync::Mutex<Option<LastFetch>>,
    /// The shortest time between two key fetches ([`MIN_REFRESH`] unless a test sets it).
    min_refresh: Duration,
}

impl fmt::Debug for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provider")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

/// `scheme://authority` of an http(s) URL, or None.
fn origin(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(&url[..scheme.len() + 3 + end])
}

/// 32 bytes from the CSPRNG, base64url without padding.
fn random_value() -> Result<Secret, LoginError> {
    let mut bytes = [0u8; 32];
    aws_lc_rs::rand::fill(&mut bytes).map_err(|_| LoginError::Random)?;
    Ok(Secret::new(URL_SAFE_NO_PAD.encode(bytes)))
}

/// The PKCE S256 challenge of a verifier (RFC 7636 4.2).
#[must_use]
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(aws_lc_rs::digest::digest(
        &aws_lc_rs::digest::SHA256,
        verifier.as_bytes(),
    ))
}

/// Seconds since the Unix epoch, or None for a clock before it.
#[must_use]
pub fn unix_now() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn well_formed_kid(kid: &str) -> bool {
    (1..=128).contains(&kid.len()) && kid.bytes().all(|b| b.is_ascii_graphic())
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

/// Reads a body of at most [`MAX_DOCUMENT_BYTES`].
async fn bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len().saturating_add(chunk.len()) > MAX_DOCUMENT_BYTES {
            return Err("over 64 KiB".to_owned());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

impl Provider {
    /// Reads the issuer's discovery document and checks it. Keys load on first use.
    ///
    /// # Errors
    /// [`DiscoveryError`]: dex can't be reached, its document is malformed, it names another
    /// issuer, an endpoint is on another origin, or it lacks S256.
    pub async fn discover(config: &Config) -> Result<Self, DiscoveryError> {
        let http = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| DiscoveryError::Http {
                what: "the HTTP client",
                error: e.to_string(),
            })?;
        let what = "the discovery document";
        let url = format!("{}/.well-known/openid-configuration", config.issuer);
        let response = http
            .get(&url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| DiscoveryError::Http {
                what,
                error: e.to_string(),
            })?;
        let body = bounded_body(response)
            .await
            .map_err(|reason| DiscoveryError::Malformed { what, reason })?;
        let d: Discovery =
            serde_json::from_slice(&body).map_err(|e| DiscoveryError::Malformed {
                what,
                reason: e.to_string(),
            })?;
        if d.issuer != config.issuer {
            return Err(DiscoveryError::IssuerMismatch {
                expected: config.issuer.clone(),
                found: d.issuer,
            });
        }
        let issuer_origin = origin(&config.issuer).unwrap_or_default().to_owned();
        for (endpoint, url) in [
            ("authorization endpoint", &d.authorization_endpoint),
            ("token endpoint", &d.token_endpoint),
            ("key set", &d.jwks_uri),
        ] {
            if origin(url) != Some(issuer_origin.as_str()) {
                return Err(DiscoveryError::CrossOrigin {
                    endpoint,
                    url: url.clone(),
                    origin: issuer_origin,
                });
            }
        }
        if !d
            .code_challenge_methods_supported
            .iter()
            .any(|m| m == "S256")
        {
            return Err(DiscoveryError::NoS256);
        }
        // The library checks the signature and the algorithm only; every claim is checked in
        // `claims`, where its edges are explicit (as the server does, D-046).
        let mut validation = Validation::new(Algorithm::RS256);
        validation.algorithms = vec![Algorithm::RS256];
        validation.required_spec_claims.clear();
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        validation.leeway = 0;
        Ok(Self {
            issuer: config.issuer.clone(),
            client_id: config.client_id.clone(),
            client_secret: config.client_secret.clone(),
            redirect_url: config.redirect_url.clone(),
            authorization_endpoint: d.authorization_endpoint,
            token_endpoint: d.token_endpoint,
            jwks_uri: d.jwks_uri,
            http,
            validation,
            keys: RwLock::new(HashMap::new()),
            fetch: tokio::sync::Mutex::new(None),
            min_refresh: MIN_REFRESH,
        })
    }

    /// Sets the shortest time between two key fetches; [`MIN_REFRESH`] by default. Tests use a
    /// shorter one to pin when the floor starts.
    #[must_use]
    pub fn with_key_refetch_floor(mut self, floor: Duration) -> Self {
        self.min_refresh = floor;
        self
    }

    /// The issuer.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Starts a login: fresh state, nonce and verifier, and the URL to send the browser to.
    ///
    /// # Errors
    /// [`LoginError::Random`] if the CSPRNG fails.
    pub fn begin(&self, next: String, now: u64) -> Result<(String, PendingLogin), LoginError> {
        let pending = PendingLogin {
            state: random_value()?,
            nonce: random_value()?,
            verifier: random_value()?,
            created: now,
            next,
        };
        let url = reqwest::Url::parse_with_params(
            &self.authorization_endpoint,
            [
                ("response_type", "code"),
                ("client_id", self.client_id.as_str()),
                ("redirect_uri", self.redirect_url.as_str()),
                ("scope", SCOPES),
                ("state", pending.state.expose()),
                ("nonce", pending.nonce.expose()),
                ("code_challenge", &challenge(pending.verifier.expose())),
                ("code_challenge_method", "S256"),
            ],
        )
        .map_err(|_| LoginError::Exchange("the authorization endpoint isn't a URL".to_owned()))?;
        Ok((url.into(), pending))
    }

    /// Exchanges a callback's code with the attempt's verifier and checks the ID token.
    ///
    /// # Errors
    /// [`LoginError`]: the exchange failed, the response is wrong, or the token fails a check.
    pub async fn finish(&self, code: &str, pending: &PendingLogin) -> Result<Identity, LoginError> {
        let response = self
            .http
            .post(&self.token_endpoint)
            .basic_auth(&self.client_id, Some(self.client_secret.expose()))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &self.redirect_url),
                ("code_verifier", pending.verifier.expose()),
            ])
            .send()
            .await
            // Only the error's kind: reqwest's text can include the URL, never a secret, but
            // there's nothing in it worth the risk.
            .map_err(|e| {
                LoginError::Exchange(
                    if e.is_timeout() {
                        "timed out"
                    } else {
                        "failed"
                    }
                    .to_owned(),
                )
            })?;
        let status = response.status();
        if status != reqwest::StatusCode::OK {
            // A redirect lands here too: the client follows none (D-057).
            return Err(LoginError::Exchange(format!(
                "the token endpoint answered {}",
                status.as_u16()
            )));
        }
        let body = bounded_body(response).await.map_err(LoginError::Exchange)?;
        let tokens: TokenResponse = serde_json::from_slice(&body).map_err(|_| {
            LoginError::Exchange("the token response isn't the expected JSON".to_owned())
        })?;
        if !tokens.token_type.eq_ignore_ascii_case("bearer") {
            return Err(LoginError::TokenType);
        }
        let id_token = tokens.id_token.ok_or(LoginError::NoIdToken)?;
        self.verify(&id_token, pending.nonce.expose()).await
    }

    /// Checks an ID token for this client and this nonce.
    ///
    /// # Errors
    /// [`LoginError::Token`] with the failed check, or [`LoginError::KeysUnavailable`].
    pub async fn verify(&self, token: &str, nonce: &str) -> Result<Identity, LoginError> {
        let refuse = LoginError::Token;
        let header = jsonwebtoken::decode_header(token).map_err(|_| refuse(Check::Header))?;
        if header.alg != Algorithm::RS256 {
            return Err(refuse(Check::Algorithm));
        }
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
            Lookup::Unavailable => return Err(LoginError::KeysUnavailable),
        };
        let data = jsonwebtoken::decode::<Claims>(token, &key, &self.validation)
            .map_err(|_| refuse(Check::Signature))?;
        // The time is read after the key lookup, which may wait on the lock and a fetch (the
        // server's P3.2 lesson, F1). A clock before 1970 can't judge anything.
        let now = unix_now().ok_or(LoginError::KeysUnavailable)?;
        let c = data.claims;
        self.claims(&c, nonce, now).map_err(refuse)?;
        let bounded = |s: Option<String>| s.filter(|s| !s.is_empty() && s.len() <= 320);
        Ok(Identity {
            id_token: Secret::new(token.to_owned()),
            email: bounded(c.email),
            name: bounded(c.name),
            exp: c.exp.unwrap_or_default(),
        })
    }

    fn claims(&self, c: &Claims, nonce: &str, now: u64) -> Result<(), Check> {
        if c.iss != self.issuer {
            return Err(Check::Issuer);
        }
        let audiences = match &c.aud {
            Audience::One(a) => std::slice::from_ref(a),
            Audience::Many(a) => a.as_slice(),
        };
        if !audiences.contains(&self.client_id) {
            return Err(Check::Audience);
        }
        // OIDC Core 3.1.3.7, items 4 and 5: several audiences need `azp`, and an `azp` present
        // must be this client.
        let azp_ok = match &c.azp {
            Some(azp) => *azp == self.client_id,
            None => audiences.len() == 1,
        };
        if !azp_ok {
            return Err(Check::AuthorizedParty);
        }
        match c.exp {
            Some(exp) if now <= exp.saturating_add(LEEWAY_SECS) => {}
            _ => return Err(Check::Expired),
        }
        if c.nbf
            .is_some_and(|nbf| now.saturating_add(LEEWAY_SECS) < nbf)
        {
            return Err(Check::NotYetValid);
        }
        match c.iat {
            Some(iat) if iat <= now.saturating_add(LEEWAY_SECS) => {}
            _ => return Err(Check::IssuedAt),
        }
        if c.sub.is_empty() || c.sub.len() > 255 {
            return Err(Check::Subject);
        }
        let nonce_ok = c.nonce.as_deref().is_some_and(|n| {
            aws_lc_rs::constant_time::verify_slices_are_equal(n.as_bytes(), nonce.as_bytes())
                .is_ok()
        });
        if !nonce_ok {
            return Err(Check::Nonce);
        }
        Ok(())
    }

    fn cached(&self, kid: &str) -> Option<DecodingKey> {
        self.keys
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(kid)
            .cloned()
    }

    /// The key for `kid`: from the cache, or after a refetch. Fetches run one at a time, and at
    /// most once per [`MIN_REFRESH`] counted from when the last one finished, so a burst of
    /// logins with an unknown `kid` can't each fetch (the server's D-046 R1-1). A caller that
    /// waited on another's fetch looks at the cache again before fetching itself.
    async fn key(&self, kid: &str) -> Lookup {
        if let Some(key) = self.cached(kid) {
            return Lookup::Found(key);
        }
        let mut last = self.fetch.lock().await;
        if let Some(key) = self.cached(kid) {
            return Lookup::Found(key);
        }
        if let Some(l) = *last
            && l.at.elapsed() < self.min_refresh
        {
            // Within the floor: the last fetch decides. It succeeded without this kid, so the
            // kid is unknown; it failed, so dex can't be asked now (the server's 503, D-046).
            return if l.ok {
                Lookup::Unknown
            } else {
                Lookup::Unavailable
            };
        }
        let fetched = self.fetch_keys().await;
        // The floor counts from the fetch's end, so a slow dex can't let queued logins each
        // fetch (D-046 R1-1).
        *last = Some(LastFetch {
            at: Instant::now(),
            ok: fetched.is_ok(),
        });
        drop(last);
        match fetched {
            Ok(keys) => {
                let found = keys.get(kid).cloned();
                *self.keys.write().unwrap_or_else(|e| e.into_inner()) = keys;
                found.map_or(Lookup::Unknown, Lookup::Found)
            }
            Err(e) => {
                tracing::error!(error = %e, "fetching dex's keys failed; keeping the last good keys");
                Lookup::Unavailable
            }
        }
    }

    async fn fetch_keys(&self) -> Result<HashMap<String, DecodingKey>, DiscoveryError> {
        let what = "the key set";
        let response = self
            .http
            .get(&self.jwks_uri)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| DiscoveryError::Http {
                what,
                error: e.to_string(),
            })?;
        let body = bounded_body(response)
            .await
            .map_err(|reason| DiscoveryError::Malformed { what, reason })?;
        let jwks: Jwks = serde_json::from_slice(&body).map_err(|e| DiscoveryError::Malformed {
            what,
            reason: e.to_string(),
        })?;
        let mut keys = HashMap::new();
        for jwk in &jwks.keys {
            if let Some((kid, key)) = rsa_key(jwk) {
                keys.entry(kid).or_insert(key);
            }
        }
        if keys.is_empty() {
            return Err(DiscoveryError::Malformed {
                what,
                reason: "no usable RS256 key".to_owned(),
            });
        }
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_matches_rfc_7636_appendix_b() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_values_are_32_bytes_and_differ() {
        let values: Vec<String> = (0..64)
            .map(|_| random_value().expect("rng").expose().to_owned())
            .collect();
        for v in &values {
            assert_eq!(URL_SAFE_NO_PAD.decode(v).expect("base64url").len(), 32);
        }
        let mut unique = values.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), values.len());
    }

    #[test]
    fn origins() {
        assert_eq!(
            origin("http://localhost:5556/dex"),
            Some("http://localhost:5556")
        );
        assert_eq!(origin("https://a.example/x?y"), Some("https://a.example"));
        assert_eq!(
            origin("http://localhost:5556"),
            Some("http://localhost:5556")
        );
        assert_eq!(
            origin("http://localhost:5557/dex"),
            Some("http://localhost:5557")
        );
        assert_eq!(origin("ftp://a/b"), None);
        assert_eq!(origin("http:///x"), None);
        assert_ne!(
            origin("http://localhost:5556.evil/dex"),
            Some("http://localhost:5556")
        );
    }

    #[test]
    fn pending_logins_print_no_secret() {
        let p = PendingLogin {
            state: Secret::new("state-value".to_owned()),
            nonce: Secret::new("nonce-value".to_owned()),
            verifier: Secret::new("verifier-value".to_owned()),
            created: 1,
            next: "/".to_owned(),
        };
        let shown = format!("{p:?}");
        for word in ["state-value", "nonce-value", "verifier-value"] {
            assert!(!shown.contains(word), "{shown}");
        }
    }
}
