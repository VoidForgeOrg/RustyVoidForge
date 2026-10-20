//! A dex stand-in and an API stand-in for the web app's login tests (P4.1, D-057), and a
//! cookie-holding client. RSA keys are generated at run time, so no private key is committed.
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rsa::{KeyPair, KeySize};
use aws_lc_rs::signature::KeyPair as _;
use axum::Router;
use axum::extract::{Query, RawForm, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use voidforge_web::config::{Config, Secret};
use voidforge_web::oidc::{Provider, challenge};
use voidforge_web::routes::{self, AppState, COOKIE_NAME};

pub const CLIENT: &str = "voidforge-web";
pub const SECRET: &str = "test-client-secret-value";
pub const EMAIL: &str = "alice@voidforge.test";
pub const PLAYER: &str = "01890a5d-ac96-774b-bcce-b302099a8057";

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The DER `RSAPrivateKey` inside a PKCS#8 `PrivateKeyInfo` (jsonwebtoken signs from PKCS#1).
fn pkcs1_from_pkcs8(der: &[u8]) -> Vec<u8> {
    fn tlv(der: &[u8], at: usize) -> (u8, usize, usize) {
        let tag = der[at];
        let first = der[at + 1];
        let (len, start) = if first < 0x80 {
            (first as usize, at + 2)
        } else {
            let n = (first & 0x7f) as usize;
            let len = der[at + 2..at + 2 + n]
                .iter()
                .fold(0usize, |l, b| (l << 8) | *b as usize);
            (len, at + 2 + n)
        };
        (tag, start, start + len)
    }
    let (_, body, _) = tlv(der, 0);
    let (_, _, version_end) = tlv(der, body);
    let (_, _, algorithm_end) = tlv(der, version_end);
    let (_, key, key_end) = tlv(der, algorithm_end);
    der[key..key_end].to_vec()
}

/// A signing key and its public JWK.
pub struct TestKey {
    pub kid: String,
    pub encoding: EncodingKey,
    pub jwk: Value,
    pub public_der: Vec<u8>,
}

impl TestKey {
    pub fn new(kid: &str) -> Self {
        let pair = KeyPair::generate(KeySize::Rsa2048).unwrap();
        let pkcs8 = AsDer::<aws_lc_rs::encoding::Pkcs8V1Der>::as_der(&pair).unwrap();
        let public = pair.public_key();
        let jwk = json!({
            "kty": "RSA", "alg": "RS256", "use": "sig", "kid": kid,
            "n": URL_SAFE_NO_PAD.encode(public.modulus().big_endian_without_leading_zero()),
            "e": URL_SAFE_NO_PAD.encode(public.exponent().big_endian_without_leading_zero()),
        });
        let public_der = AsDer::<aws_lc_rs::encoding::PublicKeyX509Der>::as_der(public)
            .unwrap()
            .as_ref()
            .to_vec();
        Self {
            kid: kid.to_owned(),
            encoding: EncodingKey::from_rsa_der(&pkcs1_from_pkcs8(pkcs8.as_ref())),
            jwk,
            public_der,
        }
    }
}

/// What the stand-in's token endpoint answers; each test changes one thing.
pub struct Behaviour {
    /// Claims merged over the defaults; a null removes the claim.
    pub claims: Value,
    /// The header's `alg` and `kid`, and whether to sign with HS256 over the public key.
    pub kid: Option<String>,
    pub hs256_with_public_key: bool,
    pub crit: bool,
    /// Which key signs: the published one, or one dex never published.
    pub unpublished_key: bool,
    pub token_type: String,
    pub omit_id_token: bool,
    /// Answer the token POST with this status and no tokens.
    pub token_status: Option<StatusCode>,
    /// Answer the token POST with a 307 to `/dex/token-moved`, which issues a real token: a
    /// web app that followed it would log in.
    pub token_redirect: bool,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            claims: json!({}),
            kid: None,
            hs256_with_public_key: false,
            crit: false,
            unpublished_key: false,
            token_type: "bearer".to_owned(),
            omit_id_token: false,
            token_status: None,
            token_redirect: false,
        }
    }
}

struct Attempt {
    nonce: String,
    challenge: String,
    redirect_uri: String,
}

/// The dex stand-in's state.
pub struct Dex {
    pub issuer: String,
    pub key: TestKey,
    pub other_key: TestKey,
    pub behaviour: Mutex<Behaviour>,
    attempts: Mutex<HashMap<String, Attempt>>,
    codes: AtomicUsize,
    pub key_fetches: AtomicUsize,
    /// Every token the token endpoint issued, for the leak checks.
    pub issued: Mutex<Vec<String>>,
    /// Discovery's overrides: replace these fields in the document.
    pub discovery: Mutex<Value>,
}

/// The API stand-in's state.
pub struct Api {
    /// The status and body `/players/me` answers.
    pub answer: Mutex<(StatusCode, Value)>,
    /// The bearer tokens it saw.
    pub seen: Mutex<Vec<String>>,
}

async fn discovery(State(d): State<Arc<Dex>>) -> axum::Json<Value> {
    let mut doc = json!({
        "issuer": d.issuer,
        "authorization_endpoint": format!("{}/auth", d.issuer),
        "token_endpoint": format!("{}/token", d.issuer),
        "jwks_uri": format!("{}/keys", d.issuer),
        "code_challenge_methods_supported": ["S256", "plain"],
    });
    if let Value::Object(over) = &*d.discovery.lock().unwrap() {
        for (k, v) in over {
            doc[k] = v.clone();
        }
    }
    axum::Json(doc)
}

async fn keys(State(d): State<Arc<Dex>>) -> axum::Json<Value> {
    d.key_fetches.fetch_add(1, Ordering::SeqCst);
    axum::Json(json!({ "keys": [d.key.jwk] }))
}

async fn auth(State(d): State<Arc<Dex>>, Query(q): Query<HashMap<String, String>>) -> Response {
    assert_eq!(q.get("response_type").map(String::as_str), Some("code"));
    assert_eq!(
        q.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert!(
        !q.contains_key("code_verifier"),
        "the verifier never leaves the web app"
    );
    let code = format!("code-{}", d.codes.fetch_add(1, Ordering::SeqCst));
    d.attempts.lock().unwrap().insert(
        code.clone(),
        Attempt {
            nonce: q["nonce"].clone(),
            challenge: q["code_challenge"].clone(),
            redirect_uri: q["redirect_uri"].clone(),
        },
    );
    let to = format!(
        "{}?code={code}&state={}",
        q["redirect_uri"],
        urlencode(&q["state"])
    );
    Redirect::to(&to).into_response()
}

pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn token(State(d): State<Arc<Dex>>, headers: HeaderMap, raw: RawForm) -> Response {
    if d.behaviour.lock().unwrap().token_redirect {
        return Redirect::temporary(&format!("{}/token-moved", d.issuer)).into_response();
    }
    issue(d, headers, raw)
}

async fn token_moved(State(d): State<Arc<Dex>>, headers: HeaderMap, raw: RawForm) -> Response {
    issue(d, headers, raw)
}

fn issue(d: Arc<Dex>, headers: HeaderMap, RawForm(raw): RawForm) -> Response {
    let f: HashMap<String, String> =
        reqwest::Url::parse(&format!("http://x/?{}", String::from_utf8_lossy(&raw)))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
    let b = d.behaviour.lock().unwrap();
    let basic = format!("Basic {}", STANDARD.encode(format!("{CLIENT}:{SECRET}")));
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(basic.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error": "invalid_client"})),
        )
            .into_response();
    }
    let Some(attempt) = d.attempts.lock().unwrap().remove(&f["code"]) else {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    };
    // PKCE: the verifier must hash to the challenge sent at /auth.
    let verifier_ok = f
        .get("code_verifier")
        .is_some_and(|v| challenge(v) == attempt.challenge);
    if !verifier_ok || f.get("redirect_uri") != Some(&attempt.redirect_uri) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    }
    let t = now();
    let mut claims = json!({
        "iss": d.issuer, "sub": "alice-subject", "aud": CLIENT,
        "exp": t + 3600, "iat": t, "nonce": attempt.nonce,
        "email": EMAIL, "name": "alice",
    });
    if let Value::Object(over) = &b.claims {
        for (k, v) in over {
            if v.is_null() {
                claims.as_object_mut().unwrap().remove(k);
            } else {
                claims[k] = v.clone();
            }
        }
    }
    let key = if b.unpublished_key {
        &d.other_key
    } else {
        &d.key
    };
    let id_token = if b.hs256_with_public_key {
        let mut h = Header::new(Algorithm::HS256);
        h.kid = Some(key.kid.clone());
        jsonwebtoken::encode(&h, &claims, &EncodingKey::from_secret(&key.public_der)).unwrap()
    } else {
        let mut h = Header::new(Algorithm::RS256);
        h.kid = b.kid.clone().or_else(|| Some(key.kid.clone()));
        if b.kid.as_deref() == Some("") {
            h.kid = None;
        }
        if b.crit {
            h.crit = Some(vec!["exp".to_owned()]);
        }
        jsonwebtoken::encode(&h, &claims, &key.encoding).unwrap()
    };
    d.issued.lock().unwrap().push(id_token.clone());
    let mut body =
        json!({"access_token": "opaque-access", "token_type": b.token_type, "expires_in": 3600});
    if !b.omit_id_token {
        body["id_token"] = json!(id_token);
    }
    // A configured error status still carries a full token body, so only the status check can
    // refuse it.
    (b.token_status.unwrap_or(StatusCode::OK), axum::Json(body)).into_response()
}

async fn player_me(State(a): State<Arc<Api>>, headers: HeaderMap) -> Response {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_owned();
    a.seen.lock().unwrap().push(bearer);
    let (status, body) = a.answer.lock().unwrap().clone();
    (status, axum::Json(body)).into_response()
}

pub fn player_view(score: i64) -> Value {
    json!({
        "player_id": PLAYER, "homeworld_id": 7, "planet_ids": [7],
        "score": score, "as_of": "2026-10-20T12:00:00.000Z", "valid_until": null,
    })
}

async fn listen() -> (tokio::net::TcpListener, SocketAddr) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    (l, a)
}

/// The dex and API stand-ins, serving.
async fn stand_ins() -> (Arc<Dex>, Arc<Api>, SocketAddr) {
    let (dex_l, dex_a) = listen().await;
    let dex = Arc::new(Dex {
        issuer: format!("http://{dex_a}/dex"),
        key: TestKey::new("key-1"),
        other_key: TestKey::new("key-2"),
        behaviour: Mutex::new(Behaviour::default()),
        attempts: Mutex::new(HashMap::new()),
        codes: AtomicUsize::new(0),
        key_fetches: AtomicUsize::new(0),
        issued: Mutex::new(Vec::new()),
        discovery: Mutex::new(json!({})),
    });
    let dex_app = Router::new()
        .route("/dex/.well-known/openid-configuration", get(discovery))
        .route("/dex/keys", get(keys))
        .route("/dex/auth", get(auth))
        .route("/dex/token", post(token))
        .route("/dex/token-moved", post(token_moved))
        .with_state(dex.clone());
    tokio::spawn(async move { axum::serve(dex_l, dex_app).await.unwrap() });
    let (api_l, api_a) = listen().await;
    let api = Arc::new(Api {
        answer: Mutex::new((StatusCode::OK, player_view(42))),
        seen: Mutex::new(Vec::new()),
    });
    let api_app = Router::new()
        .route("/players/me", get(player_me))
        .with_state(api.clone());
    tokio::spawn(async move { axum::serve(api_l, api_app).await.unwrap() });
    (dex, api, api_a)
}

/// Everything one test needs: the stand-ins, the web app, and its address.
pub struct World {
    pub dex: Arc<Dex>,
    pub api: Arc<Api>,
    pub web: String,
    pub config: Config,
}

pub struct Options {
    pub secure_cookie: bool,
}

impl World {
    pub async fn start() -> Self {
        Self::start_with(
            Options {
                secure_cookie: false,
            },
            |_| {},
        )
        .await
    }

    /// Starts the stand-ins, lets `tweak` change discovery before the web app reads it, then
    /// starts the web app.
    pub async fn start_with(options: Options, tweak: impl FnOnce(&Dex)) -> Self {
        let (dex, api, api_a) = stand_ins().await;
        tweak(&dex);
        let (web_l, web_a) = listen().await;
        let web = format!("http://{web_a}");
        let config = Config {
            issuer: dex.issuer.clone(),
            client_id: CLIENT.to_owned(),
            client_secret: Secret::new(SECRET.to_owned()),
            redirect_url: format!("{web}/callback"),
            bind: web_a,
            api_url: format!("http://{api_a}"),
            secure_cookie: options.secure_cookie,
        };
        let provider = Provider::discover(&config).await.expect("discovery");
        let state = AppState {
            provider: Arc::new(provider),
            api: voidforge_web::api::client().unwrap(),
            api_url: config.api_url.clone(),
        };
        let app = routes::app(state, config.secure_cookie);
        tokio::spawn(async move { axum::serve(web_l, app).await.unwrap() });
        World {
            dex,
            api,
            web,
            config,
        }
    }

    /// The stand-ins and a config for them, without starting the web app: for discovery tests.
    pub async fn config_only(tweak: impl FnOnce(&Dex)) -> (Arc<Dex>, Config) {
        let (dex, _, _) = stand_ins().await;
        tweak(&dex);
        let config = Config {
            issuer: dex.issuer.clone(),
            client_id: CLIENT.to_owned(),
            client_secret: Secret::new(SECRET.to_owned()),
            redirect_url: "http://127.0.0.1:1/callback".to_owned(),
            bind: "127.0.0.1:0".parse().unwrap(),
            api_url: "http://127.0.0.1:1".to_owned(),
            secure_cookie: false,
        };
        (dex, config)
    }

    pub fn behave(&self, change: impl FnOnce(&mut Behaviour)) {
        change(&mut self.dex.behaviour.lock().unwrap());
    }

    pub fn browser(&self) -> Browser {
        Browser {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            cookie: None,
            web: self.web.clone(),
            set_cookies: Vec::new(),
        }
    }
}

/// A browser: one session cookie, no automatic redirects.
pub struct Browser {
    pub http: reqwest::Client,
    pub cookie: Option<String>,
    pub web: String,
    /// Every Set-Cookie header it received.
    pub set_cookies: Vec<String>,
}

pub struct Page {
    pub status: u16,
    pub location: Option<String>,
    pub body: String,
}

impl Browser {
    pub async fn send(&mut self, req: reqwest::RequestBuilder) -> Page {
        let req = match &self.cookie {
            Some(c) => req.header("cookie", format!("{COOKIE_NAME}={c}")),
            None => req,
        };
        let r = req.send().await.unwrap();
        for v in r.headers().get_all("set-cookie") {
            let v = v.to_str().unwrap().to_owned();
            if let Some(rest) = v.strip_prefix(&format!("{COOKIE_NAME}=")) {
                let value = rest.split(';').next().unwrap().to_owned();
                let gone = v.to_ascii_lowercase().contains("max-age=0") || value.is_empty();
                self.cookie = if gone { None } else { Some(value) };
            }
            self.set_cookies.push(v);
        }
        Page {
            status: r.status().as_u16(),
            location: r
                .headers()
                .get("location")
                .map(|l| l.to_str().unwrap().to_owned()),
            body: r.text().await.unwrap(),
        }
    }

    pub async fn get(&mut self, path: &str) -> Page {
        let url = if path.starts_with("http") {
            path.to_owned()
        } else {
            format!("{}{path}", self.web)
        };
        let req = self.http.get(url);
        self.send(req).await
    }

    pub async fn post(&mut self, path: &str) -> Page {
        let req = self.http.post(format!("{}{path}", self.web));
        self.send(req).await
    }

    /// `/login`, then dex's `/auth`: the callback URL dex sends the browser back to.
    pub async fn start_login(&mut self, login_path: &str) -> String {
        let login = self.get(login_path).await;
        assert_eq!(login.status, 303, "{}", login.body);
        let auth = self.http.get(login.location.unwrap()).send().await.unwrap();
        assert_eq!(auth.status().as_u16(), 303);
        auth.headers()["location"].to_str().unwrap().to_owned()
    }

    /// A whole login: the callback's page.
    pub async fn login(&mut self) -> Page {
        let callback = self.start_login("/login").await;
        self.get(&callback).await
    }
}
