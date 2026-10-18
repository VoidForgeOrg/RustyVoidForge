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
    let rules = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules/v0.toml");
    let out = Command::new(BIN)
        .args([
            "seed",
            "--seed",
            &SEED.to_string(),
            "--players",
            &PLAYERS.to_string(),
        ])
        .args(["--rules", rules.to_str().unwrap(), "--json"])
        .env("DATABASE_URL", url(pool))
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap();
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
