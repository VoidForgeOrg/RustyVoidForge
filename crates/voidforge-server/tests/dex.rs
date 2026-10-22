//! The token checks against the real dex (Technical Plan P3.2; decision log D-046): a
//! password-grant ID token for the static test user alice verifies through real discovery and
//! real keys, and a tampered copy doesn't. It needs the local stack with the CI dex config
//! (`DEX_CONFIG=config.ci.yaml just up`, password grant on), so it's `#[ignore]`d and run by
//! `just dex-test`; P3.6's end-to-end job brings it into CI.

use axum::http::{HeaderMap, HeaderValue};
use voidforge_server::auth::{AuthArgs, Refusal, TokenVerifier};

const ISSUER: &str = "http://localhost:5556/dex";

/// alice's ID token from dex's password grant, through the public CLI client (TEST-ONLY
/// credentials from deploy/dex/config.ci.yaml).
async fn alice_token() -> String {
    let body = "grant_type=password&username=alice%40voidforge.test\
        &password=alice-local-test-password&client_id=voidforge-cli&scope=openid%20email";
    let response = reqwest::Client::new()
        .post(format!("{ISSUER}/token"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .expect("dex answers: run `DEX_CONFIG=config.ci.yaml just up`");
    let reply: serde_json::Value = response.json().await.unwrap();
    reply["id_token"]
        .as_str()
        .unwrap_or_else(|| panic!("an ID token (is the password grant on?): {reply}"))
        .to_owned()
}

fn bearer(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    headers
}

fn verifier() -> TokenVerifier {
    let args = AuthArgs {
        issuer: ISSUER.to_owned(),
        audiences: vec!["voidforge-cli".to_owned()],
        token_leeway_secs: 30,
        jwks_refresh_secs: 900,
        jwks_min_refresh_secs: 10,
        jwks_max_stale_secs: 86_400,
    };
    TokenVerifier::new(args.config().unwrap()).unwrap()
}

/// dex's real discovery document and key set load: the issuer matches, and at least one usable
/// RS256 key comes back. Any dex config will do.
#[tokio::test]
#[ignore = "needs dex: just up, then just dex-test"]
async fn the_real_dex_keys_load_through_discovery() {
    let keys = verifier().refresh().await.expect("dex's keys load");
    assert!(keys >= 1, "{keys} keys");
}

#[tokio::test]
#[ignore = "needs dex with the password grant: DEX_CONFIG=config.ci.yaml just up, then just dex-test"]
async fn a_real_dex_token_verifies_and_a_tampered_one_does_not() {
    let verifier = verifier();
    let token = alice_token().await;
    let principal = verifier.verify(&bearer(&token)).await.unwrap();
    assert_eq!(principal.issuer, ISSUER);
    assert!(!principal.subject.is_empty());
    // The subject is personal data, never printed (D-047); its length shows it arrived.
    println!("alice's subject: {} bytes", principal.subject.len());
    // One character of the signature changed.
    let mut tampered = token.clone();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'A' { 'B' } else { 'A' });
    assert!(matches!(
        verifier.verify(&bearer(&tampered)).await,
        Err(Refusal::Invalid(_))
    ));
}
