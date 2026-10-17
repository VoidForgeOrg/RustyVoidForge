//! Event loop tests, on the hand-built sample world: same-instant order, stale skips,
//! `advance_to` boundaries, the scheduling guard, poison, and a round trip part-way through.
//!
//! The sample world at 10 ms has planet 1's next event at 100 (a stale older entry at 50), fleet
//! 13 arriving at 200, fleet 14 returning at 150 and fleet 15 arriving at 300.

use super::super::tests::{json, reload, sample_world};
use super::*;
use crate::state::{FleetState, PlanetId, ShipId};

const P1: PlanetId = PlanetId(1);
const P2: PlanetId = PlanetId(2);
const P3: PlanetId = PlanetId(3);
const F13: FleetId = FleetId(13);
const F14: FleetId = FleetId(14);
const F15: FleetId = FleetId(15);

/// Steps that run a hook, then report (instant, kind) as their event.
struct Probe<F>(F);

impl<F: FnMut(&mut World, QueueKind) -> Result<(), EngineError>> Probe<F> {
    fn step(
        &mut self,
        w: &mut World,
        kind: QueueKind,
    ) -> Result<Vec<(i64, QueueKind)>, EngineError> {
        (self.0)(w, kind)?;
        Ok(vec![(w.time_ms, kind)])
    }
}

impl<F: FnMut(&mut World, QueueKind) -> Result<(), EngineError>> Steps for Probe<F> {
    type Event = (i64, QueueKind);

    fn planet_change(
        &mut self,
        w: &mut World,
        p: PlanetId,
    ) -> Result<Vec<Self::Event>, EngineError> {
        self.step(w, QueueKind::PlanetChange(p))
    }

    fn fleet_arrival(
        &mut self,
        w: &mut World,
        f: FleetId,
    ) -> Result<Vec<Self::Event>, EngineError> {
        self.step(w, QueueKind::FleetArrival(f))
    }

    fn fleet_return(&mut self, w: &mut World, f: FleetId) -> Result<Vec<Self::Event>, EngineError> {
        self.step(w, QueueKind::FleetReturn(f))
    }
}

/// The least a step must do: a fleet that arrives or returns is stationed there. A planet keeps
/// no next event.
fn settle(w: &mut World, kind: QueueKind) -> Result<(), EngineError> {
    if let QueueKind::FleetArrival(id) | QueueKind::FleetReturn(id) = kind {
        let fleet = w.fleets.get_mut(&id).unwrap();
        let at = match fleet.state {
            FleetState::InTransit { target, .. } => target,
            FleetState::Returning { origin, .. } => origin,
            FleetState::Stationed { .. } => panic!("fleet {} is already stationed", id.0),
        };
        fleet.state = FleetState::Stationed { at };
    }
    Ok(())
}

/// A step hook as a plain function.
type Hook = fn(&mut World, QueueKind) -> Result<(), EngineError>;

fn settled() -> Probe<Hook> {
    Probe(settle)
}

/// Moves a fleet's arrival (or return) to `at_ms` and schedules it.
fn reschedule_fleet(w: &mut World, id: FleetId, at_ms: i64) {
    match &mut w.fleets.get_mut(&id).unwrap().state {
        FleetState::InTransit { arrives_at_ms, .. }
        | FleetState::Returning { arrives_at_ms, .. } => *arrives_at_ms = at_ms,
        FleetState::Stationed { .. } => panic!("fleet {} is stationed", id.0),
    }
    w.schedule_fleet(id).unwrap();
}

/// The sample world with every planet and fleet due at 500 ms, scheduled in `order`.
fn crowded(order: &[QueueKind]) -> World {
    let mut w = sample_world();
    for kind in order {
        match *kind {
            QueueKind::PlanetChange(p) => w.schedule_planet(p, Some(500)).unwrap(),
            QueueKind::FleetArrival(f) | QueueKind::FleetReturn(f) => {
                reschedule_fleet(&mut w, f, 500)
            }
        }
    }
    w
}

#[test]
fn at_one_instant_planets_run_by_id_then_fleet_entries_by_fleet_id() {
    let order = [
        QueueKind::FleetArrival(F15),
        QueueKind::PlanetChange(P3),
        QueueKind::FleetReturn(F14),
        QueueKind::PlanetChange(P1),
        QueueKind::FleetArrival(F13),
        QueueKind::PlanetChange(P2),
    ];
    let reversed: Vec<QueueKind> = order.iter().rev().copied().collect();
    let mut forward = crowded(&order);
    let mut backward = crowded(&reversed);

    let expected = [
        (500, QueueKind::PlanetChange(P1)),
        (500, QueueKind::PlanetChange(P2)),
        (500, QueueKind::PlanetChange(P3)),
        // Arrivals and returns share a rank: 13's arrival, 14's return, 15's arrival.
        (500, QueueKind::FleetArrival(F13)),
        (500, QueueKind::FleetReturn(F14)),
        (500, QueueKind::FleetArrival(F15)),
    ];
    assert_eq!(forward.advance_with(500, &mut settled()).unwrap(), expected);
    assert_eq!(
        backward.advance_with(500, &mut settled()).unwrap(),
        expected
    );
    assert_eq!(forward, backward);
}

#[test]
fn earlier_entries_run_first_whatever_their_rank() {
    let mut w = sample_world();
    w.schedule_planet(P2, Some(250)).unwrap();
    let events = w.advance_with(400, &mut settled()).unwrap();
    assert_eq!(
        events,
        [
            (100, QueueKind::PlanetChange(P1)),
            (150, QueueKind::FleetReturn(F14)),
            (200, QueueKind::FleetArrival(F13)),
            (250, QueueKind::PlanetChange(P2)),
            (300, QueueKind::FleetArrival(F15)),
        ]
    );
}

#[test]
fn work_a_step_schedules_runs_in_the_same_advance() {
    let mut w = sample_world();
    let mut steps = Probe(|w: &mut World, kind| {
        if kind == QueueKind::PlanetChange(P1) && w.time_ms < 130 {
            w.schedule_planet(P1, Some(w.time_ms + 10))?;
        }
        settle(w, kind)
    });
    let events = w.advance_with(140, &mut steps).unwrap();
    let planet: Vec<i64> = events
        .iter()
        .filter(|(_, k)| *k == QueueKind::PlanetChange(P1))
        .map(|(t, _)| *t)
        .collect();
    assert_eq!(planet, [100, 110, 120, 130]);
    assert_eq!(w.planets[&P1].next_event, None);
}

#[test]
fn a_rescheduled_planet_skips_its_old_entries() {
    let mut w = sample_world();
    w.schedule_planet(P1, Some(300)).unwrap(); // the 100 ms entry (version 3) goes stale
    let events = w.advance_with(299, &mut settled()).unwrap();
    assert!(
        !events
            .iter()
            .any(|(_, k)| *k == QueueKind::PlanetChange(P1)),
        "{events:?}"
    );
    assert_eq!(
        w.advance_with(300, &mut settled()).unwrap(),
        [
            (300, QueueKind::PlanetChange(P1)),
            (300, QueueKind::FleetArrival(F15))
        ]
    );
}

#[test]
fn a_cleared_planet_event_is_skipped() {
    let mut w = sample_world();
    w.schedule_planet(P1, None).unwrap();
    assert_eq!(w.planets[&P1].next_event, None);
    w.check_links().unwrap();
    let events = w.advance_with(120, &mut settled()).unwrap();
    assert_eq!(events, []);
}

#[test]
fn a_recalled_fleet_skips_its_arrival() {
    let mut w = sample_world();
    // Recall fleet 13 (left at 0, due at 200) at 10: it returns at 20.
    w.fleets.get_mut(&F13).unwrap().state = FleetState::Returning {
        origin: P1,
        arrives_at_ms: 20,
    };
    w.schedule_fleet(F13).unwrap();
    let events = w.advance_with(400, &mut settled()).unwrap();
    let fleet13: Vec<_> = events.iter().filter(|(_, k)| k.entity() == 13).collect();
    assert_eq!(fleet13, [&(20, QueueKind::FleetReturn(F13))]);
    assert_eq!(w.fleets[&F13].state, FleetState::Stationed { at: P1 });
}

#[test]
fn a_removed_fleet_skips_its_entries() {
    let mut w = sample_world();
    w.fleets.remove(&F15);
    w.ships.remove(&ShipId(16));
    let events = w.advance_with(400, &mut settled()).unwrap();
    assert!(!events.iter().any(|(_, k)| k.entity() == 15), "{events:?}");
    assert!(w.queue().is_empty());
}

#[test]
fn advancing_to_now_does_nothing() {
    let mut w = sample_world();
    let before = w.clone();
    assert_eq!(w.advance_with(10, &mut settled()).unwrap(), []);
    assert_eq!(w.advance_to(10).unwrap(), []);
    assert_eq!(w, before);
}

#[test]
fn advancing_backwards_is_an_error_that_changes_nothing() {
    let mut w = sample_world();
    let before = w.clone();
    assert_eq!(
        w.advance_with(9, &mut settled()),
        Err(EngineError::TimeBeforeNow { t: 9, now: 10 })
    );
    assert_eq!(
        w.advance_to(9),
        Err(EngineError::TimeBeforeNow { t: 9, now: 10 })
    );
    assert!(!w.is_poisoned());
    assert_eq!(w, before);
}

#[test]
fn entries_at_t_run_and_later_ones_wait() {
    let mut w = sample_world();
    let events = w.advance_with(150, &mut settled()).unwrap();
    assert_eq!(
        events,
        [
            (100, QueueKind::PlanetChange(P1)),
            (150, QueueKind::FleetReturn(F14)),
        ]
    );
    assert_eq!(w.time_ms, 150);
    let left: Vec<_> = w
        .queue()
        .iter()
        .map(|e| (e.at_ms, e.kind, e.version))
        .collect();
    assert_eq!(
        left,
        [
            (200, QueueKind::FleetArrival(F13), 1),
            (300, QueueKind::FleetArrival(F15), 1),
        ]
    );
    assert_eq!(w.fleets[&F13].version, 1);
    w.check_links().unwrap();
}

/// The planet keeps rescheduling itself, so the run is more than a replay of the queue.
fn busy(w: &mut World, kind: QueueKind) -> Result<(), EngineError> {
    if kind == QueueKind::PlanetChange(P1) && w.time_ms < 300 {
        w.schedule_planet(P1, Some(w.time_ms + 75))?;
    }
    settle(w, kind)
}

#[test]
fn a_world_part_way_through_round_trips_and_finishes_the_same() {
    let mut straight = sample_world();
    let all = straight.advance_with(400, &mut Probe(busy)).unwrap();

    let mut halfway = sample_world();
    let mut events = halfway.advance_with(160, &mut Probe(busy)).unwrap();
    assert_eq!(halfway.planets[&P1].next_event.map(|e| e.at_ms), Some(175));
    let mut restored = reload(&json(&halfway)).expect("a world between instants loads");
    assert_eq!(restored, halfway);
    events.extend(restored.advance_with(400, &mut Probe(busy)).unwrap());

    assert_eq!(events, all);
    assert_eq!(restored, straight);
    assert_eq!(straight.time_ms, 400);
}

#[test]
fn scheduling_at_or_before_the_instant_being_processed_stops_and_poisons() {
    for offset in [0, -1] {
        let mut w = sample_world();
        let mut steps = Probe(|w: &mut World, kind| {
            if kind == QueueKind::PlanetChange(P1) {
                w.schedule_planet(P1, Some(w.time_ms + offset))?;
            }
            settle(w, kind)
        });
        assert_eq!(
            w.advance_with(400, &mut steps),
            Err(EngineError::NotAfterNow {
                at_ms: 100 + offset,
                now: 100
            })
        );
        assert!(w.is_poisoned());
        assert_eq!(
            w.advance_with(400, &mut settled()),
            Err(EngineError::Poisoned)
        );
        assert_eq!(w.advance_to(400), Err(EngineError::Poisoned));
        assert_eq!(
            w.schedule_planet(P2, Some(1_000)),
            Err(EngineError::Poisoned)
        );
        assert_eq!(w.schedule_fleet(F13), Err(EngineError::Poisoned));
        let error = serde_json::to_string(&w).expect_err("a poisoned world does not serialize");
        assert!(error.to_string().contains("poisoned"), "{error}");
    }
}

#[test]
fn scheduling_outside_an_advance_is_checked_and_changes_nothing_on_error() {
    let mut w = sample_world();
    w.fleets.get_mut(&F15).unwrap().state = FleetState::Stationed { at: P2 };
    let before = w.clone();
    assert_eq!(
        w.schedule_planet(P2, Some(10)),
        Err(EngineError::NotAfterNow { at_ms: 10, now: 10 })
    );
    reschedule_fleet_expecting(&mut w, F13, 10);
    assert_eq!(w.schedule_fleet(F15), Err(EngineError::FleetNotMoving(F15)));
    assert!(matches!(
        w.schedule_planet(PlanetId(99), Some(20)),
        Err(EngineError::State(_))
    ));
    assert!(!w.is_poisoned());
    let mut w13 = w.clone();
    let FleetState::InTransit { arrives_at_ms, .. } = &mut w13.fleets.get_mut(&F13).unwrap().state
    else {
        unreachable!()
    };
    *arrives_at_ms = 200;
    assert_eq!(w13, before);
}

/// Sets fleet 13's arrival to `at_ms` and expects scheduling it to fail as not after now.
fn reschedule_fleet_expecting(w: &mut World, id: FleetId, at_ms: i64) {
    let FleetState::InTransit { arrives_at_ms, .. } = &mut w.fleets.get_mut(&id).unwrap().state
    else {
        panic!("fleet {} is not in transit", id.0)
    };
    *arrives_at_ms = at_ms;
    assert_eq!(
        w.schedule_fleet(id),
        Err(EngineError::NotAfterNow { at_ms, now: 10 })
    );
}

#[test]
fn production_steps_drop_stale_entries_and_refuse_live_ones() {
    let mut w = sample_world();
    assert_eq!(w.advance_to(99).unwrap(), []); // only the stale 50 ms entry was due
    assert_eq!(w.time_ms, 99);
    assert_eq!(
        w.advance_to(100),
        Err(EngineError::StepNotBuilt(QueueKind::PlanetChange(P1)))
    );
    assert!(w.is_poisoned());
}

#[test]
fn a_step_that_leaves_its_fleet_unscheduled_stops_the_engine() {
    let mut w = sample_world();
    let mut idle = Probe(|_: &mut World, _| Ok(()));
    let error = w
        .advance_with(400, &mut idle)
        .expect_err("fleet 14 is left returning");
    assert!(
        error
            .to_string()
            .contains("fleet 14: arrival at 150 ms is not after world time 150"),
        "{error}"
    );
    assert_eq!(w.time_ms, 150);
    assert!(w.is_poisoned());
}

#[test]
fn a_live_entry_behind_world_time_is_an_error() {
    let mut w = sample_world();
    w.time_ms = 120; // planet 1's live 100 ms entry is now behind
    assert_eq!(
        w.advance_with(130, &mut settled()),
        Err(EngineError::LiveEntryBehind {
            entry: QueueEntry {
                at_ms: 100,
                kind: QueueKind::PlanetChange(P1),
                version: 3
            },
            now: 120
        })
    );
    assert!(w.is_poisoned());
}

#[test]
fn an_invalid_entry_stops_the_engine_rather_than_being_skipped() {
    let mut w = sample_world();
    w.queue.insert(QueueEntry {
        at_ms: 60,
        kind: QueueKind::PlanetChange(P2),
        version: 9,
    });
    let error = w
        .advance_with(400, &mut settled())
        .expect_err("invalid entry");
    assert!(error.to_string().contains("newer than planet 2"), "{error}");
    assert!(w.is_poisoned());
}
