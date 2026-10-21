//! The login by effect against stand-ins (P4.1, D-057): a dex that checks PKCE and the client's
//! credentials and signs what each test asks for, and an API that answers `/players/me`.
//! Each check the lead's ruling names (T1) has a test here that fails when the check is gone.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::StatusCode;
use common::{Browser, CLIENT, EMAIL, Options, PLAYER, World, now, player_view};
use serde_json::json;
use voidforge_web::oidc::{DiscoveryError, Provider};
use voidforge_web::pages::EXPIRED;

fn logged_in(body: &str) -> bool {
    body.contains("Logged in as")
}

async fn logged_in_browser(w: &World) -> Browser {
    let mut b = w.browser();
    let page = b.login().await;
    assert_eq!(page.status, 303, "{}", page.body);
    assert!(logged_in(&b.get("/").await.body));
    b
}

/// A login whose token the stand-in built with `change`: refused, 400, and nobody logged in.
async fn refused_with(change: impl FnOnce(&mut common::Behaviour)) -> (World, Browser) {
    let w = World::start().await;
    w.behave(change);
    let mut b = w.browser();
    let page = b.login().await;
    assert_eq!(page.status, 400, "{}", page.body);
    assert!(page.body.contains("Login failed"), "{}", page.body);
    assert!(!logged_in(&b.get("/").await.body));
    (w, b)
}

// --- The whole flow ---

#[tokio::test]
async fn a_user_logs_in_sees_home_and_logs_out() {
    let w = World::start().await;
    let mut b = w.browser();
    let home = b.get("/").await;
    assert_eq!(home.status, 200);
    assert!(home.body.contains(r#"href="/login""#) && !logged_in(&home.body));
    assert!(b.cookie.is_none(), "Home sets no cookie before a login");

    let callback = b.start_login("/login").await;
    let before = b.cookie.clone().expect("the attempt's session");
    let page = b.get(&callback).await;
    assert_eq!((page.status, page.location.as_deref()), (303, Some("/")));
    let after = b.cookie.clone().expect("the login's session");
    assert_ne!(before, after, "a new session id at login (fixation)");

    let home = b.get("/").await;
    assert!(
        home.body
            .contains(&format!("Logged in as <strong>{EMAIL}</strong>")),
        "{}",
        home.body
    );
    assert!(
        home.body
            .contains(&format!(r#"<span title="{PLAYER}">player 099a8057</span>"#)),
        "{}",
        home.body
    );
    assert!(home.body.contains("Score: 42"), "{}", home.body);
    let issued = w.dex.issued.lock().unwrap().clone();
    assert_eq!(
        *w.api.seen.lock().unwrap(),
        issued,
        "the API got the ID token itself"
    );

    let out = b.post("/logout").await;
    assert_eq!(
        (out.status, out.location.as_deref()),
        (303, Some("/?logged_out"))
    );
    // §4.1 (E9): the logged-out Home says so, only where the logout sent it.
    let bye = b.get("/?logged_out").await.body;
    assert!(
        !logged_in(&bye) && bye.contains("You're logged out."),
        "{bye}"
    );
    assert!(!b.get("/").await.body.contains("logged out"));
}

#[tokio::test]
async fn the_old_session_id_is_dead_after_login_and_logout() {
    let w = World::start().await;
    let mut b = w.browser();
    let callback = b.start_login("/login").await;
    let planted = b.cookie.clone().unwrap();
    b.get(&callback).await;
    // Someone holding the pre-login id (fixation) isn't logged in.
    let mut attacker = w.browser();
    attacker.cookie = Some(planted);
    assert!(!logged_in(&attacker.get("/").await.body));
    // After logout, the logged-in id is dead too.
    let logged = b.cookie.clone().unwrap();
    b.post("/logout").await;
    let mut stale = w.browser();
    stale.cookie = Some(logged);
    assert!(!logged_in(&stale.get("/").await.body));
}

#[tokio::test]
async fn the_cookie_is_http_only_lax_and_secure_only_when_configured() {
    let w = World::start().await;
    let mut b = w.browser();
    b.login().await;
    let set = b.set_cookies.last().unwrap().to_ascii_lowercase();
    assert!(
        set.contains("httponly") && set.contains("samesite=lax") && set.contains("path=/"),
        "{set}"
    );
    assert!(!set.contains("secure"), "{set}");

    let w = World::start_with(
        Options {
            secure_cookie: true,
            ..Options::default()
        },
        |_| {},
    )
    .await;
    let mut b = w.browser();
    b.get("/login").await;
    let set = b.set_cookies.last().unwrap().to_ascii_lowercase();
    assert!(
        set.contains("secure") && set.contains("httponly") && set.contains("samesite=lax"),
        "{set}"
    );
}

#[tokio::test]
async fn the_session_lasts_the_token_plus_the_grace() {
    let w = World::start().await;
    let mut b = w.browser();
    let page = b.login().await;
    assert_eq!(page.status, 303);
    let set = b.set_cookies.last().unwrap().to_ascii_lowercase();
    let max_age: u64 = set
        .split(';')
        .find_map(|a| a.trim().strip_prefix("max-age="))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no max-age: {set}"));
    // The stand-in's token lives 3600 s; the session outlives it by the grace (A4).
    let want = 3600 + voidforge_web::routes::EXPIRED_GRACE_SECS;
    assert!(
        (want - 5..=want).contains(&max_age),
        "{max_age} vs {want}: {set}"
    );
}

#[tokio::test]
async fn an_expired_login_asks_to_log_in_again() {
    let w = World::start().await;
    // Valid when issued (within the leeway) and past `exp` a second later.
    w.behave(|b| b.claims = json!({ "exp": now() + 1 }));
    let mut b = w.browser();
    assert_eq!(b.login().await.status, 303);
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let home = b.get("/").await;
    assert_eq!(home.status, 200);
    assert!(
        home.body.contains(EXPIRED) && !logged_in(&home.body),
        "{}",
        home.body
    );
    assert!(home.body.contains(r#"href="/login""#));
    // The session is gone: the next Home is plainly logged out.
    assert!(!b.get("/").await.body.contains(EXPIRED));
}

#[tokio::test]
async fn the_api_refusing_the_token_ends_the_session() {
    let w = World::start().await;
    let mut b = logged_in_browser(&w).await;
    *w.api.answer.lock().unwrap() = (
        StatusCode::UNAUTHORIZED,
        json!({"code": "invalid_token", "message": "no"}),
    );
    let home = b.get("/").await;
    assert!(
        home.body.contains(EXPIRED) && !logged_in(&home.body),
        "{}",
        home.body
    );
    *w.api.answer.lock().unwrap() = (StatusCode::OK, player_view(1));
    assert!(
        !logged_in(&b.get("/").await.body),
        "the session was ended, not just hidden"
    );
}

#[tokio::test]
async fn a_full_universe_is_its_own_page_and_the_user_stays_logged_in() {
    let w = World::start().await;
    let mut b = logged_in_browser(&w).await;
    *w.api.answer.lock().unwrap() = (
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({"code": "no_free_homeworld", "message": "full"}),
    );
    // phase4-ux §4.1 (the advisor's A2): its own page, the copy with its code, and Log out as
    // the only action. The login stays: retrying can't change it, and logging out is the way on.
    for _ in 0..2 {
        let home = b.get("/").await;
        assert_eq!(home.status, 200);
        assert!(
            home.body.contains(
                "<h1>The universe is full</h1><p>There's no free homeworld for a new player, so your account can't join this universe. Trying again won't change that. (<code>no_free_homeworld</code>)</p><form"
            ) && home.body.matches("no free homeworld").count() == 1
                && home
                    .body
                    .contains(r#"<form method="post" action="/logout">"#)
                && !home.body.contains("<a "),
            "{}",
            home.body
        );
    }
    assert_eq!(b.post("/logout").await.status, 303);
    assert!(b.get("/").await.body.contains(r#"href="/login""#));
}

#[tokio::test]
async fn logout_is_a_post() {
    let w = World::start().await;
    let mut b = logged_in_browser(&w).await;
    assert_eq!(b.get("/logout").await.status, 405);
    assert!(logged_in(&b.get("/").await.body));
}

#[tokio::test]
async fn a_login_returns_to_a_local_next_and_never_off_site() {
    let w = World::start().await;
    let mut b = w.browser();
    let callback = b.start_login("/login?next=%2Ffleets%3Fx%3D1").await;
    assert_eq!(
        b.get(&callback).await.location.as_deref(),
        Some("/fleets?x=1")
    );
    for bad in [
        "%2F%2Fevil.example",
        "https%3A%2F%2Fevil.example",
        "%2F%5Cevil.example",
    ] {
        let mut b = w.browser();
        let callback = b.start_login(&format!("/login?next={bad}")).await;
        assert_eq!(
            b.get(&callback).await.location.as_deref(),
            Some("/"),
            "{bad}"
        );
    }
}

#[tokio::test]
async fn pages_are_not_cached() {
    let w = World::start().await;
    let r = reqwest::get(format!("{}/", w.web)).await.unwrap();
    assert_eq!(r.headers()["cache-control"], "no-store");
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
}

// --- state, the callback's own checks ---

#[tokio::test]
async fn a_replayed_callback_is_refused() {
    let w = World::start().await;
    let mut b = w.browser();
    let callback = b.start_login("/login").await;
    assert_eq!(b.get(&callback).await.status, 303);
    let again = b.get(&callback).await;
    assert_eq!(again.status, 400);
    // QA's F1: the page matches the session it kept.
    assert!(
        again.body.contains("You are still logged in") && !again.body.contains("not logged in"),
        "{}",
        again.body
    );
    assert!(
        logged_in(&b.get("/").await.body),
        "the refusal leaves the real login alone"
    );
    let mut other = w.browser();
    let refused = other.get(&callback).await;
    assert_eq!(refused.status, 400);
    assert!(
        refused.body.contains("You are not logged in"),
        "{}",
        refused.body
    );
    assert!(!logged_in(&other.get("/").await.body));
}

#[tokio::test]
async fn a_tampered_missing_or_foreign_state_is_refused() {
    let w = World::start().await;
    let mut b = w.browser();
    let callback = b.start_login("/login").await;
    let tampered = callback.replace("state=", "state=x");
    assert_eq!(b.get(&tampered).await.status, 400);
    let missing = callback.split("&state=").next().unwrap().to_owned();
    assert_eq!(b.get(&missing).await.status, 400);
    // Another browser's valid callback: its state isn't in this session.
    let mut other = w.browser();
    assert_eq!(other.get(&callback).await.status, 400);
    assert!(!logged_in(&other.get("/").await.body));
    // None of these used the attempt up: the real callback still works.
    assert_eq!(b.get(&callback).await.status, 303);
}

#[tokio::test]
async fn a_provider_error_is_refused_and_uses_the_attempt_up() {
    let w = World::start().await;
    let mut b = w.browser();
    let callback = b.start_login("/login").await;
    let errored = format!("{callback}&error=access_denied");
    let page = b.get(&errored).await;
    assert_eq!(page.status, 400);
    assert!(page.body.contains("didn't complete the login"));
    assert_eq!(
        b.get(&callback).await.status,
        400,
        "the attempt was used up"
    );
}

#[tokio::test]
async fn a_callback_without_a_code_is_refused() {
    let w = World::start().await;
    let mut b = w.browser();
    let callback = b.start_login("/login").await;
    let no_code = callback.replace("code=", "nocode=");
    assert_eq!(b.get(&no_code).await.status, 400);
}

// --- PKCE and the token response ---

#[tokio::test]
async fn the_verifier_matches_the_challenge() {
    // The stand-in refuses a verifier that doesn't hash to /auth's challenge; a login passing
    // through it proves the web app sent the attempt's own verifier.
    let w = World::start().await;
    let mut b = w.browser();
    let first = b.start_login("/login").await;
    let second = b.start_login("/login").await;
    // Each attempt's code pairs with its own verifier: both complete, in either order.
    assert_eq!(b.get(&second).await.status, 303);
    let mut c = w.browser();
    let third = c.start_login("/login").await;
    assert_eq!(c.get(&third).await.status, 303);
    assert_eq!(
        b.get(&first).await.status,
        400,
        "a logged-in session dropped its other attempts"
    );
}

#[tokio::test]
async fn a_failed_exchange_is_refused() {
    refused_with(|b| b.token_status = Some(StatusCode::INTERNAL_SERVER_ERROR)).await;
}

#[tokio::test]
async fn a_redirect_on_the_token_post_is_not_followed() {
    // The redirect's target issues a real token for the same code, so only the client's refusal
    // to follow redirects keeps this from logging in.
    refused_with(|b| b.token_redirect = true).await;
}

#[tokio::test]
async fn token_type_must_be_bearer_in_any_case() {
    refused_with(|b| b.token_type = "mac".to_owned()).await;
    let w = World::start().await;
    w.behave(|b| b.token_type = "Bearer".to_owned());
    assert_eq!(w.browser().login().await.status, 303);
}

#[tokio::test]
async fn a_response_without_an_id_token_is_refused() {
    refused_with(|b| b.omit_id_token = true).await;
}

// --- The ID token: one test per check ---

#[tokio::test]
async fn a_wrong_or_missing_nonce_is_refused() {
    refused_with(|b| b.claims = json!({ "nonce": "someone-elses-nonce" })).await;
    refused_with(|b| b.claims = json!({ "nonce": null })).await;
}

#[tokio::test]
async fn the_issuer_must_match_exactly() {
    refused_with(|b| b.claims = json!({ "iss": "http://127.0.0.1:1/dex" })).await;
}

#[tokio::test]
async fn our_client_must_be_an_audience() {
    refused_with(|b| b.claims = json!({ "aud": "voidforge-cli" })).await;
}

#[tokio::test]
async fn several_audiences_need_azp_to_be_our_client() {
    refused_with(|b| b.claims = json!({ "aud": [CLIENT, "other"] })).await;
    refused_with(|b| b.claims = json!({ "aud": [CLIENT, "other"], "azp": "other" })).await;
    let w = World::start().await;
    w.behave(|b| b.claims = json!({ "aud": [CLIENT, "other"], "azp": CLIENT }));
    assert_eq!(w.browser().login().await.status, 303);
}

#[tokio::test]
async fn an_azp_naming_another_client_is_refused() {
    refused_with(|b| b.claims = json!({ "azp": "other" })).await;
}

#[tokio::test]
async fn exp_is_required_and_enforced_with_a_bounded_leeway() {
    refused_with(|b| b.claims = json!({ "exp": null })).await;
    refused_with(|b| b.claims = json!({ "exp": now() - 31 })).await;
}

#[tokio::test]
async fn iat_is_required_and_not_in_the_future() {
    refused_with(|b| b.claims = json!({ "iat": null })).await;
    refused_with(|b| b.claims = json!({ "iat": now() + 120 })).await;
}

#[tokio::test]
async fn a_future_nbf_is_refused() {
    refused_with(|b| b.claims = json!({ "nbf": now() + 120 })).await;
}

#[tokio::test]
async fn the_subject_must_be_present() {
    refused_with(|b| b.claims = json!({ "sub": "" })).await;
}

#[tokio::test]
async fn only_rs256_signed_by_dex_is_accepted() {
    refused_with(|b| b.hs256_with_public_key = true).await;
    refused_with(|b| b.unpublished_key = true).await;
    refused_with(|b| b.crit = true).await;
    refused_with(|b| b.kid = Some(String::new())).await;
}

#[tokio::test]
async fn concurrent_first_logins_share_one_key_fetch() {
    let w = World::start().await;
    // 8 logins at once before any key is cached: one fetches, and the others, waiting on it,
    // find the key in the cache rather than being judged against the floor.
    let mut handles = Vec::new();
    for _ in 0..8 {
        let mut b = w.browser();
        handles.push(tokio::spawn(async move { b.login().await.status }));
    }
    for h in handles {
        assert_eq!(h.await.unwrap(), 303);
    }
    assert_eq!(w.dex.key_fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_missing_or_malformed_kid_is_refused_before_any_key_fetch() {
    // On a first login nothing is cached and no floor applies, so a kid that reached the key
    // lookup would fetch: refusing it first means no fetch at all.
    for kid in [
        String::new(),
        "a kid with spaces".to_owned(),
        "k".repeat(129),
    ] {
        let (w, _) = refused_with(|b| b.kid = Some(kid.clone())).await;
        assert_eq!(w.dex.key_fetches.load(Ordering::SeqCst), 0, "{kid:?}");
    }
}

#[tokio::test]
async fn an_unknown_kid_refetches_once_per_floor_and_never_per_login() {
    let w = World::start().await;
    w.behave(|b| b.kid = Some("rotated-in".to_owned()));
    // 8 concurrent logins with a kid dex doesn't publish: one fetch (the first ever), single
    // flight; then more logins within the floor fetch nothing.
    let mut handles = Vec::new();
    for _ in 0..8 {
        let mut b = w.browser();
        handles.push(tokio::spawn(async move { b.login().await.status }));
    }
    for h in handles {
        assert_eq!(h.await.unwrap(), 400);
    }
    assert_eq!(w.dex.key_fetches.load(Ordering::SeqCst), 1);
    for _ in 0..3 {
        assert_eq!(w.browser().login().await.status, 400);
    }
    assert_eq!(w.dex.key_fetches.load(Ordering::SeqCst), 1, "rate-floored");
    // A known kid is served from the cache.
    w.behave(|b| b.kid = None);
    assert_eq!(w.browser().login().await.status, 303);
    assert_eq!(w.dex.key_fetches.load(Ordering::SeqCst), 1);
}

// --- Discovery ---

#[tokio::test]
async fn discovery_refuses_another_issuer_other_origins_and_no_s256() {
    let cases = [
        json!({ "issuer": "http://127.0.0.1:1/dex" }),
        json!({ "token_endpoint": "http://127.0.0.1:1/dex/token" }),
        json!({ "jwks_uri": "http://127.0.0.1:1/dex/keys" }),
        json!({ "authorization_endpoint": "http://127.0.0.1:1/dex/auth" }),
        json!({ "code_challenge_methods_supported": ["plain"] }),
    ];
    for (i, over) in cases.into_iter().enumerate() {
        let (_, config) = World::config_only(|d| *d.discovery.lock().unwrap() = over.clone()).await;
        let e = Provider::discover(&config).await.expect_err("refused");
        let kind_ok = match i {
            0 => matches!(e, DiscoveryError::IssuerMismatch { .. }),
            1..=3 => matches!(e, DiscoveryError::CrossOrigin { .. }),
            _ => matches!(e, DiscoveryError::NoS256),
        };
        assert!(kind_ok, "{over}: {e}");
    }
    let (_, config) = World::config_only(|_| {}).await;
    Provider::discover(&config)
        .await
        .expect("the stand-in's own document is fine");
}

// --- Nothing secret is shown or logged: tests/logs.rs ---

// --- The session store's bound (R1-1) ---

fn max_age(set_cookie: &str) -> u64 {
    set_cookie
        .to_ascii_lowercase()
        .split(';')
        .find_map(|a| a.trim().strip_prefix("max-age=").map(str::to_owned))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no max-age: {set_cookie}"))
}

#[tokio::test]
async fn an_anonymous_session_lives_ten_minutes() {
    let w = World::start().await;
    let mut b = w.browser();
    b.get("/login").await;
    assert_eq!(max_age(b.set_cookies.last().unwrap()), 600);
}

#[tokio::test]
async fn past_the_cap_a_new_login_is_refused_and_stored_sessions_keep_working() {
    let w = World::start_with(
        Options {
            max_sessions: 3,
            ..Options::default()
        },
        |_| {},
    )
    .await;
    let mut alice = logged_in_browser(&w).await;
    for _ in 0..2 {
        assert_eq!(w.browser().get("/login").await.status, 303);
    }
    // Three sessions stored: a fourth anonymous /login is refused, and stores nothing.
    let mut late = w.browser();
    let busy = late.get("/login").await;
    assert_eq!(busy.status, 503);
    assert!(
        busy.body.contains("Try again in a few minutes"),
        "{}",
        busy.body
    );
    assert!(late.cookie.is_none());
    // The stored sessions are untouched: alice is logged in, can start another login, and can
    // log out.
    assert!(logged_in(&alice.get("/").await.body));
    assert_eq!(alice.get("/login").await.status, 303);
    assert_eq!(alice.post("/logout").await.status, 303);
    // Her logout freed a place.
    assert_eq!(w.browser().get("/login").await.status, 303);
}

#[tokio::test]
async fn a_logged_in_session_keeps_its_lifetime_when_changed() {
    // A save recomputes the expiry from the anonymous default unless the handler sets it again:
    // /login and a refused callback both change a logged-in session.
    let w = World::start().await;
    let mut b = w.browser();
    let callback = b.start_login("/login").await;
    b.get(&callback).await;
    let want = 3600 + voidforge_web::routes::EXPIRED_GRACE_SECS;
    let lifetime = |b: &Browser| max_age(b.set_cookies.last().unwrap());
    assert!((want - 5..=want).contains(&lifetime(&b)));
    b.get("/login").await;
    assert!(
        (want - 5..=want).contains(&lifetime(&b)),
        "after /login: {}",
        lifetime(&b)
    );
    assert_eq!(b.get(&callback).await.status, 400);
    assert!(
        (want - 5..=want).contains(&lifetime(&b)),
        "after a replay: {}",
        lifetime(&b)
    );
    assert!(logged_in(&b.get("/").await.body));
}

// --- Slow and failing key fetches (R1-2: W1 and W2; R1's INFO) ---

#[tokio::test]
async fn the_refetch_floor_counts_from_the_end_of_a_slow_fetch() {
    // W2 (D-046 R1-1): with a 2 s floor and a 1.5 s fetch, a login 2.5 s after the fetch
    // started but 1 s after it ended must not fetch again. A floor stamped at the start would.
    let w = World::start_with(
        Options {
            key_floor: Some(Duration::from_secs(2)),
            ..Options::default()
        },
        |d| d.key_delay_ms.store(1500, Ordering::SeqCst),
    )
    .await;
    w.behave(|b| b.kid = Some("rotated-in".to_owned()));
    let start = std::time::Instant::now();
    assert_eq!(w.browser().login().await.status, 400);
    assert_eq!(w.dex.key_fetches.load(Ordering::SeqCst), 1);
    let ended = start.elapsed();
    assert!(
        ended >= Duration::from_millis(1500),
        "the fetch was slow: {ended:?}"
    );
    tokio::time::sleep(Duration::from_millis(2500).saturating_sub(start.elapsed())).await;
    assert_eq!(w.browser().login().await.status, 400);
    assert_eq!(
        w.dex.key_fetches.load(Ordering::SeqCst),
        1,
        "within the floor after the fetch's end"
    );
    // Past the floor from the end, it fetches again.
    tokio::time::sleep((ended + Duration::from_millis(2100)).saturating_sub(start.elapsed())).await;
    w.dex.key_delay_ms.store(0, Ordering::SeqCst);
    assert_eq!(w.browser().login().await.status, 400);
    assert_eq!(w.dex.key_fetches.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_token_is_judged_after_the_wait_for_keys() {
    // W1: a token valid when the callback arrives (exp + leeway is one second away) but expired
    // by the time a 2.2 s key fetch returns is refused: the clock is read after the lookup.
    let w = World::start().await;
    w.dex.key_delay_ms.store(2200, Ordering::SeqCst);
    w.behave(|b| b.claims = json!({ "exp": now() - 29 }));
    assert_eq!(w.browser().login().await.status, 400);
    // The same claims, judged at once with the keys cached, are accepted: the wait refused it.
    w.dex.key_delay_ms.store(0, Ordering::SeqCst);
    w.behave(|b| b.claims = json!({ "exp": now() - 29 }));
    assert_eq!(w.browser().login().await.status, 303);
}

#[tokio::test]
async fn while_keys_cant_be_fetched_the_page_says_so() {
    // R1's INFO: after a failed fetch, an unknown kid within the floor is "can't check now"
    // (503), as the server answers, not "rejected".
    let w = World::start().await;
    w.dex.keys_fail.store(true, Ordering::SeqCst);
    for _ in 0..2 {
        let page = w.browser().login().await;
        assert_eq!(page.status, 503, "{}", page.body);
        assert!(
            page.body.contains("can't be reached right now"),
            "{}",
            page.body
        );
    }
    assert_eq!(
        w.dex.key_fetches.load(Ordering::SeqCst),
        1,
        "the floor holds after a failure too"
    );
}

// --- Round 2: the cap's boundary (QA's D1) and its log (QA's D2) ---

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn at_the_cap_concurrent_logins_get_a_login_or_the_busy_page_never_a_500() {
    // The room check and the save race only when requests overlap between them, which a single
    // round of 32 does about half the time; 20 fresh rounds make a missed race vanishingly rare.
    for round in 0..20 {
        let w = World::start_with(
            Options {
                max_sessions: 4,
                ..Options::default()
            },
            |_| {},
        )
        .await;
        let mut handles = Vec::new();
        for _ in 0..32 {
            let mut b = w.browser();
            handles.push(tokio::spawn(async move { b.get("/login").await.status }));
        }
        let mut statuses = Vec::new();
        for h in handles {
            statuses.push(h.await.unwrap());
        }
        let logins = statuses.iter().filter(|&&s| s == 303).count();
        let busy = statuses.iter().filter(|&&s| s == 503).count();
        assert_eq!((logins, busy), (4, 28), "round {round}: {statuses:?}");
    }
}
