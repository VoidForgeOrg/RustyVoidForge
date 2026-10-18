//! P2.3: the engine host's write path against the in-memory store (decision log D-036). One test
//! per step of the Architectural Design's write path and per crash-table row that applies before
//! idempotency (P2.4) and recovery (P2.5). The clock is manual, so every test is deterministic.

mod common;

use std::time::Duration;

use common::{EPOCH, GatedLog, PATIENCE, build, clock_at, config, settle};
use voidforge_core::command::{Action, Command, Rejection};
use voidforge_core::quantity::MS_PER_HOUR;
use voidforge_core::state::{BuildingKind, World};
use voidforge_host::{Fatal, HostError, ManualClock};
use voidforge_store::log::{LogRecord, MemoryLog, Outcome, Payload, RecordKind};
use voidforge_testkit::determinism::{Economy, slow_build_rules};
use voidforge_testkit::{ALICE, BOB, WorldBuilder, v0_rules};

const H: i64 = MS_PER_HOUR;

/// Alice's homeworld at time 0 under Rules v0.
fn alice() -> (World, voidforge_core::state::PlanetId) {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    (b.build(), home)
}

/// A solved economy under Rules v0 (seed 7, three planets): its first queue entry, at 1 h, fires
/// an event. Planet 1 is Alice's homeworld.
fn solved() -> (World, voidforge_core::state::PlanetId) {
    let e = Economy {
        seed: 7,
        planets: 3,
        fleets: false,
        commands: 0,
    };
    let w = e.world(&v0_rules());
    let home = voidforge_core::state::PlanetId(1);
    assert_eq!(w.planets[&home].owner, Some(ALICE));
    (w, home)
}

fn build_drill(planet: voidforge_core::state::PlanetId) -> Command {
    Command {
        player: ALICE,
        action: Action::BuildBuilding {
            planet,
            kind: BuildingKind::Drill,
        },
    }
}

fn join(player: voidforge_core::state::PlayerId) -> Command {
    Command {
        player,
        action: Action::JoinGame,
    }
}

async fn timeout<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(PATIENCE, f).await.expect("in time")
}

// --- Steps 3 to 6: stamp, step, append, reply ---

#[tokio::test]
async fn a_command_is_applied_logged_durably_then_acknowledged() {
    // The first queue entry (1 h) is due when the command is applied at 2 h, so the apply's own
    // advance runs it and the command's record holds those events first. To make the command,
    // not the timer, take that turn: queue it while the host is inside an earlier append; a
    // waiting command always goes before a due timer.
    let (world, home) = solved();
    let gated = GatedLog::closed(MemoryLog::new());
    let clock = clock_at(0);
    let (handle, task) = voidforge_host::spawn(
        world.clone(),
        v0_rules(),
        {
            let s = gated.clone();
            move || s
        },
        clock.clone(),
        config(),
    )
    .unwrap();
    let first = tokio::spawn({
        let handle = handle.clone();
        async move { handle.submit(join(ALICE)).await }
    });
    gated.entered(1).await;
    clock.set(EPOCH + 2 * H);
    let second = tokio::spawn({
        let handle = handle.clone();
        async move { handle.submit(build_drill(home)).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    gated.release(2);
    assert_eq!(timeout(first).await.unwrap().unwrap().seq, 1);
    let receipt = timeout(second).await.unwrap().unwrap();
    assert_eq!((receipt.seq, receipt.t), (2, 2 * H));

    // The same steps on a copy of the World: the record holds exactly what apply returned.
    let mut expected = world;
    expected.apply(&join(ALICE), 0, &v0_rules()).unwrap();
    let applied = expected
        .apply(&build_drill(home), 2 * H, &v0_rules())
        .unwrap();
    assert!(
        !applied.advanced.is_empty(),
        "the apply's own advance ran events"
    );
    let own = applied.outcome.clone().unwrap();
    assert_eq!(receipt.outcome, Ok(own.clone()));
    let mut events = applied.advanced.clone();
    events.extend(own);
    assert_eq!(
        gated.inner.snapshot()[1],
        LogRecord {
            seq: 2,
            kind: RecordKind::Command,
            game_time: 2 * H,
            wall_time_ms: EPOCH + 2 * H,
            player: Some(ALICE),
            idempotency_key: None,
            payload: Payload::Command(build_drill(home)),
            outcome: Outcome::Accepted,
            events,
            rules_version: 0,
            engine_build: build(),
        }
    );
    assert_eq!(
        gated.inner.snapshot().len(),
        2,
        "no separate advance record"
    );
    // 6. The read model is the World after the step.
    let rm = handle.read();
    assert_eq!((rm.seq, &*rm.world), (2, &expected));
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn a_rejected_command_is_logged_with_its_reason() {
    let (world, _) = alice();
    let log = MemoryLog::new();
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let s = log.clone();
            move || s
        },
        clock_at(H),
        config(),
    )
    .unwrap();
    let receipt = timeout(handle.submit(join(ALICE))).await.unwrap();
    assert_eq!(receipt.outcome, Err(Rejection::AlreadyJoined));
    let records = log.snapshot();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].outcome,
        Outcome::Rejected(Rejection::AlreadyJoined)
    );
    assert_eq!(records[0].kind, RecordKind::Command);
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn the_stamp_is_wall_clock_minus_epoch_and_never_goes_backwards() {
    let (world, home) = alice();
    let log = MemoryLog::new();
    let clock = clock_at(3 * H);
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let s = log.clone();
            move || s
        },
        clock.clone(),
        config(),
    )
    .unwrap();
    let a = timeout(handle.submit(build_drill(home))).await.unwrap();
    // The wall clock jumps back two hours: the stamp holds at the last t.
    clock.set(EPOCH + H);
    let b = timeout(handle.submit(join(ALICE))).await.unwrap();
    clock.set(EPOCH + 5 * H);
    let c = timeout(handle.submit(join(ALICE))).await.unwrap();
    assert_eq!([a.t, b.t, c.t], [3 * H, 3 * H, 5 * H]);
    let times: Vec<i64> = log.snapshot().iter().map(|r| r.game_time).collect();
    assert_eq!(times, vec![3 * H, 3 * H, 5 * H]);
    assert!(!task.is_finished(), "a clock going back is not an error");
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn the_stamp_never_precedes_the_world() {
    // A World ahead of the wall clock: commands are stamped at its time, never before it.
    let mut b = WorldBuilder::new().at(10 * H);
    b.player(ALICE);
    let (handle, task) =
        voidforge_host::spawn(b.build(), v0_rules(), MemoryLog::new, clock_at(H), config())
            .unwrap();
    assert_eq!(timeout(handle.submit(join(ALICE))).await.unwrap().t, 10 * H);
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn sequence_numbers_are_gap_free_from_one() {
    let (world, home) = alice();
    let log = MemoryLog::new();
    let clock = clock_at(H);
    let (mut handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let s = log.clone();
            move || s
        },
        clock.clone(),
        config(),
    )
    .unwrap();
    let mut seqs = Vec::new();
    for i in 0..5 {
        clock.set(EPOCH + H + i * H);
        settle(&mut handle, &clock).await;
        let command = if i % 2 == 0 {
            build_drill(home)
        } else {
            join(ALICE)
        };
        seqs.push(timeout(handle.submit(command)).await.unwrap().seq);
    }
    // Let due events run with no command, so advance records take numbers too.
    clock.set(EPOCH + 200 * H);
    settle(&mut handle, &clock).await;
    let logged: Vec<i64> = log.snapshot().iter().map(|r| r.seq).collect();
    let n = i64::try_from(logged.len()).unwrap();
    assert_eq!(logged, (1..=n).collect::<Vec<_>>(), "gap-free from 1");
    assert!(n > 5, "advance records were logged too");
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn sequence_numbers_continue_after_existing_records() {
    let (world, home) = alice();
    let log = MemoryLog::new();
    // A first host writes three records and stops.
    let clock = clock_at(H);
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let s = log.clone();
            move || s
        },
        clock.clone(),
        config(),
    )
    .unwrap();
    for _ in 0..3 {
        timeout(handle.submit(join(ALICE))).await.unwrap();
    }
    let world = handle.read().world.as_ref().clone();
    drop(handle);
    task.join().unwrap();
    // A second host on the same log, with that World (what P2.5's recovery will provide).
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let s = log.clone();
            move || s
        },
        clock,
        config(),
    )
    .unwrap();
    assert_eq!(
        timeout(handle.submit(build_drill(home))).await.unwrap().seq,
        4
    );
    drop(handle);
    task.join().unwrap();
    let seqs: Vec<i64> = log.snapshot().iter().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4]);
}

#[tokio::test]
async fn a_world_behind_the_log_is_refused_at_start() {
    let (world, _) = alice();
    let log = MemoryLog::new();
    let (handle, task) = voidforge_host::spawn(
        world.clone(),
        v0_rules(),
        {
            let s = log.clone();
            move || s
        },
        clock_at(5 * H),
        config(),
    )
    .unwrap();
    timeout(handle.submit(join(ALICE))).await.unwrap();
    drop(handle);
    task.join().unwrap();
    // The original World is at time 0, but the log's last record is at 5 h.
    let err = voidforge_host::spawn(world, v0_rules(), move || log, clock_at(5 * H), config())
        .unwrap_err();
    assert!(
        matches!(err, Fatal::WorldBehindLog { world_ms: 0, log_ms } if log_ms == 5 * H),
        "{err}"
    );
}

// --- Step 1: waking ---

#[tokio::test]
async fn due_events_wake_the_host_and_are_logged_as_an_advance() {
    let (world, _) = solved();
    let due = world.queue().first().unwrap().at_ms;
    let log = MemoryLog::new();
    let clock = clock_at(0);
    let (mut handle, task) = voidforge_host::spawn(
        world.clone(),
        v0_rules(),
        {
            let s = log.clone();
            move || s
        },
        clock.clone(),
        config(),
    )
    .unwrap();
    assert!(log.snapshot().is_empty(), "nothing is due yet");
    clock.set(EPOCH + due);
    settle(&mut handle, &clock).await;
    let mut expected = world;
    let events = expected.advance_to(due, &v0_rules()).unwrap();
    assert!(!events.is_empty());
    assert_eq!(
        log.snapshot(),
        vec![LogRecord {
            seq: 1,
            kind: RecordKind::Advance,
            game_time: due,
            wall_time_ms: EPOCH + due,
            player: None,
            idempotency_key: None,
            payload: Payload::Empty,
            outcome: Outcome::Empty,
            events,
            rules_version: 0,
            engine_build: build(),
        }]
    );
    assert_eq!(*handle.read().world, expected);
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn an_eventless_advance_is_not_logged_but_the_read_model_is_published() {
    // The storage-full crossing at 17,999,999 ms in this economy fires no event but moves planet
    // 1's valid_until (the lead's Q7 correction): views must not go Stale until the next command.
    let rules = slow_build_rules();
    let e = Economy {
        seed: 7,
        planets: 3,
        fleets: false,
        commands: 0,
    };
    let mut world = e.world(&rules);
    // Run the first (eventful) entry directly, so the crossing is next.
    let first = world.queue().first().unwrap().at_ms;
    world.advance_to(first, &rules).unwrap();
    let crossing = world.queue().first().unwrap().at_ms;
    let planet = voidforge_core::state::PlanetId(1);
    assert_eq!(world.planet_valid_until(planet), Some(crossing));
    let mut expected = world.clone();
    assert!(
        expected.advance_to(crossing, &rules).unwrap().is_empty(),
        "eventless"
    );
    let after = expected.planet_valid_until(planet);
    assert!(after > Some(crossing), "the crossing moves valid_until");

    let log = MemoryLog::new();
    let clock = clock_at(first);
    let (mut handle, task) = voidforge_host::spawn(
        world,
        rules.clone(),
        {
            let s = log.clone();
            move || s
        },
        clock.clone(),
        config(),
    )
    .unwrap();
    clock.set(EPOCH + crossing);
    settle(&mut handle, &clock).await;
    let rm = handle.read();
    assert!(
        log.snapshot().is_empty(),
        "an eventless advance is not logged"
    );
    assert_eq!(rm.world.time_ms, crossing);
    assert_eq!(rm.world.planet_valid_until(planet), after);
    // So a view at the new time is exact, not Stale.
    let query = voidforge_core::view::Query::Planet { planet };
    assert!(rm.world.view(&query, crossing, &rules).is_ok());
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn an_idle_host_does_not_wake() {
    let (world, home) = alice();
    let (handle, task) =
        voidforge_host::spawn(world, v0_rules(), MemoryLog::new, clock_at(0), config()).unwrap();
    // Nothing due and nothing submitted: no turns, however long it waits.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(handle.stats().wakes(), 0);
    timeout(handle.submit(build_drill(home))).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(handle.stats().wakes(), 1, "one command, one turn");
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn an_empty_queue_sleeps_until_a_command() {
    let world = World::new(0, 0, 1);
    assert!(world.queue().is_empty());
    let (handle, task) =
        voidforge_host::spawn(world, v0_rules(), MemoryLog::new, clock_at(0), config()).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(handle.stats().wakes(), 0);
    drop(handle);
    task.join().unwrap();
}

// --- Step 5: group commit, durability before acknowledgement ---

#[tokio::test]
async fn nothing_is_acknowledged_before_its_record_is_durable() {
    let (world, home) = alice();
    let gated = GatedLog::closed(MemoryLog::new());
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let s = gated.clone();
            move || s
        },
        clock_at(H),
        config(),
    )
    .unwrap();
    let submit = tokio::spawn({
        let handle = handle.clone();
        async move { handle.submit(build_drill(home)).await }
    });
    gated.entered(1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !submit.is_finished(),
        "no reply while the append is not durable"
    );
    assert_eq!(handle.read().seq, 0, "nor a read model");
    assert!(gated.inner.snapshot().is_empty());
    gated.release(1);
    let receipt = timeout(submit).await.unwrap().unwrap();
    assert_eq!(receipt.seq, 1);
    assert_eq!(gated.inner.snapshot().len(), 1);
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn commands_waiting_together_share_one_durable_write() {
    let (world, home) = alice();
    let gated = GatedLog::closed(MemoryLog::new());
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let s = gated.clone();
            move || s
        },
        clock_at(H),
        config(),
    )
    .unwrap();
    // The first command holds the host inside its append while ten more queue up.
    let first = tokio::spawn({
        let handle = handle.clone();
        async move { handle.submit(build_drill(home)).await }
    });
    gated.entered(1).await;
    let mut rest = Vec::new();
    for _ in 0..10 {
        let handle = handle.clone();
        rest.push(tokio::spawn(
            async move { handle.submit(join(ALICE)).await },
        ));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    gated.release(2);
    assert_eq!(timeout(first).await.unwrap().unwrap().seq, 1);
    let mut seqs = Vec::new();
    for r in rest {
        seqs.push(timeout(r).await.unwrap().unwrap().seq);
    }
    seqs.sort_unstable();
    assert_eq!(seqs, (2..=11).collect::<Vec<_>>());
    assert_eq!(gated.inner.appends(), 2, "ten records, one write");
    assert_eq!(handle.stats().appends(), 2);
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn a_turn_takes_at_most_max_batch_commands() {
    let (world, home) = alice();
    let gated = GatedLog::closed(MemoryLog::new());
    let mut cfg = config();
    cfg.max_batch = 4;
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let s = gated.clone();
            move || s
        },
        clock_at(H),
        cfg,
    )
    .unwrap();
    let first = tokio::spawn({
        let handle = handle.clone();
        async move { handle.submit(build_drill(home)).await }
    });
    gated.entered(1).await;
    let mut rest = Vec::new();
    for _ in 0..10 {
        let handle = handle.clone();
        rest.push(tokio::spawn(
            async move { handle.submit(join(ALICE)).await },
        ));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    gated.release(10);
    timeout(first).await.unwrap().unwrap();
    for r in rest {
        timeout(r).await.unwrap().unwrap();
    }
    // 1, then 4 + 4 + 2.
    assert_eq!(gated.inner.appends(), 4);
    drop(handle);
    task.join().unwrap();
}

// --- The crash table and fatal errors ---

#[tokio::test]
async fn a_failed_append_stops_the_host_and_acknowledges_nothing() {
    let (world, home) = alice();
    let log = MemoryLog::new();
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let s = log.clone();
            move || s
        },
        clock_at(H),
        config(),
    )
    .unwrap();
    timeout(handle.submit(join(ALICE))).await.unwrap();
    log.fail_appends(true);
    // Crash before the append: the command never happened, and its caller is not acknowledged.
    assert_eq!(
        timeout(handle.submit(build_drill(home))).await,
        Err(HostError::Stopped)
    );
    assert_eq!(log.snapshot().len(), 1, "no record for it");
    assert!(matches!(task.join(), Err(Fatal::Append(_))));
    // The host is gone: later commands are refused, not queued.
    assert_eq!(handle.submit(join(ALICE)).await, Err(HostError::Stopped));
    assert_eq!(
        handle.read().seq,
        1,
        "the read model never showed the failed step"
    );
}

#[tokio::test]
async fn an_engine_failure_stops_the_host_after_the_turns_earlier_steps() {
    // A demolition ending after the end of time poisons the World (the commands suite's case).
    let mut b = WorldBuilder::new().at(i64::MAX - 1);
    let home = b.player(ALICE);
    let world = b.build();
    let drill = *world
        .buildings
        .iter()
        .find(|(_, b)| b.planet == home && b.kind == BuildingKind::Drill)
        .unwrap()
        .0;
    let poison = Command {
        player: ALICE,
        action: Action::DemolishBuilding { building: drill },
    };
    let gated = GatedLog::closed(MemoryLog::new());
    let clock = ManualClock::new(i64::MAX - 1);
    let mut cfg = config();
    cfg.epoch_unix_ms = 0;
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let s = gated.clone();
            move || s
        },
        clock,
        cfg,
    )
    .unwrap();
    let hold = tokio::spawn({
        let handle = handle.clone();
        async move { handle.submit(join(BOB)).await }
    });
    gated.entered(1).await;
    // One turn: an ordinary command, the poisoning one, and one after it.
    let spawn = |c: Command| {
        let handle = handle.clone();
        tokio::spawn(async move { handle.submit(c).await })
    };
    let ok = spawn(join(ALICE));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let bad = spawn(poison);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after = spawn(join(ALICE));
    tokio::time::sleep(Duration::from_millis(100)).await;
    gated.release(2);
    assert!(timeout(hold).await.unwrap().is_ok());
    let ok = timeout(ok).await.unwrap().unwrap();
    assert_eq!(
        ok.outcome,
        Err(Rejection::AlreadyJoined),
        "the earlier step is acknowledged"
    );
    assert_eq!(timeout(bad).await.unwrap(), Err(HostError::Stopped));
    assert_eq!(timeout(after).await.unwrap(), Err(HostError::Stopped));
    assert!(matches!(task.join(), Err(Fatal::Engine(_))));
    let seqs: Vec<i64> = gated.inner.snapshot().iter().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![1, 2], "the poisoning command is not logged");
}

#[tokio::test]
async fn a_full_inbox_is_busy() {
    let (world, home) = alice();
    let gated = GatedLog::closed(MemoryLog::new());
    let mut cfg = config();
    cfg.inbox_capacity = 2;
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let s = gated.clone();
            move || s
        },
        clock_at(H),
        cfg,
    )
    .unwrap();
    let hold = tokio::spawn({
        let handle = handle.clone();
        async move { handle.submit(build_drill(home)).await }
    });
    gated.entered(1).await;
    let mut queued = Vec::new();
    for _ in 0..2 {
        let handle = handle.clone();
        queued.push(tokio::spawn(
            async move { handle.submit(join(ALICE)).await },
        ));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        handle.submit(join(ALICE)).await,
        Err(HostError::Busy),
        "at once, not queued"
    );
    gated.release(2);
    timeout(hold).await.unwrap().unwrap();
    for q in queued {
        timeout(q).await.unwrap().unwrap();
    }
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn the_host_stops_when_every_handle_is_gone() {
    let (world, _) = alice();
    let (handle, task) =
        voidforge_host::spawn(world, v0_rules(), MemoryLog::new, clock_at(0), config()).unwrap();
    let other = handle.clone();
    drop(handle);
    assert!(!task.is_finished());
    drop(other);
    task.join().unwrap();
}

// --- Step 6: the read model ---

#[tokio::test]
async fn the_read_model_follows_every_turn() {
    let (world, home) = alice();
    let clock = clock_at(H);
    let (mut handle, task) = voidforge_host::spawn(
        world.clone(),
        v0_rules(),
        MemoryLog::new,
        clock.clone(),
        config(),
    )
    .unwrap();
    assert_eq!((handle.read().seq, &*handle.read().world), (0, &world));
    let r = timeout(handle.submit(build_drill(home))).await.unwrap();
    let rm = handle.read();
    assert_eq!((rm.seq, rm.world.time_ms), (r.seq, r.t));
    // A view at the published state succeeds through the read model, not the inbox.
    let query = voidforge_core::view::Query::Planet { planet: home };
    assert!(rm.world.view(&query, r.t, &rm.rules).is_ok());
    clock.set(EPOCH + 300 * H);
    settle(&mut handle, &clock).await;
    assert!(handle.read().world.time_ms > r.t);
    drop(handle);
    task.join().unwrap();
}
