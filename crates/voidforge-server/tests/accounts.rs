//! Accounts through the real host and Postgres (Technical Plan P3.3; decision log D-047): a new
//! dex user gets a homeworld exactly once, across repeats, concurrency, a stopped or busy host,
//! restarts, and a crash at every point between the account row and the join.
//!
//! Every database test checks effects, never only answers: the `accounts` rows, the accepted
//! `join_game` records per player, the total record count (so "nothing appended" is checked),
//! and the World recovered from Postgres. The crash tests re-run this binary as a child (the
//! ignored `accounts_child`) that recovers, serves one call and aborts at a named kill point, in
//! the test's own store wrappers: no production code has a hook (as D-042).
//!
//! The database tests need Postgres, so they are `#[ignore]`d outside the database stage
//! (`just db-test`, the CI `database` job).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::unix::process::ExitStatusExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::IntoResponse;
use sqlx::PgPool;
use tokio::sync::{Semaphore, watch};
use voidforge_api_types::error::ErrorCode;
use voidforge_host::{
    Action, Command, HostConfig, HostHandle, HostTask, ManualClock, PlayerId, Rejection,
};
use voidforge_server::accounts::{Player, PlayerError, Players};
use voidforge_server::auth::Principal;
use voidforge_store::MIGRATOR;
use voidforge_store::accounts::{AccountError, AccountStore, PgAccounts};
use voidforge_store::log::{
    IdempotencyKey, LogError, LogRecord, LogStore, Outcome, Payload, PgLog, SnapshotMeta,
    SnapshotRow,
};
use voidforge_store::snapshot::{self, EngineBuild};
use voidforge_testkit::v0_rules;

const ISSUER: &str = "http://localhost:5556/dex";
const OTHER_ISSUER: &str = "https://accounts.example.com";
const EPOCH: i64 = 1_760_000_000_000;
const SEED: u64 = 33;
/// The longest any one wait may take before the test fails instead of hanging.
const PATIENCE: Duration = Duration::from_secs(30);

fn principal(issuer: &str, subject: &str) -> Principal {
    Principal {
        issuer: issuer.to_owned(),
        subject: subject.to_owned(),
    }
}

fn build() -> EngineBuild {
    EngineBuild::new("accounts-test").unwrap()
}

fn url(pool: &PgPool) -> String {
    let base = std::env::var("DATABASE_URL").expect("the database stage sets DATABASE_URL");
    let db = pool.connect_options().get_database().unwrap().to_owned();
    format!("{}/{db}", base.rsplit_once('/').unwrap().0)
}

/// Seeds a universe with `homeworlds` free candidates, as `seed` does.
async fn universe(url: &str, homeworlds: u32) {
    let world = voidforge_worldgen::generate(SEED, homeworlds, &v0_rules()).unwrap();
    assert_eq!(world.homeworld_candidates.len(), homeworlds as usize);
    let snap = snapshot::encode(&world, &build()).unwrap();
    voidforge_store::bootstrap(url, &snap).await.unwrap();
}

// --- Fault injection, in the test's wrappers only ---

/// What a test does to the host's log store.
struct Faults {
    /// Every append fails, so the host stops (`Fatal::Append`).
    fail: AtomicBool,
    /// Appends wait at the gate while this is set.
    hold: AtomicBool,
    gate: Semaphore,
    entered: watch::Sender<u64>,
    /// In a child: abort at this kill point.
    kill: Option<String>,
}

impl Default for Faults {
    fn default() -> Self {
        Self::killing(None)
    }
}

impl Faults {
    fn killing(point: Option<String>) -> Self {
        Self {
            fail: AtomicBool::new(false),
            hold: AtomicBool::new(false),
            gate: Semaphore::new(0),
            entered: watch::Sender::new(0),
            kill: point,
        }
    }

    fn kill_at(&self, point: &str) {
        if self.kill.as_deref() == Some(point) {
            println!("@accounts killed {point}");
            std::io::stdout().flush().unwrap();
            std::process::abort();
        }
    }

    async fn entered(&self, n: u64) {
        let mut rx = self.entered.subscribe();
        tokio::time::timeout(PATIENCE, rx.wait_for(|e| *e >= n))
            .await
            .expect("the host reached the append")
            .unwrap();
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
            self.faults
                .gate
                .acquire()
                .await
                .expect("the gate stays open")
                .forget();
        }
        if self.faults.fail.load(Ordering::SeqCst) {
            return Err(LogError::Memory("injected append failure".to_owned()));
        }
        self.faults.kill_at("append.before");
        self.inner.append(records).await?;
        self.faults.kill_at("append.after");
        Ok(())
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

/// The account store, with kill points either side of the lookup.
struct FaultyAccounts {
    inner: PgAccounts,
    faults: Arc<Faults>,
}

impl AccountStore for FaultyAccounts {
    async fn lookup_or_create(
        &self,
        issuer: &str,
        subject: &str,
        fresh: PlayerId,
    ) -> Result<PlayerId, AccountError> {
        self.faults.kill_at("account.before");
        let id = self.inner.lookup_or_create(issuer, subject, fresh).await?;
        self.faults.kill_at("account.after");
        Ok(id)
    }
}

// --- A running server's account path ---

struct Running {
    players: Arc<Players<FaultyAccounts>>,
    handle: HostHandle,
    task: HostTask,
    faults: Arc<Faults>,
}

/// Recovers the universe from Postgres and starts a host on it, as the server does.
async fn start(url: &str, faults: Faults, config: HostConfig) -> Running {
    let rules = v0_rules();
    let mut log = PgLog::lazy_url(url).unwrap();
    let recovered = voidforge_host::recover(&mut log, &rules, &build())
        .await
        .unwrap();
    let faults = Arc::new(faults);
    let (f, u) = (faults.clone(), url.to_owned());
    let (handle, task) = voidforge_host::spawn(
        recovered.world,
        rules,
        move || FaultyLog {
            inner: PgLog::lazy_url(&u).unwrap(),
            faults: f,
        },
        ManualClock::new(EPOCH),
        config,
    )
    .unwrap();
    let pool = PgPool::connect(url).await.unwrap();
    let accounts = FaultyAccounts {
        inner: PgAccounts::new(pool),
        faults: faults.clone(),
    };
    Running {
        players: Arc::new(Players::new(accounts, handle.clone())),
        handle,
        task,
        faults,
    }
}

impl Running {
    async fn player(&self, p: &Principal) -> Result<Player, PlayerError> {
        tokio::time::timeout(PATIENCE, self.players.player(p))
            .await
            .expect("the account path answers")
    }

    /// Stops the host and returns how it ended. The host stops once every handle is gone, so a
    /// handle a test still holds fails here within `PATIENCE` rather than hanging the run.
    async fn stop(self) -> Result<(), voidforge_host::Fatal> {
        drop(self.players);
        drop(self.handle);
        let task = self.task;
        tokio::time::timeout(PATIENCE, tokio::task::spawn_blocking(move || task.join()))
            .await
            .expect("the host stops once its handles are gone")
            .unwrap()
    }
}

// --- Effects ---

#[derive(Debug, Default, PartialEq, Eq)]
struct Effects {
    /// `accounts` rows.
    rows: i64,
    /// Every log record.
    records: usize,
    /// Accepted Join game records, per player.
    joins: BTreeMap<PlayerId, usize>,
    /// Rejected Join game records, per reason.
    refusals: BTreeMap<&'static str, usize>,
}

async fn effects(url: &str) -> Effects {
    let pool = PgPool::connect(url).await.unwrap();
    let rows = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM accounts")
        .fetch_one(&pool)
        .await
        .unwrap();
    let records = PgLog::new(pool).records().await.unwrap();
    let mut e = Effects {
        rows,
        records: records.len(),
        ..Effects::default()
    };
    for r in records {
        if let Payload::Command(Command {
            player,
            action: Action::JoinGame,
        }) = r.payload
        {
            match r.outcome {
                Outcome::Accepted { .. } => *e.joins.entry(player).or_default() += 1,
                Outcome::Rejected(why) => *e.refusals.entry(why.code()).or_default() += 1,
                Outcome::Empty => panic!("a command record without an outcome"),
            }
        }
    }
    e
}

/// The players in the World recovered from Postgres, each with their homeworld.
async fn recovered_players(url: &str) -> BTreeMap<PlayerId, u64> {
    let mut log = PgLog::lazy_url(url).unwrap();
    let world = voidforge_host::recover(&mut log, &v0_rules(), &build())
        .await
        .unwrap()
        .world;
    world
        .players
        .values()
        .map(|p| (p.id, p.homeworld.0))
        .collect()
}

async fn account_id(url: &str, issuer: &str, subject: &str) -> Option<String> {
    let pool = PgPool::connect(url).await.unwrap();
    sqlx::query_scalar::<_, String>(
        "SELECT player_id::text FROM accounts WHERE issuer = $1 AND subject = $2",
    )
    .bind(issuer)
    .bind(subject)
    .fetch_optional(&pool)
    .await
    .unwrap()
}

fn uuid_text(id: PlayerId) -> String {
    voidforge_store::log::player_uuid(id)
}

// --- The tests ---

/// The done-when: a new principal gets one row and one homeworld, under a UUIDv7.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_new_principal_gets_one_account_and_one_homeworld(pool: PgPool) {
    let url = url(&pool);
    universe(&url, 4).await;
    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    let alice = s.player(&principal(ISSUER, "alice")).await.unwrap();
    assert_eq!(uuid::Uuid::from_u128(alice.id.0).get_version_num(), 7);
    s.stop().await.unwrap();
    let e = effects(&url).await;
    assert_eq!(e.rows, 1);
    assert_eq!(e.joins, BTreeMap::from([(alice.id, 1)]));
    assert_eq!(e.refusals, BTreeMap::new());
    assert_eq!(
        account_id(&url, ISSUER, "alice").await,
        Some(uuid_text(alice.id))
    );
    assert!(recovered_players(&url).await.contains_key(&alice.id));
}

/// The same principal again is the same player, and nothing is written: no row, no record.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_same_principal_again_is_the_same_player_and_writes_nothing(pool: PgPool) {
    let url = url(&pool);
    universe(&url, 4).await;
    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    let p = principal(ISSUER, "alice");
    let first = s.player(&p).await.unwrap();
    let before = effects(&url).await;
    for _ in 0..5 {
        assert_eq!(s.player(&p).await.unwrap(), first);
    }
    assert_eq!(effects(&url).await, before, "nothing appended or inserted");
    s.stop().await.unwrap();
}

/// Many first calls at once, for each of several principals: one row, one homeworld and one ID
/// per principal. Repeated, so the race between the read-model check and the join is exercised:
/// the core's `already_joined` must turn up, and must count as joined.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn concurrent_first_calls_end_with_one_account_and_one_homeworld(pool: PgPool) {
    const PRINCIPALS: usize = 12;
    const CALLS: usize = 32;
    let url = url(&pool);
    universe(&url, PRINCIPALS as u32).await;
    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    let mut ids = Vec::new();
    for n in 0..PRINCIPALS {
        let p = principal(ISSUER, &format!("racer-{n}"));
        let mut calls = tokio::task::JoinSet::new();
        for _ in 0..CALLS {
            let (players, p) = (s.players.clone(), p.clone());
            calls.spawn(async move { players.player(&p).await });
        }
        let answers = tokio::time::timeout(PATIENCE, calls.join_all())
            .await
            .expect("every call answers");
        let first = answers[0].as_ref().unwrap().id;
        for a in &answers {
            assert_eq!(a.as_ref().unwrap().id, first, "one ID for one principal");
        }
        ids.push(first);
    }
    s.stop().await.unwrap();
    let e = effects(&url).await;
    assert_eq!(e.rows, PRINCIPALS as i64);
    assert_eq!(e.joins, ids.iter().map(|id| (*id, 1)).collect());
    let raced = e.refusals.get("already_joined").copied().unwrap_or(0);
    assert!(
        raced > 0,
        "the race never reached the core, so already_joined went untested: {e:?}"
    );
    assert_eq!(e.refusals.len(), 1, "no other refusal: {e:?}");
    assert_eq!(recovered_players(&url).await.len(), PRINCIPALS);
}

/// A full universe: the next principal is refused with `no_free_homeworld`, keeps its row, and
/// asking again appends nothing.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_full_universe_refuses_keeps_the_account_and_appends_nothing(pool: PgPool) {
    let url = url(&pool);
    universe(&url, 2).await;
    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    for name in ["alice", "bob"] {
        s.player(&principal(ISSUER, name)).await.unwrap();
    }
    let before = effects(&url).await;
    let carol = principal(ISSUER, "carol");
    for _ in 0..3 {
        let err = s.player(&carol).await.unwrap_err();
        assert!(
            matches!(err, PlayerError::Refused(Rejection::NoFreeHomeworld)),
            "{err:?}"
        );
        assert_eq!(err.code(), ErrorCode::NoFreeHomeworld);
    }
    s.stop().await.unwrap();
    let after = effects(&url).await;
    assert_eq!(after.rows, before.rows + 1, "carol's account stays");
    assert!(account_id(&url, ISSUER, "carol").await.is_some());
    assert_eq!(after.records, before.records, "a refusal appends nothing");
    assert_eq!(recovered_players(&url).await.len(), 2);
}

/// Racing for the last homeworlds: some refusals come from the core's receipt, not the read
/// model, and every one is a refusal, never a join.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn racing_for_the_last_homeworlds_refuses_the_rest_through_the_core(pool: PgPool) {
    const HOMEWORLDS: usize = 4;
    const PRINCIPALS: usize = 16;
    let url = url(&pool);
    universe(&url, HOMEWORLDS as u32).await;
    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    let mut calls = tokio::task::JoinSet::new();
    for n in 0..PRINCIPALS {
        let (players, p) = (s.players.clone(), principal(ISSUER, &format!("late-{n}")));
        calls.spawn(async move { players.player(&p).await });
    }
    let answers = tokio::time::timeout(PATIENCE, calls.join_all())
        .await
        .expect("every call answers");
    s.stop().await.unwrap();
    let joined: Vec<PlayerId> = answers
        .iter()
        .filter_map(|a| a.as_ref().ok().map(|p| p.id))
        .collect();
    assert_eq!(joined.len(), HOMEWORLDS, "{answers:?}");
    for a in &answers {
        if let Err(e) = a {
            assert!(
                matches!(e, PlayerError::Refused(Rejection::NoFreeHomeworld)),
                "{e:?}"
            );
        }
    }
    let e = effects(&url).await;
    assert_eq!(e.rows, PRINCIPALS as i64);
    assert_eq!(e.joins, joined.iter().map(|id| (*id, 1)).collect());
    assert!(
        e.refusals.get("no_free_homeworld").copied().unwrap_or(0) > 0,
        "no refusal came from the core, so the receipt's refusal went untested: {e:?}"
    );
    let players = recovered_players(&url).await;
    assert_eq!(players.keys().copied().collect::<Vec<_>>(), {
        let mut j = joined.clone();
        j.sort();
        j
    });
}

/// The issuer is part of the identity: one subject under two issuers is two players.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn two_issuers_with_one_subject_are_two_players(pool: PgPool) {
    let url = url(&pool);
    universe(&url, 4).await;
    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    let a = s.player(&principal(ISSUER, "same")).await.unwrap();
    let b = s.player(&principal(OTHER_ISSUER, "same")).await.unwrap();
    s.stop().await.unwrap();
    assert_ne!(a, b);
    let e = effects(&url).await;
    assert_eq!(e.rows, 2);
    assert_eq!(e.joins, BTreeMap::from([(a.id, 1), (b.id, 1)]));
    let players = recovered_players(&url).await;
    assert_ne!(players[&a.id], players[&b.id], "two homeworlds");
}

/// An account with no homeworld (a crash after the row, before the join) is repaired by that
/// user's next call, under the same ID.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn an_account_without_a_homeworld_joins_on_the_next_call(pool: PgPool) {
    let url = url(&pool);
    universe(&url, 4).await;
    let orphan = PlayerId(uuid::Uuid::now_v7().as_u128());
    let stored = PgAccounts::new(PgPool::connect(&url).await.unwrap())
        .lookup_or_create(ISSUER, "alice", orphan)
        .await
        .unwrap();
    assert_eq!(stored, orphan);
    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    let alice = s.player(&principal(ISSUER, "alice")).await.unwrap();
    s.stop().await.unwrap();
    assert_eq!(alice.id, orphan);
    let e = effects(&url).await;
    assert_eq!((e.rows, e.joins), (1, BTreeMap::from([(orphan, 1)])));
}

/// A host that stops under the join answers `unavailable`, never joined, and keeps the row; the
/// next host joins the same player once.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_stopped_host_is_unavailable_and_the_next_host_joins_once(pool: PgPool) {
    let url = url(&pool);
    universe(&url, 4).await;
    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    s.faults.fail.store(true, Ordering::SeqCst);
    let p = principal(ISSUER, "alice");
    let err = s.player(&p).await.unwrap_err();
    assert!(matches!(err, PlayerError::Stopped), "{err:?}");
    assert_eq!(err.code(), ErrorCode::Unavailable);
    let again = s.player(&p).await.unwrap_err();
    assert!(matches!(again, PlayerError::Stopped), "{again:?}");
    assert!(
        matches!(s.stop().await, Err(voidforge_host::Fatal::Append(_))),
        "the host stopped on the failed append"
    );
    let e = effects(&url).await;
    assert_eq!((e.rows, e.records), (1, 0), "the row stays; no record");
    let id = account_id(&url, ISSUER, "alice").await.unwrap();

    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    let alice = s.player(&p).await.unwrap();
    s.stop().await.unwrap();
    assert_eq!(uuid_text(alice.id), id);
    assert_eq!(effects(&url).await.joins, BTreeMap::from([(alice.id, 1)]));
}

/// A full inbox answers `busy`, never joined, and keeps the row.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_busy_host_is_busy_and_never_joined(pool: PgPool) {
    // On a current-thread runtime, a yield runs every ready task to its next await, so the
    // filler below is in the inbox before the account path tries it.
    assert_eq!(
        tokio::runtime::Handle::current().runtime_flavor(),
        tokio::runtime::RuntimeFlavor::CurrentThread
    );
    let url = url(&pool);
    universe(&url, 4).await;
    let config = HostConfig {
        inbox_capacity: 1,
        max_batch: 1,
        ..HostConfig::new(EPOCH, build())
    };
    let s = start(&url, Faults::default(), config).await;
    s.faults.hold.store(true, Ordering::SeqCst);
    let join = |n: u128| {
        let handle = s.handle.clone();
        tokio::spawn(async move {
            handle
                .submit(Command {
                    player: PlayerId(n),
                    action: Action::JoinGame,
                })
                .await
        })
    };
    // One join held inside its append, one waiting in the inbox: the inbox is full.
    let held = join(1);
    s.faults.entered(1).await;
    let waiting = join(2);
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished(), "the filler waits in the inbox");
    let err = s.player(&principal(ISSUER, "alice")).await.unwrap_err();
    assert!(matches!(err, PlayerError::Busy), "{err:?}");
    assert_eq!(err.code(), ErrorCode::Busy);
    s.faults.hold.store(false, Ordering::SeqCst);
    s.faults.gate.add_permits(2);
    assert!(held.await.unwrap().unwrap().outcome.is_ok());
    assert!(waiting.await.unwrap().unwrap().outcome.is_ok());
    let alice = s.player(&principal(ISSUER, "alice")).await.unwrap();
    s.stop().await.unwrap();
    let e = effects(&url).await;
    assert_eq!(e.rows, 1, "alice's row, kept through busy");
    assert_eq!(e.joins.len(), 3);
    assert_eq!(e.joins.get(&alice.id), Some(&1));
}

/// After a restart, the recovered host knows the player: the same ID, no second join.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_restarted_server_knows_the_player(pool: PgPool) {
    let url = url(&pool);
    universe(&url, 4).await;
    let p = principal(ISSUER, "alice");
    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    let first = s.player(&p).await.unwrap();
    s.stop().await.unwrap();
    let before = effects(&url).await;
    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    assert_eq!(s.player(&p).await.unwrap(), first);
    s.stop().await.unwrap();
    assert_eq!(effects(&url).await, before);
}

/// A subject Postgres text can't hold is a permanent 500, never a 503 to retry forever, and
/// nothing is written.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_subject_that_cannot_be_stored_is_internal_not_retry(pool: PgPool) {
    let url = url(&pool);
    universe(&url, 4).await;
    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    let err = s.player(&principal(ISSUER, "nul\0byte")).await.unwrap_err();
    s.stop().await.unwrap();
    assert!(
        matches!(err, PlayerError::Store(AccountError::Unstorable)),
        "{err:?}"
    );
    let response = err.into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.headers().get("retry-after").is_none());
    assert_eq!(effects(&url).await, Effects::default());
}

/// Each error's answer: the code, its status, and Retry-After exactly on the 503s.
#[test]
fn each_error_answers_its_code_status_and_retry_after() {
    let cases = [
        (
            PlayerError::Refused(Rejection::NoFreeHomeworld),
            ErrorCode::NoFreeHomeworld,
            422,
            None,
        ),
        (PlayerError::Busy, ErrorCode::Busy, 503, Some("1")),
        (PlayerError::Stopped, ErrorCode::Unavailable, 503, Some("5")),
        (
            PlayerError::Store(AccountError::Vanished),
            ErrorCode::Unavailable,
            503,
            Some("5"),
        ),
        (
            PlayerError::Store(AccountError::Unstorable),
            ErrorCode::Internal,
            500,
            None,
        ),
        (PlayerError::Internal("x"), ErrorCode::Internal, 500, None),
    ];
    for (err, code, status, retry) in cases {
        assert_eq!(err.code(), code, "{err:?}");
        let response = err.into_response();
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(
            response
                .headers()
                .get("retry-after")
                .map(|v| v.to_str().unwrap().to_owned())
                .as_deref(),
            retry
        );
    }
}

/// `Principal`'s `Debug` never shows the subject.
#[test]
fn a_principal_debug_hides_the_subject() {
    let shown = format!("{:?}", principal(ISSUER, "secret-subject-123"));
    assert!(!shown.contains("secret-subject-123"), "{shown}");
    assert!(shown.contains(ISSUER), "{shown}");
}

// --- Privacy: the subject never reaches a log line ---

/// Everything `tracing` and `log` emit in this process, at every level.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        let line = format!(
            "LOG {} {}: {}\n",
            record.level(),
            record.target(),
            record.args()
        );
        self.0.lock().unwrap().extend_from_slice(line.as_bytes());
    }

    fn flush(&self) {}
}

/// The whole account path at TRACE, sqlx's own logging included, through every answer: a new
/// account, a repeat, a refusal in a full universe, a stopped host and an unstorable subject.
/// The subjects never appear; the player UUIDs do. (nextest runs each test in its own process,
/// so the global subscriber sees only this test.)
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_subject_is_never_logged_at_any_level(pool: PgPool) {
    let capture = Capture::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(capture.clone())
            .with_ansi(false)
            .finish(),
    )
    .unwrap();
    log::set_boxed_logger(Box::new(capture.clone())).unwrap();
    log::set_max_level(log::LevelFilter::Trace);

    let url = url(&pool);
    universe(&url, 1).await;
    let subjects = ["Cg1zdWJqZWN0LWFsaWNlEgVsb2NhbA", "Cg1zdWJqZWN0LWNhcm9s"];
    let s = start(&url, Faults::default(), HostConfig::new(EPOCH, build())).await;
    let alice = s.player(&principal(ISSUER, subjects[0])).await.unwrap();
    s.player(&principal(ISSUER, subjects[0])).await.unwrap();
    s.player(&principal(ISSUER, subjects[1])).await.unwrap_err();
    let unstorable = format!("{}\0", subjects[1]);
    s.player(&principal(ISSUER, &unstorable)).await.unwrap_err();
    s.faults.fail.store(true, Ordering::SeqCst);
    let _ = s.player(&principal(ISSUER, "Cg1zdWJqZWN0LWJvYg")).await;
    let _ = s.stop().await;

    let text = String::from_utf8_lossy(&capture.0.lock().unwrap()).into_owned();
    assert!(
        text.contains(&uuid::Uuid::from_u128(alice.id.0).to_string()),
        "the capture saw the account path"
    );
    // sqlx logs each statement's text (never its bound values) at DEBUG, through tracing.
    assert!(
        text.contains("sqlx::query")
            && text.contains("INSERT INTO accounts (player_id, issuer, subject)"),
        "the capture saw sqlx's own statements on the accounts path: {text}"
    );
    for subject in subjects.iter().copied().chain(["Cg1zdWJqZWN0LWJvYg"]) {
        assert!(
            !text.contains(subject),
            "a subject reached the logs:\n{text}"
        );
    }
}

// --- Crashes between the row and the join ---

const CHILD: &str = "VOIDFORGE_ACCOUNTS_CHILD";

/// Runs this binary's `accounts_child` against `url` with `kill` armed (or none); returns how
/// it ended and what it printed. A child that outlives `PATIENCE` is killed and fails the test.
fn child(url: &str, kill: Option<&str>) -> (std::process::ExitStatus, String) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["accounts_child", "--exact", "--ignored", "--nocapture"])
        .env(CHILD, kill.unwrap_or("none"))
        .env("VOIDFORGE_ACCOUNTS_URL", url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + PATIENCE;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("the child hung at kill point {kill:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut out).unwrap();
    (status, out)
}

/// The child: recovers, starts the host and serves one call for alice, aborting at the armed
/// kill point. Outside a crash test it does nothing.
#[test]
#[ignore = "run only as a crash test's child"]
fn accounts_child() {
    let Ok(kill) = std::env::var(CHILD) else {
        return;
    };
    let url = std::env::var("VOIDFORGE_ACCOUNTS_URL").unwrap();
    let kill = (kill != "none").then_some(kill);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let s = start(&url, Faults::killing(kill), HostConfig::new(EPOCH, build())).await;
            let alice = s.player(&principal(ISSUER, "alice")).await.unwrap();
            println!("@accounts ok {}", uuid_text(alice.id));
            s.stop().await.unwrap();
        });
}

/// A crash at each point between the row and the join, then a fresh server: one row, one
/// homeworld, and the player the survivor reports is the one in the World.
async fn crash_then_retry(pool: PgPool, point: &str, row_survives: bool) {
    let url = url(&pool);
    universe(&url, 4).await;
    let (status, out) = child(&url, Some(point));
    assert_eq!(status.signal(), Some(6), "aborted at {point}: {out}");
    assert!(out.contains(&format!("@accounts killed {point}")), "{out}");
    assert_eq!(
        account_id(&url, ISSUER, "alice").await.is_some(),
        row_survives,
        "the row after a crash at {point}"
    );
    let (status, out) = child(&url, None);
    assert!(status.success(), "{out}");
    let id = out
        .lines()
        .find_map(|l| l.split("@accounts ok ").nth(1))
        .unwrap_or_else(|| panic!("no answer: {out}"))
        .to_owned();
    assert_eq!(account_id(&url, ISSUER, "alice").await, Some(id.clone()));
    let e = effects(&url).await;
    assert_eq!(e.rows, 1);
    assert_eq!(e.joins.len(), 1, "one homeworld: {e:?}");
    let (player, joins) = e.joins.into_iter().next().unwrap();
    assert_eq!((uuid_text(player), joins), (id, 1));
    assert_eq!(
        recovered_players(&url)
            .await
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![player]
    );
}

/// Before the row exists: nothing survives, and the retry starts afresh.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_crash_before_the_account_row_then_a_retry_joins_once(pool: PgPool) {
    crash_then_retry(pool, "account.before", false).await;
}

/// After the row, before the join: the retry finds the row and joins it.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_crash_after_the_account_row_then_a_retry_joins_once(pool: PgPool) {
    crash_then_retry(pool, "account.after", true).await;
}

/// The join submitted, its append never made: the retry joins.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_crash_before_the_join_is_durable_then_a_retry_joins_once(pool: PgPool) {
    crash_then_retry(pool, "append.before", true).await;
}

/// The join durable, its reply lost: the retry sees the player and joins nothing.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_crash_after_the_join_is_durable_then_a_retry_joins_nothing_more(pool: PgPool) {
    crash_then_retry(pool, "append.after", true).await;
}
