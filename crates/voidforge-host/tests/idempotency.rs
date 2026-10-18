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
    IdempotencyKey, InvalidKey, LogError, LogRecord, LogStore, MemoryLog, RecordKind,
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
