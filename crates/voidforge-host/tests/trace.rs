//! P5.1 (D-064): one command's trace through the host, captured in memory. The caller's span is
//! the parent of the inbox wait and of the command; the step and the append are the command's
//! children; a batch's one append shows in each command's trace; no idempotency key is an
//! attribute. Each test finds its own spans by trace id, so they can share a process.

mod common;

use std::time::Duration;

use common::{GatedLog, PATIENCE, clock_at, config};
use tracing::Instrument as _;
use voidforge_core::command::{Action, Command};
use voidforge_core::quantity::MS_PER_HOUR;
use voidforge_core::state::{BuildingKind, PlanetId};
use voidforge_store::log::{IdempotencyKey, MemoryLog};
use voidforge_telemetry::testing::{SpanData, SpanId, capture};
use voidforge_testkit::{ALICE, BOB, WorldBuilder, v0_rules};

const KEY: &str = "p51-secret-idempotency-key";

fn world() -> (voidforge_core::state::World, PlanetId, PlanetId) {
    let mut b = WorldBuilder::new();
    let alice = b.player(ALICE);
    let bob = b.player(BOB);
    (b.build(), alice, bob)
}

fn drill(player: voidforge_core::state::PlayerId, planet: PlanetId) -> Command {
    Command {
        player,
        action: Action::BuildBuilding {
            planet,
            kind: BuildingKind::Drill,
        },
    }
}

fn named<'a>(spans: &'a [SpanData], name: &str) -> Vec<&'a SpanData> {
    spans.iter().filter(|s| s.name == name).collect()
}

fn one<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
    let found = named(spans, name);
    assert_eq!(
        found.len(),
        1,
        "{name}: {:?}",
        spans.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
    found[0]
}

fn attr(span: &SpanData, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.to_string())
}

fn id(span: &SpanData) -> SpanId {
    span.span_context.span_id()
}

async fn in_time<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(PATIENCE, f).await.expect("in time")
}

#[tokio::test]
async fn a_command_s_trace_runs_from_its_caller_through_the_step_and_the_append() {
    let captured = capture();
    let (world, home, _) = world();
    // A snapshot after every record, slow: the turn stays open 300 ms after its reply, so a
    // command span that closed only with the turn would be missing when the caller hears.
    let log = GatedLog::closed(MemoryLog::new()).slow_snapshots(Duration::from_millis(300));
    log.release(16);
    let mut config = config();
    config.snapshot_every_records = 1;
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let log = log.clone();
            move || log
        },
        clock_at(MS_PER_HOUR),
        config,
    )
    .unwrap();
    let request = tracing::info_span!("test.request");
    let trace = voidforge_telemetry::trace_id(&request).expect("exported");
    let receipt = in_time(
        handle
            .submit_with_key(drill(ALICE, home), IdempotencyKey::new(KEY).unwrap())
            .instrument(request.clone()),
    )
    .await
    .unwrap();
    drop(request);
    let spans = captured.trace(&trace);
    let request = one(&spans, "test.request");
    let inbox = one(&spans, "host.inbox");
    let command = one(&spans, "host.command");
    let step = one(&spans, "host.step");
    let append = one(&spans, "host.append");
    assert_eq!(
        inbox.parent_span_id,
        id(request),
        "the inbox wait is the caller's child"
    );
    assert_eq!(
        command.parent_span_id,
        id(request),
        "the command is the caller's child"
    );
    assert_eq!(step.parent_span_id, id(command));
    assert_eq!(append.parent_span_id, id(command));
    assert_eq!(
        attr(command, "command.type").as_deref(),
        Some("build_building")
    );
    assert_eq!(attr(command, "player_id"), Some(ALICE.0.to_string()));
    assert_eq!(attr(command, "seq"), Some(receipt.seq.to_string()));
    assert_eq!(attr(command, "outcome").as_deref(), Some("accepted"));
    assert_eq!(attr(append, "batch.size").as_deref(), Some("1"));
    // M1: the key never reaches a span, whole or in part.
    for span in &spans {
        for kv in &span.attributes {
            assert!(
                !kv.value.to_string().contains(&KEY[..12]),
                "{} carries the key in {}",
                span.name,
                kv.key
            );
        }
    }
    // The turn is a root of its own, linked to the command it carried. It ends after the reply
    // (its snapshot), so wait for it.
    let linked = |s: &SpanData| {
        s.name == "host.turn"
            && s.links
                .iter()
                .any(|l| l.span_context.trace_id().to_string() == trace)
    };
    let turns: Vec<SpanData> = in_time(async {
        loop {
            let turns: Vec<SpanData> = captured.spans().into_iter().filter(linked).collect();
            if !turns.is_empty() {
                break turns;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert_eq!(turns.len(), 1, "one turn linked to this trace");
    assert_eq!(turns[0].parent_span_id, SpanId::INVALID);
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn two_commands_in_one_turn_each_show_the_shared_append() {
    let captured = capture();
    let (world, alice_home, bob_home) = world();
    let gated = GatedLog::closed(MemoryLog::new());
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let gated = gated.clone();
            move || gated
        },
        clock_at(MS_PER_HOUR),
        config(),
    )
    .unwrap();
    // A first command holds the host in its append, so the next two arrive as one batch.
    let hold = tokio::spawn({
        let h = handle.clone();
        async move { h.submit(drill(ALICE, alice_home)).await }
    });
    gated.entered(1).await;
    let mut traces = Vec::new();
    let mut sent = Vec::new();
    for (player, planet) in [(ALICE, alice_home), (BOB, bob_home)] {
        let span = tracing::info_span!("test.request");
        traces.push(voidforge_telemetry::trace_id(&span).unwrap());
        let h = handle.clone();
        sent.push(tokio::spawn(
            async move {
                h.submit(Command {
                    player,
                    action: Action::BuildBuilding {
                        planet,
                        kind: BuildingKind::Refinery,
                    },
                })
                .await
            }
            .instrument(span),
        ));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    gated.release(2);
    in_time(hold).await.unwrap().unwrap();
    for s in sent {
        in_time(s).await.unwrap().unwrap();
    }
    for trace in traces {
        let spans = captured.trace(&trace);
        let command = one(&spans, "host.command");
        let append = one(&spans, "host.append");
        assert_eq!(append.parent_span_id, id(command));
        assert_eq!(
            attr(append, "batch.size").as_deref(),
            Some("2"),
            "the append was shared"
        );
    }
    drop(handle);
    task.join().unwrap();
}

#[tokio::test]
async fn a_replayed_key_has_a_command_span_and_no_step_or_append() {
    let captured = capture();
    let (world, home, _) = world();
    let log = MemoryLog::new();
    let (handle, task) = voidforge_host::spawn(
        world,
        v0_rules(),
        {
            let log = log.clone();
            move || log
        },
        clock_at(MS_PER_HOUR),
        config(),
    )
    .unwrap();
    let key = || IdempotencyKey::new("p51-replay").unwrap();
    let first = in_time(handle.submit_with_key(drill(ALICE, home), key()))
        .await
        .unwrap();
    let again = tracing::info_span!("test.request");
    let trace = voidforge_telemetry::trace_id(&again).unwrap();
    in_time(
        handle
            .submit_with_key(drill(ALICE, home), key())
            .instrument(again.clone()),
    )
    .await
    .unwrap();
    drop(again);
    let spans = captured.trace(&trace);
    let command = one(&spans, "host.command");
    assert_eq!(attr(command, "outcome").as_deref(), Some("replayed"));
    assert_eq!(attr(command, "seq"), Some(first.seq.to_string()));
    assert!(named(&spans, "host.step").is_empty() && named(&spans, "host.append").is_empty());
    drop(handle);
    task.join().unwrap();
}
