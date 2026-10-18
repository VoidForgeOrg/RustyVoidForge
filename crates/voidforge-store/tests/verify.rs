//! `verify_log` over a store (Technical Plan P2.7; decision log D-041), on `MemoryLog`: a golden
//! loaded as a log with snapshots as checkpoints. dev-qa's P2.7 findings on c29210c are pinned
//! here: a snapshot taken after an unlogged eventless advance verifies (F1), and every later
//! snapshot is decoded strictly and refused when corrupt, past the head or inconsistent (F2).

use std::path::Path;

use voidforge_core::state::World;
use voidforge_store::golden::{Files, fixture};
use voidforge_store::log::{LogRecord, LogStore, MemoryLog, SnapshotRow};
use voidforge_store::replay;
use voidforge_store::snapshot::{self, Expected};
use voidforge_store::verify::{VerifyError, verify_log};
use voidforge_testkit::v0_rules;

const H: i64 = 3_600_000;

fn golden(name: &str) -> Files {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden")
        .join(name);
    let read = |f: &str| std::fs::read(dir.join(f)).unwrap();
    Files {
        start: read("start.vfsnap"),
        records: read("records.jsonl"),
        manifest: read("manifest.json"),
    }
}

/// The whole-loop golden as a log: its start at seq 0, its records, and the World after each.
struct Loaded {
    log: MemoryLog,
    records: Vec<LogRecord>,
    worlds: Vec<World>,
}

async fn load() -> Loaded {
    let files = golden("whole-loop");
    let rules = v0_rules();
    let build = fixture();
    let mut log = MemoryLog::new();
    let start = snapshot::decode(
        &files.start,
        &Expected {
            rules_version: rules.version,
            engine_build: &build,
        },
    )
    .unwrap()
    .world;
    log.write_snapshot(&row(0, &start)).await.unwrap();
    let records = files.records().unwrap();
    log.append(&records).await.unwrap();
    let mut world = start;
    let mut worlds = Vec::new();
    for r in &records {
        replay::replay_record(&mut world, r, &rules, &build).unwrap();
        worlds.push(world.clone());
    }
    Loaded {
        log,
        records,
        worlds,
    }
}

fn row(seq: i64, world: &World) -> SnapshotRow {
    SnapshotRow::new(seq, &snapshot::encode(world, &fixture()).unwrap())
}

async fn verify(log: &mut MemoryLog) -> Result<(i64, usize), VerifyError> {
    verify_log(log, &v0_rules(), &fixture(), None, None)
        .await
        .map(|r| (r.last_seq, r.checkpoints))
}

fn finding(e: VerifyError) -> (Option<i64>, &'static str) {
    let f = e.finding().expect("a finding").clone();
    (f.seq, f.what)
}

#[tokio::test]
async fn snapshots_after_records_are_checkpoints() {
    let mut l = load().await;
    for seq in [4, 8] {
        l.log
            .write_snapshot(&row(seq, &l.worlds[seq as usize - 1]))
            .await
            .unwrap();
    }
    assert_eq!(verify(&mut l.log).await.unwrap(), (12, 3));
}

/// F1: the host snapshots at shutdown after an advance that fired no events, which is not logged
/// (D-036). The snapshot is the log's World brought to its own game time, and it verifies.
#[tokio::test]
async fn a_snapshot_after_an_unlogged_eventless_advance_verifies() {
    let mut l = load().await;
    let last = l.records.last().unwrap();
    let mut later = l.worlds.last().unwrap().clone();
    let events = later
        .advance_to(last.game_time + 10 * H, &v0_rules())
        .unwrap();
    assert!(
        events.is_empty(),
        "the advance must be eventless: {events:?}"
    );
    assert_ne!(
        snapshot::state_hash(&later).unwrap(),
        snapshot::state_hash(l.worlds.last().unwrap()).unwrap(),
        "the advance changes the World, so hashing at the record's time would differ"
    );
    l.log.write_snapshot(&row(last.seq, &later)).await.unwrap();
    assert_eq!(verify(&mut l.log).await.unwrap(), (12, 2));
}

/// The snapshot's time is past an event the log does not hold: a divergence at the snapshot.
#[tokio::test]
async fn a_snapshot_past_an_unlogged_event_diverges() {
    let mut l = load().await;
    // After record 9 (the fleet sent at 60 h), the arrival at 64.5 h is logged as record 10. A
    // snapshot at 9 dated 65 h holds the arrival without its record.
    let mut later = l.worlds[8].clone();
    let events = later.advance_to(65 * H, &v0_rules()).unwrap();
    assert!(!events.is_empty());
    l.log.write_snapshot(&row(9, &later)).await.unwrap();
    assert_eq!(
        finding(verify(&mut l.log).await.unwrap_err()),
        (Some(9), "snapshot")
    );
}

/// F2: a later snapshot is decoded strictly, so a corrupt payload is refused even with its hash
/// column intact.
#[tokio::test]
async fn a_corrupt_later_snapshot_diverges() {
    let mut l = load().await;
    let mut bad = row(8, &l.worlds[7]);
    let last = bad.payload.len() - 1;
    bad.payload[last] ^= 1;
    l.log.write_snapshot(&bad).await.unwrap();
    assert_eq!(
        finding(verify(&mut l.log).await.unwrap_err()),
        (Some(8), "snapshot")
    );
}

/// F2: a snapshot past the log head is refused, as recovery refuses it.
#[tokio::test]
async fn a_snapshot_past_the_head_diverges() {
    let mut l = load().await;
    l.log
        .write_snapshot(&row(13, l.worlds.last().unwrap()))
        .await
        .unwrap();
    assert_eq!(
        finding(verify(&mut l.log).await.unwrap_err()),
        (Some(13), "snapshot")
    );
}

/// F2: a later snapshot whose columns disagree with its header is refused.
#[tokio::test]
async fn a_later_snapshot_with_a_wrong_column_diverges() {
    let mut l = load().await;
    let mut bad = row(8, &l.worlds[7]);
    bad.game_time += 1;
    l.log.write_snapshot(&bad).await.unwrap();
    assert_eq!(
        finding(verify(&mut l.log).await.unwrap_err()),
        (Some(8), "snapshot")
    );
}

/// A snapshot whose World differs from the replay (a valid snapshot of another state).
#[tokio::test]
async fn a_later_snapshot_of_another_state_diverges_on_its_hash() {
    let mut l = load().await;
    l.log.write_snapshot(&row(8, &l.worlds[6])).await.unwrap();
    assert_eq!(
        finding(verify(&mut l.log).await.unwrap_err()),
        (Some(8), "state_hash")
    );
}
