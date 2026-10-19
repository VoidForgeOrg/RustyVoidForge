//! Token checks over real HTTP (Technical Plan P3.2; decision log D-046). A local stand-in for
//! dex serves a discovery document and a key set of RSA keys generated here (so no private key
//! is committed), counts its fetches, and can fail on demand; a test app guards one route with
//! the `Principal` extractor; requests go through reqwest. Every refusal is checked for its
//! status, code and headers, and for saying nothing about the token or the failed check.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use clap::Parser;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use voidforge_server::auth::{AuthArgs, AuthConfig, FetchError, Principal, TokenVerifier};

mod common;
use common::dex::*;

async fn whoami(principal: Principal) -> axum::Json<Value> {
    axum::Json(json!({"issuer": principal.issuer, "subject": principal.subject}))
}

/// A dex stand-in and a test app over a verifier.
struct Harness {
    dex: Arc<Dex>,
    issuer: String,
    app: String,
    verifier: Arc<TokenVerifier>,
    http: reqwest::Client,
}

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

impl Harness {
    /// The default settings, except the refresh floor and how long keys stay usable.
    async fn new(keys: &[&TestKey], min_refresh: Duration, max_stale: Duration) -> Self {
        let dex = Arc::new(Dex::default());
        *dex.keys.lock().unwrap() = keys.iter().map(|k| k.jwk.clone()).collect();
        let base = serve(
            Router::new()
                .route("/dex/.well-known/openid-configuration", get(discovery))
                .route("/dex/keys", get(jwks))
                .with_state(dex.clone()),
        )
        .await;
        let issuer = format!("{base}/dex");
        *dex.issuer.lock().unwrap() = issuer.clone();
        let config = AuthConfig {
            issuer: issuer.clone(),
            audiences: vec!["voidforge-web".to_owned(), "voidforge-cli".to_owned()],
            leeway: Duration::from_secs(30),
            refresh: Duration::from_secs(900),
            min_refresh,
            max_stale,
        };
        let verifier = Arc::new(TokenVerifier::new(config).unwrap());
        let app = serve(
            Router::new()
                .route("/whoami", get(whoami))
                .with_state(verifier.clone()),
        )
        .await;
        Self {
            dex,
            issuer,
            app,
            verifier,
            http: reqwest::Client::new(),
        }
    }

    async fn default(keys: &[&TestKey]) -> Self {
        Self::new(keys, Duration::from_secs(10), Duration::from_secs(86_400)).await
    }

    async fn call(&self, authorization: &[&str]) -> Reply {
        let mut request = self.http.get(format!("{}/whoami", self.app));
        for value in authorization {
            request = request.header("authorization", *value);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .map(|v| v.to_str().unwrap().to_owned())
        };
        let (www, retry) = (header("www-authenticate"), header("retry-after"));
        let body: Value = response.json().await.unwrap();
        Reply {
            status,
            body,
            www,
            retry,
        }
    }

    async fn token(&self, token: &str) -> Reply {
        self.call(&[&format!("Bearer {token}")]).await
    }

    fn fetches(&self) -> usize {
        self.dex.fetches.load(Ordering::SeqCst)
    }
}

#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
    www: Option<String>,
    retry: Option<String>,
}

impl Reply {
    fn accepted(&self) -> &str {
        assert_eq!(self.status, 200, "{self:?}");
        self.body["subject"].as_str().unwrap()
    }

    /// A 401 `invalid_token` that says nothing about why.
    fn invalid(&self, token: &str, why: &str) {
        assert_eq!(
            (self.status, self.body["code"].as_str()),
            (401, Some("invalid_token")),
            "{why}: {self:?}"
        );
        assert_eq!(
            self.www.as_deref(),
            Some("Bearer realm=\"voidforge\", error=\"invalid_token\""),
            "{why}"
        );
        let text = self.body.to_string();
        assert!(
            token.len() < 8 || !text.contains(token),
            "{why}: the token is echoed"
        );
        for word in [
            "expired",
            "signature",
            "issuer",
            "audience",
            "kid",
            "alg",
            "subject",
        ] {
            assert!(
                !text.to_lowercase().contains(word),
                "{why}: names {word}: {text}"
            );
        }
        assert_eq!(
            self.body.as_object().unwrap().len(),
            2,
            "{why}: only code and message"
        );
    }

    fn missing(&self) {
        assert_eq!(
            (self.status, self.body["code"].as_str()),
            (401, Some("missing_token")),
            "{self:?}"
        );
        assert_eq!(self.www.as_deref(), Some("Bearer realm=\"voidforge\""));
    }

    fn unavailable(&self) {
        assert_eq!(
            (self.status, self.body["code"].as_str()),
            (503, Some("unavailable")),
            "{self:?}"
        );
        let secs: u64 = self.retry.as_deref().expect("Retry-After").parse().unwrap();
        assert!((1..=30).contains(&secs), "Retry-After {secs}");
    }
}

// --- The checks ---

#[tokio::test]
async fn a_valid_token_gives_the_principal() {
    let key = TestKey::new("k1");
    let h = Harness::default(&[&key]).await;
    let token = key.sign(&claims(&h.issuer));
    let reply = h.token(&token).await;
    assert_eq!(reply.accepted(), "CiQwYjZjNWEyZS1hbGljZRIFbG9jYWw");
    assert_eq!(reply.body["issuer"], json!(h.issuer));
    // The scheme is case-insensitive; an audience list of allowed clients is fine.
    let mut c = claims(&h.issuer);
    c["aud"] = json!(["voidforge-cli", "voidforge-web"]);
    let reply = h.call(&[&format!("bearer {}", key.sign(&c))]).await;
    reply.accepted();
}

#[tokio::test]
async fn every_claim_is_checked_with_its_edges() {
    let key = TestKey::new("k1");
    let h = Harness::default(&[&key]).await;
    let t = now();
    let base = claims(&h.issuer);
    let set = |field: &str, value: Value| {
        let mut c = base.clone();
        c[field] = value;
        c
    };
    let drop = |field: &str| {
        let mut c = base.clone();
        c.as_object_mut().unwrap().remove(field);
        c
    };
    // Accepted at the edges.
    for (c, why) in [
        (set("exp", json!(t - 30 + 5)), "expired, within the leeway"),
        (
            set("nbf", json!(t + 30 - 5)),
            "not yet valid, within the leeway",
        ),
        (
            set("iat", json!(t + 30 - 5)),
            "issued ahead, within the leeway",
        ),
        (set("sub", json!("s".repeat(255))), "a 255-byte subject"),
        (
            set("exp", json!(u64::MAX)),
            "exp at u64::MAX: no overflow, far future",
        ),
    ] {
        let reply = h.token(&key.sign(&c)).await;
        assert_eq!(reply.status, 200, "{why}: {reply:?}");
    }
    // Refused.
    for (c, why) in [
        (
            set("exp", json!(t - 30 - 5)),
            "expired just past the leeway",
        ),
        (set("nbf", json!(t + 30 + 5)), "not yet valid"),
        (set("iat", json!(t + 30 + 5)), "issued in the future"),
        (
            set("iss", json!("http://localhost:5556/other")),
            "a foreign issuer",
        ),
        (
            set("iss", json!(format!("{}/", h.issuer))),
            "the issuer with a slash",
        ),
        (set("aud", json!("someone-else")), "a foreign audience"),
        (
            set("aud", json!("VOIDFORGE-WEB")),
            "an audience in another case",
        ),
        (
            set("aud", json!("voidforge-web ")),
            "an audience with a trailing space",
        ),
        (
            set("aud", json!(["voidforge-cli", "someone-else"])),
            "an extra audience",
        ),
        (set("aud", json!([])), "no audience"),
        (set("sub", json!("")), "an empty subject"),
        (set("sub", json!("s".repeat(256))), "a 256-byte subject"),
        (set("sub", json!(42)), "a numeric subject"),
        (drop("sub"), "no subject"),
        (drop("exp"), "no exp"),
        (drop("iat"), "no iat"),
        (drop("iss"), "no issuer"),
        (drop("aud"), "no audience at all"),
        (set("exp", json!(-1)), "a negative exp"),
        (set("exp", json!(1.5e9)), "a float exp"),
        (set("exp", json!(format!("{}", t + 3600))), "a string exp"),
        (
            set("exp", serde_json::from_str("18446744073709551616").unwrap()),
            "exp past u64",
        ),
        (set("iat", json!(-1)), "a negative iat"),
        (set("nbf", json!("soon")), "a string nbf"),
    ] {
        let token = key.sign(&c);
        h.token(&token).await.invalid(&token, why);
    }
}

#[tokio::test]
async fn forged_and_tampered_tokens_are_refused() {
    let key = TestKey::new("k1");
    let other = TestKey::new("k1"); // the same kid, another key
    let h = Harness::default(&[&key]).await;
    let c = claims(&h.issuer);
    let good = key.sign(&c);
    h.token(&good).await.accepted();
    let parts: Vec<&str> = good.split('.').collect();
    // A tampered payload: another subject, the original signature.
    let mut evil = c.clone();
    evil["sub"] = json!("mallory");
    let tampered_payload = format!(
        "{}.{}.{}",
        parts[0],
        b64url(evil.to_string().as_bytes()),
        parts[2]
    );
    // A tampered signature: one character changed.
    let mut signature = parts[2].to_owned();
    let flipped = if signature.ends_with('A') { 'B' } else { 'A' };
    signature.pop();
    signature.push(flipped);
    let tampered_signature = format!("{}.{}.{signature}", parts[0], parts[1]);
    // alg none, and HS256 with the public key as the secret (algorithm confusion).
    let none = raw(&json!({"alg": "none", "kid": "k1"}), &c, "");
    let mut hs = Header::new(Algorithm::HS256);
    hs.kid = Some("k1".to_owned());
    let confused =
        jsonwebtoken::encode(&hs, &c, &EncodingKey::from_secret(&key.public_der)).unwrap();
    // Properly signed by the right key, but with no kid: we never guess the key.
    let no_kid = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &c, &key.encoding).unwrap();
    let es = raw(&json!({"alg": "ES256", "kid": "k1"}), &c, parts[2]);
    let ps = raw(&json!({"alg": "PS256", "kid": "k1"}), &c, parts[2]);
    for (token, why) in [
        (tampered_payload, "a tampered payload"),
        (tampered_signature, "a tampered signature"),
        (other.sign(&c), "signed by another key under the same kid"),
        (none, "alg none"),
        (confused, "HS256 with the public key as the secret"),
        (es, "ES256"),
        (ps, "PS256"),
        (raw(&json!({"alg": "RS256"}), &c, parts[2]), "no kid"),
        (no_kid, "a valid signature, but no kid"),
        (
            raw(
                &json!({"alg": "RS256", "kid": "k".repeat(129)}),
                &c,
                parts[2],
            ),
            "a 129-byte kid",
        ),
        (
            raw(&json!({"alg": "RS256", "kid": "k 1"}), &c, parts[2]),
            "a kid with a space",
        ),
        ("garbage".to_owned(), "garbage"),
        ("a.b.c".to_owned(), "three garbage parts"),
        (format!("{}.{}", parts[0], parts[1]), "no signature part"),
    ] {
        h.token(&token).await.invalid(&token, why);
    }
}

#[tokio::test]
async fn the_header_is_checked_before_anything_else() {
    let key = TestKey::new("k1");
    let h = Harness::default(&[&key]).await;
    let token = key.sign(&claims(&h.issuer));
    let mut padded = claims(&h.issuer);
    padded["padding"] = json!("p".repeat(8 * 1024));
    let big = key.sign(&padded);
    assert!(big.len() > 8 * 1024 && big.len() < 16 * 1024);
    h.call(&[]).await.missing();
    for (values, why) in [
        (vec![format!("Basic {token}")], "another scheme"),
        (vec!["Bearer".to_owned()], "no token"),
        (vec!["Bearer ".to_owned()], "an empty token"),
        (vec![format!("Bearer  {token}")], "two spaces"),
        (vec![format!("Bearer {token} extra")], "a trailing word"),
        (
            vec![format!("Bearer {token}"), format!("Bearer {token}")],
            "two headers",
        ),
        (
            vec![format!("Bearer {}", "a".repeat(9 * 1024))],
            "over 8 KiB",
        ),
        (
            vec![format!("Bearer {big}")],
            "a properly signed token over 8 KiB",
        ),
        (vec![token.clone()], "no scheme"),
    ] {
        let refs: Vec<&str> = values.iter().map(String::as_str).collect();
        h.call(&refs).await.invalid(&token, why);
    }
    assert_eq!(h.fetches(), 0, "no header failure ever fetched keys");
}

#[tokio::test]
async fn a_rotated_key_is_picked_up_through_an_unknown_kid() {
    let old = TestKey::new("2026-10-19");
    let new = TestKey::new("2026-10-20");
    let h = Harness::new(&[&old], Duration::from_secs(1), Duration::from_secs(86_400)).await;
    h.token(&old.sign(&claims(&h.issuer))).await.accepted();
    assert_eq!(h.fetches(), 1);
    // dex rotates: both keys published. A token under the new key, after the floor.
    h.dex.keys.lock().unwrap().push(new.jwk.clone());
    tokio::time::sleep(Duration::from_millis(1100)).await;
    h.token(&new.sign(&claims(&h.issuer))).await.accepted();
    assert_eq!(h.fetches(), 2);
    // The periodic refresh drops a key dex no longer publishes.
    *h.dex.keys.lock().unwrap() = vec![new.jwk.clone()];
    h.verifier.refresh().await.unwrap();
    let token = old.sign(&claims(&h.issuer));
    h.token(&token).await.invalid(&token, "a removed key");
}

#[tokio::test]
async fn unknown_kids_refresh_at_most_once_per_floor() {
    let key = TestKey::new("k1");
    let h = Harness::default(&[&key]).await;
    h.token(&key.sign(&claims(&h.issuer))).await.accepted();
    assert_eq!(h.fetches(), 1);
    // A flood of random kids within the floor: no fetch at all.
    for i in 0..50 {
        let token = raw(
            &json!({"alg": "RS256", "kid": format!("random-{i}")}),
            &claims(&h.issuer),
            "c2ln",
        );
        h.token(&token).await.invalid(&token, "an unknown kid");
    }
    assert_eq!(h.fetches(), 1, "the floor holds");
    // Another algorithm never reaches the keys, even with an unknown kid past the floor.
    let h = Harness::new(&[&key], Duration::from_secs(1), Duration::from_secs(86_400)).await;
    for alg in ["HS256", "ES256", "PS256"] {
        let token = raw(
            &json!({"alg": alg, "kid": "never-seen"}),
            &claims(&h.issuer),
            "c2ln",
        );
        h.token(&token).await.invalid(&token, alg);
    }
    assert_eq!(h.fetches(), 0, "no key fetch for another algorithm");
}

#[tokio::test]
async fn concurrent_requests_share_one_fetch() {
    let key = TestKey::new("k1");
    let h = Arc::new(Harness::default(&[&key]).await);
    let token = key.sign(&claims(&h.issuer));
    let calls: Vec<_> = (0..20)
        .map(|_| {
            let (h, token) = (h.clone(), token.clone());
            tokio::spawn(async move { h.token(&token).await.status })
        })
        .collect();
    for call in calls {
        assert_eq!(call.await.unwrap(), 200);
    }
    assert_eq!(h.fetches(), 1, "single flight");
}

#[tokio::test]
async fn dex_down_at_start_is_503_until_keys_load_never_an_accept() {
    let key = TestKey::new("k1");
    let h = Harness::new(&[&key], Duration::from_secs(1), Duration::from_secs(86_400)).await;
    h.dex.down.store(true, Ordering::SeqCst);
    let token = key.sign(&claims(&h.issuer));
    h.token(&token).await.unavailable();
    // The header still decides first: missing and malformed are 401, not 503.
    h.call(&[]).await.missing();
    h.call(&["Basic abc"])
        .await
        .invalid("abc", "malformed while dex is down");
    let none = raw(&json!({"alg": "none", "kid": "k1"}), &claims(&h.issuer), "");
    h.token(&none)
        .await
        .invalid(&none, "alg none while dex is down");
    // The algorithm is checked before any key is touched: never 503 for HS256 or ES256.
    let mut hs = Header::new(Algorithm::HS256);
    hs.kid = Some("k1".to_owned());
    let hs =
        jsonwebtoken::encode(&hs, &claims(&h.issuer), &EncodingKey::from_secret(b"s")).unwrap();
    h.token(&hs).await.invalid(&hs, "HS256 while dex is down");
    let es = raw(
        &json!({"alg": "ES256", "kid": "k1"}),
        &claims(&h.issuer),
        "c2ln",
    );
    h.token(&es).await.invalid(&es, "ES256 while dex is down");
    // dex comes up; after the floor, the same token verifies.
    h.dex.down.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    h.token(&token).await.accepted();
}

#[tokio::test]
async fn dex_down_later_keeps_known_keys_and_refuses_to_guess() {
    let key = TestKey::new("k1");
    let rotated = TestKey::new("k2");
    let h = Harness::new(&[&key], Duration::from_secs(1), Duration::from_secs(86_400)).await;
    let token = key.sign(&claims(&h.issuer));
    h.token(&token).await.accepted();
    h.dex.down.store(true, Ordering::SeqCst);
    h.token(&token).await.accepted();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    // An unknown kid whose refresh fails may be a new key: 503, not 401, and never 200.
    h.token(&rotated.sign(&claims(&h.issuer)))
        .await
        .unavailable();
    assert!(h.verifier.refresh().await.is_err());
    h.token(&token).await.accepted();
}

#[tokio::test]
async fn stale_keys_are_dropped() {
    let key = TestKey::new("k1");
    let h = Harness::new(&[&key], Duration::from_secs(1), Duration::from_secs(1)).await;
    let token = key.sign(&claims(&h.issuer));
    h.token(&token).await.accepted();
    h.dex.down.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    h.token(&token).await.unavailable();
}

#[tokio::test]
async fn a_discovery_document_naming_another_issuer_is_never_trusted() {
    let key = TestKey::new("k1");
    let h = Harness::default(&[&key]).await;
    *h.dex.claimed_issuer.lock().unwrap() = Some("http://evil.example/dex".to_owned());
    h.token(&key.sign(&claims(&h.issuer))).await.unavailable();
    assert!(matches!(
        h.verifier.refresh().await,
        Err(FetchError::IssuerMismatch { .. })
    ));
}

#[tokio::test]
async fn only_usable_rs256_keys_are_loaded() {
    let key = TestKey::new("good");
    let h = Harness::default(&[&key]).await;
    let mut junk = Vec::new();
    for (field, value) in [
        ("kty", json!("EC")),
        ("alg", json!("HS256")),
        ("use", json!("enc")),
        ("n", json!("!!")),
    ] {
        let mut jwk = key.jwk.clone();
        jwk[field] = value;
        jwk["kid"] = json!(format!("junk-{field}"));
        junk.push(jwk);
    }
    let mut nokid = key.jwk.clone();
    nokid.as_object_mut().unwrap().remove("kid");
    junk.push(nokid);
    *h.dex.keys.lock().unwrap() = junk.clone();
    assert!(matches!(
        h.verifier.refresh().await,
        Err(FetchError::NoKeys)
    ));
    junk.push(key.jwk.clone());
    *h.dex.keys.lock().unwrap() = junk;
    assert_eq!(h.verifier.refresh().await.unwrap(), 1);
    h.token(&key.sign(&claims(&h.issuer))).await.accepted();
}

#[tokio::test]
async fn an_oversized_key_set_is_refused() {
    let key = TestKey::new("k1");
    let h = Harness::default(&[&key]).await;
    let mut keys = vec![key.jwk.clone()];
    keys.extend(
        (0..200).map(|i| json!({"kty": "oct", "kid": format!("pad-{i}"), "k": "x".repeat(400)})),
    );
    *h.dex.keys.lock().unwrap() = keys;
    assert!(matches!(
        h.verifier.refresh().await,
        Err(FetchError::Malformed { .. })
    ));
    h.token(&key.sign(&claims(&h.issuer))).await.unavailable();
}

// --- Configuration ---

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    auth: AuthArgs,
}

fn config(args: &[&str]) -> Result<AuthConfig, String> {
    let cli = Cli::try_parse_from(std::iter::once("server").chain(args.iter().copied()))
        .map_err(|e| e.to_string())?;
    cli.auth.config().map_err(|e| e.to_string())
}

#[test]
fn the_defaults_match_the_local_stack_and_absurd_settings_are_refused() {
    let c = config(&[]).unwrap();
    assert_eq!(c.issuer, "http://localhost:5556/dex");
    assert_eq!(c.audiences, ["voidforge-web", "voidforge-cli"]);
    assert_eq!(
        (c.leeway, c.refresh, c.min_refresh, c.max_stale),
        (
            Duration::from_secs(30),
            Duration::from_secs(900),
            Duration::from_secs(10),
            Duration::from_secs(86_400)
        )
    );
    assert_eq!(
        config(&["--audience", "a,b"]).unwrap().audiences,
        ["a", "b"]
    );
    // The issuer is taken verbatim (R1-3): a trailing slash stays, to fail the exact match.
    assert_eq!(
        config(&["--issuer", "http://localhost:5556/dex/"])
            .unwrap()
            .issuer,
        "http://localhost:5556/dex/"
    );
    assert_eq!(
        config(&["--token-leeway-secs", "300"])
            .unwrap()
            .leeway
            .as_secs(),
        300
    );
    for (args, setting) in [
        (vec!["--token-leeway-secs", "301"], "token-leeway-secs"),
        (vec!["--jwks-refresh-secs", "0"], "jwks-refresh-secs"),
        (vec!["--jwks-refresh-secs", "59"], "jwks-refresh-secs"),
        (vec!["--jwks-refresh-secs", "86401"], "jwks-refresh-secs"),
        (
            vec!["--jwks-min-refresh-secs", "0"],
            "jwks-min-refresh-secs",
        ),
        (
            vec!["--jwks-refresh-secs", "60", "--jwks-min-refresh-secs", "61"],
            "jwks-min-refresh-secs",
        ),
        (vec!["--jwks-max-stale-secs", "899"], "jwks-max-stale-secs"),
        (
            vec!["--jwks-max-stale-secs", "604801"],
            "jwks-max-stale-secs",
        ),
        (vec!["--issuer", "localhost:5556/dex"], "issuer"),
        (vec!["--issuer", "http://"], "issuer"),
        (vec!["--audience", ""], "audience"),
    ] {
        let e = config(&args).unwrap_err();
        assert!(e.contains(setting), "{args:?}: {e}");
    }
}

// --- Repair round 1 (review-r1's R1-1, QA's Q8, Q9 and Q11, I1) ---

/// R1-1: with a dex slower than the floor, queued unknown-kid requests still make one fetch per
/// floor window: the floor counts from when a fetch finishes, and a waiter looks again.
#[tokio::test]
async fn a_slow_dex_gets_one_fetch_per_floor_however_many_wait() {
    let key = TestKey::new("k1");
    let stranger = TestKey::new("k9");
    let h =
        Arc::new(Harness::new(&[&key], Duration::from_secs(1), Duration::from_secs(86_400)).await);
    h.token(&key.sign(&claims(&h.issuer))).await.accepted();
    assert_eq!(h.fetches(), 1);
    h.dex.delay_ms.store(1500, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let token = stranger.sign(&claims(&h.issuer));
    let calls: Vec<_> = (0..20)
        .map(|_| {
            let (h, token) = (h.clone(), token.clone());
            tokio::spawn(async move { h.token(&token).await })
        })
        .collect();
    for call in calls {
        call.await
            .unwrap()
            .invalid(&token, "an unknown kid behind a slow dex");
    }
    assert_eq!(h.fetches(), 2, "one fetch for twenty waiting requests");
}

/// QA's Q9: while dex is down, the floor holds too: a failed fetch is a fetch.
#[tokio::test]
async fn a_down_dex_gets_one_fetch_per_floor() {
    let key = TestKey::new("k1");
    let h = Harness::default(&[&key]).await;
    h.dex.down.store(true, Ordering::SeqCst);
    let token = key.sign(&claims(&h.issuer));
    h.token(&token).await.unavailable();
    h.token(&token).await.unavailable();
    assert_eq!(h.fetches(), 1, "the second request waits out the floor");
}

/// QA's Q8: a malformed kid never fetches, even past the floor.
#[tokio::test]
async fn a_malformed_kid_never_fetches() {
    let key = TestKey::new("k1");
    let h = Harness::new(&[&key], Duration::from_secs(1), Duration::from_secs(86_400)).await;
    h.token(&key.sign(&claims(&h.issuer))).await.accepted();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let long = "k".repeat(129);
    for kid in ["k 1", "", long.as_str()] {
        let token = raw(
            &json!({"alg": "RS256", "kid": kid}),
            &claims(&h.issuer),
            "c2ln",
        );
        h.token(&token).await.invalid(&token, "a malformed kid");
    }
    assert_eq!(h.fetches(), 1);
}

/// QA's Q11: discovery's issuer must match exactly; a trailing slash is another issuer.
#[tokio::test]
async fn a_discovery_issuer_with_a_trailing_slash_is_not_ours() {
    let key = TestKey::new("k1");
    let h = Harness::default(&[&key]).await;
    *h.dex.claimed_issuer.lock().unwrap() = Some(format!("{}/", h.issuer));
    h.token(&key.sign(&claims(&h.issuer))).await.unavailable();
    assert!(matches!(
        h.verifier.refresh().await,
        Err(FetchError::IssuerMismatch { .. })
    ));
}

/// I1: a token with a `crit` header is refused (RFC 7515 4.1.11), properly signed or not, and
/// before any key is touched (401 even while dex is down).
#[tokio::test]
async fn a_critical_header_is_refused() {
    let key = TestKey::new("k1");
    let h = Harness::default(&[&key]).await;
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("k1".to_owned());
    header.crit = Some(vec!["exp".to_owned()]);
    let token = jsonwebtoken::encode(&header, &claims(&h.issuer), &key.encoding).unwrap();
    h.token(&token).await.invalid(&token, "crit");
    h.dex.down.store(true, Ordering::SeqCst);
    h.token(&token)
        .await
        .invalid(&token, "crit while dex is down");
    assert_eq!(h.fetches(), 0);
}

// --- Repair round 2 (dev-qa's F1 and F2) ---

fn bearer_headers(token: &str) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
    headers
}

/// F1: the time is read after the key lookup. A token valid when the request arrived, which
/// expires while the verifier waits 4 s for dex's keys, is refused.
#[tokio::test]
async fn expiry_is_judged_after_the_wait_for_keys() {
    let key = TestKey::new("k1");
    let h = Harness::default(&[&key]).await;
    h.dex.delay_ms.store(4000, Ordering::SeqCst);
    let mut c = claims(&h.issuer);
    c["exp"] = json!(now() - 30 + 2); // valid for 2 more seconds, counting the leeway
    let token = key.sign(&c);
    h.token(&token)
        .await
        .invalid(&token, "expired while waiting for keys");
}

/// F2: the second look at the cache under the lock matters. A fetches (2 s) a set that newly
/// holds kK; H (an unknown kid, whose task then blocks the thread 1.5 s) and B (kK, whose first
/// look missed it) queue behind it. B gets the lock past the floor: without a second look it
/// would fetch again for a key already cached.
#[tokio::test]
async fn a_waiter_finds_a_key_another_request_just_fetched() {
    let k0 = TestKey::new("k0");
    let kk = TestKey::new("kK");
    let h = Harness::new(&[&k0], Duration::from_secs(1), Duration::from_secs(86_400)).await;
    h.token(&k0.sign(&claims(&h.issuer))).await.accepted();
    h.dex.keys.lock().unwrap().push(kk.jwk.clone());
    h.dex.delay_ms.store(2000, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let before = h.fetches();
    let tk = kk.sign(&claims(&h.issuer));
    let stranger = TestKey::new("unknown-x");
    let tx = stranger.sign(&claims(&h.issuer));
    let (va, vh, vb) = (h.verifier.clone(), h.verifier.clone(), h.verifier.clone());
    let (ta, tb) = (tk.clone(), tk.clone());
    let a = tokio::spawn(async move { va.verify(&bearer_headers(&ta)).await.is_ok() });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let hh = tokio::spawn(async move {
        let r = vh.verify(&bearer_headers(&tx)).await.is_ok();
        std::thread::sleep(Duration::from_millis(1500)); // a busy thread
        r
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let b = tokio::spawn(async move { vb.verify(&bearer_headers(&tb)).await.is_ok() });
    let (ra, rh, rb) = (a.await.unwrap(), hh.await.unwrap(), b.await.unwrap());
    assert_eq!(
        (ra, rh, rb),
        (true, false, true),
        "A and B accepted, H refused"
    );
    assert_eq!(h.fetches() - before, 1, "B found kK on its second look");
}
