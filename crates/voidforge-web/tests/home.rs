//! Home on the API client, by effect through the router (P4.2, D-059): the player's data
//! (phase4-ux §4.1), the frame's fast-rules line (§2, §5.7), and every failure of its reads as
//! a message on the page, with the API's status (§5.5, §6). The 401 and the full universe are
//! in `login.rs`, beside the rest of the session.

mod common;

use axum::http::StatusCode;
use common::{Browser, Options, PLAYER, Reply, World, player_view, rules_view};
use serde_json::json;
use voidforge_web::messages::{UNEXPECTED_PAGE, UNREACHABLE_PAGE};

const FAST_LINE: &str = "<strong>Fast test rules:</strong> an hour of play takes a second.";

async fn logged_in(w: &World) -> Browser {
    let mut b = w.browser();
    assert_eq!(b.login().await.status, 303);
    b
}

fn me_answers(w: &World, status: u16, body: serde_json::Value) {
    *w.api.answer.lock().unwrap() = (StatusCode::from_u16(status).unwrap(), body);
}

#[tokio::test]
async fn home_shows_the_player_from_the_api() {
    let w = World::start().await;
    let mut view = player_view(3450);
    view["planet_ids"] = json!([12, 7, 9]);
    me_answers(&w, 200, view);
    let mut b = logged_in(&w).await;
    let home = b.get("/").await;
    assert_eq!(home.status, 200);
    for want in [
        format!(r#"You: <span title="{PLAYER}">player 099a8057</span> · score 3,450"#),
        "<h1>Your empire</h1>".to_owned(),
        "Score: 3,450 (current)".to_owned(),
        "Score as of 12:00:00 UTC.".to_owned(),
        r#"Homeworld: <a href="/planets/7">Planet #7</a>"#.to_owned(),
        r#"Your planets (3): <a href="/planets/7">Planet #7</a> (homeworld) · <a href="/planets/9">Planet #9</a> · <a href="/planets/12">Planet #12</a>"#.to_owned(),
    ] {
        assert!(home.body.contains(&want), "{want}\n{}", home.body);
    }
    assert!(!home.body.contains("Welcome."), "{}", home.body);

    me_answers(&w, 200, player_view(5));
    let home = b.get("/").await;
    assert!(
        home.body.contains("Welcome. Planet #7 is your homeworld"),
        "{}",
        home.body
    );
}

#[tokio::test]
async fn the_fast_rules_line_follows_the_rules_name_and_a_failed_read_only_drops_it() {
    let w = World::start().await;
    let mut anonymous = w.browser();
    let mut b = logged_in(&w).await;
    *w.api.rules.lock().unwrap() = Reply::json(StatusCode::OK, &rules_view("v0-fast"));
    assert!(anonymous.get("/").await.body.contains(FAST_LINE));
    assert!(b.get("/").await.body.contains(FAST_LINE));

    *w.api.rules.lock().unwrap() = Reply::json(StatusCode::OK, &rules_view("v0"));
    assert!(!anonymous.get("/").await.body.contains("Fast test rules"));
    assert!(!b.get("/").await.body.contains("Fast test rules"));

    for broken in [
        Reply::error(500, "internal"),
        Reply::raw(StatusCode::OK, "<html>"),
    ] {
        *w.api.rules.lock().unwrap() = broken;
        let home = b.get("/").await;
        assert_eq!(home.status, 200, "{}", home.body);
        assert!(
            home.body.contains("Score: 42 (current)") && !home.body.contains("Fast test rules"),
            "{}",
            home.body
        );
    }
    // A logged-in render reads both, side by side (QA's F1); `/rules` without a token.
    let requests = w.api.requests.lock().unwrap().clone();
    let mut last_two: Vec<_> = requests[requests.len() - 2..]
        .iter()
        .map(|r| (r.uri.as_str(), r.authorization.is_some()))
        .collect();
    last_two.sort_unstable();
    assert_eq!(last_two, [("/players/me", true), ("/rules", false)]);
}

#[tokio::test]
async fn a_429_or_503_is_the_retry_page_with_its_status_retry_after_and_code() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    for (status, code, retry, copy) in [
        (
            429,
            "rate_limited",
            "1",
            "Too many requests at once. Try again in a moment.",
        ),
        (
            503,
            "busy",
            "7",
            "The game is busy. Try again in 7 seconds.",
        ),
        (
            503,
            "unavailable",
            "2",
            "The game is starting up or catching up. Try again in a moment.",
        ),
    ] {
        // A scripted reply, for its header; the browser helper doesn't keep headers.
        w.api.reply(
            "/players/me",
            Reply::error(status, code).header("retry-after", retry),
        );
        let r = reqwest::Client::new()
            .get(format!("{}/", w.web))
            .header(
                "cookie",
                format!(
                    "{}={}",
                    voidforge_web::routes::COOKIE_NAME,
                    b.cookie.clone().unwrap()
                ),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), status, "{code}");
        assert_eq!(
            r.headers().get("retry-after").map(|v| v.to_str().unwrap()),
            Some(retry),
            "{code}"
        );
        let body = r.text().await.unwrap();
        assert!(
            body.contains("<h1>Couldn't load this page</h1>")
                && body.contains(&format!("{copy} (<code>{code}</code>)"))
                && body.contains(r#"<a href="/">Try again</a>"#)
                && body.contains("Logged in as"),
            "{code}: {body}"
        );
    }
    // The login is untouched: the next answer is Home again.
    w.api.replies.lock().unwrap().clear();
    me_answers(&w, 200, player_view(42));
    assert!(b.get("/").await.body.contains("Score: 42"));
}

#[tokio::test]
async fn an_internal_error_is_the_retry_page_at_500_without_retry_after() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    me_answers(&w, 500, json!({"code": "internal", "message": "m"}));
    let home = b.get("/").await;
    assert_eq!(home.status, 500);
    assert!(
        home.body.contains(
            "Something went wrong on the game server. Try again. (<code>internal</code>)"
        ),
        "{}",
        home.body
    );
}

#[tokio::test]
async fn an_unreachable_api_is_the_retry_page_at_502_never_a_trace() {
    let closed = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap()
    };
    let w = World::start_with(
        Options {
            api_url: Some(format!("http://{closed}")),
            ..Options::default()
        },
        |_| {},
    )
    .await;
    let mut b = logged_in(&w).await;
    let home = b.get("/").await;
    assert_eq!(home.status, 502);
    assert!(
        home.body.contains(UNREACHABLE_PAGE)
            && home.body.contains(r#"<a href="/">Try again</a>"#)
            && !home.body.contains("<code>")
            && !home.body.contains("error"),
        "{}",
        home.body
    );
    // Logged out, Home needs only `/rules`, and its failure isn't fatal.
    let mut anonymous = w.browser();
    assert_eq!(anonymous.get("/").await.status, 200);
}

#[tokio::test]
async fn an_unexpected_answer_is_the_retry_page_at_502_without_a_code() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    for (status, body) in [
        (200, json!({"not": "a player"})),
        (418, json!({"code": "busy", "message": "m"})),
        (400, json!({"code": "busy", "message": "m"})),
    ] {
        me_answers(&w, status, body);
        let home = b.get("/").await;
        assert_eq!(home.status, 502, "{status}");
        assert!(
            home.body.contains(UNEXPECTED_PAGE) && !home.body.contains("<code>"),
            "{status}: {}",
            home.body
        );
    }
}

#[tokio::test]
async fn a_refusal_a_page_cant_act_on_is_shown_with_its_code() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    me_answers(&w, 422, json!({"code": "not_joined", "message": "m"}));
    let home = b.get("/").await;
    assert_eq!(home.status, 502);
    assert!(
        home.body.contains(
            "Your account hasn't joined the game yet. Reload the page to join. (<code>not_joined</code>)"
        ),
        "{}",
        home.body
    );
}

/// An API that accepts connections and never answers.
async fn hung_api() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = l.accept().await {
            held.push(s);
        }
    });
    format!("http://{addr}")
}

/// QA's F1: the decorative `/rules` read has its own short bound, and a logged-in render reads
/// it beside `/players/me`, so a hung API holds Home for one bound, not two.
#[tokio::test]
async fn a_hung_api_holds_home_for_one_bound_at_most() {
    use std::time::{Duration, Instant};
    use voidforge_web::api::RULES_TIMEOUT;

    // The standard bounds: logged out, Home waits only for `/rules`'s 2 s, not the 10 s total.
    let w = World::start_with(
        Options {
            api_url: Some(hung_api().await),
            ..Options::default()
        },
        |_| {},
    )
    .await;
    let started = Instant::now();
    let home = w.browser().get("/").await;
    let took = started.elapsed();
    assert_eq!(home.status, 200);
    assert!(home.body.contains(r#"href="/login""#) && !home.body.contains("Fast test rules"));
    assert!(
        took >= RULES_TIMEOUT && took < RULES_TIMEOUT + Duration::from_secs(1),
        "{took:?}"
    );

    // A 1 s total bound: logged in, both reads end at 1 s together; one after the other would
    // take 2 s.
    let w = World::start_with(
        Options {
            api_url: Some(hung_api().await),
            api_timeout: Duration::from_secs(1),
            ..Options::default()
        },
        |_| {},
    )
    .await;
    let mut b = logged_in(&w).await;
    let started = Instant::now();
    let home = b.get("/").await;
    let took = started.elapsed();
    assert_eq!(home.status, 502, "{}", home.body);
    assert!(
        took >= Duration::from_secs(1) && took < Duration::from_millis(1_600),
        "{took:?}"
    );
}

/// QA's I1: HEAD is refused before any handler, so it calls no API, spends no read, joins no
/// one, and makes no session.
#[tokio::test]
async fn head_is_refused_and_calls_no_api() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    let before = w.api.requests.lock().unwrap().len();
    let cookie = format!(
        "{}={}",
        voidforge_web::routes::COOKIE_NAME,
        b.cookie.clone().unwrap()
    );
    let http = reqwest::Client::new();
    for path in ["/", "/login", "/callback", "/logout", "/nowhere"] {
        let r = http
            .head(format!("{}{path}", w.web))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 404, "{path}");
        assert!(r.headers().get("set-cookie").is_none(), "{path}");
    }
    let anonymous = http.head(format!("{}/", w.web)).send().await.unwrap();
    assert_eq!(anonymous.status().as_u16(), 404);
    assert!(anonymous.headers().get("set-cookie").is_none());
    assert_eq!(w.api.requests.lock().unwrap().len(), before, "no API call");
    // GET is unchanged, and the login survived.
    assert!(b.get("/").await.body.contains("Score: 42"));
}
