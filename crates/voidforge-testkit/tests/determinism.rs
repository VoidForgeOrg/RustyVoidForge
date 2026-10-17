//! The determinism suite (Technical Plan P1.5; Architectural Design, Testing strategy and
//! Determinism rules; decision log D-024). Run by `just determinism` and the blocking CI job of
//! that name, which also runs [`two_process_double_run`] in two separate processes.
//!
//! | Harness | Architecture rule | Tests |
//! | --- | --- | --- |
//! | Observation invariance | One rounding rule; Pure reads | `observation_invariance_*` |
//! | Property tests | the invariants that apply before the solver | `queue_order_*`, `stale_*`, `new_entries_*`, `check_links_*`, `quantities_*` |
//! | Double run | Total order | `double_run_in_process`, `two_process_double_run` |
//! | Solved planets (P1.6) | One rounding rule; Total order; the solver invariants | `solved_*` |
//! | Transitions (P1.7) | the same, with builds completing | `transitions_*` |
//! | Fleets (P1.8) | the same, with fleets arriving, colonizing, unloading and recalled | `fleets_*` |
//!
//! Every property runs a fixed number of cases from a fixed seed with no failure files, so a
//! failure reproduces by rerunning (decision log D-024). Later packages extend these harnesses:
//! see the testkit guide, "Extending the determinism suite".

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use voidforge_core::quantity::{MS_PER_HOUR, Quantity};
use voidforge_core::rules::Rules;
use voidforge_core::solver::{Solver, planet_flows};
use voidforge_core::state::DomainEvent;
use voidforge_core::state::{
    Cargo, EngineError, FleetId, FleetState, Mission, PlanetId, QueueKind, ShipKind, Steps, World,
};
use voidforge_testkit::determinism::{
    Economy, Scenario, Stepped, assert_fleet_invariants, assert_ships_conserved,
    assert_transition_invariants, canonical_bytes, even_stops, recall_due, run_bytes, run_economy,
    run_observed, run_straight, settle_stock, slow_build_rules, solves, stops,
};
use voidforge_testkit::v0_rules;
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
        let (stepped, reads) = settle_stock(q, cap, &[], &stops).unwrap();
        let (straight, _) = settle_stock(q, cap, &[], &[from + SPAN_MS]).unwrap();
        prop_assert_eq!(canonical_bytes(&stepped), canonical_bytes(&straight));
        for (&t, &read) in stops.iter().zip(&reads) {
            prop_assert_eq!(read, settle_stock(q, cap, &[], &[t]).unwrap().1[0], "read at {}", t);
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
    let (stepped, reads) = settle_stock(q, cap, &[], &stops).unwrap();
    let (straight, _) = settle_stock(q, cap, &[], &[SPAN_MS]).unwrap();
    assert_eq!(stepped, straight);
    assert_eq!(stepped.value_g(), cap);
    assert!(reads.iter().any(|&r| r > 0 && r < cap), "it fills part-way");
    let crossing = q.rising_crossing(cap).unwrap().unwrap();
    assert!(0 < crossing && crossing < SPAN_MS);
}

/// An independent reference for the rounding rule: a stock stepped one millisecond at a time,
/// carrying the remainder in 128-bit integers. It never calls `Quantity`.
#[derive(Debug, Clone, Copy)]
struct Stepper {
    value: i128,
    remainder: i128,
    rate: i128,
}

impl Stepper {
    fn of(q: Quantity) -> Self {
        Self {
            value: q.value_g().into(),
            remainder: q.remainder().into(),
            rate: q.rate_g_per_h().into(),
        }
    }

    fn step(&mut self) {
        let total = self.remainder + self.rate;
        self.value += total.div_euclid(MS_PER_HOUR.into());
        self.remainder = total.rem_euclid(MS_PER_HOUR.into());
    }
}

/// review-r1 / QA Q1: a rate change carries the remainder. Against the stepper, a stock with a
/// remainder changes rate three times, including two sign changes, and every later read and
/// remainder matches.
#[test]
fn a_rate_change_carries_the_remainder() {
    let mut q = Quantity::from_parts(5_000_000, 7_000_001, 2_345_678, 0).unwrap();
    let mut s = Stepper::of(q);
    let changes = [(1_000, -3_600_011), (2_500, 999_999), (4_000, -1)];
    let mut next = 0;
    for t in 1..=6_000i64 {
        s.step();
        if let Some(&(at, rate)) = changes.get(next)
            && at == t
        {
            q = q.with_rate(t, rate).unwrap();
            assert_eq!(
                i128::from(q.remainder()),
                s.remainder,
                "remainder after the change at {t}"
            );
            s.rate = rate.into();
            next += 1;
        }
        assert_eq!(i128::from(q.value_at(t).unwrap()), s.value, "read at {t}");
        let here = q.advanced_to(t).unwrap();
        assert_eq!(
            (i128::from(here.value_g()), i128::from(here.remainder())),
            (s.value, s.remainder),
            "checkpoint at {t}"
        );
    }
    assert_eq!(next, changes.len());
}

proptest! {
    #![proptest_config(config(64, 0x0b5e_7a71_0000_0006))]

    /// Rate changes while a remainder is carried: settled in one step or 1,000 gives the same
    /// bytes, and both equal the independent stepper. The span is short enough to step every
    /// millisecond, and the stock stays clear of its boundaries, so only the rate changes matter.
    #[test]
    fn observation_invariance_through_rate_changes_with_a_remainder(
        remainder in 1i64..MS_PER_HOUR,
        rate in -1_000_000_000i64..=1_000_000_000,
        mut changes in prop::collection::vec((1i64..200_000, -1_000_000_000i64..=1_000_000_000), 1..6),
    ) {
        const SPAN: i64 = 200_000;
        changes.sort_unstable_by_key(|c| c.0);
        changes.dedup_by_key(|c| c.0);
        let (cap, start) = (1_000_000_000_000, 500_000_000_000);
        let q = Quantity::from_parts(start, rate, remainder, 0).unwrap();
        let (stepped, reads) = settle_stock(q, cap, &changes, &even_stops(0, SPAN, ADVANCES)).unwrap();
        let (straight, _) = settle_stock(q, cap, &changes, &[SPAN]).unwrap();
        prop_assert_eq!(canonical_bytes(&stepped), canonical_bytes(&straight));

        let mut s = Stepper::of(q);
        let mut next = 0;
        let mut stepped_reads = Vec::new();
        for t in 1..=SPAN {
            s.step();
            if let Some(&(at, r)) = changes.get(next) && at == t {
                s.rate = r.into();
                next += 1;
            }
            if t % (SPAN / ADVANCES) == 0 {
                stepped_reads.push(s.value);
            }
        }
        prop_assert_eq!(
            (i128::from(straight.value_g()), i128::from(straight.remainder())),
            (s.value, s.remainder)
        );
        prop_assert_eq!(reads.iter().map(|&r| i128::from(r)).collect::<Vec<_>>(), stepped_reads);
    }
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
            let (s, reads) = settle_stock(q, cap, &[], &stops[..k]).unwrap();
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

// --- Solved planets (P1.6): the production solver through real stock crossings ---

fn economy() -> impl Strategy<Value = Economy> {
    (any::<u64>(), 1u64..=10).prop_map(|(seed, planets)| Economy {
        seed,
        planets,
        fleets: false,
    })
}

fn fleet_economy() -> impl Strategy<Value = Economy> {
    (any::<u64>(), 2u64..=6).prop_map(|(seed, planets)| Economy {
        seed,
        planets,
        fleets: true,
    })
}

/// Every stock on every planet lies within 0 and its cap at world time.
fn stocks_within_caps(w: &World) {
    for p in w.planets.values() {
        let (ore, ingots) = (
            p.ore.value_at(w.time_ms).unwrap(),
            p.ingots.value_at(w.time_ms).unwrap(),
        );
        assert!(
            (0..=p.ore_cap_g).contains(&ore),
            "planet {} ore {} at {}",
            p.id.0,
            ore,
            w.time_ms
        );
        assert!(
            (0..=p.ingot_cap_g).contains(&ingots),
            "planet {} ingots {} at {}",
            p.id.0,
            ingots,
            w.time_ms
        );
    }
}

/// 100 hours in one production advance equals 1,000, through real crossings (and, under Rules
/// v0, real completions; with fleets, real arrivals and a recall, both runs stopping at the
/// recall); at every observation the world is linked, stocks are within 0 and their caps,
/// entries are in the future and the transition and fleet invariants hold; ships are conserved.
/// Returns the solves and events of the straight run.
fn solved_invariance(e: &Economy, rules: &Rules) -> (u64, Vec<DomainEvent>) {
    let start = e.world(rules);
    let (straight, straight_events) = run_economy(e, rules, SPAN_MS, 1, |_| {}).unwrap();
    let (observed, observed_events) = run_economy(e, rules, SPAN_MS, ADVANCES, |w| {
        w.check_links().expect("linked at every observation");
        stocks_within_caps(w);
        assert!(w.queue().iter().all(|q| q.at_ms > w.time_ms));
        assert_transition_invariants(w, rules);
        assert_fleet_invariants(w, rules);
    })
    .unwrap();
    assert_eq!(observed_events, straight_events);
    assert_eq!(canonical_bytes(&observed), canonical_bytes(&straight));
    assert_ships_conserved(&start, &straight, &straight_events);
    (solves(&start, &straight), straight_events)
}

fn solved_double_run(e: &Economy, rules: &Rules) {
    let a = run_economy(e, rules, SPAN_MS, 1, |_| {}).unwrap();
    let b = run_economy(e, rules, SPAN_MS, 1, |_| {}).unwrap();
    assert_eq!(canonical_bytes(&a), canonical_bytes(&b));
}

/// The audited run, stopping at any recall; returns the audit.
fn solved_audit<'a>(e: &Economy, rules: &'a Rules) -> Audit<'a> {
    let (mut w, recalls) = e.start(rules);
    let mut audit = Audit::new(&w, rules);
    let mut events = Vec::new();
    for t in stops(SPAN_MS, 1, &recalls) {
        events.extend(w.advance_with(t, &mut audit).unwrap());
        recall_due(&mut w, &recalls).unwrap();
    }
    audit.close(&w);
    stocks_within_caps(&w);
    assert_transition_invariants(&w, rules);
    assert_fleet_invariants(&w, rules);
    assert_eq!(events.len(), audit.events, "the audit returned every event");
    audit
}

proptest! {
    #![proptest_config(config(16, 0x0b5e_7a71_0000_0007))]

    /// Flows only: builds slowed so none completes; demolitions still end.
    #[test]
    fn solved_observation_invariance(e in economy()) {
        let (solves, _) = solved_invariance(&e, &slow_build_rules());
        prop_assert!(solves >= 2, "too few crossings: {}", solves);
    }

    /// The same economy run twice from separately built worlds gives identical bytes.
    #[test]
    fn solved_double_run_in_process(e in economy()) {
        solved_double_run(&e, &slow_build_rules());
    }

    /// Mass is accounted for: between solves, each planet's ledger grows by exactly ratio ×
    /// Drill actual × elapsed time; at a solve, each snap moves a quantity by less than one
    /// millisecond of its flow plus one gram (D-018, D-025). Every next event is strictly after
    /// the solve, and stocks stay within 0 and their caps.
    #[test]
    fn solved_mass_is_accounted_for_and_next_events_are_in_the_future(e in economy()) {
        let rules = slow_build_rules();
        let solves = solved_audit(&e, &rules).solves;
        prop_assert!(solves >= 2, "too few crossings: {}", solves);
    }

    /// P1.7 under Rules v0: builds complete, ships roll off, queues advance and demolitions end,
    /// and one advance still equals 1,000 with the same events, every invariant holding.
    #[test]
    fn transitions_observation_invariance(e in economy()) {
        let (_, events) = solved_invariance(&e, &v0_rules());
        let ships = events.iter().filter(|e| matches!(e, DomainEvent::ShipCompleted { .. })).count();
        let buildings = events.iter().filter(|e| matches!(e, DomainEvent::BuildingCompleted { .. })).count();
        let demolitions = events.iter().filter(|e| matches!(e, DomainEvent::DemolitionCompleted { .. })).count();
        prop_assert!(ships >= 2 && buildings >= 1 && demolitions >= 1, "too few transitions: {:?}", events);
    }

    #[test]
    fn transitions_double_run_in_process(e in economy()) {
        solved_double_run(&e, &v0_rules());
    }

    /// The mass audit through completions: a completing build leaves the ledger at exactly its
    /// cost, moving that mass into the building or ship.
    #[test]
    fn transitions_mass_is_accounted_for_through_completions(e in economy()) {
        let rules = v0_rules();
        let completed = solved_audit(&e, &rules).completed;
        prop_assert!(completed >= 3, "too few completions: {}", completed);
    }
}

proptest! {
    #![proptest_config(config(16, 0x0b5e_7a71_0000_0008))]

    /// P1.8: fleets fly every mission, race to one planet, colonize, unload and are recalled,
    /// and one advance still equals 1,000 with the same events; ships are conserved and every
    /// invariant holds at every observation.
    #[test]
    fn fleets_observation_invariance(e in fleet_economy()) {
        let (_, events) = solved_invariance(&e, &v0_rules());
        let count = |f: fn(&DomainEvent) -> bool| events.iter().filter(|e| f(e)).count();
        let arrived = count(|e| matches!(e, DomainEvent::FleetArrived { .. }));
        let colonized = count(|e| matches!(e, DomainEvent::PlanetColonized { .. }));
        let failed = count(|e| matches!(e, DomainEvent::ColonizationFailed { .. }));
        let unloaded = count(|e| matches!(e, DomainEvent::CargoUnloaded { .. }));
        let removed = count(|e| matches!(e, DomainEvent::FleetRemoved { .. }));
        let returned = count(|e| matches!(e, DomainEvent::FleetReturned { .. }));
        prop_assert_eq!(
            (arrived, colonized, failed, removed, returned),
            (6, 2, 2, 1, 1),
            "{:?}", events
        );
        prop_assert!(unloaded >= 1, "nothing unloaded: {:?}", events);
    }

    #[test]
    fn fleets_double_run_in_process(e in fleet_economy()) {
        solved_double_run(&e, &v0_rules());
    }

    /// The mass audit with fleets: each arrival's target gains exactly what its fleet unloaded,
    /// as the unload events report, and the cargo aboard closes exactly at the end.
    #[test]
    fn fleets_mass_is_accounted_for_including_cargo_aboard(e in fleet_economy()) {
        let rules = v0_rules();
        let audit = solved_audit(&e, &rules);
        prop_assert_eq!((audit.arrivals, audit.returns), (6, 1));
        prop_assert!(audit.unloaded > 0, "nothing unloaded");
    }
}

/// A planet's mass ledger in scaled units (grams × H plus remainder): ratio × ore + ingots +
/// every build's delivered ingots. Each part comes with its weight and its boundaries (0 and
/// a cap for a stock, the cost for a build), so a snap can be told from any other change.
type Part = (Quantity, i128, Option<i64>, i64);

/// Ledger parts by key: (0, planet) ore, (1, planet) ingots, (2, entity) a build's delivered.
fn ledger(w: &World, p: PlanetId, ratio: i64, at: i64) -> (i128, BTreeMap<(u8, u64), Part>) {
    let planet = &w.planets[&p];
    let mut parts = BTreeMap::new();
    parts.insert(
        (0, p.0),
        (planet.ore, i128::from(ratio), Some(0), planet.ore_cap_g),
    );
    parts.insert((1, p.0), (planet.ingots, 1, Some(0), planet.ingot_cap_g));
    for id in &planet.buildings {
        if let voidforge_core::state::BuildingState::UnderConstruction {
            delivered, cost_g, ..
        } = w.buildings[id].state
        {
            parts.insert((2, id.0), (delivered, 1, None, cost_g));
        }
    }
    for b in w.ship_builds.values() {
        if planet.buildings.contains(&b.shipyard) {
            parts.insert((2, b.id.0), (b.delivered, 1, None, b.cost_g));
        }
    }
    let total = parts.values().map(|&(q, k, _, _)| k * scaled(q, at)).sum();
    (total, parts)
}

fn scaled(q: Quantity, at: i64) -> i128 {
    let q = q.advanced_to(at).unwrap();
    i128::from(q.value_g()) * i128::from(MS_PER_HOUR) + i128::from(q.remainder())
}

/// Where a quantity crosses `now`, if it does: the boundary it reaches.
fn crossing_now(q: Quantity, floor: Option<i64>, top: i64, now: i64) -> Option<i64> {
    let rate = q.rate_g_per_h();
    let (at, boundary) = if rate > 0 {
        (q.rising_crossing(top).unwrap(), top)
    } else if rate < 0 {
        (
            floor.and_then(|f| q.falling_crossing(f).unwrap()),
            floor.unwrap_or(0),
        )
    } else {
        (None, 0)
    };
    (at == Some(now)).then_some(boundary)
}

/// The production solve, audited: each planet's ledger is integrated from its Drill actual
/// between solves and checked exactly; each snap is checked against the allowance.
struct Audit<'a> {
    rules: &'a Rules,
    ratio: i64,
    /// Per planet: the ledger after its last solve, that solve's instant and its Drill actual.
    planets: BTreeMap<PlanetId, (i128, i64, i64)>,
    solves: u64,
    /// Domain events the solves returned.
    events: usize,
    /// Builds that completed, leaving the ledger at their cost.
    completed: usize,
    /// Fleet arrivals and returns audited.
    arrivals: usize,
    returns: usize,
    /// Cargo aboard every fleet at the start, and unloaded since, in scaled ledger units.
    aboard: i128,
    unloaded: i128,
}

impl<'a> Audit<'a> {
    fn new(w: &World, rules: &'a Rules) -> Self {
        let ratio = rules.buildings.refinery.ingots_per_ore;
        let planets = w
            .planets
            .keys()
            .map(|&p| {
                let actual = planet_flows(w, p, rules).unwrap().drill_actual;
                (p, (ledger(w, p, ratio, w.time_ms).0, w.time_ms, actual))
            })
            .collect();
        Self {
            rules,
            ratio,
            planets,
            solves: 0,
            events: 0,
            completed: 0,
            arrivals: 0,
            returns: 0,
            aboard: cargo_aboard(w, ratio),
            unloaded: 0,
        }
    }

    /// The ledger a planet should hold at `at`, integrated since its last solve.
    fn expected(&self, p: PlanetId, at: i64) -> i128 {
        let (total, since, actual) = self.planets[&p];
        total + i128::from(self.ratio) * i128::from(actual) * i128::from(at - since)
    }

    fn close(&self, w: &World) {
        for &p in self.planets.keys() {
            assert_eq!(
                ledger(w, p, self.ratio, w.time_ms).0,
                self.expected(p, w.time_ms),
                "planet {} at the end",
                p.0
            );
        }
        assert_eq!(
            cargo_aboard(w, self.ratio),
            self.aboard - self.unloaded,
            "cargo aboard at the end"
        );
    }

    /// Records a planet's ledger, the instant and its Drill actual after a change at now.
    fn settle(&mut self, w: &World, p: PlanetId) {
        let now = w.time_ms;
        let actual = planet_flows(w, p, self.rules).unwrap().drill_actual;
        self.planets
            .insert(p, (ledger(w, p, self.ratio, now).0, now, actual));
    }
}

/// Cargo aboard every fleet, weighted like the ledger (ore × ratio) and scaled like it (× H).
fn cargo_aboard(w: &World, ratio: i64) -> i128 {
    w.fleets.values().map(|f| weighted(f.cargo, ratio)).sum()
}

fn weighted(c: Cargo, ratio: i64) -> i128 {
    (i128::from(ratio) * i128::from(c.ore_g) + i128::from(c.ingots_g)) * i128::from(MS_PER_HOUR)
}

impl Steps for Audit<'_> {
    type Event = DomainEvent;

    fn planet_change(
        &mut self,
        w: &mut World,
        p: PlanetId,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        let now = w.time_ms;
        let (before, parts) = ledger(w, p, self.ratio, now);
        assert_eq!(
            before,
            self.expected(p, now),
            "planet {} drifted before its solve at {}",
            p.0,
            now
        );
        let events = w.solve_planet(p, self.rules)?;
        self.events += events.len();
        let (after, after_parts) = ledger(w, p, self.ratio, now);
        // Snaps are the only change at a solve. A quantity crossing now lands on its boundary
        // with remainder 0, moved by less than one ms of its flow plus one gram; every other
        // quantity is unchanged to the scaled unit, remainder included.
        assert!(
            after_parts.keys().all(|k| parts.contains_key(k)),
            "a solve creates no ledger part"
        );
        for (key, &(old, _, floor, top)) in &parts {
            let Some(&(new, _, _, _)) = after_parts.get(key) else {
                // Completed: it left the ledger at exactly its cost, now in a building or ship.
                let reached = crossing_now(old, floor, top, now).is_some()
                    || old.advanced_to(now).unwrap().value_g() >= top;
                assert!(
                    reached,
                    "planet {}: entity {} vanished before its cost",
                    p.0, key.1
                );
                self.completed += 1;
                continue;
            };
            let delta = scaled(new, now) - scaled(old, now);
            match crossing_now(old, floor, top, now) {
                Some(boundary) => {
                    assert_eq!(
                        (new.value_g(), new.remainder()),
                        (boundary, 0),
                        "planet {} snap at {}",
                        p.0,
                        now
                    );
                    let allowance = i128::from(old.rate_g_per_h().abs()) + i128::from(MS_PER_HOUR);
                    assert!(
                        delta.abs() < allowance,
                        "planet {} snapped by {} at {}",
                        p.0,
                        delta,
                        now
                    );
                }
                None => assert_eq!(
                    delta, 0,
                    "planet {}: a quantity not crossing moved at {}",
                    p.0, now
                ),
            }
        }
        if let Some(next) = w.planets[&p].next_event {
            assert!(
                next.at_ms > now,
                "planet {} scheduled at {} at {}",
                p.0,
                next.at_ms,
                now
            );
        }
        let actual = planet_flows(w, p, self.rules).unwrap().drill_actual;
        self.planets.insert(p, (after, now, actual));
        self.solves += 1;
        Ok(events)
    }

    /// The production arrival, audited: the target's ledger was on course before it, and after
    /// it has grown by exactly what left the fleet's cargo, which the unload event reports. The
    /// target is brought to now without a crossing (its own changes at now ran first), so nothing
    /// snaps.
    fn fleet_arrival(
        &mut self,
        w: &mut World,
        f: FleetId,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        let now = w.time_ms;
        let FleetState::InTransit { target, .. } = w.fleets[&f].state else {
            panic!("fleet {} arrived but is not in transit", f.0)
        };
        let before = ledger(w, target, self.ratio, now).0;
        assert_eq!(
            before,
            self.expected(target, now),
            "planet {} drifted before an arrival at {}",
            target.0,
            now
        );
        let aboard = w.fleets[&f].cargo;
        let events = Solver { rules: self.rules }.fleet_arrival(w, f)?;
        let left = w.fleets.get(&f).map_or(Cargo::default(), |f| f.cargo);
        let moved = Cargo {
            ore_g: aboard.ore_g - left.ore_g,
            ingots_g: aboard.ingots_g - left.ingots_g,
        };
        let reported = events.iter().find_map(|e| match e {
            DomainEvent::CargoUnloaded { cargo, .. } => Some(*cargo),
            _ => None,
        });
        assert_eq!(reported.unwrap_or_default(), moved, "fleet {} unload", f.0);
        let gained = weighted(moved, self.ratio);
        assert_eq!(
            ledger(w, target, self.ratio, now).0,
            before + gained,
            "planet {} after fleet {} arrived at {}",
            target.0,
            f.0,
            now
        );
        self.unloaded += gained;
        self.settle(w, target);
        self.arrivals += 1;
        self.events += events.len();
        Ok(events)
    }

    /// The production return, audited: it changes neither the cargo nor the origin.
    fn fleet_return(&mut self, w: &mut World, f: FleetId) -> Result<Vec<DomainEvent>, EngineError> {
        let now = w.time_ms;
        let FleetState::Returning { origin, .. } = w.fleets[&f].state else {
            panic!("fleet {} returned but is not returning", f.0)
        };
        let (cargo, before) = (w.fleets[&f].cargo, ledger(w, origin, self.ratio, now).0);
        let events = Solver { rules: self.rules }.fleet_return(w, f)?;
        assert_eq!(w.fleets[&f].cargo, cargo);
        assert_eq!(ledger(w, origin, self.ratio, now).0, before);
        self.returns += 1;
        self.events += events.len();
        Ok(events)
    }
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
        .chain((0..4u64).map(|i| {
            let rules = slow_build_rules();
            let e = Economy {
                seed: 0x3_0000 + i,
                planets: 1 + i,
                fleets: false,
            };
            canonical_bytes(&run_economy(&e, &rules, SPAN_MS, 1, |_| {}).unwrap())
        }))
        .chain((0..4u64).map(|i| {
            let rules = v0_rules();
            let e = Economy {
                seed: 0x4_0000 + i,
                planets: 2 + i,
                fleets: false,
            };
            canonical_bytes(&run_economy(&e, &rules, SPAN_MS, 1, |_| {}).unwrap())
        }))
        .chain((0..4u64).map(|i| {
            let rules = v0_rules();
            let e = Economy {
                seed: 0x5_0000 + i,
                planets: 2 + i,
                fleets: true,
            };
            canonical_bytes(&run_economy(&e, &rules, SPAN_MS, 1, |_| {}).unwrap())
        }))
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
