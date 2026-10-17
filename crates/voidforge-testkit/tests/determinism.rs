//! The determinism suite (Technical Plan P1.5; Architectural Design, Testing strategy and
//! Determinism rules; decision log D-024). Run by `just determinism` and the blocking CI job of
//! that name, which also runs [`two_process_double_run`] in two separate processes.
//!
//! | Harness | Architecture rule | Tests |
//! | --- | --- | --- |
//! | Observation invariance | One rounding rule; Pure reads | `observation_invariance_*` |
//! | Property tests | the invariants that apply before the solver | `queue_order_*`, `stale_*`, `new_entries_*`, `check_links_*`, `quantities_*` |
//! | Double run | Total order | `double_run_in_process`, `two_process_double_run` |
//!
//! Every property runs a fixed number of cases from a fixed seed with no failure files, so a
//! failure reproduces by rerunning (decision log D-024). Later packages extend these harnesses:
//! see the testkit guide, "Extending the determinism suite".

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use voidforge_core::quantity::{MS_PER_HOUR, Quantity};
use voidforge_core::state::{
    Cargo, EngineError, FleetId, FleetState, Mission, PlanetId, QueueKind, ShipKind, Steps, World,
};
use voidforge_testkit::determinism::{
    Scenario, Stepped, canonical_bytes, even_stops, run_bytes, run_observed, run_straight,
    settle_stock,
};
use voidforge_testkit::{ALICE, WorldBuilder};

/// The architecture's scale: 100 hours, in one advance or in 1,000.
const SPAN_MS: i64 = 100 * MS_PER_HOUR;
const ADVANCES: i64 = 1_000;
/// The shortest gap between a scenario's events: 10 minutes.
const MIN_GAP_MS: i64 = 10 * 60_000;

fn config(cases: u32, seed: u64) -> Config {
    Config {
        cases,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        ..Config::default()
    }
}

/// Busy scenarios: gaps of 10 minutes to 2 hours over 100 hours, so every planet sees dozens to
/// hundreds of events and a run stays fast.
fn scenario() -> impl Strategy<Value = Scenario> {
    (
        any::<u64>(),
        1u64..=6,
        0u64..=4,
        MIN_GAP_MS..=2 * MS_PER_HOUR,
    )
        .prop_map(|(seed, planets, fleets, max_gap_ms)| Scenario {
            seed,
            planets,
            fleets,
            max_gap_ms,
            horizon_ms: SPAN_MS - MS_PER_HOUR,
        })
}

/// A stock: value within `0..=cap`, any remainder, rising, falling or flat.
fn stock() -> impl Strategy<Value = (Quantity, i64)> {
    (
        1i64..=1_000_000_000_000,
        0i64..MS_PER_HOUR,
        -1_000_000_000i64..=1_000_000_000,
        0i64..=MS_PER_HOUR,
    )
        .prop_flat_map(|(cap, remainder, rate, at)| {
            (0..=cap).prop_map(move |value| {
                let q =
                    Quantity::from_parts(value, rate, remainder, at).expect("remainder in range");
                (q, cap)
            })
        })
}

/// Records what ran; stations fleets that arrive or return. Schedules nothing.
struct Recorder;

impl Steps for Recorder {
    type Event = (i64, QueueKind);

    fn planet_change(
        &mut self,
        w: &mut World,
        p: PlanetId,
    ) -> Result<Vec<Self::Event>, EngineError> {
        Ok(vec![(w.time_ms, QueueKind::PlanetChange(p))])
    }

    fn fleet_arrival(
        &mut self,
        w: &mut World,
        f: FleetId,
    ) -> Result<Vec<Self::Event>, EngineError> {
        let fleet = w.fleets.get_mut(&f).unwrap();
        let FleetState::InTransit { target, .. } = fleet.state else {
            panic!("fleet {} arrived but is not in transit", f.0)
        };
        fleet.state = FleetState::Stationed { at: target };
        Ok(vec![(w.time_ms, QueueKind::FleetArrival(f))])
    }

    fn fleet_return(&mut self, w: &mut World, f: FleetId) -> Result<Vec<Self::Event>, EngineError> {
        let fleet = w.fleets.get_mut(&f).unwrap();
        let FleetState::Returning { origin, .. } = fleet.state else {
            panic!("fleet {} returned but is not returning", f.0)
        };
        fleet.state = FleetState::Stationed { at: origin };
        Ok(vec![(w.time_ms, QueueKind::FleetReturn(f))])
    }
}

/// A world of `planets` planets and `fleets` stationed one-ship fleets, nothing scheduled.
fn quiet_world(planets: usize, fleets: usize) -> (World, Vec<PlanetId>, Vec<FleetId>) {
    let mut b = WorldBuilder::new();
    let mut ps = vec![b.player(ALICE)];
    for _ in 1..planets {
        ps.push(b.planet(None));
    }
    let mut fs = Vec::new();
    for i in 0..fleets {
        let ship = b.ship(ALICE, ShipKind::CargoVessel, ps[i % ps.len()]);
        fs.push(b.fleet(&[ship], Cargo::default()));
    }
    (b.build(), ps, fs)
}

/// Sends a stationed fleet to the first planet, arriving at `at_ms`.
fn send(w: &mut World, f: FleetId, at_ms: i64) {
    let fleet = w.fleets.get_mut(&f).unwrap();
    let FleetState::Stationed { at } = fleet.state else {
        panic!("fleet {} is not stationed", f.0)
    };
    fleet.state = FleetState::InTransit {
        origin: at,
        target: PlanetId(1),
        mission: Mission::Move,
        departed_at_ms: w.time_ms,
        arrives_at_ms: at_ms,
    };
    w.schedule_fleet(f).unwrap();
}

/// The architecture's total order for a set of entries: time, kind rank, entity ID.
fn sorted(mut entries: Vec<(i64, QueueKind)>) -> Vec<(i64, QueueKind)> {
    entries.sort_by_key(|&(t, k)| (t, k.rank(), k.entity()));
    entries
}

// --- Observation invariance (rules "One rounding rule" and "Pure reads") ---

proptest! {
    #![proptest_config(config(24, 0x0b5e_7a71_0000_0001))]

    /// 100 hours in one advance equals 1,000 advances with reads in between, byte for byte, and
    /// the world is valid at every observation.
    #[test]
    fn observation_invariance_over_the_event_loop(s in scenario()) {
        let (straight, straight_events) = run_straight(&s, SPAN_MS).unwrap();
        let mut observations = 0;
        let (observed, observed_events) = run_observed(&s, SPAN_MS, ADVANCES, |w| {
            observations += 1;
            let _ = canonical_bytes(w);
            w.check_links().expect("valid at every observation");
            for p in w.planets.values() {
                p.ore.value_at(w.time_ms).expect("stocks read at now");
            }
            assert!(w.queue().iter().all(|e| e.at_ms > w.time_ms));
        })
        .unwrap();
        prop_assert_eq!(observations, ADVANCES);
        prop_assert!(
            straight_events.len() as u64 >= 40 * s.planets,
            "too few events to prove anything: {}", straight_events.len()
        );
        prop_assert_eq!(&observed_events, &straight_events);
        prop_assert_eq!(run_bytes(&observed, &observed_events), run_bytes(&straight, &straight_events));
    }

    /// The same for a stock: settled over 100 hours in one step or 1,000, with a read at each
    /// stop equal to the read of a stock settled straight to that stop.
    #[test]
    fn observation_invariance_over_quantities((q, cap) in stock()) {
        let from = q.checkpoint_ms();
        let stops = even_stops(from, from + SPAN_MS, ADVANCES);
        let (stepped, reads) = settle_stock(q, cap, &stops).unwrap();
        let (straight, _) = settle_stock(q, cap, &[from + SPAN_MS]).unwrap();
        prop_assert_eq!(canonical_bytes(&stepped), canonical_bytes(&straight));
        for (&t, &read) in stops.iter().zip(&reads) {
            prop_assert_eq!(read, settle_stock(q, cap, &[t]).unwrap().1[0], "read at {}", t);
        }
    }
}

/// A rising stock with a remainder crosses its cap part-way: the case the generated one must
/// cover, named so it never depends on the generator's luck.
#[test]
fn observation_invariance_through_a_crossing_with_a_remainder() {
    let q = Quantity::from_parts(0, 7_000_001, 1_234_567, 0).unwrap();
    let cap = 350_000_000;
    let stops = even_stops(0, SPAN_MS, ADVANCES);
    let (stepped, reads) = settle_stock(q, cap, &stops).unwrap();
    let (straight, _) = settle_stock(q, cap, &[SPAN_MS]).unwrap();
    assert_eq!(stepped, straight);
    assert_eq!(stepped.value_g(), cap);
    assert!(reads.iter().any(|&r| r > 0 && r < cap), "it fills part-way");
    let crossing = q.rising_crossing(cap).unwrap().unwrap();
    assert!(0 < crossing && crossing < SPAN_MS);
}

// --- Property tests ---

proptest! {
    #![proptest_config(config(128, 0x0b5e_7a71_0000_0002))]

    /// Scheduling the same entries in any order gives the same queue, and they run in (time,
    /// kind rank, entity ID) order. Times are crowded so instants are shared.
    #[test]
    fn queue_order_is_independent_of_insertion_order(
        (planet_times, fleet_times, order) in (
            prop::collection::vec(1i64..=4, 1..=6),
            prop::collection::vec(1i64..=4, 0..=4),
        )
            .prop_flat_map(|(p, f)| {
                let n = p.len() + f.len();
                let order: Vec<usize> = (0..n).collect();
                (Just(p), Just(f), Just(order).prop_shuffle())
            })
    ) {
        let (mut a, ps, fs) = quiet_world(planet_times.len(), fleet_times.len());
        let mut b = a.clone();
        let schedule = |w: &mut World, i: usize| {
            if i < planet_times.len() {
                w.schedule_planet(ps[i], Some(planet_times[i])).unwrap();
            } else {
                let j = i - planet_times.len();
                send(w, fs[j], fleet_times[j]);
            }
        };
        for i in 0..order.len() {
            schedule(&mut a, i);
        }
        for &i in &order {
            schedule(&mut b, i);
        }
        prop_assert_eq!(canonical_bytes(&a), canonical_bytes(&b));

        let mut expected: Vec<(i64, QueueKind)> = ps
            .iter()
            .zip(&planet_times)
            .map(|(&p, &t)| (t, QueueKind::PlanetChange(p)))
            .collect();
        expected.extend(fs.iter().zip(&fleet_times).map(|(&f, &t)| (t, QueueKind::FleetArrival(f))));
        let ran = b.advance_with(10, &mut Recorder).unwrap();
        prop_assert_eq!(&ran, &sorted(expected));
        prop_assert_eq!(a.advance_with(10, &mut Recorder).unwrap(), ran);
    }

    /// After any mix of scheduling, rescheduling, clearing, sending, recalling and removing
    /// fleets, exactly the live entries run, in order: no stale entry runs and no live one is
    /// skipped.
    #[test]
    fn stale_entries_never_run_and_live_ones_always_do(
        first in prop::collection::vec((0usize..4, 1_000i64..2_000), 0..8),
        second in prop::collection::vec((0u8..6, 0usize..4, 501i64..2_500), 0..12),
    ) {
        let (mut w, ps, fs) = quiet_world(4, 4);
        // The model: each entity's one live entry, if any.
        let mut live: BTreeMap<QueueKind, i64> = BTreeMap::new();
        let mut moving: BTreeSet<FleetId> = BTreeSet::new();
        let mut removed: Vec<FleetId> = Vec::new();

        // At 0: planets scheduled and fleets sent.
        for (i, &(n, t)) in first.iter().enumerate() {
            if i % 2 == 0 {
                w.schedule_planet(ps[n], Some(t)).unwrap();
                live.insert(QueueKind::PlanetChange(ps[n]), t);
            } else if moving.insert(fs[n]) {
                send(&mut w, fs[n], t);
                live.insert(QueueKind::FleetArrival(fs[n]), t);
            }
        }
        // Nothing is due before 1,000, so this only moves time.
        prop_assert_eq!(w.advance_with(500, &mut Recorder).unwrap(), vec![]);

        // At 500: reschedule, clear, send, recall, remove.
        for &(op, n, t) in &second {
            let (p, f) = (ps[n], fs[n]);
            if removed.contains(&f) && op >= 2 {
                continue;
            }
            match op {
                0 => {
                    w.schedule_planet(p, Some(t)).unwrap();
                    live.insert(QueueKind::PlanetChange(p), t);
                }
                1 => {
                    w.schedule_planet(p, None).unwrap();
                    live.remove(&QueueKind::PlanetChange(p));
                }
                2 => {
                    if moving.insert(f) {
                        send(&mut w, f, t);
                        live.insert(QueueKind::FleetArrival(f), t);
                    }
                }
                3 => {
                    if let Some(FleetState::InTransit { origin, departed_at_ms, .. }) =
                        w.fleets.get(&f).map(|x| x.state)
                        && departed_at_ms < 500
                    {
                        let back = 500 + (500 - departed_at_ms);
                        w.fleets.get_mut(&f).unwrap().state =
                            FleetState::Returning { origin, arrives_at_ms: back };
                        w.schedule_fleet(f).unwrap();
                        live.remove(&QueueKind::FleetArrival(f));
                        live.insert(QueueKind::FleetReturn(f), back);
                    }
                }
                4 => {
                    let fleet = w.fleets.remove(&f).unwrap();
                    for ship in fleet.ships {
                        w.ships.remove(&ship);
                    }
                    live.remove(&QueueKind::FleetArrival(f));
                    live.remove(&QueueKind::FleetReturn(f));
                    moving.remove(&f);
                    removed.push(f);
                }
                _ => {}
            }
        }

        let expected = sorted(live.iter().map(|(&k, &t)| (t, k)).collect());
        let ran = w.advance_with(3_000, &mut Recorder).unwrap();
        prop_assert_eq!(ran, expected);
        prop_assert!(w.queue().is_empty());
        w.check_links().unwrap();
    }

    /// A step's scheduling succeeds exactly when the entry is strictly after the instant; any
    /// refusal stops and poisons the advance, and a clean advance leaves only future entries.
    #[test]
    fn new_entries_are_strictly_after_now(deltas in prop::collection::vec(-2i64..=3, 1..20)) {
        struct Attempt {
            deltas: Vec<i64>,
            next: usize,
            refused: bool,
        }
        impl Steps for Attempt {
            type Event = ();
            fn planet_change(&mut self, w: &mut World, p: PlanetId) -> Result<Vec<()>, EngineError> {
                if let Some(&d) = self.deltas.get(self.next) {
                    self.next += 1;
                    let now = w.time_ms;
                    let result = w.schedule_planet(p, Some(now + d));
                    assert_eq!(result.is_ok(), d > 0, "scheduling {d} ms from now");
                    self.refused |= result.is_err();
                }
                Ok(vec![()])
            }
            fn fleet_arrival(&mut self, _: &mut World, _: FleetId) -> Result<Vec<()>, EngineError> {
                unreachable!()
            }
            fn fleet_return(&mut self, _: &mut World, _: FleetId) -> Result<Vec<()>, EngineError> {
                unreachable!()
            }
        }
        let (mut w, ps, _) = quiet_world(1, 0);
        w.schedule_planet(ps[0], Some(1)).unwrap();
        let mut steps = Attempt { deltas, next: 0, refused: false };
        let result = w.advance_with(1_000, &mut steps);
        if steps.refused {
            prop_assert!(matches!(result, Err(EngineError::NotAfterNow { .. })), "{:?}", result);
            prop_assert!(w.is_poisoned());
        } else {
            prop_assert!(result.is_ok());
            prop_assert!(w.queue().iter().all(|e| e.at_ms > w.time_ms));
        }
    }
}

proptest! {
    #![proptest_config(config(48, 0x0b5e_7a71_0000_0003))]

    /// Advances split at arbitrary points, and exactly at event instants, keep the world linked
    /// after every one and end where a single advance ends.
    #[test]
    fn check_links_holds_after_every_advance(
        s in scenario(),
        mut cuts in prop::collection::vec(0i64..SPAN_MS, 0..40),
        every in 1usize..=7,
    ) {
        let (straight, straight_events) = run_straight(&s, SPAN_MS).unwrap();
        // Cutting at an event's own instant must process it before the advance returns.
        cuts.extend(straight_events.iter().step_by(every).map(|e| e.at_ms));
        cuts.sort_unstable();
        cuts.dedup();
        cuts.push(SPAN_MS);
        let mut w = s.world();
        let mut steps = s.steps();
        let mut events: Vec<Stepped> = Vec::new();
        for &t in &cuts {
            events.extend(w.advance_with(t, &mut steps).unwrap());
            w.check_links().unwrap();
            prop_assert!(w.queue().iter().all(|e| e.at_ms > w.time_ms), "an entry at or before {} is left", t);
        }
        prop_assert_eq!(run_bytes(&w, &events), run_bytes(&straight, &straight_events));
    }

    /// Settling keeps a stock's remainder in `0..MS_PER_HOUR` and its value within `0..=cap`
    /// at every stop, and the checkpoint is the last stop.
    #[test]
    fn quantities_stay_within_their_ranges(
        (q, cap) in stock(),
        mut stops in prop::collection::vec(0i64..SPAN_MS, 1..30),
    ) {
        stops.sort_unstable();
        let from = q.checkpoint_ms();
        let stops: Vec<i64> = stops.into_iter().map(|t| from + t).collect();
        for k in 1..=stops.len() {
            let (s, reads) = settle_stock(q, cap, &stops[..k]).unwrap();
            prop_assert!((0..MS_PER_HOUR).contains(&s.remainder()), "{:?}", s);
            prop_assert!((0..=cap).contains(&s.value_g()), "{:?}", s);
            prop_assert_eq!(s.checkpoint_ms(), stops[k - 1]);
            prop_assert!(reads.iter().all(|r| (0..=cap).contains(r)));
        }
    }
}

proptest! {
    #![proptest_config(config(256, 0x0b5e_7a71_0000_0005))]

    /// The rounding rule's crossing is the first millisecond at which the read reaches the
    /// boundary: reached at the crossing, not reached one millisecond before.
    #[test]
    fn crossings_are_the_first_millisecond_at_the_boundary((q, cap) in stock()) {
        let rising = q.rate_g_per_h() > 0;
        let crossing = if rising {
            q.rising_crossing(cap).unwrap()
        } else {
            q.falling_crossing(0).unwrap()
        };
        if let Some(c) = crossing.filter(|&c| c > q.checkpoint_ms()) {
            let (at, before) = (q.value_at(c).unwrap(), q.value_at(c - 1).unwrap());
            if rising {
                prop_assert!(at >= cap && before < cap, "{:?}: {} then {} at {}", q, before, at, c);
            } else {
                prop_assert!(at <= 0 && before > 0, "{:?}: {} then {} at {}", q, before, at, c);
            }
        }
    }
}

/// Past 64 bits a quantity is an error, never a wrapped value.
#[test]
fn quantities_report_overflow_instead_of_wrapping() {
    let q = Quantity::new(i64::MAX - 10, MS_PER_HOUR, 0);
    assert!(q.advanced_to(1_000).is_err());
    assert!(q.value_at(1_000).is_err());
    let q = Quantity::new(i64::MIN + 10, -MS_PER_HOUR, 0);
    assert!(q.advanced_to(1_000).is_err());
}

// --- Double run (rule "Total order") ---

proptest! {
    #![proptest_config(config(32, 0x0b5e_7a71_0000_0004))]

    /// The same scenario run twice from equal, separately built worlds gives identical bytes.
    #[test]
    fn double_run_in_process(s in scenario()) {
        let (a, a_events) = run_straight(&s, SPAN_MS).unwrap();
        let (b, b_events) = run_straight(&s, SPAN_MS).unwrap();
        prop_assert!(!a_events.is_empty());
        prop_assert_eq!(run_bytes(&a, &a_events), run_bytes(&b, &b_events));
    }
}

/// The scenarios the two-process run records: fixed, so both processes run the same ones.
fn recorded_runs() -> Vec<u8> {
    let runs: Vec<Vec<u8>> = (0..8u64)
        .map(|i| {
            let s = Scenario {
                seed: 0x2_0000 + i,
                planets: 1 + i % 6,
                fleets: i % 5,
                max_gap_ms: MIN_GAP_MS + (i as i64 * 997_331) % (2 * MS_PER_HOUR),
                horizon_ms: SPAN_MS - MS_PER_HOUR,
            };
            let (w, events) = run_straight(&s, SPAN_MS).unwrap();
            run_bytes(&w, &events)
        })
        .collect();
    canonical_bytes(&runs)
}

/// The two-process double run. `.github/scripts/determinism.sh` runs this test twice, as two
/// processes: `VOIDFORGE_DOUBLE_RUN=write` saves the bytes to `VOIDFORGE_DOUBLE_RUN_FILE`, then
/// `compare` runs the scenarios again and requires identical bytes. A missing file fails.
#[test]
#[ignore = "run in two processes by .github/scripts/determinism.sh"]
fn two_process_double_run() {
    let mode =
        std::env::var("VOIDFORGE_DOUBLE_RUN").expect("VOIDFORGE_DOUBLE_RUN is write or compare");
    let file =
        std::env::var("VOIDFORGE_DOUBLE_RUN_FILE").expect("VOIDFORGE_DOUBLE_RUN_FILE is set");
    let bytes = recorded_runs();
    match mode.as_str() {
        "write" => std::fs::write(&file, &bytes).expect("the bytes are written"),
        "compare" => {
            let first = std::fs::read(&file)
                .unwrap_or_else(|e| panic!("the first process's bytes are missing at {file}: {e}"));
            assert_eq!(first.len(), bytes.len(), "the runs differ in length");
            assert!(
                first == bytes,
                "the second process produced different bytes"
            );
        }
        other => panic!("VOIDFORGE_DOUBLE_RUN must be write or compare, not {other}"),
    }
}
