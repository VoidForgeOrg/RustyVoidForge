//! P5.1 (D-064): the host's five metrics move when the game does. One test in its own binary:
//! the gauges are registered on the process's global meter, so a second host in the same process
//! would add its own readings.

mod common;

use common::{EPOCH, GatedLog, PATIENCE, clock_at};
use voidforge_core::command::{Action, Command};
use voidforge_core::quantity::MS_PER_HOUR;
use voidforge_core::state::BuildingKind;
use voidforge_store::log::MemoryLog;
use voidforge_telemetry::testing::capture;
use voidforge_testkit::{ALICE, BOB, WorldBuilder, v0_rules};

fn value(name: &str) -> Option<f64> {
    capture().points(name).first().map(|(v, _)| *v)
}

fn count(name: &str, attr: &str) -> f64 {
    capture()
        .points(name)
        .iter()
        .filter(|(_, a)| attr.is_empty() || a.iter().any(|x| x == attr))
        .map(|(v, _)| *v)
        .sum()
}

#[tokio::test]
async fn the_five_metrics_move_with_the_game() {
    capture();
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let bobs = b.player(BOB);
    let world = b.build();
    let drill = move || Command {
        player: ALICE,
        action: Action::BuildBuilding {
            planet: home,
            kind: BuildingKind::Drill,
        },
    };
    let gated = GatedLog::closed(MemoryLog::new());
    let clock = clock_at(MS_PER_HOUR);
    let mut config = common::config();
    config.snapshot_every_records = 1;
    // Room for two waiting commands, so a third is refused as busy.
    config.inbox_capacity = 2;
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let gated = gated.clone();
            move || gated
        },
        clock.clone(),
        config,
    )
    .unwrap();
    assert_eq!(value("voidforge.host.inbox.depth"), Some(0.0));
    assert_eq!(value("voidforge.host.event.lag"), Some(0.0), "nothing due");
    assert_eq!(
        value("voidforge.host.snapshot.age"),
        None,
        "no snapshot yet"
    );

    // Two drills: command steps, appends, a snapshot after each (every record), and
    // two planets' entries in the queue, due at different times.
    gated.release(2);
    tokio::time::timeout(PATIENCE, handle.submit(drill()))
        .await
        .unwrap()
        .unwrap();
    // A minute later, on Bob's planet, so its entry comes due well after Alice's.
    clock.set(EPOCH + MS_PER_HOUR + 60_000);
    let refinery = Command {
        player: BOB,
        action: Action::BuildBuilding {
            planet: bobs,
            kind: BuildingKind::Drill,
        },
    };
    tokio::time::timeout(PATIENCE, handle.submit(refinery))
        .await
        .unwrap()
        .unwrap();
    assert!(count("voidforge.host.step.duration", "kind=command") >= 1.0);
    assert!(count("voidforge.host.append.duration", "") >= 1.0);
    assert_eq!(value("voidforge.host.snapshot.age"), Some(0.0));
    let due = handle
        .read()
        .world
        .queue()
        .first()
        .map(|e| e.at_ms)
        .expect("a construction is queued");
    let newest = handle.read().world.queue().last().map(|e| e.at_ms).unwrap();
    let all: Vec<i64> = handle
        .read()
        .world
        .queue()
        .iter()
        .map(|e| e.at_ms)
        .collect();
    assert!(
        newest > due + 5_000,
        "two entries far apart, so lag must come from the oldest: {all:?}"
    );

    // Hold the host in its next append: commands wait in the inbox, and time passes the queued
    // entry's due time without the host processing it.
    let held = tokio::spawn({
        let h = handle.clone();
        async move { h.submit(drill()).await }
    });
    gated.entered(3).await;
    let waiting: Vec<_> = (0..2)
        .map(|_| {
            let h = handle.clone();
            tokio::spawn(async move { h.submit(drill()).await })
        })
        .collect();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    // The inbox is full: a third is refused at once, and isn't counted as waiting.
    assert_eq!(
        handle.submit(drill()).await,
        Err(voidforge_host::HostError::Busy)
    );
    assert_eq!(
        value("voidforge.host.inbox.depth"),
        Some(2.0),
        "two commands wait"
    );
    clock.set(EPOCH + due + 5_000);
    assert_eq!(
        value("voidforge.host.event.lag"),
        Some(5_000.0),
        "the oldest due entry waited 5 s"
    );
    let age = value("voidforge.host.snapshot.age").unwrap();
    assert!(age >= 5.0, "the snapshot ages with game time: {age}");

    gated.release(10);
    tokio::time::timeout(PATIENCE, held)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    for w in waiting {
        tokio::time::timeout(PATIENCE, w)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    // The host catches up: the inbox empties, and no entry is overdue.
    let caught_up = tokio::time::timeout(PATIENCE, async {
        loop {
            if value("voidforge.host.inbox.depth") == Some(0.0)
                && value("voidforge.host.event.lag") == Some(0.0)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        caught_up.is_ok(),
        "depth {:?}, lag {:?}",
        value("voidforge.host.inbox.depth"),
        value("voidforge.host.event.lag")
    );
    // A command step after the due time ran the due entry inside it. With no command, the host
    // wakes on its own for the next one: a time-only step.
    let next = handle
        .read()
        .world
        .queue()
        .first()
        .map(|e| e.at_ms)
        .expect("more is queued");
    clock.set(EPOCH + next + 1);
    let advanced = tokio::time::timeout(PATIENCE, async {
        while count("voidforge.host.step.duration", "kind=advance") < 1.0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        advanced.is_ok(),
        "a time-only step is measured as kind=advance"
    );
    drop(handle);
    task.join().unwrap();
}
