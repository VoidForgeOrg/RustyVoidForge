//! The per-planet solver's named tests, part 1 (Technical Plan P1.6 and the solver bullets under
//! the Phase 1 table; Architectural Design, Per-planet solver; decision log D-025), on Rules v0
//! worlds. Names start with the bullet they cover: `flows_*` (Drill, Refinery, ingot
//! consumers, halted and throttled), `even_split_*`, `next_event_*`, `crossing_*`,
//! `halt_chain_*` and `design_example_*`. The pure even-split cases are in core's
//! `solver/tests.rs`.

use voidforge_core::quantity::{MS_PER_HOUR, Quantity};
use voidforge_core::rules::{GRAMS_PER_TON, Rules};
use voidforge_core::solver::{FlowLabel, PlanetFlows, planet_flows};
use voidforge_core::state::{
    BuildingId, BuildingKind, BuildingState, EngineError, PlanetId, QueueKind, ShipBuildStatus,
    ShipKind, World,
};
use voidforge_testkit::{WorldBuilder, v0_rules};

const T: i64 = GRAMS_PER_TON;
const H: i64 = MS_PER_HOUR;

/// An unowned planet at time 0 with `drills` and `refineries` operational and the given stocks,
/// in tons. Ownership does not matter to the solver.
fn planet(drills: u32, refineries: u32, ore_t: i64, ingots_t: i64) -> (WorldBuilder, PlanetId) {
    let mut b = WorldBuilder::new();
    let p = b.planet(None);
    for _ in 0..drills {
        b.building(p, BuildingKind::Drill, BuildingState::Operational);
    }
    for _ in 0..refineries {
        b.building(p, BuildingKind::Refinery, BuildingState::Operational);
    }
    b.stocks(
        p,
        Quantity::new(ore_t * T, 0, 0),
        Quantity::new(ingots_t * T, 0, 0),
    );
    (b, p)
}

/// The world built and the planet solved, with its flows.
fn solve(b: WorldBuilder, p: PlanetId) -> (World, PlanetFlows) {
    let rules = v0_rules();
    let mut w = b.build();
    w.solve_planet(p, &rules).expect("the planet solves");
    w.check_links().expect("a solved world is linked");
    let flows = planet_flows(&w, p, &rules).unwrap();
    (w, flows)
}

fn of_kind(w: &World, p: PlanetId, kind: BuildingKind) -> Vec<BuildingId> {
    w.planets[&p]
        .buildings
        .iter()
        .copied()
        .filter(|id| w.buildings[id].kind == kind)
        .collect()
}

fn flow(f: &PlanetFlows, id: BuildingId) -> (i64, FlowLabel) {
    let x = f.buildings[&id];
    (x.rate_g_per_h, x.label)
}

// --- Drill ---

#[test]
fn flows_drill_runs_at_full_rate_normally() {
    let (b, p) = planet(1, 0, 100, 0);
    let (w, f) = solve(b, p);
    let drill = of_kind(&w, p, BuildingKind::Drill)[0];
    assert_eq!(flow(&f, drill), (10 * T, FlowLabel::Running));
    assert_eq!(f.ore_net, 10 * T);
    assert_eq!(w.planets[&p].ore.rate_g_per_h(), 10 * T);
}

#[test]
fn flows_drill_is_throttled_to_the_refineries_intake_when_ore_storage_is_full() {
    let (b, p) = planet(2, 1, 10_000, 0);
    let (w, f) = solve(b, p);
    assert_eq!(
        (f.drill_output, f.refinery_intake, f.drill_actual),
        (20 * T, 8 * T, 8 * T)
    );
    for d in of_kind(&w, p, BuildingKind::Drill) {
        assert_eq!(flow(&f, d), (4 * T, FlowLabel::Throttled));
    }
    assert_eq!(f.ore_net, 0);
}

#[test]
fn flows_drill_halts_only_when_ore_storage_is_full_and_nothing_draws_from_it() {
    // Full, and no Refinery draws: halted.
    let (b, p) = planet(1, 0, 10_000, 0);
    let (w, f) = solve(b, p);
    assert_eq!(
        flow(&f, of_kind(&w, p, BuildingKind::Drill)[0]),
        (0, FlowLabel::Halted)
    );
    // Full, with the only Refinery itself halted by full ingot storage: still nothing draws.
    let (b, p) = planet(1, 1, 10_000, 5_000);
    let (w, f) = solve(b, p);
    assert_eq!(
        flow(&f, of_kind(&w, p, BuildingKind::Drill)[0]),
        (0, FlowLabel::Halted)
    );
    // Not full: it runs whatever the Refineries do.
    let (b, p) = planet(1, 1, 9_999, 5_000);
    let (w, f) = solve(b, p);
    assert_eq!(
        flow(&f, of_kind(&w, p, BuildingKind::Drill)[0]),
        (10 * T, FlowLabel::Running)
    );
}

// --- Refinery ---

#[test]
fn flows_refinery_takes_its_rated_intake_while_ore_is_in_stock() {
    let (b, p) = planet(0, 1, 100, 0);
    let (w, f) = solve(b, p);
    assert_eq!(
        flow(&f, of_kind(&w, p, BuildingKind::Refinery)[0]),
        (8 * T, FlowLabel::Running)
    );
    assert_eq!(
        (f.ore_net, f.ingot_output, f.ingot_net),
        (-8 * T, 16 * T, 16 * T)
    );
}

#[test]
fn flows_refinery_takes_a_share_of_the_drills_output_when_the_ore_stock_is_empty() {
    let (b, p) = planet(1, 2, 0, 0);
    let (w, f) = solve(b, p);
    assert_eq!(f.refinery_intake, 10 * T);
    for r in of_kind(&w, p, BuildingKind::Refinery) {
        assert_eq!(flow(&f, r), (5 * T, FlowLabel::Throttled));
    }
    assert_eq!(f.ore_net, 0);
}

#[test]
fn flows_refinery_halts_when_no_ore_is_in_stock_or_arriving() {
    let (b, p) = planet(0, 1, 0, 0);
    let (w, f) = solve(b, p);
    assert_eq!(
        flow(&f, of_kind(&w, p, BuildingKind::Refinery)[0]),
        (0, FlowLabel::Halted)
    );
    assert_eq!((f.ore_net, f.ingot_net), (0, 0));
}

#[test]
fn flows_refinery_is_throttled_to_half_the_ingot_demand_when_ingot_storage_is_full() {
    // Four Refineries want 32 t/h of ore; one Drill construction wants 50 t/h of ingots, so a
    // full ingot store lets them take 25 t/h, 6.25 t/h each, exactly matching the demand.
    let (mut b, p) = planet(0, 4, 1_000, 5_000);
    b.construction(p, BuildingKind::Drill, 0);
    let (w, f) = solve(b, p);
    assert_eq!((f.ingot_demand, f.refinery_intake), (50 * T, 25 * T));
    for r in of_kind(&w, p, BuildingKind::Refinery) {
        assert_eq!(flow(&f, r), (6_250_000, FlowLabel::Throttled));
    }
    assert_eq!(f.ingot_net, 0);
}

#[test]
fn flows_refinery_halts_when_ingot_storage_is_full_and_nothing_draws_from_it() {
    let (b, p) = planet(0, 1, 1_000, 5_000);
    let (w, f) = solve(b, p);
    assert_eq!(
        flow(&f, of_kind(&w, p, BuildingKind::Refinery)[0]),
        (0, FlowLabel::Halted)
    );
    assert_eq!((f.ore_net, f.ingot_net), (0, 0));
}

// --- Ingot consumers ---

#[test]
fn flows_ingot_consumers_get_their_full_rate_while_ingots_are_in_stock() {
    let (mut b, p) = planet(1, 1, 100, 100);
    let drill = b.construction(p, BuildingKind::Drill, 0);
    let refinery = b.construction(p, BuildingKind::Refinery, 0);
    let (_, f) = solve(b, p);
    assert_eq!(flow(&f, drill), (50 * T, FlowLabel::Running));
    assert_eq!(flow(&f, refinery), (62_500_000, FlowLabel::Running));
    assert_eq!(f.ingot_net, 16 * T - 112_500_000);
}

#[test]
fn flows_ingot_consumers_split_the_refinery_output_evenly_when_the_stock_is_empty() {
    let (mut b, p) = planet(1, 1, 100, 0);
    let drill = b.construction(p, BuildingKind::Drill, 0);
    let shipyard = b.construction(p, BuildingKind::Shipyard, 0);
    let (_, f) = solve(b, p);
    assert_eq!(flow(&f, drill), (8 * T, FlowLabel::Throttled));
    assert_eq!(flow(&f, shipyard), (8 * T, FlowLabel::Throttled));
    assert_eq!(f.ingot_net, 0);
}

#[test]
fn flows_a_shipyard_halts_with_no_builds_or_no_ingots() {
    // No builds.
    let (mut b, p) = planet(0, 0, 0, 100);
    let yard = b.building(p, BuildingKind::Shipyard, BuildingState::Operational);
    let (_, f) = solve(b, p);
    assert_eq!(flow(&f, yard), (0, FlowLabel::Halted));
    // A build, but no ingots and none being made.
    let (mut b, p) = planet(0, 0, 0, 0);
    let yard = b.building(p, BuildingKind::Shipyard, BuildingState::Operational);
    let build = b.ship_build(yard, ShipKind::CargoVessel, ShipBuildStatus::Active);
    let (_, f) = solve(b, p);
    assert_eq!(flow(&f, yard), (0, FlowLabel::Halted));
    assert_eq!(f.ship_builds[&build].label, FlowLabel::Halted);
    // Ingots in stock: it runs, at its active builds' rated sum; a waiting build draws nothing.
    let (mut b, p) = planet(0, 0, 0, 100);
    let yard = b.building(p, BuildingKind::Shipyard, BuildingState::Operational);
    b.ship_build(yard, ShipKind::CargoVessel, ShipBuildStatus::Active);
    b.ship_build(yard, ShipKind::ColonyShip, ShipBuildStatus::Active);
    let waiting = b.ship_build(yard, ShipKind::CargoVessel, ShipBuildStatus::Waiting);
    let (_, f) = solve(b, p);
    assert_eq!(flow(&f, yard), (155 * T, FlowLabel::Running));
    assert!(!f.ship_builds.contains_key(&waiting));
}

#[test]
fn flows_halted_is_zero_flow_and_throttled_is_positive_below_rated() {
    // 16 t/h made, empty ingot stock, a Drill and a Refinery construction: 8 t/h each, below
    // their rated 50 and 62.5 t/h, so throttled.
    let (mut b, p) = planet(1, 1, 100, 0);
    let drill = b.construction(p, BuildingKind::Drill, 0);
    let refinery = b.construction(p, BuildingKind::Refinery, 0);
    let (_, f) = solve(b, p);
    assert_eq!(flow(&f, drill), (8 * T, FlowLabel::Throttled));
    assert_eq!(flow(&f, refinery), (8 * T, FlowLabel::Throttled));
    // Nothing made and nothing in stock: zero flow, so halted.
    let (mut b, p) = planet(0, 0, 0, 0);
    let starved = b.construction(p, BuildingKind::Drill, 0);
    let (_, f) = solve(b, p);
    assert_eq!(flow(&f, starved), (0, FlowLabel::Halted));
}

// --- Even split, on a planet ---

#[test]
fn even_split_equal_demands_get_equal_shares_on_a_planet() {
    // Two Drill constructions, empty ingot stock, 16 t/h made: 8 t/h each.
    let (mut b, p) = planet(1, 1, 100, 0);
    let x = b.construction(p, BuildingKind::Drill, 0);
    let y = b.construction(p, BuildingKind::Drill, 0);
    let (_, f) = solve(b, p);
    assert_eq!((flow(&f, x).0, flow(&f, y).0), (8 * T, 8 * T));
}

#[test]
fn even_split_integer_leftovers_go_to_the_lowest_ids_on_a_planet() {
    // 16,000,000 g/h among three: 5,333,333 each and one gram per hour left, to the lowest ID.
    let (mut b, p) = planet(1, 1, 100, 0);
    let ids: Vec<BuildingId> = (0..3)
        .map(|_| b.construction(p, BuildingKind::Drill, 0))
        .collect();
    let (_, f) = solve(b, p);
    let rates: Vec<i64> = ids.iter().map(|&id| flow(&f, id).0).collect();
    assert_eq!(rates, [5_333_334, 5_333_333, 5_333_333]);
    assert_eq!(f.ingots_delivered, 16 * T);
}

// --- Next event ---

#[test]
fn next_event_is_the_earliest_stock_crossing_to_its_cap_or_to_zero() {
    // Ore fills: 100 t free at +10 t/h is 10 h away.
    let (b, p) = planet(1, 0, 9_900, 0);
    let (w, _) = solve(b, p);
    assert_eq!(w.planets[&p].next_event.unwrap().at_ms, 10 * H);
    // Ore empties: 16 t at −8 t/h is 2 h away; ingots fill much later.
    let (b, p) = planet(0, 1, 16, 0);
    let (w, _) = solve(b, p);
    assert_eq!(w.planets[&p].next_event.unwrap().at_ms, 2 * H);
}

#[test]
fn next_event_is_a_build_reaching_its_cost_when_that_is_earliest() {
    // A Drill construction with 140 of 150 t delivered, at 50 t/h from stock: 12 minutes.
    let (mut b, p) = planet(0, 0, 0, 1_000);
    b.construction(p, BuildingKind::Drill, 140 * T);
    let (w, _) = solve(b, p);
    assert_eq!(w.planets[&p].next_event.unwrap().at_ms, H / 5);
}

#[test]
fn next_event_is_a_demolition_end_when_that_is_earliest() {
    let (mut b, p) = planet(1, 0, 0, 0);
    b.demolition(p, BuildingKind::Refinery); // ends at 1 h; ore fills after 1,000 h
    let (w, _) = solve(b, p);
    assert_eq!(w.planets[&p].next_event.unwrap().at_ms, H);
}

#[test]
fn next_event_nothing_is_scheduled_when_nothing_moves() {
    let (b, p) = planet(0, 0, 50, 50);
    let (w, f) = solve(b, p);
    assert_eq!((f.ore_net, f.ingot_net), (0, 0));
    assert_eq!(w.planets[&p].next_event, None);
    assert!(w.queue().is_empty());
    assert_eq!(
        w.planets[&p].version, 1,
        "the solve still bumps the version"
    );
}

// --- Crossing ---

#[test]
fn crossing_lands_exactly_on_the_boundary_with_its_remainder_reset() {
    let rules = v0_rules();
    // Rising: ore fills from 9,999 t plus a remainder at +2 t/h (1 Drill, 1 Refinery), with a
    // planet-sized ingot store so ingots do not fill first.
    let (mut b, p) = planet(1, 1, 0, 0);
    b.stocks(
        p,
        Quantity::from_parts(9_999 * T, 0, 1_234_567, 0).unwrap(),
        Quantity::new(0, 0, 0),
    );
    let mut w = b.build();
    w.planets.get_mut(&p).unwrap().ingot_cap_g = 100_000 * T;
    w.solve_planet(p, &rules).unwrap();
    let at = w.planets[&p].next_event.unwrap().at_ms;
    w.advance_to(at, &rules).unwrap();
    let ore = w.planets[&p].ore;
    assert_eq!(
        (ore.value_g(), ore.remainder(), ore.checkpoint_ms()),
        (10_000 * T, 0, at)
    );
    assert_eq!(
        ore.rate_g_per_h(),
        0,
        "full, and the Drill now matches the intake"
    );

    // Falling: ingots run out at −50 t/h feeding a construction.
    let (mut b, p) = planet(0, 0, 0, 10);
    b.construction(p, BuildingKind::Drill, 0);
    let mut w = b.build();
    w.solve_planet(p, &rules).unwrap();
    let at = w.planets[&p].next_event.unwrap().at_ms;
    w.advance_to(at, &rules).unwrap();
    let ingots = w.planets[&p].ingots;
    assert_eq!(
        (ingots.value_g(), ingots.remainder(), ingots.checkpoint_ms()),
        (0, 0, at)
    );
    w.check_links().unwrap();
}

#[test]
fn crossing_a_re_solve_off_a_crossing_carries_every_remainder() {
    let rules = v0_rules();
    // Ore +2 t/h and ingots +16 t/h, both with remainders and far from their boundaries; a
    // re-solve at an odd instant must leave every value and remainder exactly where reading
    // forward puts them.
    let (mut b, p) = planet(1, 1, 0, 0);
    b.stocks(
        p,
        Quantity::from_parts(5_000 * T, 0, 1_234_567, 0).unwrap(),
        Quantity::from_parts(1_000 * T, 0, 2_345_678, 0).unwrap(),
    );
    let mut w = b.build();
    w.solve_planet(p, &rules).unwrap();
    let at = 3 * H + 17;
    w.advance_to(at, &rules).unwrap();
    let expected = (
        w.planets[&p].ore.advanced_to(at).unwrap(),
        w.planets[&p].ingots.advanced_to(at).unwrap(),
    );
    assert!(expected.0.remainder() != 0 && expected.1.remainder() != 0);
    w.solve_planet(p, &rules).unwrap();
    let (ore, ingots) = (w.planets[&p].ore, w.planets[&p].ingots);
    assert_eq!(
        (ore.value_g(), ore.remainder()),
        (expected.0.value_g(), expected.0.remainder())
    );
    assert_eq!(
        (ingots.value_g(), ingots.remainder()),
        (expected.1.value_g(), expected.1.remainder())
    );
}

// --- The halt chain (later crossings on one planet) ---

#[test]
fn halt_chain_ingot_storage_fills_and_halts_the_refinery_then_ore_storage_fills_and_halts_the_drill()
 {
    let rules = v0_rules();
    // Ingots fill first (4,920 t free at +16 t/h: 307.5 h); ore rises at +2 t/h meanwhile, then
    // at +10 t/h once the Refinery halts, and fills; then nothing moves.
    let (b, p) = planet(1, 1, 8_000, 80);
    let mut w = b.build();
    w.solve_planet(p, &rules).unwrap();
    let ingots_full = w.planets[&p].next_event.unwrap().at_ms;
    assert_eq!(ingots_full, 307 * H + H / 2);

    w.advance_to(ingots_full, &rules).unwrap();
    let f = planet_flows(&w, p, &rules).unwrap();
    let (drill, refinery) = (
        of_kind(&w, p, BuildingKind::Drill)[0],
        of_kind(&w, p, BuildingKind::Refinery)[0],
    );
    assert_eq!(flow(&f, refinery), (0, FlowLabel::Halted));
    assert_eq!(flow(&f, drill), (10 * T, FlowLabel::Running));
    assert_eq!(w.planets[&p].ingots.value_g(), 5_000 * T);
    // Ore at 8,000 + 2 × 307.5 = 8,615 t; 1,385 t free at +10 t/h is 138.5 h more.
    let ore_full = w.planets[&p].next_event.unwrap().at_ms;
    assert_eq!(ore_full, ingots_full + 138 * H + H / 2);

    w.advance_to(ore_full, &rules).unwrap();
    let f = planet_flows(&w, p, &rules).unwrap();
    assert_eq!(flow(&f, drill), (0, FlowLabel::Halted));
    assert_eq!(w.planets[&p].ore.value_g(), 10_000 * T);
    assert_eq!(w.planets[&p].next_event, None);
    w.advance_to(ore_full + 10_000 * H, &rules).unwrap();
    assert_eq!(w.planets[&p].ore.value_at(w.time_ms).unwrap(), 10_000 * T);
}

// --- Transitions are P1.7's: an explicit error, never a skip ---

#[test]
fn a_build_reaching_its_cost_stops_the_engine_until_transitions_exist() {
    let rules = v0_rules();
    let (mut b, p) = planet(0, 0, 0, 1_000);
    let drill = b.construction(p, BuildingKind::Drill, 140 * T);
    let mut w = b.build();
    w.solve_planet(p, &rules).unwrap();
    assert_eq!(
        w.advance_to(H, &rules),
        Err(EngineError::TransitionNotBuilt {
            planet: p,
            entity: drill.0
        })
    );
    assert!(w.is_poisoned());
}

#[test]
fn a_demolition_ending_stops_the_engine_until_transitions_exist() {
    let rules = v0_rules();
    let (mut b, p) = planet(0, 0, 0, 0);
    let refinery = b.demolition(p, BuildingKind::Refinery);
    let mut w = b.build();
    w.solve_planet(p, &rules).unwrap();
    assert_eq!(
        w.advance_to(2 * H, &rules),
        Err(EngineError::TransitionNotBuilt {
            planet: p,
            entity: refinery.0
        })
    );
}

#[test]
fn fleet_entries_still_stop_the_engine_until_fleets_exist() {
    use voidforge_core::state::{Cargo, Mission};
    use voidforge_testkit::ALICE;
    let rules = v0_rules();
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let ship = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let other = b.planet(None);
    b.send(fleet, other, Mission::Move, H);
    let mut w = b.build();
    w.planets.get_mut(&home).unwrap().ingot_cap_g = 100_000 * T; // nothing on home crosses first
    assert_eq!(
        w.advance_to(2 * H, &rules),
        Err(EngineError::StepNotBuilt(QueueKind::FleetArrival(fleet)))
    );
}

// --- The game design's examples, verbatim ---

/// "Ore storage with 1,000 t of free space, gaining 2 t/h, schedules a full-storage event at
/// hour 500." One Drill (10 t/h) and one Refinery (8 t/h) give +2 t/h. The ingot store is made
/// large so it does not fill first; the example is about the ore store alone.
#[test]
fn design_example_ore_storage_with_1000_t_free_gaining_2_t_per_hour_is_full_at_hour_500() {
    let rules = v0_rules();
    let (b, p) = planet(1, 1, 9_000, 0);
    let mut w = b.build();
    w.planets.get_mut(&p).unwrap().ingot_cap_g = 100_000 * T;
    w.solve_planet(p, &rules).unwrap();
    assert_eq!(w.planets[&p].ore.rate_g_per_h(), 2 * T);
    assert_eq!(w.planets[&p].next_event.unwrap().at_ms, 500 * H);
    w.advance_to(500 * H, &rules).unwrap();
    assert_eq!(w.planets[&p].ore.value_g(), 10_000 * T);
}

/// "A planet producing 10 t of ore per hour with two Refineries that each want 8 t per hour
/// gives each Refinery 5 t per hour."
#[test]
fn design_example_10_t_per_hour_shared_by_two_refineries_wanting_8_gives_5_each() {
    let (b, p) = planet(1, 2, 0, 0);
    let (w, f) = solve(b, p);
    let shares: Vec<i64> = of_kind(&w, p, BuildingKind::Refinery)
        .into_iter()
        .map(|r| flow(&f, r).0)
        .collect();
    assert_eq!(shares, [5 * T, 5 * T]);
}

#[test]
fn the_rules_given_must_be_the_worlds() {
    let mut rules: Rules = v0_rules();
    rules.version = 7;
    let (b, _) = planet(1, 0, 0, 0);
    let mut w = b.build();
    assert_eq!(
        w.advance_to(H, &rules),
        Err(EngineError::RulesMismatch { world: 0, given: 7 })
    );
}
