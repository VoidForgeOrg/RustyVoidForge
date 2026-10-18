//! The replay CLI on a Postgres log (Technical Plan P2.7; decision log D-041): a golden loaded into
//! Postgres, with snapshots as hash checkpoints, verifies; a tampered row or checkpoint is
//! pinpointed; another build is refused. These need Postgres, so they are `#[ignore]`d outside the
//! database stage (`just db-test`, the CI `database` job).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sqlx::PgPool;
use voidforge_store::MIGRATOR;
use voidforge_store::golden::{Files, fixture};
use voidforge_store::log::{LogStore, PgLog, SnapshotRow};
use voidforge_store::replay;
use voidforge_store::snapshot::{self, Expected};

const BIN: &str = env!("CARGO_BIN_EXE_voidforge-replay");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rules_file() -> String {
    repo().join("rules/v0.toml").to_str().unwrap().to_owned()
}

fn golden(name: &str) -> Files {
    let dir = repo().join("tests/golden").join(name);
    let read = |f: &str| std::fs::read(dir.join(f)).unwrap();
    Files {
        start: read("start.vfsnap"),
        records: read("records.jsonl"),
        manifest: read("manifest.json"),
    }
}

/// The test database's URL: the stage's `DATABASE_URL` with this test's database.
fn url(pool: &PgPool) -> String {
    let base = std::env::var("DATABASE_URL").expect("the database stage sets DATABASE_URL");
    let db = pool.connect_options().get_database().unwrap().to_owned();
    let (server, _) = base.rsplit_once('/').unwrap();
    format!("{server}/{db}")
}

/// Loads a golden into Postgres: its start as the snapshot at seq 0, its records, and snapshots
/// after records 4 and 8 (the log's hash checkpoints).
async fn load(pool: &PgPool, name: &str) {
    let files = golden(name);
    let rules = voidforge_rules::parse(&std::fs::read_to_string(rules_file()).unwrap()).unwrap();
    let build = fixture();
    let mut pg = PgLog::new(pool.clone());
    let start = snapshot::decode(
        &files.start,
        &Expected {
            rules_version: rules.version,
            engine_build: &build,
        },
    )
    .unwrap();
    pg.write_snapshot(&SnapshotRow::new(
        0,
        &snapshot::encode(&start.world, &build).unwrap(),
    ))
    .await
    .unwrap();
    let records = files.records().unwrap();
    pg.append(&records).await.unwrap();
    let mut world = start.world;
    for r in &records {
        replay::replay_record(&mut world, r, &rules, &build).unwrap();
        if r.seq == 4 || r.seq == 8 {
            let snap = snapshot::encode(&world, &build).unwrap();
            pg.write_snapshot(&SnapshotRow::new(r.seq, &snap))
                .await
                .unwrap();
        }
    }
}

fn run(pool: &PgPool, args: &[&str]) -> (i32, String) {
    let out = Command::new(BIN)
        .args(args)
        .env("DATABASE_URL", url(pool))
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        !stdout.contains("password"),
        "the database URL is never printed"
    );
    (out.status.code().unwrap(), stdout)
}

fn replay(pool: &PgPool, extra: &[&str]) -> (i32, Value) {
    let rules = rules_file();
    let mut args = vec!["postgres", "--rules", rules.as_str(), "--json"];
    args.extend_from_slice(extra);
    let (code, out) = run(pool, &args);
    (code, serde_json::from_str(&out).unwrap())
}

const GOLDEN_BUILD: [&str; 2] = ["--build", "0.1.0+golden"];

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_postgres_log_verifies_with_its_snapshots_as_checkpoints(pool: PgPool) {
    load(&pool, "whole-loop").await;
    let (code, v) = replay(&pool, &GOLDEN_BUILD);
    assert_eq!(code, 0, "{v}");
    let s = &v["sources"][0];
    assert_eq!(s["result"], "verified");
    assert_eq!(s["source"], "postgres");
    assert_eq!(
        (s["build"].as_str(), s["build_from_flag"].as_bool()),
        (Some("0.1.0+golden"), Some(true))
    );
    assert_eq!(
        (s["records"].as_u64(), s["last_seq"].as_u64()),
        (Some(12), Some(12))
    );
    assert_eq!(
        s["checkpoints"].as_u64(),
        Some(3),
        "the start and the snapshots at 4 and 8"
    );
    // From a later snapshot, and up to a seq.
    let (code, v) = replay(
        &pool,
        &[
            GOLDEN_BUILD[0],
            GOLDEN_BUILD[1],
            "--from",
            "4",
            "--to",
            "10",
        ],
    );
    assert_eq!(code, 0, "{v}");
    let s = &v["sources"][0];
    assert_eq!(
        (s["first_seq"].as_u64(), s["last_seq"].as_u64()),
        (Some(5), Some(10))
    );
    // The text form says what a pass under --build means.
    let rules = rules_file();
    let (code, text) = run(
        &pool,
        &["postgres", "--rules", &rules, "--build", "0.1.0+golden"],
    );
    assert_eq!(code, 0);
    assert!(
        text.contains("asserted by --build: a pass means this binary reproduces the log"),
        "{text}"
    );
    // Byte-identical output.
    assert_eq!(
        run(
            &pool,
            &[
                "postgres",
                "--rules",
                &rules,
                "--build",
                "0.1.0+golden",
                "--json"
            ]
        ),
        run(
            &pool,
            &[
                "postgres",
                "--rules",
                &rules,
                "--build",
                "0.1.0+golden",
                "--json"
            ]
        )
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_tampered_postgres_log_is_pinpointed(pool: PgPool) {
    load(&pool, "whole-loop").await;
    // A row's events edited in place (two events swapped).
    sqlx::query!(
        "UPDATE log_records SET events = jsonb_build_array(events->1, events->0, events->2) WHERE seq = $1",
        10_i64
    )
    .execute(&pool)
    .await
    .unwrap();
    let (code, v) = replay(&pool, &GOLDEN_BUILD);
    let d = &v["sources"][0]["divergence"];
    assert_eq!(
        (code, d["seq"].as_i64(), d["what"].as_str()),
        (1, Some(10), Some("events"))
    );
    // From the snapshot at 4, with the row restored and the checkpoint at 8 edited.
    sqlx::query!(
        "UPDATE log_records SET events = jsonb_build_array(events->1, events->0, events->2) WHERE seq = $1",
        10_i64
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(replay(&pool, &GOLDEN_BUILD).0, 0, "restored");
    sqlx::query!(
        "UPDATE snapshots SET state_hash = $1 WHERE seq = $2",
        vec![0_u8; 32],
        8_i64
    )
    .execute(&pool)
    .await
    .unwrap();
    // The column no longer matches the snapshot's own header: a divergence at that snapshot
    // (a valid snapshot of another state is `state_hash`; see store's tests/verify.rs).
    let (code, v) = replay(&pool, &GOLDEN_BUILD);
    let d = &v["sources"][0]["divergence"];
    assert_eq!(
        (code, d["seq"].as_i64(), d["what"].as_str()),
        (1, Some(8), Some("snapshot"))
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn another_build_is_refused(pool: PgPool) {
    load(&pool, "start-and-build").await;
    // This binary's own build is not the log's: the start snapshot is refused.
    let (code, v) = replay(&pool, &[]);
    let s = &v["sources"][0];
    assert_eq!((code, s["result"].as_str()), (3, Some("refused")));
    assert_eq!(s["divergence"]["what"], "build");
    assert_eq!(s["build_from_flag"], false);
    // Another build's record in the tail.
    sqlx::query!(
        "UPDATE log_records SET engine_build = $1 WHERE seq = $2",
        "0.1.0+other",
        5_i64
    )
    .execute(&pool)
    .await
    .unwrap();
    let (code, v) = replay(&pool, &GOLDEN_BUILD);
    let d = &v["sources"][0]["divergence"];
    assert_eq!(
        (code, d["seq"].as_i64(), d["what"].as_str()),
        (3, Some(5), Some("build"))
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn inspect_postgres_reports_unverified_metadata(pool: PgPool) {
    load(&pool, "whole-loop").await;
    let (code, out) = run(&pool, &["inspect", "postgres", "--json"]);
    assert_eq!(code, 0, "{out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["result"], "unverified");
    let s = &v["sources"][0];
    assert_eq!(s["verified"], false);
    assert_eq!(s["snapshots"].as_array().unwrap().len(), 3);
    assert_eq!(s["records"]["count"], 12);
    assert_eq!(s["records"]["engine_builds"][0], "0.1.0+golden");
}

/// dev-qa's F1 and F2 on c29210c, through the binary: a shutdown snapshot after an unlogged
/// eventless advance verifies; a snapshot past the head is refused as a divergence.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn later_snapshots_are_checked_at_their_own_time(pool: PgPool) {
    load(&pool, "whole-loop").await;
    let files = golden("whole-loop");
    let rules = voidforge_rules::parse(&std::fs::read_to_string(rules_file()).unwrap()).unwrap();
    let build = fixture();
    let mut world = snapshot::decode(
        &files.start,
        &Expected {
            rules_version: rules.version,
            engine_build: &build,
        },
    )
    .unwrap()
    .world;
    let records = files.records().unwrap();
    for r in &records {
        replay::replay_record(&mut world, r, &rules, &build).unwrap();
    }
    let last = records.last().unwrap();
    let events = world
        .advance_to(last.game_time + 10 * 3_600_000, &rules)
        .unwrap();
    assert!(events.is_empty());
    let mut pg = PgLog::new(pool.clone());
    let shutdown = snapshot::encode(&world, &build).unwrap();
    pg.write_snapshot(&SnapshotRow::new(last.seq, &shutdown))
        .await
        .unwrap();
    let (code, v) = replay(&pool, &GOLDEN_BUILD);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["sources"][0]["checkpoints"].as_u64(), Some(4));
    pg.write_snapshot(&SnapshotRow::new(last.seq + 1, &shutdown))
        .await
        .unwrap();
    let (code, v) = replay(&pool, &GOLDEN_BUILD);
    let d = &v["sources"][0]["divergence"];
    assert_eq!(
        (code, d["seq"].as_i64(), d["what"].as_str()),
        (1, Some(last.seq + 1), Some("snapshot"))
    );
}
