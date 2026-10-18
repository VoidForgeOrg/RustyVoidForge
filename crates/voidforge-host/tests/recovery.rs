//! P2.5: snapshots and recovery (decision log D-038), on the in-memory store with a manual clock.
//! A host that stops, cleanly or not, comes back byte-identical to one that never stopped. The
//! Postgres side is in `postgres.rs`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use common::{AppendThenFail, EPOCH, PATIENCE, build, clock_at, config, drive, economy, key_for};
use proptest::prelude::*;
use voidforge_core::command::{Action, Command};
use voidforge_core::quantity::MS_PER_HOUR;
use voidforge_core::rules::Rules;
use voidforge_core::state::{BuildingKind, World};
use voidforge_host::{Fatal, HostConfig, HostError, ManualClock, Receipt, RecoverError};
use voidforge_store::log::{LogStore, MemoryLog, SnapshotRow};
use voidforge_store::replay::ReplayError;
use voidforge_store::snapshot::{self, EngineBuild};
use voidforge_testkit::determinism::{Economy, canonical_bytes, slow_build_rules};
use voidforge_testkit::{ALICE, v0_rules};

const H: i64 = MS_PER_HOUR;
const END: i64 = 110 * H;

async fn timeout<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(PATIENCE, f).await.expect("in time")
}

fn spawn_on(
    world: World,
    rules: &Rules,
    log: &MemoryLog,
    clock: &ManualClock,
    cfg: HostConfig,
) -> (voidforge_host::HostHandle, voidforge_host::HostTask) {
    let log = log.clone();
    voidforge_host::spawn(world, rules.clone(), move || log, clock.clone(), cfg).unwrap()
}

async fn recover(
    log: &MemoryLog,
    rules: &Rules,
) -> Result<voidforge_host::Recovered, RecoverError> {
    voidforge_host::recover(&mut log.clone(), rules, &build()).await
}

/// The economy run to the end, uninterrupted: the reference World.
async fn uninterrupted(e: &Economy, rules: &Rules) -> World {
    let log = MemoryLog::new();
    let clock = clock_at(0);
    let (mut handle, task) = spawn_on(e.world(rules), rules, &log, &clock, config());
    drive(&mut handle, &clock, e, END).await;
    let w = handle.read().world.as_ref().clone();
    drop(handle);
    task.join().unwrap();
    w
}

/// The same run, stopped after the first `split` scripted commands (`crash`: by a failed append,
/// so no shutdown snapshot; otherwise cleanly), recovered from the store, and continued.
async fn interrupted(e: &Economy, rules: &Rules, split: usize, crash: bool, every: u64) -> World {
    let log = MemoryLog::new();
    let clock = clock_at(0);
    let mut cfg = config();
    cfg.snapshot_every_records = every;
    // The first host starts from the bootstrap snapshot, as P2.9 will write it.
    let start = e.world(rules);
    let mut l = log.clone();
    l.write_snapshot(&SnapshotRow::new(
        0,
        &snapshot::encode(&start, &build()).unwrap(),
    ))
    .await
    .unwrap();
    let first = recover(&log, rules).await.unwrap();
    assert_eq!((first.snapshot_seq, first.last_seq), (0, 0));
    let (mut handle, task) = spawn_on(first.world, rules, &log, &clock, cfg.clone());
    let script = e.script();
    for &(at, k) in &script[..split] {
        clock.set(EPOCH + at);
        common::settle(&mut handle, &clock).await;
        let command = e.command(k, &handle.read().world);
        timeout(handle.submit_with_key(command, key_for(k)))
            .await
            .unwrap();
    }
    if crash {
        log.fail_appends(true);
        let (at, k) = script[split];
        clock.set(EPOCH + at);
        common::settle(&mut handle, &clock).await;
        let command = e.command(k, &handle.read().world);
        assert_eq!(
            timeout(handle.submit_with_key(command, key_for(k))).await,
            Err(HostError::Stopped)
        );
        assert!(matches!(task.join(), Err(Fatal::Append(_))));
        log.fail_appends(false);
    } else {
        drop(handle);
        task.join().unwrap();
    }
    // Restart: recover, then the same script from where it stopped (a crashed command is
    // retried with its key, which applies it fresh: it never reached the log).
    let back = recover(&log, rules).await.unwrap();
    let (mut handle, task) = spawn_on(back.world, rules, &log, &clock, cfg);
    for &(at, k) in &script[split..] {
        clock.set((EPOCH + at).max(clock_now(&clock)));
        common::settle(&mut handle, &clock).await;
        let command = e.command(k, &handle.read().world);
        timeout(handle.submit_with_key(command, key_for(k)))
            .await
            .unwrap();
    }
    clock.set(EPOCH + END);
    common::settle(&mut handle, &clock).await;
    let w = handle.read().world.as_ref().clone();
    drop(handle);
    task.join().unwrap();
    // And the log is gap-free from 1, as one host would have written it.
    let seqs: Vec<i64> = log.snapshot().iter().map(|r| r.seq).collect();
    assert_eq!(seqs, (1..=seqs.len() as i64).collect::<Vec<_>>());
    w
}

fn clock_now(c: &ManualClock) -> i64 {
    use voidforge_host::Clock;
    c.now_unix_ms()
}

fn same_world(a: &World, b: &World) {
    assert!(
        canonical_bytes(a) == canonical_bytes(b),
        "canonical bytes differ"
    );
    assert_eq!(
        snapshot::state_hash(a).unwrap(),
        snapshot::state_hash(b).unwrap()
    );
}

// --- Byte identity ---

#[tokio::test]
async fn a_recovered_host_is_byte_identical_to_one_that_never_stopped() {
    let rules = v0_rules();
    let e = economy(0x2_5001);
    let reference = uninterrupted(&e, &rules).await;
    // A clean shutdown (snapshot at the head), and a crash (periodic snapshot plus a tail).
    same_world(&interrupted(&e, &rules, 30, false, 1_000).await, &reference);
    same_world(&interrupted(&e, &rules, 30, true, 7).await, &reference);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 10, rng_seed: proptest::test_runner::RngSeed::Fixed(0x2_5002), failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn recovery_is_byte_identical_at_any_stop(seed in any::<u64>(), split in 1usize..59, crash in any::<bool>(), every in 1u64..20) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let rules = v0_rules();
            let e = economy(seed);
            let reference = uninterrupted(&e, &rules).await;
            same_world(&interrupted(&e, &rules, split, crash, every).await, &reference);
        });
    }
}

#[tokio::test]
async fn replay_reproduces_an_unlogged_eventless_advance() {
    // D-036's obligation: a snapshot taken after an eventless advance (the storage crossing at
    // 17,999,999 ms in this economy), then recovery and continuing, equals the uninterrupted run.
    let rules = slow_build_rules();
    let e = Economy {
        seed: 7,
        planets: 3,
        fleets: false,
        commands: 0,
    };
    let world = e.world(&rules);
    let first = world.queue().first().unwrap().at_ms;
    let mut probe = world.clone();
    probe.advance_to(first, &rules).unwrap();
    let crossing = probe.queue().first().unwrap().at_ms;
    assert!(
        probe
            .clone()
            .advance_to(crossing, &rules)
            .unwrap()
            .is_empty(),
        "eventless"
    );

    let run = |stop: bool| {
        let world = world.clone();
        let rules = rules.clone();
        async move {
            let log = MemoryLog::new();
            let clock = clock_at(0);
            let (mut handle, task) = spawn_on(world, &rules, &log, &clock, config());
            clock.set(EPOCH + crossing);
            common::settle(&mut handle, &clock).await;
            assert_eq!(
                log.snapshot().len(),
                1,
                "only the first entry's advance is logged"
            );
            let (mut handle, task) = if stop {
                drop(handle);
                task.join().unwrap();
                let row = log.snapshot_rows().pop().unwrap();
                assert_eq!(
                    (row.seq, row.game_time),
                    (1, crossing),
                    "the snapshot is after the eventless advance"
                );
                let back = recover(&log, &rules).await.unwrap();
                assert_eq!(back.world.time_ms, crossing);
                spawn_on(back.world, &rules, &log, &clock, config())
            } else {
                (handle, task)
            };
            clock.set(EPOCH + 200 * H);
            common::settle(&mut handle, &clock).await;
            let w = handle.read().world.as_ref().clone();
            drop(handle);
            task.join().unwrap();
            (w, log.snapshot())
        }
    };
    let (straight, straight_log) = run(false).await;
    let (recovered, recovered_log) = run(true).await;
    same_world(&recovered, &straight);
    let strip = |l: Vec<voidforge_store::log::LogRecord>| {
        l.into_iter()
            .map(|r| (r.seq, r.game_time, r.events))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        strip(recovered_log),
        strip(straight_log),
        "the same log either way"
    );
}

// --- Snapshots ---

#[tokio::test]
async fn snapshots_follow_the_schedule_between_turns() {
    let rules = v0_rules();
    let e = economy(0x2_5003);
    let log = MemoryLog::new();
    let clock = clock_at(0);
    let mut cfg = config();
    cfg.snapshot_every_records = 5;
    let (mut handle, task) = spawn_on(e.world(&rules), &rules, &log, &clock, cfg);
    drive(&mut handle, &clock, &e, END).await;
    let written = handle.stats().snapshots();
    drop(handle);
    task.join().unwrap();
    let rows = log.snapshot_rows();
    assert!(
        rows.len() as u64 > written / 2 && written >= 5,
        "{written} snapshots"
    );
    let record_seqs: Vec<i64> = log.snapshot().iter().map(|r| r.seq).collect();
    let mut last = 0;
    for row in &rows {
        // Each is after a turn: its seq is a record's, it moves forward, and it decodes to a
        // World whose hash is the column's.
        assert!(
            record_seqs.contains(&row.seq) && row.seq > last,
            "seq {}",
            row.seq
        );
        last = row.seq;
        let d = snapshot::decode(
            &row.payload,
            &snapshot::Expected {
                rules_version: 0,
                engine_build: &build(),
            },
        )
        .unwrap();
        assert_eq!(d.header.state_hash.to_vec(), row.state_hash);
    }
    assert_eq!(
        rows.last().unwrap().seq,
        *record_seqs.last().unwrap(),
        "the shutdown snapshot is at the head"
    );
}

#[tokio::test]
async fn a_snapshot_is_never_taken_mid_turn() {
    // Ten records in one turn with a snapshot due every 3: one snapshot, after the whole turn.
    let rules = v0_rules();
    let mut b = voidforge_testkit::WorldBuilder::new();
    b.player(ALICE);
    let gated = common::GatedLog::closed(MemoryLog::new());
    let mut cfg = config();
    cfg.snapshot_every_records = 3;
    let (handle, task) = voidforge_host::spawn(
        b.build(),
        rules,
        {
            let g = gated.clone();
            move || g
        },
        clock_at(H),
        cfg,
    )
    .unwrap();
    let join = Command {
        player: ALICE,
        action: Action::JoinGame,
    };
    let hold = tokio::spawn({
        let h = handle.clone();
        let c = join.clone();
        async move { h.submit(c).await }
    });
    gated.entered(1).await;
    let mut rest = Vec::new();
    for _ in 0..10 {
        let h = handle.clone();
        let c = join.clone();
        rest.push(tokio::spawn(async move { h.submit(c).await }));
    }
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    gated.release(2);
    timeout(hold).await.unwrap().unwrap();
    for r in rest {
        timeout(r).await.unwrap().unwrap();
    }
    // Snapshots follow the replies, so read them once the host has stopped (the shutdown
    // snapshot is skipped: the newest one is already at the head).
    drop(handle);
    task.join().unwrap();
    let seqs: Vec<i64> = gated.inner.snapshot_rows().iter().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![11], "after the turn of ten, never at 3, 6 or 9");
}

/// A subscriber that counts error events, for the "loud" half of a failed snapshot.
struct ErrorCount(Arc<AtomicU64>);

impl tracing::Subscriber for ErrorCount {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if *event.metadata().level() == tracing::Level::ERROR {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn a_failed_snapshot_is_loud_but_not_fatal() {
    // nextest runs each test in its own process, so the global subscriber is this test's.
    let errors = Arc::new(AtomicU64::new(0));
    tracing::subscriber::set_global_default(ErrorCount(errors.clone())).unwrap();
    let rules = v0_rules();
    let mut b = voidforge_testkit::WorldBuilder::new();
    b.player(ALICE);
    let log = MemoryLog::new();
    log.fail_snapshots(true);
    let mut cfg = config();
    cfg.snapshot_every_records = 1;
    let (handle, task) = spawn_on(b.build(), &rules, &log, &clock_at(H), cfg);
    let join = Command {
        player: ALICE,
        action: Action::JoinGame,
    };
    timeout(handle.submit(join.clone())).await.unwrap();
    // The snapshot follows the reply: wait for the attempt.
    timeout(async {
        while handle.stats().snapshot_failures() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await;
    assert_eq!(handle.stats().snapshot_failures(), 1);
    assert!(errors.load(Ordering::Relaxed) >= 1, "an error event");
    assert!(!task.is_finished(), "not fatal");
    log.fail_snapshots(false);
    let r = timeout(handle.submit(join)).await.unwrap();
    timeout(async {
        while handle.stats().snapshots() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await;
    assert_eq!(handle.stats().snapshot_failures(), 1, "the retry succeeded");
    assert_eq!(
        log.snapshot_rows().last().unwrap().seq,
        r.seq,
        "the next turn retries"
    );
    drop(handle);
    task.join().unwrap();
}

// --- Choosing and checking the snapshot ---

#[tokio::test]
async fn no_snapshot_means_not_bootstrapped() {
    assert!(matches!(
        recover(&MemoryLog::new(), &v0_rules()).await,
        Err(RecoverError::NoSnapshot)
    ));
}

/// A log with the bootstrap snapshot and a host that ran `n` keyed joins, then stopped cleanly
/// (`clean`) or by a failed append.
async fn ran(n: usize, every: u64) -> (MemoryLog, World) {
    let rules = v0_rules();
    let mut b = voidforge_testkit::WorldBuilder::new();
    b.player(ALICE);
    let world = b.build();
    let log = MemoryLog::new();
    log.clone()
        .write_snapshot(&SnapshotRow::new(
            0,
            &snapshot::encode(&world, &build()).unwrap(),
        ))
        .await
        .unwrap();
    let mut cfg = config();
    cfg.snapshot_every_records = every;
    let (handle, task) = spawn_on(world, &rules, &log, &clock_at(H), cfg);
    for i in 0..n {
        let c = Command {
            player: ALICE,
            action: Action::JoinGame,
        };
        timeout(handle.submit_with_key(
            c,
            voidforge_store::log::IdempotencyKey::new(format!("j{i}")).unwrap(),
        ))
        .await
        .unwrap();
    }
    let w = handle.read().world.as_ref().clone();
    drop(handle);
    task.join().unwrap();
    (log, w)
}

#[tokio::test]
async fn recovery_takes_the_newest_snapshot_of_its_build() {
    let (log, world) = ran(6, 4).await;
    let rows: Vec<i64> = log.snapshot_rows().iter().map(|r| r.seq).collect();
    assert_eq!(rows, vec![0, 4, 6]);
    let back = recover(&log, &v0_rules()).await.unwrap();
    assert_eq!((back.snapshot_seq, back.last_seq), (6, 6));
    same_world(&back.world, &world);
    // Another build's newer snapshot is not chosen while one of ours exists.
    let mut other = log.snapshot_rows()[1].clone();
    other.seq = 5;
    other.engine_build = "0.1.0+other".to_owned();
    log.clone().write_snapshot(&other).await.unwrap();
    assert_eq!(recover(&log, &v0_rules()).await.unwrap().snapshot_seq, 6);
}

#[tokio::test]
async fn upgrades_recover_through_the_memory_store() {
    // Carry (f): an upgrade start, a crash after it, rollback before and after (D-038).
    let log = MemoryLog::new();
    common::upgrade_cases(log.clone(), move || log.clone()).await;
    let log = MemoryLog::new();
    common::upgrade_over_another_builds_tail_is_refused(log.clone(), move || log.clone()).await;
}

#[tokio::test]
async fn a_snapshot_row_that_disagrees_with_its_header_is_refused() {
    let (log, _) = ran(2, 1_000).await;
    let good = log.snapshot_rows().pop().unwrap();
    for (column, change) in [
        (
            "game_time",
            Box::new(|r: &mut SnapshotRow| r.game_time += 1) as Box<dyn Fn(&mut SnapshotRow)>,
        ),
        (
            "state_hash",
            Box::new(|r: &mut SnapshotRow| r.state_hash[0] ^= 1),
        ),
    ] {
        let mut store = MemoryLog::new();
        for r in log.snapshot() {
            store.append(&[r]).await.unwrap();
        }
        let mut row = good.clone();
        change(&mut row);
        store.write_snapshot(&row).await.unwrap();
        match recover(&store, &v0_rules()).await {
            Err(RecoverError::ColumnMismatch { column: c, .. }) => assert_eq!(c, column),
            other => panic!("{column}: {other:?}"),
        }
    }
    // A payload that does not decode strictly is refused too (here: another rules version).
    let mut rules = v0_rules();
    rules.version = 1;
    assert!(matches!(
        recover(&log, &rules).await,
        Err(RecoverError::Snapshot { .. })
    ));
}

// --- The tail ---

#[tokio::test]
async fn a_tampered_tail_record_is_a_divergence() {
    let (log, _) = ran(4, 1_000).await;
    // Recover from the bootstrap snapshot only, so every record is replayed.
    let mut store = MemoryLog::new();
    for r in log.snapshot() {
        store.append(&[r]).await.unwrap();
    }
    store.write_snapshot(&log.snapshot_rows()[0]).await.unwrap();
    assert_eq!(recover(&store, &v0_rules()).await.unwrap().last_seq, 4);
    let mut tampered = store.snapshot()[2].clone();
    tampered.outcome = voidforge_store::log::Outcome::Accepted { own_events: 0 };
    store.tamper(tampered);
    assert!(matches!(
        recover(&store, &v0_rules()).await,
        Err(RecoverError::Replay(ReplayError::Diverged {
            seq: 3,
            what: "outcome"
        }))
    ));
}

#[tokio::test]
async fn another_builds_record_in_the_tail_is_refused() {
    let (log, _) = ran(3, 1_000).await;
    let mut store = MemoryLog::new();
    for r in log.snapshot() {
        store.append(&[r]).await.unwrap();
    }
    store.write_snapshot(&log.snapshot_rows()[0]).await.unwrap();
    let mut foreign = store.snapshot()[1].clone();
    foreign.engine_build = EngineBuild::new("0.1.0+other").unwrap();
    store.tamper(foreign);
    assert!(matches!(
        recover(&store, &v0_rules()).await,
        Err(RecoverError::Replay(ReplayError::CrossBuildRecord {
            seq: 2,
            ..
        }))
    ));
}

// --- The crash table, with a restart and a retry with the same key ---

fn drill_on(home: voidforge_core::state::PlanetId) -> Command {
    Command {
        player: ALICE,
        action: Action::BuildBuilding {
            planet: home,
            kind: BuildingKind::Drill,
        },
    }
}

/// Bootstraps Alice's world into `log` and returns (world, home).
async fn bootstrapped(log: &MemoryLog) -> (World, voidforge_core::state::PlanetId) {
    let mut b = voidforge_testkit::WorldBuilder::new();
    let home = b.player(ALICE);
    let world = b.build();
    log.clone()
        .write_snapshot(&SnapshotRow::new(
            0,
            &snapshot::encode(&world, &build()).unwrap(),
        ))
        .await
        .unwrap();
    (world, home)
}

async fn restart_and_retry(log: &MemoryLog, command: Command, key: &str) -> Receipt {
    let back = recover(log, &v0_rules()).await.unwrap();
    let (handle, task) = spawn_on(back.world, &v0_rules(), log, &clock_at(2 * H), config());
    let r = timeout(handle.submit_with_key(
        command,
        voidforge_store::log::IdempotencyKey::new(key).unwrap(),
    ))
    .await
    .unwrap();
    drop(handle);
    task.join().unwrap();
    r
}

#[tokio::test]
async fn crash_before_the_append_the_retry_applies_fresh() {
    let log = MemoryLog::new();
    let (world, home) = bootstrapped(&log).await;
    log.fail_appends(true);
    let (handle, task) = spawn_on(world, &v0_rules(), &log, &clock_at(H), config());
    let key = voidforge_store::log::IdempotencyKey::new("c1").unwrap();
    assert_eq!(
        timeout(handle.submit_with_key(drill_on(home), key)).await,
        Err(HostError::Stopped)
    );
    assert!(matches!(task.join(), Err(Fatal::Append(_))));
    log.fail_appends(false);
    assert!(log.snapshot().is_empty(), "the command never happened");
    let r = restart_and_retry(&log, drill_on(home), "c1").await;
    assert_eq!((r.seq, r.t), (1, 2 * H), "applied fresh after the restart");
    assert_eq!(log.snapshot().len(), 1);
}

#[tokio::test]
async fn crash_after_the_append_before_the_reply_the_retry_returns_the_logged_outcome() {
    let log = MemoryLog::new();
    let (world, home) = bootstrapped(&log).await;
    let (handle, task) = {
        let store = AppendThenFail(log.clone());
        voidforge_host::spawn(world, v0_rules(), move || store, clock_at(H), config()).unwrap()
    };
    let key = voidforge_store::log::IdempotencyKey::new("c2").unwrap();
    assert_eq!(
        timeout(handle.submit_with_key(drill_on(home), key)).await,
        Err(HostError::Stopped),
        "no reply"
    );
    assert!(matches!(task.join(), Err(Fatal::Append(_))));
    assert_eq!(log.snapshot().len(), 1, "the command is in the log");
    let r = restart_and_retry(&log, drill_on(home), "c2").await;
    assert_eq!(
        (r.seq, r.t),
        (1, H),
        "the logged outcome, not a second application"
    );
    assert_eq!(log.snapshot().len(), 1);
}

#[tokio::test]
async fn after_the_reply_the_retry_returns_the_same_receipt() {
    let log = MemoryLog::new();
    let (world, home) = bootstrapped(&log).await;
    let (handle, task) = spawn_on(world, &v0_rules(), &log, &clock_at(H), config());
    let key = voidforge_store::log::IdempotencyKey::new("c3").unwrap();
    let first = timeout(handle.submit_with_key(drill_on(home), key))
        .await
        .unwrap();
    drop(handle);
    task.join().unwrap();
    assert_eq!(restart_and_retry(&log, drill_on(home), "c3").await, first);
    assert_eq!(log.snapshot().len(), 1);
}

// --- The shared function ---

#[test]
fn the_host_changes_the_world_only_through_the_shared_function() {
    // Recovery and replay can't drift from the live path if the live path is the shared function
    // (Project layering; D-038).
    let source = include_str!("../src/lib.rs");
    for direct in [".apply(", ".advance_to(", ".advance_with("] {
        assert!(!source.contains(direct), "the host calls {direct} directly");
    }
    assert!(source.contains("replay::step(&mut self.world, &Step::Command("));
    assert!(source.contains("replay::step(&mut self.world, &Step::Advance,"));
}

#[tokio::test]
async fn a_gap_in_the_tail_is_refused() {
    let (log, _) = ran(4, 1_000).await;
    // The bootstrap snapshot and records 1, 3 and 4: record 2 is missing.
    let mut store = MemoryLog::new();
    for r in log.snapshot().into_iter().filter(|r| r.seq != 2) {
        store.append(&[r]).await.unwrap();
    }
    store.write_snapshot(&log.snapshot_rows()[0]).await.unwrap();
    assert!(matches!(
        recover(&store, &v0_rules()).await,
        Err(RecoverError::Replay(ReplayError::Gap {
            expected: 2,
            found: 3
        }))
    ));
}

#[tokio::test]
async fn a_tail_record_with_other_events_is_a_divergence() {
    let (log, _) = ran(3, 1_000).await;
    let mut store = MemoryLog::new();
    for r in log.snapshot() {
        store.append(&[r]).await.unwrap();
    }
    store.write_snapshot(&log.snapshot_rows()[0]).await.unwrap();
    let mut tampered = store.snapshot()[1].clone();
    tampered
        .events
        .push(voidforge_core::state::DomainEvent::BuildingCompleted {
            planet: voidforge_core::state::PlanetId(1),
            building: voidforge_core::state::BuildingId(1),
            kind: BuildingKind::Drill,
        });
    store.tamper(tampered);
    assert!(matches!(
        recover(&store, &v0_rules()).await,
        Err(RecoverError::Replay(ReplayError::Diverged {
            seq: 2,
            what: "events"
        }))
    ));
}

#[tokio::test]
async fn a_host_spawned_on_a_non_empty_log_reads_the_log_head() {
    // Carry (e): the read model's seq is "the last durable record", from the first moment,
    // including after an eventless turn and before any new record.
    let rules = slow_build_rules();
    let e = Economy {
        seed: 7,
        planets: 3,
        fleets: false,
        commands: 0,
    };
    let log = MemoryLog::new();
    let start = e.world(&rules);
    log.clone()
        .write_snapshot(&SnapshotRow::new(
            0,
            &snapshot::encode(&start, &build()).unwrap(),
        ))
        .await
        .unwrap();
    let first = start.queue().first().unwrap().at_ms;
    let clock = clock_at(0);
    let (mut handle, task) = spawn_on(start, &rules, &log, &clock, config());
    clock.set(EPOCH + first);
    common::settle(&mut handle, &clock).await;
    drop(handle);
    task.join().unwrap();
    let head = log.snapshot().last().unwrap().seq;
    assert_eq!(head, 1);
    let back = recover(&log, &rules).await.unwrap();
    let mut probe = back.world.clone();
    let crossing = probe.queue().first().unwrap().at_ms;
    assert!(
        probe.advance_to(crossing, &rules).unwrap().is_empty(),
        "the next entry is eventless"
    );
    let (mut handle, task) = spawn_on(back.world, &rules, &log, &clock, config());
    assert_eq!(handle.read().seq, head, "at start");
    clock.set(EPOCH + crossing);
    common::settle(&mut handle, &clock).await;
    assert_eq!(
        handle.read().world.time_ms,
        crossing,
        "an eventless turn was published"
    );
    assert_eq!(handle.read().seq, head, "after an eventless turn");
    drop(handle);
    task.join().unwrap();
}

// --- Bounded repair 1 (review M1, L1 to L3) ---

#[tokio::test]
async fn a_snapshot_past_the_log_head_is_refused() {
    let log = MemoryLog::new();
    common::snapshot_past_head_is_refused(log.clone(), move || log.clone()).await;
}

/// An economy run from the bootstrap snapshot, stopped by a crash (so no later snapshot): a log
/// of advance and command records, every one replayed by `recover`.
async fn economy_log() -> MemoryLog {
    let rules = v0_rules();
    let e = economy(0x2_5301);
    let log = MemoryLog::new();
    let start = e.world(&rules);
    log.clone()
        .write_snapshot(&SnapshotRow::new(
            0,
            &snapshot::encode(&start, &build()).unwrap(),
        ))
        .await
        .unwrap();
    let mut cfg = config();
    cfg.snapshot_every_records = 1_000_000;
    let clock = clock_at(0);
    let (mut handle, task) = {
        let store = common::FailAfter::new(log.clone(), 40);
        voidforge_host::spawn(start, rules.clone(), move || store, clock.clone(), cfg).unwrap()
    };
    for (at, k) in e.script() {
        clock.set(EPOCH + at);
        common::settle(&mut handle, &clock).await;
        let command = e.command(k, &handle.read().world);
        if handle.submit_with_key(command, key_for(k)).await.is_err() {
            break;
        }
    }
    drop(handle);
    assert!(task.join().is_err(), "stopped by the crash");
    assert_eq!(log.snapshot_rows().len(), 1, "only the bootstrap snapshot");
    assert_eq!(recover(&log, &rules).await.unwrap().last_seq, 40);
    log
}

fn first_where(
    log: &MemoryLog,
    f: impl Fn(&voidforge_store::log::LogRecord) -> bool,
) -> voidforge_store::log::LogRecord {
    log.snapshot().into_iter().find(f).expect("such a record")
}

#[tokio::test]
async fn a_record_of_another_rules_version_is_refused() {
    // Review L1: replay refuses a rules mismatch (Determinism rules).
    let log = economy_log().await;
    let mut r = first_where(&log, |r| r.seq == 5);
    r.rules_version = 1;
    log.tamper(r);
    assert!(matches!(
        recover(&log, &v0_rules()).await,
        Err(RecoverError::Replay(ReplayError::RulesMismatch {
            seq: 5,
            expected: 0,
            found: 1
        }))
    ));
}

#[tokio::test]
async fn a_command_records_player_column_must_match_its_command() {
    // Review L2.
    let log = economy_log().await;
    let mut r = first_where(&log, |r| {
        r.kind == voidforge_store::log::RecordKind::Command
    });
    let seq = r.seq;
    r.player = Some(voidforge_core::state::PlayerId(r.player.unwrap().0 ^ 1));
    log.tamper(r);
    assert!(matches!(
        recover(&log, &v0_rules()).await,
        Err(RecoverError::Replay(ReplayError::Malformed { seq: s, .. })) if s == seq
    ));
}

#[tokio::test]
async fn a_record_whose_events_were_emptied_diverges() {
    // Review L3, QA's M10: verification also covers a record that claims no events.
    let log = economy_log().await;
    let mut r = first_where(&log, |r| !r.events.is_empty());
    let seq = r.seq;
    r.events.clear();
    log.tamper(r);
    assert!(matches!(
        recover(&log, &v0_rules()).await,
        Err(RecoverError::Replay(ReplayError::Diverged { seq: s, what: "events" })) if s == seq
    ));
}

#[tokio::test]
async fn another_builds_advance_record_is_refused() {
    // Review L3, QA's M11: the build check covers advance records, not only commands.
    let log = economy_log().await;
    let mut r = first_where(&log, |r| {
        r.kind == voidforge_store::log::RecordKind::Advance
    });
    let seq = r.seq;
    r.engine_build = EngineBuild::new("0.1.0+other").unwrap();
    log.tamper(r);
    assert!(matches!(
        recover(&log, &v0_rules()).await,
        Err(RecoverError::Replay(ReplayError::CrossBuildRecord { seq: s, .. })) if s == seq
    ));
}
