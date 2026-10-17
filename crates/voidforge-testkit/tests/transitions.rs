//! The per-planet solver's named tests, part 2 (Technical Plan P1.7 and the "Transitions at
//! one instant", "Cascades" and "Loop guard" bullets; Architectural Design, Per-planet solver
//! step 2; decision log D-026), on Rules v0 worlds. Names start with the bullet they cover:
//! `transitions_*`, `cascade_*` and `events_*`; the loop-guard cases are core unit tests
//! (`solver/tests.rs`), and the halt chain is P1.6's `halt_chain_*` in `tests/solver.rs`.

use voidforge_core::quantity::{MS_PER_HOUR, Quantity};
use voidforge_core::rules::GRAMS_PER_TON;
use voidforge_core::state::{
    BuildingId, BuildingKind, BuildingState, DomainEvent, EngineError, PlanetId, ShipBuildId,
    ShipBuildStatus, ShipId, ShipKind, ShipLocation, World,
};
use voidforge_testkit::{ALICE, WorldBuilder, v0_rules};

const T: i64 = GRAMS_PER_TON;
const H: i64 = MS_PER_HOUR;

/// Alice's Rules v0 homeworld at time 0: 1 Drill and 1 Refinery operational, 2,000 t of ore
/// and 1,500 t of ingots.
fn home() -> (WorldBuilder, PlanetId) {
    let mut b = WorldBuilder::new();
    let p = b.player(ALICE);
    (b, p)
}

/// Sets a build's delivered ingots, as if it had been fed that much so far.
fn delivered(w: &mut World, id: ShipBuildId, grams: i64) {
    w.ship_builds.get_mut(&id).unwrap().delivered = Quantity::new(grams, 0, w.time_ms);
}

fn construction_delivered(w: &mut World, id: BuildingId, grams: i64) {
    if let BuildingState::UnderConstruction { delivered, .. } =
        &mut w.buildings.get_mut(&id).unwrap().state
    {
        *delivered = Quantity::new(grams, 0, w.time_ms);
    }
}

fn solved(b: WorldBuilder, p: PlanetId, edit: impl FnOnce(&mut World)) -> World {
    let mut w = b.build();
    edit(&mut w);
    w.solve_planet(p, &v0_rules()).expect("the planet solves");
    w
}

#[test]
fn transitions_a_construction_completes_when_its_delivered_ingots_reach_its_cost() {
    let (mut b, p) = home();
    let drill = b.construction(p, BuildingKind::Drill, 0);
    let mut w = solved(b, p, |w| construction_delivered(w, drill, 140 * T));
    // 10 t to go at 50 t/h from stock: 12 minutes.
    assert_eq!(w.planets[&p].next_event.unwrap().at_ms, H / 5);
    let events = w.advance_to(H, &v0_rules()).unwrap();
    assert_eq!(
        events,
        [DomainEvent::BuildingCompleted {
            planet: p,
            building: drill,
            kind: BuildingKind::Drill
        }]
    );
    assert_eq!(w.buildings[&drill].state, BuildingState::Operational);
    w.check_links().unwrap();
}

#[test]
fn cascade_a_completed_building_changes_the_flows_within_the_same_solve() {
    let (mut b, p) = home();
    let drill = b.construction(p, BuildingKind::Drill, 0);
    let mut w = solved(b, p, |w| construction_delivered(w, drill, 140 * T));
    assert_eq!(
        w.planets[&p].ore.rate_g_per_h(),
        2 * T,
        "one Drill (10) less one Refinery (8)"
    );
    w.advance_to(H / 5, &v0_rules()).unwrap();
    // The completing solve already counts the new Drill: 20 − 8.
    assert_eq!(w.planets[&p].ore.rate_g_per_h(), 12 * T);
    assert_eq!(w.planets[&p].ore.checkpoint_ms(), H / 5);
}

/// A homeworld with an operational Shipyard, `active` cargo builds and `waiting` more.
fn shipyard(
    active: usize,
    waiting: usize,
) -> (WorldBuilder, PlanetId, BuildingId, Vec<ShipBuildId>) {
    let (mut b, p) = home();
    let yard = b.building(p, BuildingKind::Shipyard, BuildingState::Operational);
    let mut builds = Vec::new();
    for _ in 0..active {
        builds.push(b.ship_build(yard, ShipKind::CargoVessel, ShipBuildStatus::Active));
    }
    for _ in 0..waiting {
        builds.push(b.ship_build(yard, ShipKind::CargoVessel, ShipBuildStatus::Waiting));
    }
    (b, p, yard, builds)
}

#[test]
fn transitions_a_ship_build_completes_joins_the_roster_and_starts_the_next_waiting_build() {
    let (b, p, yard, builds) = shipyard(3, 2);
    let mut w = solved(b, p, |w| delivered(w, builds[0], 290 * T));
    let ship = ShipId(w.next_id);
    // 10 t to go at 75 t/h from stock: 8 minutes.
    let events = w.advance_to(H / 6, &v0_rules()).unwrap();
    assert_eq!(
        events,
        [DomainEvent::ShipCompleted {
            planet: p,
            shipyard: yard,
            ship,
            kind: ShipKind::CargoVessel
        }]
    );
    let s = &w.ships[&ship];
    assert_eq!(
        (s.owner, s.location, s.kind),
        (ALICE, ShipLocation::Roster(p), ShipKind::CargoVessel)
    );
    assert!(w.planets[&p].roster.contains(&ship));
    assert!(!w.ship_builds.contains_key(&builds[0]));
    // The lowest-ID waiting build started; the other still waits.
    assert_eq!(w.ship_builds[&builds[3]].status, ShipBuildStatus::Active);
    assert_eq!(w.ship_builds[&builds[4]].status, ShipBuildStatus::Waiting);
    assert_eq!(
        w.ship_builds[&builds[3]].delivered.checkpoint_ms(),
        8 * 60_000
    );
    w.check_links().unwrap();
}

#[test]
fn transitions_a_shipyard_starts_waiting_builds_up_to_its_parallel_limit() {
    let (b, p, _, builds) = shipyard(3, 4);
    let mut w = solved(b, p, |w| {
        for &id in &builds[..3] {
            delivered(w, id, 290 * T);
        }
    });
    let first = w.next_id;
    let events = w.advance_to(H / 6, &v0_rules()).unwrap();
    let ships: Vec<ShipId> = events
        .iter()
        .map(|e| match e {
            DomainEvent::ShipCompleted { ship, .. } => *ship,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(ships, [ShipId(first), ShipId(first + 1), ShipId(first + 2)]);
    let status = |i: usize| w.ship_builds[&builds[i]].status;
    assert_eq!(
        (status(3), status(4), status(5), status(6)),
        (
            ShipBuildStatus::Active,
            ShipBuildStatus::Active,
            ShipBuildStatus::Active,
            ShipBuildStatus::Waiting
        )
    );
}

#[test]
fn transitions_a_demolition_ends_and_frees_its_slot() {
    let (mut b, p) = home();
    let refinery = b.demolition(p, BuildingKind::Refinery); // ends at 1 h
    let mut w = solved(b, p, |_| {});
    let before = w.planets[&p].buildings.len();
    let events = w.advance_to(2 * H, &v0_rules()).unwrap();
    assert_eq!(
        events,
        [DomainEvent::DemolitionCompleted {
            planet: p,
            building: refinery,
            kind: BuildingKind::Refinery
        }]
    );
    assert!(!w.buildings.contains_key(&refinery));
    assert_eq!(w.planets[&p].buildings.len(), before - 1);
    w.check_links().unwrap();
}

#[test]
fn transitions_a_demolition_end_before_now_is_a_missed_event() {
    let mut b = WorldBuilder::new().at(2 * H);
    let p = b.player(ALICE);
    let refinery = b.building(
        p,
        BuildingKind::Refinery,
        BuildingState::Demolishing { ends_at_ms: H },
    );
    let mut w = b.build();
    let before = w.clone();
    assert_eq!(
        w.solve_planet(p, &v0_rules()),
        Err(EngineError::MissedCrossing {
            planet: p,
            entity: refinery.0,
            at_ms: H,
            now: 2 * H
        })
    );
    assert_eq!(w, before);
}

#[test]
fn transitions_several_at_once_all_apply_before_rates_are_set() {
    // At 1 h: a Drill construction completes (100 t + 50 t/h), an extra Refinery's demolition
    // ends, and a Cargo Vessel build completes (225 t + 75 t/h). IDs: construction, demolition,
    // Shipyard, build.
    let (mut b, p) = home();
    let drill = b.construction(p, BuildingKind::Drill, 0);
    let refinery = b.demolition(p, BuildingKind::Refinery);
    let yard = b.building(p, BuildingKind::Shipyard, BuildingState::Operational);
    let build = b.ship_build(yard, ShipKind::CargoVessel, ShipBuildStatus::Active);
    let mut w = solved(b, p, |w| {
        construction_delivered(w, drill, 100 * T);
        delivered(w, build, 225 * T);
    });
    assert_eq!(w.planets[&p].next_event.unwrap().at_ms, H);
    let ship = ShipId(w.next_id);
    let events = w.advance_to(H, &v0_rules()).unwrap();
    assert_eq!(
        events,
        [
            DomainEvent::BuildingCompleted {
                planet: p,
                building: drill,
                kind: BuildingKind::Drill
            },
            DomainEvent::DemolitionCompleted {
                planet: p,
                building: refinery,
                kind: BuildingKind::Refinery
            },
            DomainEvent::ShipCompleted {
                planet: p,
                shipyard: yard,
                ship,
                kind: ShipKind::CargoVessel
            },
        ]
    );
    // Rates were set once, after all three: two Drills (20) less one Refinery (8).
    assert_eq!(w.planets[&p].ore.rate_g_per_h(), 12 * T);
    assert_eq!(
        w.planets[&p].version, 2,
        "two solves: at 0, then once at 1 h for all three"
    );
}

#[test]
fn events_come_out_of_an_advance_in_the_order_they_happened() {
    let (mut b, p) = home();
    let drill = b.construction(p, BuildingKind::Drill, 0);
    let refinery = b.demolition(p, BuildingKind::Refinery);
    let mut w = solved(b, p, |w| construction_delivered(w, drill, 140 * T));
    let events = w.advance_to(2 * H, &v0_rules()).unwrap();
    assert_eq!(
        events,
        [
            DomainEvent::BuildingCompleted {
                planet: p,
                building: drill,
                kind: BuildingKind::Drill
            },
            DomainEvent::DemolitionCompleted {
                planet: p,
                building: refinery,
                kind: BuildingKind::Refinery
            },
        ]
    );
}

#[test]
fn transitions_a_ship_completing_on_an_unowned_planet_is_an_error_that_changes_nothing() {
    let mut b = WorldBuilder::new();
    let p = b.planet(None);
    let yard = b.building(p, BuildingKind::Shipyard, BuildingState::Operational);
    let build = b.ship_build(yard, ShipKind::CargoVessel, ShipBuildStatus::Active);
    let mut w = b.build();
    delivered(&mut w, build, 300 * T);
    let before = w.clone();
    assert_eq!(
        w.solve_planet(p, &v0_rules()),
        Err(EngineError::UnownedShipyard {
            planet: p,
            shipyard: yard
        })
    );
    assert_eq!(w, before);
}

#[test]
fn transitions_a_shipyard_demolished_with_builds_is_an_error_that_changes_nothing() {
    let (b, p, yard, _) = shipyard(0, 1);
    let mut w = b.build();
    w.buildings.get_mut(&yard).unwrap().state = BuildingState::Demolishing { ends_at_ms: 0 };
    let before = w.clone();
    assert_eq!(
        w.solve_planet(p, &v0_rules()),
        Err(EngineError::DemolishedShipyardHasBuilds {
            planet: p,
            shipyard: yard
        })
    );
    assert_eq!(w, before);
}

#[test]
fn transitions_inside_an_advance_an_error_poisons() {
    let mut b = WorldBuilder::new();
    let p = b.planet(None);
    let yard = b.building(p, BuildingKind::Shipyard, BuildingState::Operational);
    let build = b.ship_build(yard, ShipKind::CargoVessel, ShipBuildStatus::Active);
    let mut w = b.build();
    w.planets.get_mut(&p).unwrap().ingots = Quantity::new(1_000 * T, 0, 0);
    delivered(&mut w, build, 290 * T);
    w.solve_planet(p, &v0_rules()).unwrap();
    assert_eq!(
        w.advance_to(H, &v0_rules()),
        Err(EngineError::UnownedShipyard {
            planet: p,
            shipyard: yard
        })
    );
    assert!(w.is_poisoned());
}
