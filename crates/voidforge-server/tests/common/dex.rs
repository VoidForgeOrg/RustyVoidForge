//! The dex stand-in the server's HTTP tests share: RSA keys generated at run time (so no
//! private key is committed), tokens, and discovery and key-set routes (D-046).
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rsa::{KeyPair, KeySize};
use aws_lc_rs::signature::KeyPair as _;
use axum::extract::State;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};

// --- Keys and tokens ---

pub fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// The DER `RSAPrivateKey` inside a PKCS#8 `PrivateKeyInfo` (jsonwebtoken signs from PKCS#1).
pub fn pkcs1_from_pkcs8(der: &[u8]) -> Vec<u8> {
    // Read one TLV at `at`: (tag, content start, content end).
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
    let (tag, body, _) = tlv(der, 0);
    assert_eq!(tag, 0x30, "a SEQUENCE");
    let (_, _, version_end) = tlv(der, body); // INTEGER 0
    let (_, _, algorithm_end) = tlv(der, version_end); // AlgorithmIdentifier
    let (tag, key, key_end) = tlv(der, algorithm_end);
    assert_eq!(tag, 0x04, "an OCTET STRING");
    der[key..key_end].to_vec()
}

/// A signing key and its public JWK.
pub struct TestKey {
    pub kid: String,
    pub encoding: EncodingKey,
    pub jwk: Value,
    /// The public key's DER, for the HS256-with-the-public-key forgery.
    pub public_der: Vec<u8>,
}

impl TestKey {
    pub fn new(kid: &str) -> Self {
        let pair = KeyPair::generate(KeySize::Rsa2048).unwrap();
        let pkcs8 = AsDer::<aws_lc_rs::encoding::Pkcs8V1Der>::as_der(&pair).unwrap();
        let public = pair.public_key();
        let jwk = json!({
            "kty": "RSA", "alg": "RS256", "use": "sig", "kid": kid,
            "n": b64url(public.modulus().big_endian_without_leading_zero()),
            "e": b64url(public.exponent().big_endian_without_leading_zero()),
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

    pub fn sign(&self, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.kid.clone());
        jsonwebtoken::encode(&header, claims, &self.encoding).unwrap()
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Valid claims for `issuer`, for alice through the CLI client.
pub fn claims(issuer: &str) -> Value {
    let t = now();
    json!({"iss": issuer, "sub": "CiQwYjZjNWEyZS1hbGljZRIFbG9jYWw", "aud": "voidforge-cli",
        "exp": t + 3600, "iat": t, "email": "alice@voidforge.test"})
}

/// A token from raw parts: a header and claims as JSON, and a signature.
pub fn raw(header: &Value, claims: &Value, signature: &str) -> String {
    format!(
        "{}.{}.{signature}",
        b64url(header.to_string().as_bytes()),
        b64url(claims.to_string().as_bytes())
    )
}

// --- The dex stand-in ---

#[derive(Default)]
pub struct Dex {
    pub issuer: Mutex<String>,
    /// The issuer the discovery document claims (normally the real one).
    pub claimed_issuer: Mutex<Option<String>>,
    pub keys: Mutex<Vec<Value>>,
    /// Discovery requests: every fetch attempt, failed ones included.
    pub fetches: AtomicUsize,
    pub down: AtomicBool,
    /// How long discovery takes to answer, in milliseconds.
    pub delay_ms: AtomicUsize,
}

pub async fn discovery(
    State(dex): State<Arc<Dex>>,
) -> Result<axum::Json<Value>, axum::http::StatusCode> {
    dex.fetches.fetch_add(1, Ordering::SeqCst);
    let delay = dex.delay_ms.load(Ordering::SeqCst) as u64;
    tokio::time::sleep(Duration::from_millis(delay)).await;
    if dex.down.load(Ordering::SeqCst) {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }
    let issuer = dex.issuer.lock().unwrap().clone();
    let claimed = dex
        .claimed_issuer
        .lock()
        .unwrap()
        .clone()
        .unwrap_or(issuer.clone());
    Ok(axum::Json(
        json!({"issuer": claimed, "jwks_uri": format!("{issuer}/keys")}),
    ))
}

pub async fn jwks(
    State(dex): State<Arc<Dex>>,
) -> Result<axum::Json<Value>, axum::http::StatusCode> {
    if dex.down.load(Ordering::SeqCst) {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(axum::Json(
        json!({"keys": dex.keys.lock().unwrap().clone()}),
    ))
}
