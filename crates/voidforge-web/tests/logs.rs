//! The tests that read the log (P4.1, D-057; P4.2, D-059): nothing secret is logged, and the
//! session store's refusals don't flood it. They're in a binary of their own, with one global
//! subscriber writing to one sink and the tests taking turns, so they hold under plain
//! `cargo test` too: libtest runs a binary's tests as threads of one process, and a per-thread
//! subscriber there loses events to tracing's process-wide callsite cache (QA's I2).

mod common;

use std::sync::{Mutex, MutexGuard, OnceLock};

use axum::http::StatusCode;
use common::{Browser, Options, Reply, SECRET, World, api_stand_in};
use serde_json::json;
use voidforge_api_types::command::{BuildBuilding, CommandRequest, IdempotencyKey};
use voidforge_api_types::wire::{BuildingType, PlanetId};
use voidforge_web::api::Api;
use voidforge_web::config::Secret;

static SINK: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static TURN: Mutex<()> = Mutex::new(());

struct Writer;

impl std::io::Write for Writer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        SINK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Installs the global subscriber once, takes this test's turn, and empties the sink.
fn capture() -> MutexGuard<'static, ()> {
    use tracing_subscriber::Layer as _;
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(|| Writer)
                    .with_ansi(false)
                    .with_filter(tracing_subscriber::filter::LevelFilter::TRACE),
            )
            .init();
    });
    let turn = TURN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    SINK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    turn
}

fn captured() -> String {
    String::from_utf8_lossy(
        &SINK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_owned()
}

async fn logged_in_browser(w: &World) -> Browser {
    let mut b = w.browser();
    assert_eq!(b.login().await.status, 303);
    b
}

const TOKEN: &str = "eyJ.the-id-token-itself.sig";

fn token() -> Secret {
    Secret::new(TOKEN.to_owned())
}

fn client(base: &str) -> Api {
    Api::new(base.to_owned()).unwrap()
}

fn key() -> IdempotencyKey {
    IdempotencyKey::parse(Some("form-key-1")).unwrap()
}

fn build() -> CommandRequest {
    CommandRequest::BuildBuilding(BuildBuilding {
        planet_id: PlanetId(4),
        building_type: BuildingType::Drill,
    })
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "the turn orders whole tests; no task inside a test takes it"
)]
async fn no_token_code_or_secret_reaches_a_page_or_the_log() {
    let _logs = capture();

    let w = World::start().await;
    let mut b = w.browser();
    let login = b.get("/login").await;
    let auth_url = login.location.clone().unwrap();
    let callback = {
        let r = b.http.get(&auth_url).send().await.unwrap();
        r.headers()["location"].to_str().unwrap().to_owned()
    };
    let mut pages = vec![login.body, b.get(&callback).await.body];
    pages.push(b.get("/").await.body);
    pages.push(b.get(&callback).await.body); // a refused replay
    pages.push(b.post("/logout").await.body);
    w.behave(|x| x.claims = json!({ "nonce": "wrong" }));
    pages.push(b.login().await.body); // a refused token

    let query = |url: &str, key: &str| {
        url.split(['?', '&'])
            .find_map(|kv| kv.strip_prefix(&format!("{key}=")).map(str::to_owned))
            .unwrap()
    };
    let mut secrets = vec![
        query(&auth_url, "state"),
        query(&auth_url, "nonce"),
        query(&auth_url, "code_challenge"),
        query(&callback, "code"),
        SECRET.to_owned(),
    ];
    secrets.extend(w.dex.issued.lock().unwrap().iter().cloned());
    let log = captured();
    assert!(
        log.contains("a login completed"),
        "the log was captured: {log}"
    );
    for s in &secrets {
        assert!(
            !log.contains(s.as_str()),
            "the log holds a secret: {s:.12}…"
        );
        for p in &pages {
            assert!(!p.contains(s.as_str()), "a page holds a secret: {s:.12}…");
        }
    }
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "the turn orders whole tests; no task inside a test takes it"
)]
async fn refusals_at_the_cap_log_one_warn_then_one_info_on_recovery() {
    let _logs = capture();

    let w = World::start_with(
        Options {
            max_sessions: 1,
            ..Options::default()
        },
        |_| {},
    )
    .await;
    let mut alice = logged_in_browser(&w).await;
    for _ in 0..40 {
        assert_eq!(w.browser().get("/login").await.status, 503);
    }
    alice.post("/logout").await;
    assert_eq!(w.browser().get("/login").await.status, 303);
    // INFO and above, as the server logs by default (the capture keeps every level).
    let log: String = captured()
        .lines()
        .filter(|l| !l.contains(" DEBUG ") && !l.contains(" TRACE "))
        .map(|l| format!("{l}\n"))
        .collect();
    let lines = |what: &str| log.lines().filter(|l| l.contains(what)).count();
    assert_eq!(lines("refusing new logins"), 1, "{log}");
    assert!(log.contains("WARN") && log.contains("refused=1 "), "{log}");
    assert_eq!(lines("has room again"), 1, "{log}");
    assert!(log.contains("refused=40"), "{log}");
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "the turn orders whole tests; no task inside a test takes it"
)]
async fn no_failure_logs_or_shows_the_token() {
    let _logs = capture();

    let (stand_in, base) = api_stand_in().await;
    let api = client(&base);
    let mut errors = Vec::new();
    for reply in [
        Reply::error(401, "invalid_token"),
        Reply::error(503, "busy").header("retry-after", "1"),
        Reply::error(400, "busy"),
        Reply::raw(StatusCode::OK, "nope"),
        Reply::raw(StatusCode::FOUND, "").header("location", "/elsewhere"),
    ] {
        stand_in.reply("/commands", reply);
        errors.push(api.command(&token(), &key(), &build()).await.unwrap_err());
    }
    let closed = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap()
    };
    errors.push(
        client(&format!("http://{closed}"))
            .me(&token())
            .await
            .unwrap_err(),
    );

    let log = captured();
    assert!(log.contains("the API refused"), "{log}");
    assert!(log.contains("the API can't be reached"), "{log}");
    assert!(!log.contains(TOKEN), "{log}");
    assert!(!log.contains("the-id-token-itself"), "{log}");
    let shown = format!("{errors:?} {api:?} {:?}", token());
    assert!(!shown.contains("the-id-token-itself"), "{shown}");
}
