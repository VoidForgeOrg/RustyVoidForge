//! The universe bootstrap against Postgres (Technical Plan P2.9; decision log D-043): seeding
//! an empty database always gives the same state hash, a database holding a universe is never
//! overwritten, and the host recovers from the seeded snapshot and serves. These need Postgres,
//! so they are `#[ignore]`d outside the database stage (`just db-test`, the CI `database` job).

use std::path::Path;
use std::process::Command;

use serde_json::Value;
use sqlx::PgPool;
use voidforge_core::command::{Action, Command as GameCommand};
use voidforge_host::{HostConfig, ManualClock};
use voidforge_store::MIGRATOR;
use voidforge_store::log::{LogStore, PgLog};
use voidforge_store::snapshot::{self, ENGINE_BUILD, EngineBuild};
use voidforge_store::verify::hex;
use voidforge_testkit::{ALICE, v0_rules};

const BIN: &str = env!("CARGO_BIN_EXE_voidforge-server");

/// `just seed`'s fixed seed and size (D-043).
const SEED: u64 = 20_261_018;
const PLAYERS: u32 = 100;
/// Its state hash under Rules v0: the done-when's "always the same", pinned. A change to world
/// generation or the World's encoding changes it, deliberately.
const SEED_HASH: &str = "63185376ae382891972632b6f15eb52cf87816dbce27a20a48c2efa23ea487f5";

fn url(pool: &PgPool) -> String {
    let base = std::env::var("DATABASE_URL").expect("the database stage sets DATABASE_URL");
    let db = pool.connect_options().get_database().unwrap().to_owned();
    format!("{}/{db}", base.rsplit_once('/').unwrap().0)
}

fn seed(pool: &PgPool) -> (i32, Value) {
    finish(seed_process(pool).wait_with_output().unwrap())
}

/// `seed` started, not yet finished.
fn seed_process(pool: &PgPool) -> std::process::Child {
    let rules = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules/v0.toml");
    Command::new(BIN)
        .args([
            "seed",
            "--seed",
            &SEED.to_string(),
            "--players",
            &PLAYERS.to_string(),
        ])
        .args(["--rules", rules.to_str().unwrap(), "--json"])
        .env("VOIDFORGE_DATABASE_URL", url(pool))
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}

fn finish(out: std::process::Output) -> (i32, Value) {
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(!stdout.contains("password"), "the URL is never printed");
    (
        out.status.code().unwrap(),
        serde_json::from_str(&stdout).unwrap(),
    )
}

/// The hash `generate` gives in-process for the same seed, size and rules.
fn expected_hash() -> String {
    let world = voidforge_worldgen::generate(SEED, PLAYERS, &v0_rules()).unwrap();
    hex(&snapshot::state_hash(&world).unwrap())
}

/// The done-when, on one fresh database (the next test is another).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn seeding_an_empty_database_gives_the_fixed_state_hash(pool: PgPool) {
    let (code, v) = seed(&pool);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["state_hash"].as_str(), Some(SEED_HASH), "{v}");
    assert_eq!(v["state_hash"].as_str(), Some(expected_hash().as_str()));
    assert_eq!(v["planets"], 2_000);
    // Exactly one snapshot, at seq 0, of this build, and no records.
    let mut pg = PgLog::new(pool.clone());
    let index = pg.snapshot_index().await.unwrap();
    assert_eq!(index.len(), 1);
    assert_eq!((index[0].seq, index[0].game_time), (0, 0));
    assert_eq!(index[0].engine_build, ENGINE_BUILD);
    assert_eq!(hex(&index[0].state_hash), SEED_HASH);
    assert!(pg.records().await.unwrap().is_empty());
}

/// The done-when again, on a second fresh database, and then the refusal: a database holding a
/// universe is never overwritten, and the refusal names that universe's hash.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_second_seed_is_refused_and_changes_nothing(pool: PgPool) {
    let (code, v) = seed(&pool);
    assert_eq!((code, v["state_hash"].as_str()), (0, Some(SEED_HASH)));
    let before = PgLog::new(pool.clone())
        .snapshot_at(0)
        .await
        .unwrap()
        .unwrap();
    let (code, v) = seed(&pool);
    assert_eq!(code, 3, "{v}");
    assert_eq!(v["result"], "refused");
    assert_eq!(v["existing_state_hash"].as_str(), Some(SEED_HASH));
    let mut pg = PgLog::new(pool.clone());
    assert_eq!(pg.snapshot_index().await.unwrap().len(), 1);
    assert_eq!(pg.snapshot_at(0).await.unwrap().unwrap(), before);
}

/// D-043's carry, ruled in D-047: accounts without a universe mean the universe was lost, so
/// `seed` refuses (exit 3, naming the count) and writes nothing, and the accounts stay.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn an_accounts_only_database_is_refused_and_changes_nothing(pool: PgPool) {
    for subject in ["alice", "bob"] {
        sqlx::query(
            "INSERT INTO accounts (player_id, issuer, subject) \
             VALUES (uuidv7(), 'http://localhost:5556/dex', $1)",
        )
        .bind(subject)
        .execute(&pool)
        .await
        .unwrap();
    }
    let (code, v) = seed(&pool);
    assert_eq!(code, 3, "{v}");
    assert_eq!(v["result"], "refused");
    let error = v["error"].as_str().unwrap();
    assert!(error.contains("2 accounts"), "{error}");
    assert!(error.contains("clear them by hand"), "{error}");
    assert!(v["existing_state_hash"].is_null(), "{v}");
    let mut pg = PgLog::new(pool.clone());
    assert!(pg.snapshot_index().await.unwrap().is_empty());
    assert!(pg.records().await.unwrap().is_empty());
    let rows = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM accounts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 2, "nothing deleted");
}

/// Every table outside the system schemas, and the migrations sqlx recorded (none without its
/// table): what a refused seed must leave exactly as it found.
async fn schema(pool: &PgPool) -> (Vec<String>, Vec<i64>) {
    let tables = sqlx::query_scalar::<_, String>(
        "SELECT schemaname || '.' || tablename FROM pg_tables \
         WHERE schemaname NOT IN ('pg_catalog', 'information_schema') ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let applied = if tables.iter().any(|t| t == "public._sqlx_migrations") {
        sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations ORDER BY 1")
            .fetch_all(pool)
            .await
            .unwrap()
    } else {
        Vec::new()
    };
    (tables, applied)
}

/// D-055 (review-r1's R1-G3): an unmigrated database that isn't empty is refused **before
/// anything is migrated**. Before the fix, seed migrated it, found no universe, and seeded beside
/// the stranger's table.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn an_unmigrated_database_with_other_tables_is_refused_and_gets_no_schema(pool: PgPool) {
    sqlx::query("CREATE TABLE notes (body text)")
        .execute(&pool)
        .await
        .unwrap();
    let before = schema(&pool).await;
    assert_eq!(before, (vec!["public.notes".to_owned()], vec![]));
    let (code, v) = seed(&pool);
    assert_eq!(code, 3, "{v}");
    assert_eq!(v["result"], "refused");
    let error = v["error"].as_str().unwrap();
    assert!(
        error.contains("1 tables that aren't a Voidforge universe's (for example public.notes)"),
        "{error}"
    );
    assert_eq!(schema(&pool).await, before, "nothing was migrated");
}

/// D-055: a refused seed writes no schema. An accounts-only database migrated only to 0003 (no
/// universe table yet) is refused before migrating: still no universe table, nothing recorded.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn a_refused_seed_on_a_partly_migrated_database_changes_no_schema(pool: PgPool) {
    for m in MIGRATOR.iter().filter(|m| m.version <= 3) {
        // The repository's own migration files, applied by hand to stop at 0003.
        sqlx::raw_sql(m.sql.clone()).execute(&pool).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO accounts (player_id, issuer, subject) \
         VALUES (uuidv7(), 'http://localhost:5556/dex', 'alice')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let before = schema(&pool).await;
    assert!(
        before.0.contains(&"public.accounts".to_owned()),
        "{before:?}"
    );
    assert!(
        !before.0.contains(&"public.universe".to_owned()),
        "{before:?}"
    );
    let (code, v) = seed(&pool);
    assert_eq!(code, 3, "{v}");
    let error = v["error"].as_str().unwrap();
    assert!(error.contains("1 accounts"), "{error}");
    assert_eq!(schema(&pool).await, before, "nothing was migrated");
}

/// D-038 after the bootstrap: the host recovers from the seq-0 snapshot, serves a Join game, and
/// recovers again with the player in its homeworld.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_host_recovers_from_the_seeded_universe_and_serves(pool: PgPool) {
    assert_eq!(seed(&pool).0, 0);
    let rules = v0_rules();
    let build = EngineBuild::new(ENGINE_BUILD).unwrap();
    let recovered = voidforge_host::recover(&mut PgLog::new(pool.clone()), &rules, &build)
        .await
        .unwrap();
    assert_eq!((recovered.snapshot_seq, recovered.last_seq), (0, 0));
    assert!(recovered.world.players.is_empty());
    let options = (*pool.connect_options()).clone();
    let epoch = 1_760_000_000_000;
    let (handle, task) = voidforge_host::spawn(
        recovered.world,
        rules.clone(),
        move || PgLog::lazy(options),
        ManualClock::new(epoch),
        HostConfig::new(epoch, build.clone()),
    )
    .unwrap();
    let receipt = handle
        .submit(GameCommand {
            player: ALICE,
            action: Action::JoinGame,
        })
        .await
        .unwrap();
    assert!(receipt.outcome.is_ok(), "{receipt:?}");
    drop(handle);
    task.join().unwrap();
    let again = voidforge_host::recover(&mut PgLog::new(pool.clone()), &rules, &build)
        .await
        .unwrap();
    assert!(again.world.players.contains_key(&ALICE));
}

/// The refusal is sound under a race: `seed` locks `accounts` with the universe's tables, so an
/// account inserted but not yet committed when seed starts makes seed wait, then refuse, rather
/// than check a count that can't see it and seed beside it (D-047).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn an_account_committed_during_seed_is_seen_and_refused(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO accounts (player_id, issuer, subject) \
         VALUES (uuidv7(), 'http://localhost:5556/dex', 'alice')",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    let mut child = seed_process(&pool);
    // Seed must block on the lock while the insert is open; a seed that finishes first didn't.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let waiting = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if waiting > 0 {
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("seed finished ({status}) without waiting for the open account insert");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "seed never reached the lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    tx.commit().await.unwrap();
    let out = tokio::task::spawn_blocking(move || child.wait_with_output().unwrap());
    let (code, v) = finish(
        tokio::time::timeout(std::time::Duration::from_secs(60), out)
            .await
            .expect("seed finishes once the insert commits")
            .unwrap(),
    );
    assert_eq!(code, 3, "{v}");
    assert!(v["error"].as_str().unwrap().contains("1 accounts"), "{v}");
    assert!(
        PgLog::new(pool.clone())
            .snapshot_index()
            .await
            .unwrap()
            .is_empty()
    );
}
