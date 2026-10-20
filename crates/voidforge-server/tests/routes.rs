//! The routes over real HTTP, through the real router, host, `Players` and Postgres (Technical
//! Plan P3.4; decision log D-045, D-049). Each test seeds a universe built with testkit's
//! `WorldBuilder` and maps two dex subjects to its players' UUIDv7s, so every view is
//! deterministic and the contract snapshots (insta) are stable.
//!
//! **The parity rule (T7):** the harness records the status of every response, per route, and
//! each test ends by checking that the OpenAPI document declares every status it saw for that
//! route and method. A handler can't answer something the contract doesn't name. Since the Phase
//! 3 gate (D-055, R1-G1) it checks the headers too: a response carries `Retry-After` and
//! `WWW-Authenticate` exactly when the document declares them for its status.
//!
//! These need Postgres, so they are `#[ignore]`d outside the database stage (`just db-test`).

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::dex::{Dex, TestKey, claims, discovery, jwks};
use reqwest::Method;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::{Semaphore, watch};
use voidforge_host::{
    BuildingKind, Cargo, HostConfig, HostHandle, ManualClock, Mission, PlanetId, PlayerId,
    Position, ShipKind, World,
};
use voidforge_server::accounts::Players;
use voidforge_server::api::{AppState, MAX_NEAR_RESULTS, router, routes};
use voidforge_server::auth::{AuthConfig, TokenVerifier};
use voidforge_server::limit::{Budget, Limiter, Limits};
use voidforge_store::MIGRATOR;
use voidforge_store::accounts::PgAccounts;
use voidforge_store::log::{
    IdempotencyKey, LogError, LogRecord, LogStore, PgLog, SnapshotMeta, SnapshotRow,
};
use voidforge_store::snapshot::{self, EngineBuild};
use voidforge_testkit::{WorldBuilder, v0_rules};

const EPOCH: i64 = 1_760_000_000_000;
const PATIENCE: Duration = Duration::from_secs(30);
/// Fixed UUIDv7s, so snapshots name the same players every run.
const ALICE: PlayerId = PlayerId(0x0192_8c5e_7a3b_7c2d_9e4f_5a6b_7c8d_9e0f);
const BOB: PlayerId = PlayerId(0x0192_8c5e_7a3b_7c2d_9e4f_5a6b_7c8d_9e10);
const ALICE_SUB: &str = "CgVhbGljZRIFbG9jYWw";
const BOB_SUB: &str = "CgNib2ISBWxvY2Fs";
const CAROL_SUB: &str = "CgVjYXJvbBIFbG9jYWw";
/// When Bob's colony fleet lands on the free planet.
const BOB_ARRIVES: i64 = 3_600_000;

fn build() -> EngineBuild {
    EngineBuild::new("routes-test").unwrap()
}

fn url(pool: &PgPool) -> String {
    let base = std::env::var("DATABASE_URL").expect("the database stage sets DATABASE_URL");
    let db = pool.connect_options().get_database().unwrap().to_owned();
    format!("{}/{db}", base.rsplit_once('/').unwrap().0)
}

/// The planets the tests name.
#[derive(Debug, Clone, Copy)]
struct Ids {
    alice_home: PlanetId,
    bob_home: PlanetId,
    free: PlanetId,
}

/// Alice and Bob, each on a homeworld 1,000 apart, and a free planet between them. Alice has a
/// Drill under construction; one of Bob's Cargo Vessels sits on Alice's roster; Bob's colony
/// fleet is on its way to the free planet. `homeworlds` more free planets are homeworld
/// candidates, and `extra` more planets sit at the origin (for the near cap).
fn world(homeworlds: usize, extra: usize) -> (World, Ids) {
    let mut b = WorldBuilder::new();
    let alice_home = b.player(ALICE);
    let bob_home = b.player(BOB);
    let free = b.planet(None);
    b.position(alice_home, Position { x: 0, y: 0, z: 0 });
    b.position(
        bob_home,
        Position {
            x: 1_000,
            y: 0,
            z: 0,
        },
    );
    b.position(free, Position { x: 500, y: 0, z: 0 });
    b.construction(alice_home, BuildingKind::Drill, 0);
    b.ship(BOB, ShipKind::CargoVessel, alice_home);
    let colony = b.ship(BOB, ShipKind::ColonyShip, bob_home);
    let fleet = b.fleet(&[colony], Cargo::default());
    b.send(fleet, free, Mission::Colonize, BOB_ARRIVES);
    let candidates: Vec<PlanetId> = (0..homeworlds)
        .map(|i| {
            let p = b.planet(None);
            b.position(
                p,
                Position {
                    x: -10_000 - i as i64,
                    y: 0,
                    z: 0,
                },
            );
            p
        })
        .collect();
    b.candidates(&candidates);
    for i in 0..extra {
        let p = b.planet(None);
        b.position(
            p,
            Position {
                x: 0,
                y: i as i64 + 1,
                z: 0,
            },
        );
    }
    let mut w = b.build();
    let rules = v0_rules();
    w.solve_planet(alice_home, &rules).unwrap();
    w.solve_planet(bob_home, &rules).unwrap();
    (
        w,
        Ids {
            alice_home,
            bob_home,
            free,
        },
    )
}

// --- Fault injection on the host's log, in the test only ---

struct Faults {
    fail: AtomicBool,
    hold: AtomicBool,
    gate: Semaphore,
    entered: watch::Sender<u64>,
}

impl Faults {
    fn new() -> Self {
        Self {
            fail: AtomicBool::new(false),
            hold: AtomicBool::new(false),
            gate: Semaphore::new(0),
            entered: watch::Sender::new(0),
        }
    }

    async fn entered(&self, n: u64) {
        let mut rx = self.entered.subscribe();
        tokio::time::timeout(PATIENCE, rx.wait_for(|e| *e >= n))
            .await
            .expect("the host reached the append")
            .unwrap();
    }

    fn release(&self) {
        self.hold.store(false, Ordering::SeqCst);
        self.gate.add_permits(1_000);
    }
}

struct FaultyLog {
    inner: PgLog,
    faults: Arc<Faults>,
}

impl LogStore for FaultyLog {
    async fn last(&mut self) -> Result<Option<(i64, i64)>, LogError> {
        self.inner.last().await
    }

    async fn append(&mut self, records: &[LogRecord]) -> Result<(), LogError> {
        self.faults.entered.send_modify(|e| *e += 1);
        if self.faults.hold.load(Ordering::SeqCst) {
            self.faults.gate.acquire().await.unwrap().forget();
        }
        if self.faults.fail.load(Ordering::SeqCst) {
            return Err(LogError::Memory("injected append failure".to_owned()));
        }
        self.inner.append(records).await
    }

    async fn records(&mut self) -> Result<Vec<LogRecord>, LogError> {
        self.inner.records().await
    }

    async fn find_keys(
        &mut self,
        keys: &[(PlayerId, IdempotencyKey)],
    ) -> Result<Vec<LogRecord>, LogError> {
        self.inner.find_keys(keys).await
    }

    async fn keyed_since(&mut self, since: i64) -> Result<Vec<LogRecord>, LogError> {
        self.inner.keyed_since(since).await
    }

    async fn records_after(&mut self, seq: i64) -> Result<Vec<LogRecord>, LogError> {
        self.inner.records_after(seq).await
    }

    async fn write_snapshot(&mut self, row: &SnapshotRow) -> Result<(), LogError> {
        self.inner.write_snapshot(row).await
    }

    async fn latest_snapshot(
        &mut self,
        build: Option<&str>,
    ) -> Result<Option<SnapshotRow>, LogError> {
        self.inner.latest_snapshot(build).await
    }

    async fn snapshot_index(&mut self) -> Result<Vec<SnapshotMeta>, LogError> {
        self.inner.snapshot_index().await
    }

    async fn snapshot_at(&mut self, seq: i64) -> Result<Option<SnapshotRow>, LogError> {
        self.inner.snapshot_at(seq).await
    }
}

// --- The API over HTTP ---

#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
    retry_after: Option<String>,
    www_authenticate: Option<String>,
}

/// One response, for the parity rule: method, route, status, and whether `Retry-After` and
/// `WWW-Authenticate` were sent.
type Seen = (String, String, u16, bool, bool);

struct Api {
    base: String,
    http: reqwest::Client,
    key: TestKey,
    issuer: String,
    dex: Arc<Dex>,
    clock: ManualClock,
    host: HostHandle,
    faults: Arc<Faults>,
    /// Where the leaderboard is published (P3.5): tests build at the manual clock and publish.
    board: voidforge_server::leaderboard::Publish,
    ids: Ids,
    pool: PgPool,
    /// Every response seen, for the parity rule.
    seen: Mutex<BTreeSet<Seen>>,
}

/// Limits no test here reaches, so only the rate-limit tests meet a 429 (P3.8).
const GENEROUS: Limits = Limits {
    read: Budget {
        per_sec: 1_000,
        burst: 10_000,
    },
    write: Budget {
        per_sec: 1_000,
        burst: 10_000,
    },
};

/// Seeds `world`, maps Alice's and Bob's subjects to their players, and serves the router.
async fn api(pool: &PgPool, world: (World, Ids), config: impl FnOnce(&mut HostConfig)) -> Api {
    api_with(pool, world, config, None, None).await
}

/// The same, with `limiter` instead of [`GENEROUS`] limits on the system clock, and `extra`
/// routes beside the real ones (P3.8's probes; they're outside the document, so a test calls them
/// directly, never through [`Api::send`]).
async fn api_with(
    pool: &PgPool,
    (w, ids): (World, Ids),
    config: impl FnOnce(&mut HostConfig),
    limiter: Option<Arc<Limiter>>,
    extra: Option<axum::Router<AppState>>,
) -> Api {
    let url = url(pool);
    let snap = snapshot::encode(&w, &build()).unwrap();
    voidforge_store::bootstrap(&url, &snap, EPOCH)
        .await
        .unwrap();
    for (player, sub) in [(ALICE, ALICE_SUB), (BOB, BOB_SUB)] {
        sqlx::query("INSERT INTO accounts (player_id, issuer, subject) VALUES ($1::uuid, $2, $3)")
            .bind(voidforge_store::log::player_uuid(player))
            .bind("placeholder")
            .bind(sub)
            .execute(pool)
            .await
            .unwrap();
    }
    // The dex stand-in, and the issuer the accounts rows name.
    let dex = Arc::new(Dex::default());
    let key = TestKey::new("k1");
    *dex.keys.lock().unwrap() = vec![key.jwk.clone()];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dex_base = format!("http://{}", listener.local_addr().unwrap());
    let dex_router = axum::Router::new()
        .route(
            "/dex/.well-known/openid-configuration",
            axum::routing::get(discovery),
        )
        .route("/dex/keys", axum::routing::get(jwks))
        .with_state(dex.clone());
    tokio::spawn(async move { axum::serve(listener, dex_router).await.unwrap() });
    let issuer = format!("{dex_base}/dex");
    *dex.issuer.lock().unwrap() = issuer.clone();
    sqlx::query("UPDATE accounts SET issuer = $1")
        .bind(&issuer)
        .execute(pool)
        .await
        .unwrap();
    let verifier = Arc::new(
        TokenVerifier::new(AuthConfig {
            issuer: issuer.clone(),
            audiences: vec!["voidforge-cli".to_owned()],
            leeway: Duration::from_secs(30),
            refresh: Duration::from_secs(900),
            min_refresh: Duration::from_secs(10),
            max_stale: Duration::from_secs(86_400),
        })
        .unwrap(),
    );
    // The host, recovered from Postgres as the server does.
    let rules = v0_rules();
    let recovered = voidforge_host::recover(&mut PgLog::lazy_url(&url).unwrap(), &rules, &build())
        .await
        .unwrap();
    let faults = Arc::new(Faults::new());
    let clock = ManualClock::new(EPOCH);
    let mut host_config = HostConfig::new(EPOCH, build());
    config(&mut host_config);
    let (f, u) = (faults.clone(), url.clone());
    let (host, task) = voidforge_host::spawn(
        recovered.world,
        rules,
        move || FaultyLog {
            inner: PgLog::lazy_url(&u).unwrap(),
            faults: f,
        },
        clock.clone(),
        host_config,
    )
    .unwrap();
    // The task joins when the process ends; a test that stops the host checks `is_finished`.
    std::mem::forget(task);
    let players = Arc::new(Players::new(
        PgAccounts::new(PgPool::connect(&url).await.unwrap()),
        host.clone(),
    ));
    let c = clock.clone();
    let (board, leaderboard) = voidforge_server::leaderboard::channel();
    let rules_body = voidforge_server::rules_view::body(&v0_rules());
    let state = AppState {
        verifier,
        players,
        host: host.clone(),
        now_unix_ms: Arc::new(move || voidforge_host::Clock::now_unix_ms(&c)),
        epoch_unix_ms: EPOCH,
        leaderboard,
        limiter: limiter
            .unwrap_or_else(|| Arc::new(Limiter::new(GENEROUS, Arc::new(Instant::now)))),
        rules: rules_body,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = match extra {
        Some(extra) => router(state.clone()).merge(extra.with_state(state)),
        None => router(state),
    };
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Api {
        base,
        http: reqwest::Client::new(),
        key,
        issuer,
        dex,
        clock,
        host,
        faults,
        board,
        ids,
        pool: pool.clone(),
        seen: Mutex::new(BTreeSet::new()),
    }
}

/// The route template a concrete path belongs to.
fn template(method: &str, path: &str) -> String {
    let path = path.split('?').next().unwrap();
    let known = routes();
    known
        .iter()
        .find(|r| {
            r.method.as_str() == method && {
                let (a, b): (Vec<_>, Vec<_>) =
                    (r.path.split('/').collect(), path.split('/').collect());
                a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.starts_with('{') || x == y)
            }
        })
        .map_or_else(|| format!("unrouted {path}"), |r| r.path.to_owned())
}

impl Api {
    fn token(&self, sub: &str) -> String {
        let mut c = claims(&self.issuer);
        c["sub"] = json!(sub);
        self.key.sign(&c)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Reply {
        let mut request = self
            .http
            .request(method.clone(), format!("{}{path}", self.base));
        if let Some(t) = token {
            request = request.bearer_auth(t);
        }
        if let Some(b) = body {
            request = request
                .header("content-type", "application/json")
                .body(b.to_owned());
        }
        for (k, v) in headers {
            request = request.header(*k, *v);
        }
        let response = tokio::time::timeout(PATIENCE, request.send())
            .await
            .expect("the API answers")
            .unwrap();
        let status = response.status().as_u16();
        let header = |n: &str| {
            response
                .headers()
                .get(n)
                .map(|v| v.to_str().unwrap().to_owned())
        };
        let (retry_after, www_authenticate) = (header("retry-after"), header("www-authenticate"));
        let text = response.text().await.unwrap();
        let body = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{method} {path}: not JSON ({e}): {text:?}"));
        self.seen.lock().unwrap().insert((
            method.as_str().to_owned(),
            template(method.as_str(), path),
            status,
            retry_after.is_some(),
            www_authenticate.is_some(),
        ));
        Reply {
            status,
            body,
            retry_after,
            www_authenticate,
        }
    }

    async fn get(&self, path: &str, sub: &str) -> Reply {
        let token = self.token(sub);
        self.send(Method::GET, path, Some(&token), &[], None).await
    }

    async fn command(&self, sub: &str, key: &str, body: &Value) -> Reply {
        let token = self.token(sub);
        self.send(
            Method::POST,
            "/commands",
            Some(&token),
            &[("idempotency-key", key)],
            Some(&body.to_string()),
        )
        .await
    }

    async fn records(&self) -> usize {
        PgLog::new(self.pool.clone()).records().await.unwrap().len()
    }

    async fn accounts(&self) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM accounts")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// The next due planet event's time, from the published World.
    fn alice_drill_done(&self) -> i64 {
        self.host.read().world.planets[&self.ids.alice_home]
            .next_event
            .expect("Alice's Drill is under construction")
            .at_ms
    }
}

impl Drop for Api {
    /// The parity rule, per test: every status seen is one the document declares for the route,
    /// with exactly the headers it declares for that status.
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let doc = voidforge_api_types::openapi();
        for (method, route, status, retry, www) in self.seen.lock().unwrap().iter() {
            let declared =
                &doc["paths"][route][method.to_lowercase()]["responses"][status.to_string()];
            let unrouted = route.starts_with("unrouted") && *status == 404;
            assert!(
                unrouted || !declared.is_null(),
                "{method} {route} answered {status}, which the document doesn't declare"
            );
            for (header, sent) in [("Retry-After", retry), ("WWW-Authenticate", www)] {
                assert_eq!(
                    *sent,
                    !declared["headers"][header].is_null(),
                    "{method} {route} {status}: {header} sent: {sent}; the document must agree"
                );
            }
        }
    }
}

fn code(r: &Reply) -> &str {
    r.body["code"]
        .as_str()
        .unwrap_or_else(|| panic!("no code: {r:?}"))
}

/// An error answer's shape: exactly `code` and `message`, the code as given.
fn assert_error(r: &Reply, status: u16, expected: &str) {
    assert_eq!((r.status, code(r)), (status, expected), "{r:?}");
    let keys: Vec<&String> = r.body.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["code", "message"], "{r:?}");
    assert!(!r.body["message"].as_str().unwrap().is_empty());
}

fn build_drill(planet: PlanetId) -> Value {
    json!({"type": "build_building", "payload": {"planet_id": planet.0, "building_type": "drill"}})
}

// --- POST /commands ---

/// Accepted: 200 with seq, game time and the command's own events only (D-045's privacy rule).
/// The host is held inside an earlier append while the clock passes Alice's Drill completion,
/// so Bob's command is applied in the same step that fires it: Alice's event is in Bob's
/// record, and never in his answer, fresh or repeated. A repeat writes nothing.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn an_accepted_command_answers_its_own_events_only_and_a_repeat_the_same(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    assert_eq!(
        api.get("/players/me", BOB_SUB).await.status,
        200,
        "warm Bob's account"
    );
    let done = api.alice_drill_done();
    api.faults.hold.store(true, Ordering::SeqCst);
    let earlier = {
        let host = api.host.clone();
        tokio::spawn(async move {
            host.submit(voidforge_host::Command {
                player: ALICE,
                action: voidforge_host::Action::CancelShipBuild {
                    build: voidforge_host::ShipBuildId(999),
                },
            })
            .await
        })
    };
    api.faults.entered(1).await;
    api.clock.set(EPOCH + done + 1);
    let body = build_drill(api.ids.bob_home);
    let bob = {
        let token = api.token(BOB_SUB);
        let http = api.http.clone();
        let url = format!("{}/commands", api.base);
        let body = body.to_string();
        tokio::spawn(async move {
            http.post(url)
                .bearer_auth(token)
                .header("idempotency-key", "bob-1")
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        })
    };
    // Bob's request is in the inbox once the host's next append starts; let it through.
    tokio::time::sleep(Duration::from_millis(300)).await;
    api.faults.release();
    earlier.await.unwrap().unwrap();
    let first = tokio::time::timeout(PATIENCE, bob).await.unwrap().unwrap();
    assert_eq!(
        first["events"],
        json!([]),
        "Alice's completion is not Bob's: {first}"
    );
    let seq = first["seq"].as_i64().unwrap();
    let records = PgLog::new(api.pool.clone()).records().await.unwrap();
    let bobs = records.iter().find(|r| r.seq == seq).unwrap();
    assert!(
        bobs.events
            .iter()
            .any(|e| matches!(e, voidforge_host::DomainEvent::BuildingCompleted { .. })),
        "the test must put Alice's event in Bob's record: {bobs:?}"
    );
    // Later, the repeat still answers the first stamp: its game time is the receipt's, not now.
    api.clock.set(EPOCH + done + 60_000);
    let again = api.command(BOB_SUB, "bob-1", &body).await;
    assert_eq!((again.status, &again.body), (200, &first));
    assert_eq!(
        api.records().await,
        records.len(),
        "a repeat writes nothing"
    );
    insta::assert_json_snapshot!("command_accepted", first);
}

/// A game rule: 422 with the core's code, and the same answer for a repeat.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_refused_command_is_422_with_the_cores_code_every_time(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let body = build_drill(api.ids.bob_home);
    let first = api.command(ALICE_SUB, "a-1", &body).await;
    assert_error(&first, 422, "not_owner");
    let again = api.command(ALICE_SUB, "a-1", &body).await;
    assert_eq!((again.status, &again.body), (422, &first.body));
    insta::assert_json_snapshot!("command_refused", first.body);
}

/// The same key for another command: 422 `idempotency_key_conflict`, nothing applied.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_key_reused_for_another_command_is_a_conflict(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    assert_eq!(
        api.command(ALICE_SUB, "k", &build_drill(api.ids.alice_home))
            .await
            .status,
        200
    );
    let records = api.records().await;
    let other = json!({"type": "cancel_ship_build", "payload": {"ship_build_id": 1}});
    let r = api.command(ALICE_SUB, "k", &other).await;
    assert_error(&r, 422, "idempotency_key_conflict");
    assert_eq!(api.records().await, records);
    insta::assert_json_snapshot!("idempotency_key_conflict", r.body);
}

/// The key header: missing, empty, a space, too long, not ASCII, or twice.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_idempotency_key_is_required_and_checked(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let token = api.token(ALICE_SUB);
    let body = build_drill(api.ids.alice_home).to_string();
    let missing = api
        .send(Method::POST, "/commands", Some(&token), &[], Some(&body))
        .await;
    assert_error(&missing, 400, "missing_idempotency_key");
    insta::assert_json_snapshot!("missing_idempotency_key", missing.body);
    let long = "k".repeat(129);
    for key in ["", "a b", long.as_str()] {
        let r = api
            .send(
                Method::POST,
                "/commands",
                Some(&token),
                &[("idempotency-key", key)],
                Some(&body),
            )
            .await;
        assert_error(&r, 400, "invalid_idempotency_key");
    }
    let twice = api
        .send(
            Method::POST,
            "/commands",
            Some(&token),
            &[("idempotency-key", "a"), ("idempotency-key", "b")],
            Some(&body),
        )
        .await;
    assert_error(&twice, 400, "invalid_idempotency_key");
    let edge = "k".repeat(128);
    let ok = api
        .send(
            Method::POST,
            "/commands",
            Some(&token),
            &[("idempotency-key", &edge)],
            Some(&body),
        )
        .await;
    assert_eq!(ok.status, 200, "128 bytes is the longest key: {ok:?}");
    assert_eq!(api.records().await, 1, "only the valid one was applied");
}

/// Every malformed body is 400 `malformed_body`, never 415, 413, 422 or 500: `join_game`, an
/// unknown type or field, broken JSON, a wrong or missing content type, an integral float, an
/// out-of-range or negative ID (the P3.1 carries), and a body over the limit. A repeated ship
/// is `duplicate_ship_ids`.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn every_malformed_body_is_400_and_applies_nothing(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let token = api.token(ALICE_SUB);
    let home = api.ids.alice_home.0;
    // A valid command, padded with JSON whitespace past the 64 KiB limit: only the limit
    // refuses it.
    let big = format!(
        r#"{{"type":"build_building","payload":{{"planet_id":{home},"building_type":"drill"}}}}{}"#,
        " ".repeat(70 * 1024)
    );
    let cases: Vec<(&str, String)> = vec![
        ("join_game", r#"{"type":"join_game"}"#.to_owned()),
        ("join_game with a payload", r#"{"type":"join_game","payload":{}}"#.to_owned()),
        ("unknown type", r#"{"type":"self_destruct","payload":{}}"#.to_owned()),
        ("unknown field", format!(r#"{{"type":"build_building","payload":{{"planet_id":{home},"building_type":"drill","extra":1}}}}"#)),
        ("missing field", format!(r#"{{"type":"build_building","payload":{{"planet_id":{home}}}}}"#)),
        ("broken JSON", "{".to_owned()),
        ("not an object", "[]".to_owned()),
        ("integral float ID", format!(r#"{{"type":"build_building","payload":{{"planet_id":{home}.0,"building_type":"drill"}}}}"#)),
        ("ID beyond u64", r#"{"type":"build_building","payload":{"planet_id":18446744073709551616,"building_type":"drill"}}"#.to_owned()),
        ("negative ID", r#"{"type":"build_building","payload":{"planet_id":-1,"building_type":"drill"}}"#.to_owned()),
        ("integral float ship", format!(r#"{{"type":"assemble_fleet","payload":{{"planet_id":{home},"ship_ids":[1.0],"cargo":{{"iron_ore_g":0,"iron_ingot_g":0}}}}}}"#)),
        ("integral float cargo", format!(r#"{{"type":"assemble_fleet","payload":{{"planet_id":{home},"ship_ids":[1],"cargo":{{"iron_ore_g":0.0,"iron_ingot_g":0}}}}}}"#)),
        ("over the body limit", big),
    ];
    for (name, body) in &cases {
        let r = api
            .send(
                Method::POST,
                "/commands",
                Some(&token),
                &[("idempotency-key", name)],
                Some(body),
            )
            .await;
        assert_error(&r, 400, "malformed_body");
        if *name == "join_game" {
            insta::assert_json_snapshot!("malformed_body_join_game", r.body["code"]);
        }
    }
    let body = build_drill(api.ids.alice_home).to_string();
    for content_type in ["text/plain", "application/x-www-form-urlencoded"] {
        let r = api
            .send(
                Method::POST,
                "/commands",
                Some(&token),
                &[
                    ("idempotency-key", content_type),
                    ("content-type", content_type),
                ],
                None,
            )
            .await;
        assert_error(&r, 400, "malformed_body");
    }
    let no_type = api
        .http
        .post(format!("{}/commands", api.base))
        .bearer_auth(&token)
        .header("idempotency-key", "no-type")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(no_type.status().as_u16(), 400);
    let dup = json!({"type": "assemble_fleet", "payload": {"planet_id": home, "ship_ids": [5, 5],
        "cargo": {"iron_ore_g": 0, "iron_ingot_g": 0}}});
    let r = api.command(ALICE_SUB, "dup", &dup).await;
    assert_error(&r, 400, "duplicate_ship_ids");
    insta::assert_json_snapshot!("duplicate_ship_ids", r.body);
    assert_eq!(api.records().await, 0, "nothing was applied");
}

/// The body limit's exact boundary (dev-qa's Q3): a command padded to exactly 64 KiB is read
/// (and refused by the game, 422), one byte more is 400 `malformed_body`.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_body_limit_is_exactly_64_kib(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let token = api.token(ALICE_SUB);
    let command = r#"{"type":"recall_fleet","payload":{"fleet_id":424242}}"#;
    let padded = |len: usize| format!("{command}{}", " ".repeat(len - command.len()));
    // The number itself, as the contract states it, not the server's constant: a test that
    // read the constant would move with it (the mutation table's Q7).
    let limit = 65_536;
    assert_eq!(voidforge_server::api::MAX_BODY_BYTES, limit);
    let at = api
        .send(
            Method::POST,
            "/commands",
            Some(&token),
            &[("idempotency-key", "at")],
            Some(&padded(limit)),
        )
        .await;
    assert_error(&at, 422, "unknown_fleet");
    let over = api
        .send(
            Method::POST,
            "/commands",
            Some(&token),
            &[("idempotency-key", "over")],
            Some(&padded(limit + 1)),
        )
        .await;
    assert_error(&over, 400, "malformed_body");
}

/// A key byte that isn't ASCII is 400 `invalid_idempotency_key`, never read as lossy text, and
/// nothing is applied (dev-qa's Q2).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_non_ascii_key_byte_is_invalid(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let r = api
        .http
        .post(format!("{}/commands", api.base))
        .bearer_auth(api.token(ALICE_SUB))
        .header("content-type", "application/json")
        .header(
            "idempotency-key",
            reqwest::header::HeaderValue::from_bytes(b"caf\xe9").unwrap(),
        )
        .body(build_drill(api.ids.alice_home).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["code"], "invalid_idempotency_key");
    assert_eq!(api.records().await, 0);
}

/// A wall clock behind the World's time (an NTP step back, or recovery on a slower clock) still
/// answers every view at the World's time, never 500 (dev-qa's Q1).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_clock_behind_the_world_answers_at_the_worlds_time(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    api.clock.set(EPOCH + 5_000);
    assert_eq!(
        api.command(ALICE_SUB, "at-5s", &build_drill(api.ids.alice_home))
            .await
            .status,
        200
    );
    api.clock.set(EPOCH);
    for path in [
        format!("/planets/{}", api.ids.alice_home.0),
        "/players/me".to_owned(),
        "/players/me/fleets".to_owned(),
        "/planets?near=0,0,0&radius=10".to_owned(),
    ] {
        let r = api.get(&path, ALICE_SUB).await;
        assert_eq!(r.status, 200, "{path}: {r:?}");
        assert_eq!(r.body["as_of"], json!("2025-10-09T08:53:25.000Z"), "{path}");
    }
}

/// S1 (D-049): another player's fleet or ship answers byte for byte as one that doesn't exist:
/// the status, every header but the date, and the body, fresh and for a repeated key.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn another_players_fleet_or_ship_answers_byte_for_byte_as_an_unknown_one(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let token = api.token(ALICE_SUB);
    let read = api.host.read();
    let bobs_fleet = read
        .world
        .fleets
        .values()
        .find(|f| f.owner == BOB)
        .unwrap()
        .id
        .0;
    let bobs_ship = read
        .world
        .ships
        .values()
        .find(|s| s.owner == BOB && s.kind == ShipKind::CargoVessel)
        .unwrap()
        .id
        .0;
    let ghost = read.world.next_id + 1_000;
    drop(read);
    let raw = |body: String, key: String| {
        let (http, url, token) = (
            api.http.clone(),
            format!("{}/commands", api.base),
            token.clone(),
        );
        async move {
            let r = http
                .post(url)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .header("idempotency-key", key)
                .body(body)
                .send()
                .await
                .unwrap();
            let status = r.status().as_u16();
            let mut headers: Vec<(String, String)> = r
                .headers()
                .iter()
                .filter(|(k, _)| *k != "date")
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_owned()))
                .collect();
            headers.sort();
            (status, headers, r.bytes().await.unwrap().to_vec())
        }
    };
    let home = api.ids.alice_home.0;
    let fleet_actions = |f: u64| {
        vec![
            json!({"type": "recall_fleet", "payload": {"fleet_id": f}}),
            json!({"type": "disband_fleet", "payload": {"fleet_id": f}}),
            json!({"type": "trigger_unload", "payload": {"fleet_id": f}}),
            json!({"type": "send_fleet", "payload": {"fleet_id": f, "target_planet_id": home, "mission": "move"}}),
            json!({"type": "send_fleet", "payload": {"fleet_id": f, "target_planet_id": 999_999, "mission": "move"}}),
        ]
    };
    let ship_actions = |s: u64| {
        vec![
            json!({"type": "assemble_fleet", "payload": {"planet_id": home, "ship_ids": [s],
                "cargo": {"iron_ore_g": 0, "iron_ingot_g": 0}}}),
            json!({"type": "assemble_fleet", "payload": {"planet_id": home, "ship_ids": [s],
                "cargo": {"iron_ore_g": -1, "iron_ingot_g": 0}}}),
        ]
    };
    let pairs = fleet_actions(bobs_fleet)
        .into_iter()
        .zip(fleet_actions(ghost))
        .chain(ship_actions(bobs_ship).into_iter().zip(ship_actions(ghost)));
    for (i, (foreign, unknown)) in pairs.enumerate() {
        let a = raw(foreign.to_string(), format!("foreign-{i}")).await;
        let b = raw(unknown.to_string(), format!("unknown-{i}")).await;
        assert_eq!(a, b, "{foreign} against {unknown}");
        assert_eq!(a.0, 422, "{foreign}");
        // A repeated key gives the same bytes again.
        assert_eq!(
            raw(foreign.to_string(), format!("foreign-{i}")).await,
            a,
            "{foreign} repeated"
        );
    }
}

/// A full inbox: 503 `busy`, Retry-After 1. A stopped host: 503 `unavailable`, Retry-After 5.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_busy_or_stopped_host_is_503_with_retry_after(pool: PgPool) {
    let api = api(&pool, world(0, 0), |c| {
        c.inbox_capacity = 1;
        c.max_batch = 1;
    })
    .await;
    // Warm both accounts first, so the account path doesn't meet the full inbox.
    assert_eq!(api.get("/players/me", ALICE_SUB).await.status, 200);
    assert_eq!(api.get("/players/me", BOB_SUB).await.status, 200);
    api.faults.hold.store(true, Ordering::SeqCst);
    let held = {
        let host = api.host.clone();
        tokio::spawn(async move {
            host.submit(voidforge_host::Command {
                player: ALICE,
                action: voidforge_host::Action::CancelShipBuild {
                    build: voidforge_host::ShipBuildId(1),
                },
            })
            .await
        })
    };
    api.faults.entered(1).await;
    let waiting = {
        let host = api.host.clone();
        tokio::spawn(async move {
            host.submit(voidforge_host::Command {
                player: BOB,
                action: voidforge_host::Action::CancelShipBuild {
                    build: voidforge_host::ShipBuildId(2),
                },
            })
            .await
        })
    };
    let r = api
        .command(ALICE_SUB, "busy", &build_drill(api.ids.alice_home))
        .await;
    assert_error(&r, 503, "busy");
    assert_eq!(r.retry_after.as_deref(), Some("1"));
    insta::assert_json_snapshot!("busy", r.body);
    api.faults.fail.store(true, Ordering::SeqCst);
    api.faults.release();
    let _ = held.await;
    let _ = waiting.await;
    let r = api
        .command(ALICE_SUB, "stopped", &build_drill(api.ids.alice_home))
        .await;
    assert_error(&r, 503, "unavailable");
    assert_eq!(r.retry_after.as_deref(), Some("5"));
}

// --- The views ---

/// A planet is public, but its roster shows each viewer only their own ships, and its
/// `valid_until` hides other players' arrivals (D-049, S1 and S2).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_planet_view_is_public_but_roster_ships_and_arrivals_are_owner_only(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let home = format!("/planets/{}", api.ids.alice_home.0);
    let alice = api.get(&home, ALICE_SUB).await;
    let bob = api.get(&home, BOB_SUB).await;
    assert_eq!((alice.status, bob.status), (200, 200));
    assert_eq!(
        alice.body["roster"],
        json!([]),
        "Bob's ship is hidden from Alice"
    );
    assert_eq!(
        bob.body["roster"].as_array().unwrap().len(),
        1,
        "Bob sees his own"
    );
    let mut a = alice.body.clone();
    let mut b = bob.body.clone();
    a["roster"] = json!(null);
    b["roster"] = json!(null);
    assert_eq!(a, b, "everything else is the same for both");
    insta::assert_json_snapshot!("planet_view_alice_home", alice.body);
    let free = format!("/planets/{}", api.ids.free.0);
    let alice = api.get(&free, ALICE_SUB).await;
    let bob = api.get(&free, BOB_SUB).await;
    assert_eq!(
        alice.body["valid_until"],
        json!(null),
        "Bob's landing is hidden"
    );
    assert_eq!(bob.body["valid_until"], json!("2025-10-09T09:53:20.000Z"));
}

/// An unknown planet is 404 `unknown_planet`; an ID that isn't a u64 is 400 `invalid_query`.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_planet_id_is_checked(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let r = api.get("/planets/999999", ALICE_SUB).await;
    assert_error(&r, 404, "unknown_planet");
    insta::assert_json_snapshot!("unknown_planet", r.body);
    for bad in ["abc", "1.0", "-1", "18446744073709551616", "1e3"] {
        let r = api.get(&format!("/planets/{bad}"), ALICE_SUB).await;
        assert_error(&r, 400, "invalid_query");
    }
}

/// Planets near: the summary, a radius of 0, every bad parameter (a negative radius, a
/// coordinate beyond i64, a missing or extra part), and the extremes of i64, which answer or
/// refuse but never panic, wrap or 500. Bob's landing is hidden from Alice's `valid_until`.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn planets_near_checks_its_query_and_survives_the_extremes(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let near = api.get("/planets?near=500,0,0&radius=500", ALICE_SUB).await;
    assert_eq!(near.status, 200, "{near:?}");
    assert_eq!(near.body["planets"].as_array().unwrap().len(), 3);
    assert_eq!(
        near.body["valid_until"],
        json!(null),
        "Bob's landing is hidden from Alice"
    );
    insta::assert_json_snapshot!("planets_near", near.body);
    let bob = api.get("/planets?near=500,0,0&radius=500", BOB_SUB).await;
    assert_eq!(bob.body["valid_until"], json!("2025-10-09T09:53:20.000Z"));
    let zero = api.get("/planets?near=500,0,0&radius=0", ALICE_SUB).await;
    assert_eq!(
        zero.body["planets"].as_array().unwrap().len(),
        1,
        "{zero:?}"
    );
    for bad in [
        "near=0,0,0&radius=-1",
        "near=9223372036854775808,0,0&radius=1",
        "near=0,-9223372036854775809,0&radius=1",
        "near=0,0&radius=1",
        "near=0,0,0,0&radius=1",
        "near=0,0,0&radius=9223372036854775808",
        "near=0,0,0&radius=1.0",
        "near=0,0,0",
        "radius=1",
        "",
    ] {
        let r = api.get(&format!("/planets?{bad}"), ALICE_SUB).await;
        assert_error(&r, 400, "invalid_query");
    }
    let max = i64::MAX;
    let min = i64::MIN;
    for (q, planets) in [
        (format!("near=0,0,0&radius={max}"), 3),
        (format!("near={max},{max},{max}&radius={max}"), 0),
        (format!("near={min},{min},{min}&radius={max}"), 0),
        // From i64::MIN to 0 is 2^63, one past i64::MAX: out of every radius, correctly.
        (format!("near={min},0,0&radius={max}"), 0),
        // The farthest planet (x = 1,000) is exactly i64::MAX away: inside, at the edge.
        (format!("near=-{},0,0&radius={max}", max - 1_000), 3),
        (format!("near=-{},0,0&radius={}", max - 1_000, max - 1), 2),
        (format!("near={max},0,0&radius=0"), 0),
    ] {
        let r = api.get(&format!("/planets?{q}"), ALICE_SUB).await;
        assert_eq!(r.status, 200, "{q}: {r:?}");
        assert_eq!(r.body["planets"].as_array().unwrap().len(), planets, "{q}");
    }
    // A comma written as %2C is the same query.
    let encoded = api
        .get("/planets?near=500%2C0%2C0&radius=0", ALICE_SUB)
        .await;
    assert_eq!(encoded.status, 200);
}

/// Exactly MAX_NEAR_RESULTS matches is an answer (the three named planets plus the extras, all
/// within the radius), and the document states the number.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn planets_near_answers_up_to_the_cap(pool: PgPool) {
    let api = api(&pool, world(0, MAX_NEAR_RESULTS - 3), |_| {}).await;
    let r = api
        .get("/planets?near=0,0,0&radius=100000", ALICE_SUB)
        .await;
    assert_eq!(r.status, 200, "{:?}", r.body["code"]);
    assert_eq!(
        r.body["planets"].as_array().unwrap().len(),
        MAX_NEAR_RESULTS
    );
    let doc = voidforge_api_types::openapi();
    let description = doc["paths"]["/planets"]["get"]["parameters"][1]["description"]
        .as_str()
        .unwrap();
    assert!(
        description.contains(&MAX_NEAR_RESULTS.to_string()),
        "{description}"
    );
}

/// One past the cap is 400 `invalid_query`, never a truncated list.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn one_planet_past_the_cap_is_invalid_query(pool: PgPool) {
    let api = api(&pool, world(0, MAX_NEAR_RESULTS - 2), |_| {}).await;
    let r = api
        .get("/planets?near=0,0,0&radius=100000", ALICE_SUB)
        .await;
    assert_error(&r, 400, "invalid_query");
    insta::assert_json_snapshot!("planets_near_over_the_cap", r.body);
}

/// `/players/me` and `/players/me/fleets`: the caller's own, with other players' arrivals
/// hidden from `valid_until`.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_player_and_fleet_views_are_the_callers_own(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let alice = api.get("/players/me", ALICE_SUB).await;
    assert_eq!(alice.status, 200);
    assert_eq!(
        alice.body["player_id"],
        json!("01928c5e-7a3b-7c2d-9e4f-5a6b7c8d9e0f")
    );
    insta::assert_json_snapshot!("player_alice", alice.body);
    let bob = api.get("/players/me", BOB_SUB).await;
    assert_eq!(
        bob.body["valid_until"],
        json!("2025-10-09T09:53:20.000Z"),
        "his fleet"
    );
    let alice_fleets = api.get("/players/me/fleets", ALICE_SUB).await;
    assert_eq!(alice_fleets.body["fleets"], json!([]));
    assert_eq!(alice_fleets.body["valid_until"], json!(null));
    let bob_fleets = api.get("/players/me/fleets", BOB_SUB).await;
    assert_eq!(bob_fleets.status, 200);
    assert_eq!(bob_fleets.body["fleets"].as_array().unwrap().len(), 1);
    insta::assert_json_snapshot!("fleets_bob", bob_fleets.body);
}

/// A read model a step behind waits for the host: published within the wait, the view
/// answers; not published, 503 `unavailable` with Retry-After 1. Neither crosses an event.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_stale_read_model_waits_for_the_host_or_answers_503(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let path = format!("/planets/{}", api.ids.alice_home.0);
    assert_eq!(
        api.get(&path, ALICE_SUB).await.status,
        200,
        "warm the account"
    );
    let done = api.alice_drill_done();
    api.faults.hold.store(true, Ordering::SeqCst);
    api.clock.set(EPOCH + done + 1);
    api.faults.entered(1).await;
    // Never published within the wait.
    let r = api.get(&path, ALICE_SUB).await;
    assert_error(&r, 503, "unavailable");
    assert_eq!(r.retry_after.as_deref(), Some("1"));
    // Published during the wait.
    let faults = api.faults.clone();
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        faults.release();
    });
    let r = api.get(&path, ALICE_SUB).await;
    release.await.unwrap();
    assert_eq!(r.status, 200, "{r:?}");
    let building = &r.body["buildings"][r.body["buildings"].as_array().unwrap().len() - 1];
    assert_eq!(
        building["state"], "operational",
        "projected past the event: {r:?}"
    );
}

/// `/leaderboard` before its first build: 503 `unavailable` with Retry-After 1, because the job
/// retries every second until a build succeeds (D-051, T5).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_leaderboard_is_not_built_yet(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let r = api.get("/leaderboard", ALICE_SUB).await;
    assert_error(&r, 503, "unavailable");
    assert_eq!(r.retry_after.as_deref(), Some("1"));
    insta::assert_json_snapshot!("leaderboard_not_built", r.body);
}

/// `/openapi.json` is public, the exact contract, and reading it joins nobody.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_contract_is_public_and_joins_nobody(pool: PgPool) {
    let api = api(&pool, world(1, 0), |_| {}).await;
    let accounts = api.accounts().await;
    for token in [None, Some("not-a-token")] {
        let r = api
            .send(Method::GET, "/openapi.json", token, &[], None)
            .await;
        assert_eq!(r.status, 200);
        assert_eq!(r.body, voidforge_api_types::openapi());
    }
    let carol = api.token(CAROL_SUB);
    let r = api
        .send(Method::GET, "/openapi.json", Some(&carol), &[], None)
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(api.accounts().await, accounts, "no account was made");
}

/// `/rules` is public, the loaded rules exactly, and reading it joins nobody (P4.1r, D-058).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_rules_are_public_exact_and_join_nobody(pool: PgPool) {
    let api = api(&pool, world(1, 0), |_| {}).await;
    let accounts = api.accounts().await;
    // The harness's host runs v0, so the answer is v0's view (the field-by-field check against
    // the file is tests/rules_view.rs).
    let want = serde_json::to_value(voidforge_server::rules_view::view(&v0_rules())).unwrap();
    for token in [None, Some("not-a-token")] {
        let r = api.send(Method::GET, "/rules", token, &[], None).await;
        assert_eq!(r.status, 200, "{r:?}");
        assert_eq!(r.body, want);
    }
    let carol = api.token(CAROL_SUB);
    let r = api
        .send(Method::GET, "/rules", Some(&carol), &[], None)
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(api.accounts().await, accounts, "no account was made");
    assert_eq!(api.records().await, 0, "nobody joined");
    let r = api
        .http
        .get(format!("{}/rules", api.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["content-type"], "application/json");
    assert_eq!(r.headers()["cache-control"], "no-cache");
    insta::assert_json_snapshot!("rules_v0", want);
}

// --- Auth and accounts on every route ---

fn authenticated(ids: Ids) -> Vec<(Method, String)> {
    vec![
        (Method::POST, "/commands".to_owned()),
        (Method::GET, format!("/planets/{}", ids.alice_home.0)),
        (Method::GET, "/planets?near=0,0,0&radius=1".to_owned()),
        (Method::GET, "/players/me".to_owned()),
        (Method::GET, "/players/me/fleets".to_owned()),
        (Method::GET, "/leaderboard".to_owned()),
    ]
}

/// Every authenticated route: no token is 401 `missing_token`, a bad one 401 `invalid_token`,
/// and a dex that can't be reached 503 `unavailable` (D-046).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn every_route_but_the_contract_needs_a_valid_token(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let body = build_drill(api.ids.alice_home).to_string();
    for (method, path) in authenticated(api.ids) {
        let headers = [("idempotency-key", "k")];
        let r = api
            .send(method.clone(), &path, None, &headers, Some(&body))
            .await;
        assert_error(&r, 401, "missing_token");
        assert!(r.www_authenticate.is_some());
        let r = api
            .send(
                method.clone(),
                &path,
                Some("e30.e30.e30"),
                &headers,
                Some(&body),
            )
            .await;
        assert_error(&r, 401, "invalid_token");
    }
    assert_eq!(api.accounts().await, 2, "no refusal made an account");
    insta::assert_json_snapshot!(
        "missing_token",
        api.send(Method::GET, "/players/me", None, &[], None)
            .await
            .body
    );
    api.dex.down.store(true, Ordering::SeqCst);
    let fresh = api.token(ALICE_SUB);
    for (method, path) in authenticated(api.ids) {
        let r = api
            .send(
                method,
                &path,
                Some(&fresh),
                &[("idempotency-key", "k")],
                Some(&body),
            )
            .await;
        assert_error(&r, 503, "unavailable");
    }
}

/// D-047, owner-confirmed: a refused account gets 422 `no_free_homeworld` on every route that
/// takes a player, reads included, and its row stays.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_refused_account_gets_no_free_homeworld_on_every_route(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let token = api.token(CAROL_SUB);
    let body = build_drill(api.ids.alice_home).to_string();
    for (method, path) in authenticated(api.ids) {
        let r = api
            .send(
                method,
                &path,
                Some(&token),
                &[("idempotency-key", "k")],
                Some(&body),
            )
            .await;
        assert_error(&r, 422, "no_free_homeworld");
    }
    assert_eq!(api.accounts().await, 3, "Carol's row stays");
    assert_eq!(api.records().await, 0, "nothing appended");
}

/// A new principal on a universe with a free homeworld joins on its first call, whatever the
/// route.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_new_principal_joins_on_its_first_read(pool: PgPool) {
    let api = api(&pool, world(1, 0), |_| {}).await;
    let r = api.get("/players/me/fleets", CAROL_SUB).await;
    assert_eq!(r.status, 200, "{r:?}");
    let me = api.get("/players/me", CAROL_SUB).await;
    assert_eq!(me.body["planet_ids"].as_array().unwrap().len(), 1);
}

// --- Routing ---

/// An unknown route, or a known route with another method, is 404 `not_found`.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn an_unknown_route_or_method_is_not_found(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    let token = api.token(ALICE_SUB);
    for (method, path) in [
        (Method::GET, "/nothing"),
        (Method::GET, "/"),
        (Method::GET, "/commands"),
        (Method::DELETE, "/players/me"),
        (Method::POST, "/planets"),
        (Method::PUT, "/openapi.json"),
        (Method::POST, "/rules"),
        (Method::GET, "/planets/1/extra"),
    ] {
        let r = api
            .send(method.clone(), path, Some(&token), &[], None)
            .await;
        assert_error(&r, 404, "not_found");
    }
    insta::assert_json_snapshot!(
        "not_found",
        api.send(Method::GET, "/nothing", None, &[], None)
            .await
            .body
    );
}

/// The router's route table and the document's paths are the same set, and every route in the
/// table is served (none falls through to `not_found`).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_router_and_the_document_have_the_same_routes(pool: PgPool) {
    let doc = voidforge_api_types::openapi();
    let documented: BTreeSet<(String, String)> = doc["paths"]
        .as_object()
        .unwrap()
        .iter()
        .flat_map(|(path, item)| {
            item.as_object()
                .unwrap()
                .keys()
                .map(move |m| (m.to_uppercase(), path.clone()))
        })
        .collect();
    let routed: BTreeSet<(String, String)> = routes()
        .iter()
        .map(|r| (r.method.as_str().to_owned(), r.path.to_owned()))
        .collect();
    assert_eq!(routed, documented);
    // Public exactly when the document has no security.
    for r in routes() {
        let security = &doc["paths"][r.path][r.method.as_str().to_lowercase()]["security"];
        assert_eq!(
            r.authenticated,
            !security.is_null(),
            "{} {}",
            r.method,
            r.path
        );
    }
    let api = api(&pool, world(0, 0), |_| {}).await;
    let token = api.token(ALICE_SUB);
    let body = build_drill(api.ids.alice_home).to_string();
    for r in routes() {
        let path = r.path.replace("{id}", &api.ids.alice_home.0.to_string());
        let reply = api
            .send(
                r.method.clone(),
                &path,
                Some(&token),
                &[("idempotency-key", "p")],
                Some(&body),
            )
            .await;
        assert_ne!(
            reply.body["code"],
            json!("not_found"),
            "{} {path}",
            r.method
        );
    }
}

/// A panicking handler, behind the router's own layers, answers 500 `internal` with a generic
/// body, never a dropped connection or the panic's text.
#[tokio::test]
async fn a_panicking_handler_answers_500_internal() {
    let app = voidforge_server::api::layered(axum::Router::new().route(
        "/boom",
        axum::routing::get(|| async {
            panic!("secret detail");
            #[allow(unreachable_code)]
            ""
        }),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let r = reqwest::get(format!("{base}/boom")).await.unwrap();
    assert_eq!(r.status().as_u16(), 500);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["code"], "internal");
    assert!(!body.to_string().contains("secret"), "{body}");
}

/// The statuses this suite saw, per route, are summed here only as documentation; each test's
/// own check runs when its harness drops.
#[test]
fn the_statuses_each_route_may_give_are_declared() {
    let doc = voidforge_api_types::openapi();
    let declared: BTreeMap<String, Vec<String>> = routes()
        .iter()
        .map(|r| {
            let responses = doc["paths"][r.path][r.method.as_str().to_lowercase()]["responses"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            (format!("{} {}", r.method, r.path), responses)
        })
        .collect();
    // Every route that takes a player can refuse one (422) and be unavailable (503).
    for r in routes().iter().filter(|r| r.authenticated) {
        let d = &declared[&format!("{} {}", r.method, r.path)];
        for status in ["200", "401", "422", "500", "503"] {
            assert!(
                d.contains(&status.to_owned()),
                "{} {} lacks {status}",
                r.method,
                r.path
            );
        }
    }
    insta::assert_json_snapshot!("declared_statuses", declared);
}

// --- The leaderboard (P3.5, D-051) ---

impl Api {
    /// Builds the board at the manual clock, as the job does, and publishes it.
    async fn build_board(&self) -> Result<i64, voidforge_server::leaderboard::BuildError> {
        let c = self.clock.clone();
        let now = move || voidforge_host::Clock::now_unix_ms(&c);
        let built =
            voidforge_server::leaderboard::build_now(&mut self.host.clone(), &now, EPOCH).await?;
        let t = built.t;
        self.board.send_replace(Some(Arc::new(built)));
        Ok(t)
    }

    /// `/leaderboard` as `alice`, checked: 200, deserialized into the contract's type, and
    /// valid against the document's schema.
    async fn board(&self) -> voidforge_api_types::view::Leaderboard {
        let r = self.get("/leaderboard", ALICE_SUB).await;
        assert_eq!(r.status, 200, "{:?}", r.body);
        leaderboard_schema()
            .validate(&r.body)
            .unwrap_or_else(|e| panic!("the board fits the schema: {e}"));
        serde_json::from_value(r.body.clone()).expect("the board is the contract's type")
    }
}

/// The document's `Leaderboard` schema, compiled as the contract tests compile theirs.
fn leaderboard_schema() -> jsonschema::Validator {
    let doc: Value = serde_json::from_str(&voidforge_api_types::openapi_json()).unwrap();
    let root = json!({
        "allOf": [{"$ref": "#/components/schemas/Leaderboard"}],
        "components": doc["components"],
    });
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .should_validate_formats(true)
        .build(&root)
        .unwrap()
}

/// New principals, each joining onto a free homeworld candidate: identical fresh homeworlds, so
/// identical scores (ties).
const NEWCOMERS: [&str; 3] = [
    "CgVjYXJvbBIFbG9jYWw",
    "CgRkYXZlEgVsb2NhbA",
    "CgRlcmluEgVsb2NhbA",
];

/// The board's score for each player equals that player's own `/players/me` at the same time,
/// for every player; ranks are competition ranks over the scores; the order is (score desc, ID
/// asc). Ties: three newcomers on identical homeworlds. Partial credit: Alice's drill under
/// construction, Bob's ships. Then the clock passes Alice's drill completing: the next build's
/// `as_of` advances, and the board still equals every `/players/me`.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_board_equals_every_players_own_score_at_the_same_time(pool: PgPool) {
    let api = api(&pool, world(3, 0), |_| {}).await;
    let subs: Vec<&str> = [ALICE_SUB, BOB_SUB].into_iter().chain(NEWCOMERS).collect();
    for sub in &subs {
        assert_eq!(api.get("/players/me", sub).await.status, 200);
    }
    let mut as_of = None;
    for at in [EPOCH + 60_000, EPOCH + 5 * 3_600_000] {
        api.clock.set(at);
        let t = api.build_board().await.unwrap();
        let board = api.board().await;
        assert_eq!(board.entries.len(), subs.len(), "every joined player");
        for sub in &subs {
            let me = api.get("/players/me", sub).await;
            assert_eq!(me.status, 200);
            let id = me.body["player_id"].as_str().unwrap();
            let entry = board
                .entries
                .iter()
                .find(|e| serde_json::to_value(&e.player_id).unwrap() == id)
                .unwrap_or_else(|| panic!("{id} is on the board"));
            assert_eq!(json!(entry.score), me.body["score"], "{id} at t {t}");
            assert_eq!(me.body["as_of"], json!(board.as_of), "the same time");
        }
        for e in &board.entries {
            let ahead = board.entries.iter().filter(|o| o.score > e.score).count();
            assert_eq!(e.rank as usize, ahead + 1);
        }
        for w in board.entries.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let ids = (
                serde_json::to_value(&a.player_id).unwrap(),
                serde_json::to_value(&b.player_id).unwrap(),
            );
            assert!(
                a.score > b.score || (a.score == b.score && ids.0.as_str() < ids.1.as_str()),
                "{a:?} before {b:?}"
            );
        }
        let tied = board
            .entries
            .iter()
            .filter(|e| board.entries.iter().filter(|o| o.score == e.score).count() >= 3)
            .count();
        assert!(tied >= 3, "the newcomers tie: {:?}", board.entries);
        if let Some(previous) = as_of.replace(board.as_of.clone()) {
            assert!(
                serde_json::to_value(&board.as_of).unwrap().as_str()
                    > serde_json::to_value(&previous).unwrap().as_str(),
                "the as_of advances"
            );
        }
    }
}

/// A failed build keeps the previous board: with the host's append held past a due event (Bob's
/// arrival), the read model is stale, the build fails after its 1 s wait, and the route still
/// serves the previous board, as_of and all. Released, the next build succeeds and advances.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_failed_build_keeps_the_previous_board(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    assert_eq!(api.get("/players/me", ALICE_SUB).await.status, 200);
    api.build_board().await.unwrap();
    let before = api.board().await;
    api.faults.hold.store(true, Ordering::SeqCst);
    api.clock.set(EPOCH + BOB_ARRIVES + 1);
    api.faults.entered(1).await;
    let failed = tokio::time::timeout(Duration::from_secs(5), api.build_board())
        .await
        .expect("the build gives up after its 1 s wait");
    assert_eq!(
        failed,
        Err(voidforge_server::leaderboard::BuildError::Stale)
    );
    assert_eq!(api.board().await, before, "the previous board, unchanged");
    api.faults.release();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if api.build_board().await.is_ok() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "never rebuilt");
    }
    let after = api.board().await;
    assert!(
        serde_json::to_value(&after.as_of).unwrap().as_str()
            > serde_json::to_value(&before.as_of).unwrap().as_str()
    );
}

/// The job itself, on the manual clock: it waits, then builds and publishes, again and again,
/// and the route's as_of follows.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_job_rebuilds_on_its_interval(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    assert_eq!(api.get("/players/me", ALICE_SUB).await.status, 200);
    api.build_board().await.unwrap();
    let c = api.clock.clone();
    let job = tokio::spawn(voidforge_server::leaderboard::run(
        api.host.clone(),
        Arc::new(move || voidforge_host::Clock::now_unix_ms(&c)),
        EPOCH,
        Duration::from_millis(50),
        api.board.clone(),
    ));
    let mut seen = vec![api.board().await.as_of];
    for step in 1..=3 {
        api.clock.set(EPOCH + step * 1_000);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let b = api.board().await;
            if &b.as_of != seen.last().unwrap() {
                seen.push(b.as_of);
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the job never rebuilt"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    job.abort();
    assert_eq!(seen.len(), 4);
}

/// The route serves the build, not the request's time: built at T1, read at a later T2, the
/// board's `as_of` is still T1 (up to the interval old, by design).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_board_keeps_its_builds_as_of(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    assert_eq!(api.get("/players/me", ALICE_SUB).await.status, 200);
    api.clock.set(EPOCH + 10_000);
    api.build_board().await.unwrap();
    let built = api.board().await;
    api.clock.set(EPOCH + 50_000);
    let later = api.board().await;
    assert_eq!(later, built, "the same build, as_of and all");
    let me = api.get("/players/me", ALICE_SUB).await;
    assert_ne!(
        me.body["as_of"],
        json!(later.as_of),
        "a player's own view is current"
    );
}

/// Before its first board, the job retries every second, not every interval: with an hour's
/// interval and nothing built, a board still appears within a couple of seconds.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_job_retries_every_second_until_the_first_board(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    assert_eq!(api.get("/players/me", ALICE_SUB).await.status, 200);
    let c = api.clock.clone();
    let job = tokio::spawn(voidforge_server::leaderboard::run(
        api.host.clone(),
        Arc::new(move || voidforge_host::Clock::now_unix_ms(&c)),
        EPOCH,
        Duration::from_secs(3_600),
        api.board.clone(),
    ));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if api.get("/leaderboard", ALICE_SUB).await.status == 200 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no board within 3 s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    job.abort();
}

/// The job itself keeps the previous board when its builds fail (the mutation table's survivor:
/// the direct-build test never ran the job's own error path). With the host's append held past
/// Bob's arrival, every build is stale; over 2.5 s the job, on a 50 ms interval with its 1 s
/// stale wait, attempts and fails at least twice, and the route still serves the previous board,
/// unchanged. Released, the job's next build advances it, which shows the job was running.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_job_keeps_the_previous_board_when_its_builds_fail(pool: PgPool) {
    let api = api(&pool, world(0, 0), |_| {}).await;
    assert_eq!(api.get("/players/me", ALICE_SUB).await.status, 200);
    api.build_board().await.unwrap();
    let before = api.board().await;
    let c = api.clock.clone();
    let job = tokio::spawn(voidforge_server::leaderboard::run(
        api.host.clone(),
        Arc::new(move || voidforge_host::Clock::now_unix_ms(&c)),
        EPOCH,
        Duration::from_millis(50),
        api.board.clone(),
    ));
    api.faults.hold.store(true, Ordering::SeqCst);
    api.clock.set(EPOCH + BOB_ARRIVES + 1);
    api.faults.entered(1).await;
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert_eq!(api.board().await, before, "the previous board, unchanged");
    api.faults.release();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let now = api.board().await;
        if now.as_of != before.as_of {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the job never rebuilt"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    job.abort();
}

// --- Rate limits (P3.8, D-054) ---

/// A monotonic clock the test steps, for the limiter.
#[derive(Clone)]
struct Stepped(Arc<Mutex<Instant>>);

impl Stepped {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Instant::now())))
    }

    fn step(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }

    fn limiter(&self, read: (u32, u32), write: (u32, u32)) -> Arc<Limiter> {
        let c = self.0.clone();
        let budget = |(per_sec, burst)| Budget { per_sec, burst };
        Arc::new(Limiter::new(
            Limits {
                read: budget(read),
                write: budget(write),
            },
            Arc::new(move || *c.lock().unwrap()),
        ))
    }
}

/// A limited answer: 429 `rate_limited`, D-045's body, an instruction, and a true Retry-After.
fn assert_limited(r: &Reply) {
    assert_error(r, 429, "rate_limited");
    assert_eq!(r.retry_after.as_deref(), Some("1"), "{r:?}");
    assert_eq!(r.body["message"], "too many requests; retry after 1 s");
}

impl Api {
    /// Sends `method path` as `sub`, a command with a fresh key and a valid body.
    async fn call(&self, method: &Method, path: &str, sub: &str, n: usize) -> Reply {
        let token = self.token(sub);
        let key = format!("rl-{sub}-{n}");
        let body = build_drill(self.ids.alice_home).to_string();
        let post = *method == Method::POST;
        let keyed = [("idempotency-key", key.as_str())];
        let headers: &[(&str, &str)] = if post { &keyed } else { &[] };
        self.send(
            method.clone(),
            path,
            Some(&token),
            headers,
            post.then_some(body.as_str()),
        )
        .await
    }
}

/// D-049's route table, every route: an authenticated one answers 429 once its class's budget is
/// spent, and succeeds again a Retry-After later; the public `/openapi.json` and `/rules` (D-058)
/// never do. A new route must be
/// added to `concrete`, so none can bypass the limiter by not taking `Player` unnoticed (the
/// lead's T3 addition).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn every_authenticated_route_is_limited_and_the_contract_never_is(pool: PgPool) {
    let clock = Stepped::new();
    let limiter = clock.limiter((1, 3), (1, 2));
    let api = api_with(&pool, world(0, 0), |_| {}, Some(limiter), None).await;
    let concrete = |path: &str| -> String {
        match path {
            "/commands" | "/players/me" | "/players/me/fleets" | "/leaderboard" | "/rules"
            | "/openapi.json" => path.to_owned(),
            "/planets/{id}" => format!("/planets/{}", api.ids.alice_home.0),
            "/planets" => "/planets?near=0,0,0&radius=1".to_owned(),
            other => panic!("a new route, {other}: say here how to call it"),
        }
    };
    let mut n = 0;
    let all = routes();
    assert_eq!(all.len(), 8, "the table this test was written against");
    for route in all {
        let path = concrete(route.path);
        // A full budget for each route.
        clock.step(Duration::from_secs(10));
        let burst = if route.method == Method::POST { 2 } else { 3 };
        if !route.authenticated {
            for _ in 0..50 {
                n += 1;
                let r = api.call(&route.method, &path, ALICE_SUB, n).await;
                assert_eq!(r.status, 200, "{} {path}: {r:?}", route.method);
            }
            continue;
        }
        for _ in 0..burst {
            n += 1;
            let r = api.call(&route.method, &path, ALICE_SUB, n).await;
            assert_ne!(
                r.status, 429,
                "{} {path}: within the burst: {r:?}",
                route.method
            );
        }
        n += 1;
        assert_limited(&api.call(&route.method, &path, ALICE_SUB, n).await);
        clock.step(Duration::from_secs(1));
        n += 1;
        let r = api.call(&route.method, &path, ALICE_SUB, n).await;
        assert_ne!(
            r.status, 429,
            "{} {path}: after Retry-After: {r:?}",
            route.method
        );
    }
}

/// One player's spent budget doesn't limit another.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn one_players_burst_doesnt_limit_another(pool: PgPool) {
    let clock = Stepped::new();
    let api = api_with(
        &pool,
        world(0, 0),
        |_| {},
        Some(clock.limiter((1, 2), (1, 1))),
        None,
    )
    .await;
    for _ in 0..2 {
        assert_eq!(api.get("/players/me", ALICE_SUB).await.status, 200);
    }
    assert_limited(&api.get("/players/me", ALICE_SUB).await);
    for _ in 0..2 {
        assert_eq!(api.get("/players/me", BOB_SUB).await.status, 200);
    }
    assert_limited(&api.get("/players/me", BOB_SUB).await);
}

/// A 429'd command never reaches the host and never consumes its key: retried later with the same
/// key, it gets its real first outcome, and a repeat of that is the same answer (D-037).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_limited_command_appends_nothing_and_its_key_gets_the_real_outcome(pool: PgPool) {
    let clock = Stepped::new();
    let api = api_with(
        &pool,
        world(0, 0),
        |_| {},
        Some(clock.limiter((100, 100), (1, 1))),
        None,
    )
    .await;
    let drill = build_drill(api.ids.alice_home);
    let first = api.command(ALICE_SUB, "first", &drill).await;
    assert_eq!(first.status, 200, "{first:?}");
    let records = api.records().await;
    let limited = api.command(ALICE_SUB, "second", &drill).await;
    assert_limited(&limited);
    assert_eq!(api.records().await, records, "a 429 appended nothing");
    // The Retry-After it was given; the key was never seen, so this is its first outcome.
    clock.step(Duration::from_secs(1));
    let real = api.command(ALICE_SUB, "second", &drill).await;
    assert_ne!(real.status, 429, "{real:?}");
    assert_eq!(
        api.records().await,
        records + 1,
        "the retry was applied, once"
    );
    clock.step(Duration::from_secs(1));
    let repeat = api.command(ALICE_SUB, "second", &drill).await;
    assert_eq!((repeat.status, &repeat.body), (real.status, &real.body));
    assert_eq!(api.records().await, records + 1, "a repeat writes nothing");
}

/// The limit comes before the account: a limited new principal makes no account and joins
/// nothing, and is joined on its first call within budget.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_limited_principal_makes_no_account_and_joins_nothing(pool: PgPool) {
    let clock = Stepped::new();
    let limiter = clock.limiter((1, 2), (1, 1));
    let api = api_with(&pool, world(1, 0), |_| {}, Some(limiter.clone()), None).await;
    let carol = voidforge_server::auth::Principal {
        issuer: api.issuer.clone(),
        subject: CAROL_SUB.to_owned(),
    };
    for _ in 0..2 {
        limiter
            .check(&carol, voidforge_server::limit::Class::Read)
            .unwrap();
    }
    let (accounts, records) = (api.accounts().await, api.records().await);
    assert_limited(&api.get("/players/me", CAROL_SUB).await);
    assert_eq!(api.accounts().await, accounts, "no account was made");
    assert_eq!(api.records().await, records, "no join was appended");
    clock.step(Duration::from_secs(1));
    assert_eq!(api.get("/players/me", CAROL_SUB).await.status, 200);
    assert_eq!(api.accounts().await, accounts + 1);
}

/// A refused account's 422s spend its budget (the advisor's A3); 401s have no principal and are
/// never counted, so they can't spend anyone's.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn refused_accounts_are_counted_and_unauthenticated_requests_are_not(pool: PgPool) {
    let clock = Stepped::new();
    let api = api_with(
        &pool,
        world(0, 0),
        |_| {},
        Some(clock.limiter((1, 2), (1, 1))),
        None,
    )
    .await;
    for _ in 0..2 {
        assert_error(
            &api.get("/players/me", CAROL_SUB).await,
            422,
            "no_free_homeworld",
        );
    }
    assert_limited(&api.get("/players/me", CAROL_SUB).await);
    for _ in 0..10 {
        let r = api.send(Method::GET, "/players/me", None, &[], None).await;
        assert_error(&r, 401, "missing_token");
        let r = api
            .send(Method::GET, "/players/me", Some("e30.e30.e30"), &[], None)
            .await;
        assert_error(&r, 401, "invalid_token");
    }
    for _ in 0..2 {
        assert_eq!(api.get("/players/me", ALICE_SUB).await.status, 200);
    }
}

/// The class is the method's, never the path's: a `POST` to any route taking `Player` spends the
/// write budget, and a `GET` the read budget (the lead's named mutant). The probes sit beside
/// the real router; they're outside the document, so they're called directly.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_budget_is_chosen_by_the_method(pool: PgPool) {
    use voidforge_server::accounts::Player;
    let clock = Stepped::new();
    let probe = axum::Router::new().route(
        "/probe",
        axum::routing::post(|_: Player| async { "posted" }).get(|_: Player| async { "got" }),
    );
    let api = api_with(
        &pool,
        world(0, 0),
        |_| {},
        Some(clock.limiter((1, 3), (1, 2))),
        Some(probe),
    )
    .await;
    let token = api.token(ALICE_SUB);
    let probe = |method: Method| {
        api.http
            .request(method, format!("{}/probe", api.base))
            .bearer_auth(&token)
            .send()
    };
    for _ in 0..2 {
        assert_eq!(probe(Method::POST).await.unwrap().status(), 200);
    }
    assert_eq!(probe(Method::POST).await.unwrap().status(), 429);
    // The write budget is the one /commands spends too.
    assert_limited(
        &api.command(ALICE_SUB, "k", &build_drill(api.ids.alice_home))
            .await,
    );
    for _ in 0..3 {
        assert_eq!(probe(Method::GET).await.unwrap().status(), 200);
    }
    assert_limited(&api.get("/players/me", ALICE_SUB).await);
}

/// HEAD is refused on every route as 404 `not_found`, before any extractor runs: it never joins
/// and never spends read budget (D-049 T3, D-055; review-r1's R1-G2). A HEAD response has no
/// body, so these are sent directly, outside the parity bookkeeping.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn head_is_not_found_and_runs_nothing(pool: PgPool) {
    let clock = Stepped::new();
    let api = api_with(
        &pool,
        world(1, 0),
        |_| {},
        Some(clock.limiter((1, 1), (1, 1))),
        None,
    )
    .await;
    let accounts = api.accounts().await;
    let paths = [
        format!("/planets/{}", api.ids.alice_home.0),
        "/planets?near=0,0,0&radius=1".to_owned(),
        "/players/me".to_owned(),
        "/players/me/fleets".to_owned(),
        "/leaderboard".to_owned(),
        "/rules".to_owned(),
        "/openapi.json".to_owned(),
        "/nothing".to_owned(),
    ];
    let gets = routes()
        .into_iter()
        .filter(|r| r.method == Method::GET)
        .count();
    assert_eq!(gets + 1, paths.len(), "every GET route, and one unknown");
    for sub in [CAROL_SUB, ALICE_SUB] {
        let token = api.token(sub);
        for path in &paths {
            for _ in 0..3 {
                let r = api
                    .http
                    .head(format!("{}{path}", api.base))
                    .bearer_auth(&token)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(r.status().as_u16(), 404, "HEAD {path}");
            }
        }
    }
    assert_eq!(api.accounts().await, accounts, "a HEAD made no account");
    assert_eq!(api.records().await, 0, "a HEAD joined nobody");
    // Alice's one read is still there: no HEAD spent it.
    assert_eq!(api.get("/players/me", ALICE_SUB).await.status, 200);
    assert_limited(&api.get("/players/me", ALICE_SUB).await);
}
