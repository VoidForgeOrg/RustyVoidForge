//! P2.4: idempotency (decision log D-037), on the in-memory store with a manual clock. A repeated
//! (player, key) gets its first receipt at any age, through the cache or the log, and a duplicate
//! never reaches the append, where it would be fatal. The Postgres side is in `postgres.rs`.

mod common;

use std::time::Duration;

use common::{EPOCH, GatedLog, PATIENCE, clock_at, config};
use voidforge_core::command::{Action, Command, Rejection};
use voidforge_core::quantity::MS_PER_HOUR;
use voidforge_core::state::{BuildingKind, PlanetId, PlayerId, World};
use voidforge_host::{Fatal, HostError, HostHandle, HostTask, ManualClock};
use voidforge_store::log::{
    IdempotencyKey, InvalidKey, LogError, LogRecord, LogStore, MemoryLog, RecordKind, SnapshotRow,
};
use voidforge_testkit::{ALICE, BOB, WorldBuilder, v0_rules};

const H: i64 = MS_PER_HOUR;

fn key(k: &str) -> IdempotencyKey {
    IdempotencyKey::new(k).unwrap()
}

/// Alice's homeworld at time 0; Bob has not joined.
fn alice() -> (World, PlanetId) {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let free = b.planet(None);
    b.candidates(&[free]);
    (b.build(), home)
}

fn drill(home: PlanetId) -> Command {
    Command {
        player: ALICE,
        action: Action::BuildBuilding {
            planet: home,
            kind: BuildingKind::Drill,
        },
    }
}

fn refinery(home: PlanetId) -> Command {
    Command {
        player: ALICE,
        action: Action::BuildBuilding {
            planet: home,
            kind: BuildingKind::Refinery,
        },
    }
}

fn join(player: PlayerId) -> Command {
    Command {
        player,
        action: Action::JoinGame,
    }
}

fn start(world: World, log: &MemoryLog, clock: &ManualClock) -> (HostHandle, HostTask) {
    let log = log.clone();
    voidforge_host::spawn(world, v0_rules(), move || log, clock.clone(), config()).unwrap()
}

async fn timeout<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(PATIENCE, f).await.expect("in time")
}

#[test]
fn keys_are_validated() {
    for ok in ["k", "0192-8c5e", &"x".repeat(128), "a~b!c"] {
        assert_eq!(IdempotencyKey::new(ok).unwrap().as_str(), ok);
    }
    for bad in ["", " ", "has space", "tab\t", "\u{e9}", &"x".repeat(129)] {
        assert_eq!(IdempotencyKey::new(bad), Err(InvalidKey), "{bad:?}");
    }
}

#[tokio::test]
async fn a_repeated_key_returns_the_first_receipt_and_applies_nothing() {
    let (world, home) = alice();
    let log = MemoryLog::new();
    let clock = clock_at(H);
    let (handle, task) = start(world, &log, &clock);
    let first = timeout(handle.submit_with_key(drill(home), key("k1")))
        .await
        .unwrap();
    let world_after = handle.read().world.clone();
    // Later, the clock has moved: the retry still gets the original seq, t and outcome.
    clock.set(EPOCH + 2 * H);
    let again = timeout(handle.submit_with_key(drill(home), key("k1")))
        .await
        .unwrap();
    assert_eq!(again, first);
    assert_eq!(log.snapshot().len(), 1, "nothing appended");
    assert_eq!(*handle.read().world, *world_after, "nothing applied");
    assert_eq!(handle.stats().replays(), 1);
    assert_eq!(
        log.snapshot()[0].idempotency_key,
        Some(key("k1")),
        "the record holds the key"
    );
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn a_rejected_commands_key_returns_the_rejection() {
    let (world, _) = alice();
    let log = MemoryLog::new();
    let (handle, task) = start(world, &log, &clock_at(H));
    let first = timeout(handle.submit_with_key(join(ALICE), key("j")))
        .await
        .unwrap();
    assert_eq!(first.outcome, Err(Rejection::AlreadyJoined));
    assert_eq!(
        timeout(handle.submit_with_key(join(ALICE), key("j")))
            .await
            .unwrap(),
        first
    );
    assert_eq!(log.snapshot().len(), 1);
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn keys_are_per_player() {
    let (world, home) = alice();
    let log = MemoryLog::new();
    let (handle, task) = start(world, &log, &clock_at(H));
    let a = timeout(handle.submit_with_key(drill(home), key("same")))
        .await
        .unwrap();
    let b = timeout(handle.submit_with_key(join(BOB), key("same")))
        .await
        .unwrap();
    assert_ne!(a.seq, b.seq, "another player's key is another key");
    assert!(b.outcome.is_ok(), "Bob joined");
    assert_eq!(log.snapshot().len(), 2);
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn the_same_key_for_a_different_command_is_a_conflict() {
    let (world, home) = alice();
    let log = MemoryLog::new();
    let (handle, task) = start(world, &log, &clock_at(H));
    let first = timeout(handle.submit_with_key(drill(home), key("k")))
        .await
        .unwrap();
    let before = handle.read().world.clone();
    assert_eq!(
        timeout(handle.submit_with_key(refinery(home), key("k"))).await,
        Err(HostError::KeyConflict { seq: first.seq })
    );
    assert_eq!(log.snapshot().len(), 1, "nothing appended");
    assert_eq!(*handle.read().world, *before, "nothing applied");
    assert!(
        !task.is_finished(),
        "a conflict is the caller's error, not the host's"
    );
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn a_keyless_command_is_never_deduplicated() {
    let (world, home) = alice();
    let log = MemoryLog::new();
    let (handle, task) = start(world, &log, &clock_at(H));
    let a = timeout(handle.submit(drill(home))).await.unwrap();
    let b = timeout(handle.submit(drill(home))).await.unwrap();
    assert_eq!((a.seq, b.seq), (1, 2));
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn duplicates_in_one_turn_share_the_first_outcome() {
    let (world, home) = alice();
    let gated = GatedLog::closed(MemoryLog::new());
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let gated = gated.clone();
            move || gated
        },
        clock_at(H),
        config(),
    )
    .unwrap();
    let hold = tokio::spawn({
        let h = handle.clone();
        async move { h.submit(join(ALICE)).await }
    });
    gated.entered(1).await;
    // One turn: the key's first use, a repeat, and a conflicting reuse.
    let send = |c: Command| {
        let h = handle.clone();
        tokio::spawn(async move { h.submit_with_key(c, key("t")).await })
    };
    let first = send(drill(home));
    tokio::time::sleep(Duration::from_millis(30)).await;
    let repeat = send(drill(home));
    tokio::time::sleep(Duration::from_millis(30)).await;
    let conflict = send(refinery(home));
    tokio::time::sleep(Duration::from_millis(60)).await;
    // Nothing is answered before the turn's records are durable.
    assert!(!repeat.is_finished() && !conflict.is_finished());
    gated.release(2);
    timeout(hold).await.unwrap().unwrap();
    let first = timeout(first).await.unwrap().unwrap();
    assert_eq!(timeout(repeat).await.unwrap().unwrap(), first);
    assert_eq!(
        timeout(conflict).await.unwrap(),
        Err(HostError::KeyConflict { seq: first.seq })
    );
    let kinds: Vec<_> = gated.inner.snapshot().iter().map(|r| r.seq).collect();
    assert_eq!(kinds, vec![1, 2], "one record for the key");
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn a_restart_rebuilds_the_keys_from_the_log() {
    let (world, home) = alice();
    let log = MemoryLog::new();
    let clock = clock_at(H);
    let (handle, task) = start(world, &log, &clock);
    let first = timeout(handle.submit_with_key(drill(home), key("r")))
        .await
        .unwrap();
    let world = handle.read().world.as_ref().clone();
    drop(handle);
    task.join().unwrap();
    // A new host on the same log: the key is in its rebuilt cache, no lookup needed.
    let (handle, task) = start(world, &log, &clock);
    assert_eq!(
        timeout(handle.submit_with_key(drill(home), key("r")))
            .await
            .unwrap(),
        first
    );
    assert_eq!(
        handle.stats().key_lookups(),
        0,
        "served from the rebuilt cache"
    );
    assert_eq!(
        timeout(handle.submit_with_key(refinery(home), key("r"))).await,
        Err(HostError::KeyConflict { seq: first.seq })
    );
    assert_eq!(log.snapshot().len(), 1);
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn a_key_older_than_the_window_is_answered_from_the_log() {
    let (world, home) = alice();
    let log = MemoryLog::new();
    let clock = clock_at(H);
    let mut cfg = config();
    cfg.key_window_ms = 2 * H;
    let (handle, task) = {
        let log = log.clone();
        voidforge_host::spawn(world, v0_rules(), move || log, clock.clone(), cfg.clone()).unwrap()
    };
    let first = timeout(handle.submit_with_key(drill(home), key("old")))
        .await
        .unwrap();
    // Move well past the window and run a turn, so the key leaves the cache.
    clock.set(EPOCH + 10 * H);
    timeout(handle.submit(join(ALICE))).await.unwrap();
    let lookups = handle.stats().key_lookups();
    let records = log.snapshot().len();
    // The expired-key attack: the same old key again, and with a different command. Neither may
    // reach the append, where the unique (player, key) would make it fatal.
    assert_eq!(
        timeout(handle.submit_with_key(drill(home), key("old")))
            .await
            .unwrap(),
        first
    );
    assert_eq!(
        timeout(handle.submit_with_key(refinery(home), key("old"))).await,
        Err(HostError::KeyConflict { seq: first.seq })
    );
    assert!(
        handle.stats().key_lookups() > lookups,
        "answered through the log"
    );
    assert_eq!(log.snapshot().len(), records, "nothing appended");
    assert!(!task.is_finished(), "the host is still running");
    let world = handle.read().world.as_ref().clone();
    drop(handle);
    task.join().unwrap();
    // After a restart the key is outside the rebuilt window too: the log still answers it.
    let (handle, task) = {
        let log = log.clone();
        voidforge_host::spawn(world, v0_rules(), move || log, clock.clone(), cfg).unwrap()
    };
    assert_eq!(
        timeout(handle.submit_with_key(drill(home), key("old")))
            .await
            .unwrap(),
        first
    );
    assert_eq!(handle.stats().key_lookups(), 1);
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn a_key_inside_the_window_is_answered_from_the_cache() {
    let (world, home) = alice();
    let log = MemoryLog::new();
    let clock = clock_at(H);
    let (handle, task) = start(world, &log, &clock);
    timeout(handle.submit_with_key(drill(home), key("w")))
        .await
        .unwrap();
    let lookups = handle.stats().key_lookups();
    clock.set(EPOCH + 23 * H);
    timeout(handle.submit_with_key(drill(home), key("w")))
        .await
        .unwrap();
    assert_eq!(
        handle.stats().key_lookups(),
        lookups,
        "no log lookup inside the window"
    );
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn new_keys_cost_one_lookup_per_turn() {
    let (world, home) = alice();
    let gated = GatedLog::closed(MemoryLog::new());
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let gated = gated.clone();
            move || gated
        },
        clock_at(H),
        config(),
    )
    .unwrap();
    let hold = tokio::spawn({
        let h = handle.clone();
        async move { h.submit(join(ALICE)).await }
    });
    gated.entered(1).await;
    let mut sent = Vec::new();
    for i in 0..5 {
        let h = handle.clone();
        let c = drill(home);
        sent.push(tokio::spawn(async move {
            h.submit_with_key(c, key(&format!("n{i}"))).await
        }));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    gated.release(2);
    timeout(hold).await.unwrap().unwrap();
    for s in sent {
        timeout(s).await.unwrap().unwrap();
    }
    assert_eq!(
        handle.stats().key_lookups(),
        1,
        "five new keys, one turn, one lookup"
    );
    assert_eq!(gated.inner.lookups(), 1);
    drop(handle);
    task.join().unwrap();
}

/// A store whose key lookups fail.
#[derive(Clone)]
struct BlindLog(MemoryLog);

impl LogStore for BlindLog {
    async fn last(&mut self) -> Result<Option<(i64, i64)>, LogError> {
        self.0.last().await
    }
    async fn append(&mut self, records: &[LogRecord]) -> Result<(), LogError> {
        self.0.append(records).await
    }
    async fn records(&mut self) -> Result<Vec<LogRecord>, LogError> {
        self.0.records().await
    }
    async fn find_keys(
        &mut self,
        _: &[(PlayerId, IdempotencyKey)],
    ) -> Result<Vec<LogRecord>, LogError> {
        Err(LogError::Memory("lookup failed".to_owned()))
    }
    async fn keyed_since(&mut self, since: i64) -> Result<Vec<LogRecord>, LogError> {
        self.0.keyed_since(since).await
    }
    async fn records_after(&mut self, seq: i64) -> Result<Vec<LogRecord>, LogError> {
        self.0.records_after(seq).await
    }

    async fn write_snapshot(&mut self, row: &SnapshotRow) -> Result<(), LogError> {
        self.0.write_snapshot(row).await
    }

    async fn latest_snapshot(
        &mut self,
        build: Option<&str>,
    ) -> Result<Option<SnapshotRow>, LogError> {
        self.0.latest_snapshot(build).await
    }

    async fn snapshot_index(
        &mut self,
    ) -> Result<Vec<voidforge_store::log::SnapshotMeta>, LogError> {
        self.0.snapshot_index().await
    }

    async fn snapshot_at(&mut self, seq: i64) -> Result<Option<SnapshotRow>, LogError> {
        self.0.snapshot_at(seq).await
    }
}

#[tokio::test]
async fn a_failed_key_lookup_stops_the_host_before_applying() {
    let (world, home) = alice();
    let log = MemoryLog::new();
    let (handle, task) = {
        let blind = BlindLog(log.clone());
        voidforge_host::spawn(world, v0_rules(), move || blind, clock_at(H), config()).unwrap()
    };
    // Keyless commands need no lookup.
    timeout(handle.submit(join(ALICE))).await.unwrap();
    // A new key cannot be told from a used one: the host stops rather than guess.
    assert_eq!(
        timeout(handle.submit_with_key(drill(home), key("x"))).await,
        Err(HostError::Stopped)
    );
    assert!(matches!(task.join(), Err(Fatal::Lookup(_))));
    assert_eq!(log.snapshot().len(), 1, "nothing applied for the key");
}

#[tokio::test]
async fn the_memory_store_refuses_a_duplicate_key_like_the_table() {
    // So a duplicate that reached the append would stop the host on the memory store too, and
    // the tests above would see it.
    let (world, home) = alice();
    let mut w = world;
    let applied = w.apply(&drill(home), H, &v0_rules()).unwrap();
    let record = |seq: i64| LogRecord {
        seq,
        kind: RecordKind::Command,
        game_time: H,
        wall_time_ms: EPOCH + H,
        player: Some(ALICE),
        idempotency_key: Some(key("dup")),
        payload: voidforge_store::log::Payload::Command(drill(home)),
        outcome: voidforge_store::log::Outcome::Accepted {
            own_events: u32::try_from(applied.outcome.as_ref().unwrap().len()).unwrap(),
        },
        events: Vec::new(),
        rules_version: 0,
        engine_build: common::build(),
    };
    let mut log = MemoryLog::new();
    log.append(&[record(1)]).await.unwrap();
    assert!(log.append(&[record(2)]).await.is_err(), "across appends");
    let mut fresh = MemoryLog::new();
    assert!(
        fresh.append(&[record(1), record(2)]).await.is_err(),
        "within one append"
    );
    assert!(fresh.snapshot().is_empty(), "all or none");
}

/// A clock whose sleeps never end: the host never wakes on its own, so due events run only
/// inside the next command's apply, and command records hold advance events too.
#[derive(Clone)]
struct NoWake(ManualClock);

impl voidforge_host::Clock for NoWake {
    fn now_unix_ms(&self) -> i64 {
        self.0.now_unix_ms()
    }
    async fn sleep_until_unix_ms(&self, _at: i64) {
        std::future::pending::<()>().await;
    }
}

#[tokio::test]
async fn every_receipt_survives_a_restart_exactly() {
    // A whole keyed economy script with no wakes between commands, then a restart from the same
    // log: every keyed command, resubmitted, gets exactly its first receipt, from the rebuilt
    // cache and, with a tiny window, through the log. Records hold events that came due before
    // the command as well as the command's own, so the receipt must split them correctly.
    use voidforge_host::Clock;
    use voidforge_store::log::{Outcome, Payload};
    let e = common::economy(0x2_4001);
    let rules = v0_rules();
    let log = MemoryLog::new();
    let clock = NoWake(clock_at(0));
    let (handle, task) = {
        let log = log.clone();
        voidforge_host::spawn(
            common::start(&e, &rules),
            rules.clone(),
            move || log,
            clock.clone(),
            config(),
        )
        .unwrap()
    };
    let mut receipts = Vec::new();
    for (at, k) in e.script() {
        clock.0.set(EPOCH + at);
        let command = e.command(k, &handle.read().world);
        receipts.push(
            timeout(handle.submit_with_key(command, common::key_for(k)))
                .await
                .unwrap(),
        );
    }
    let world = handle.read().world.as_ref().clone();
    drop(handle);
    task.join().unwrap();
    let records = log.snapshot();
    assert_eq!(
        records.len(),
        receipts.len(),
        "only command records: the host never woke"
    );
    let advanced = records.iter().filter(|r| match r.outcome {
        Outcome::Accepted { own_events } => (own_events as usize) < r.events.len(),
        Outcome::Rejected(_) => !r.events.is_empty(),
        Outcome::Empty => false,
    });
    let own = records
        .iter()
        .filter(|r| matches!(r.outcome, Outcome::Accepted { own_events } if own_events > 0));
    let (advanced, own) = (advanced.count(), own.count());
    println!(
        "records {}, with advance events {advanced}, with own events {own}",
        records.len()
    );
    assert!(advanced > 0, "some commands ran due events first");
    for window in [config().key_window_ms, 1] {
        let mut cfg = config();
        cfg.key_window_ms = window;
        let (handle, task) = {
            let log = log.clone();
            voidforge_host::spawn(
                world.clone(),
                rules.clone(),
                move || log,
                clock.clone(),
                cfg,
            )
            .unwrap()
        };
        for (record, first) in records.iter().zip(&receipts) {
            let Payload::Command(command) = &record.payload else {
                panic!("a command record")
            };
            let key = record.idempotency_key.clone().unwrap();
            let again = timeout(handle.submit_with_key(command.clone(), key)).await;
            assert_eq!(
                again.as_ref(),
                Ok(first),
                "record {} (window {window})",
                record.seq
            );
        }
        assert_eq!(handle.stats().steps(), 0, "nothing applied");
        let _ = clock.now_unix_ms();
        drop(handle);
        task.join().unwrap();
    }
    assert_eq!(log.snapshot().len(), receipts.len(), "nothing appended");
}

#[tokio::test]
async fn a_rebuilt_receipt_returns_exactly_the_commands_own_events() {
    // A logged record whose last two of three events are the command's own (own_events = 2):
    // a restart answers the key with exactly those two, from the cache and through the log.
    // (Under Rules v0 a command's own events are rare, so the log is seeded directly.)
    use voidforge_core::state::{BuildingId, DomainEvent};
    use voidforge_store::log::{Outcome, Payload};
    let event = |n: u64| DomainEvent::BuildingCompleted {
        planet: PlanetId(1),
        building: BuildingId(n),
        kind: BuildingKind::Drill,
    };
    let mut b = WorldBuilder::new().at(100 * H);
    let home = b.player(ALICE);
    let world = b.build();
    let log = MemoryLog::new();
    {
        let mut log = log.clone();
        log.append(&[LogRecord {
            seq: 1,
            kind: RecordKind::Command,
            game_time: H,
            wall_time_ms: EPOCH + H,
            player: Some(ALICE),
            idempotency_key: Some(key("seeded")),
            payload: Payload::Command(drill(home)),
            outcome: Outcome::Accepted { own_events: 2 },
            events: vec![event(1), event(2), event(3)],
            rules_version: 0,
            engine_build: common::build(),
        }])
        .await
        .unwrap();
    }
    for window in [1_000 * H, 1] {
        let mut cfg = config();
        cfg.key_window_ms = window;
        let (handle, task) = {
            let log = log.clone();
            voidforge_host::spawn(
                world.clone(),
                v0_rules(),
                move || log,
                clock_at(100 * H),
                cfg,
            )
            .unwrap()
        };
        let receipt = timeout(handle.submit_with_key(drill(home), key("seeded")))
            .await
            .unwrap();
        assert_eq!(receipt.seq, 1);
        assert_eq!(receipt.t, H);
        assert_eq!(
            receipt.outcome,
            Ok(vec![event(2), event(3)]),
            "window {window}"
        );
        drop(handle);
        task.join().unwrap();
    }
}
