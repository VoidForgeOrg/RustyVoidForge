//! Fleets' named tests (Technical Plan P1.8: "Every mission outcome, including a failed
//! colonization and the tie, has a test"; Architectural Design, Fleets; Game Design, Missions
//! and Travel and recall; decision log D-027), on Rules v0 worlds through production
//! `advance_to`. Names start with what they cover: `travel_*`, `send_*`, `colonize_*`,
//! `transport_*`, `move_*`, `tie_*`, `same_moment_*`, `recall_*`, `post_check_*` and
//! `guard_*`. The pure distance and travel-time arithmetic is in core's `fleet/tests.rs`.

use voidforge_core::quantity::{MS_PER_HOUR, Quantity};
use voidforge_core::rules::{DISTANCE_UNITS_PER_WORLD_UNIT, GRAMS_PER_TON};
use voidforge_core::solver::{FlowLabel, planet_flows};
use voidforge_core::state::{
    BuildingKind, Cargo, DomainEvent, EngineError, FleetId, FleetState, Mission, PlanetId,
    PlayerId, Position, QueueKind, ScheduledEvent, ShipKind, StateError, Steps, World,
};
use voidforge_testkit::{ALICE, BOB, WorldBuilder, v0_rules};

const T: i64 = GRAMS_PER_TON;
const H: i64 = MS_PER_HOUR;
const U: i64 = DISTANCE_UNITS_PER_WORLD_UNIT;

/// Puts a planet `units` world units along the x axis.
fn place(b: &mut WorldBuilder, planet: PlanetId, units: i64) {
    b.position(
        planet,
        Position {
            x: units * U,
            y: 0,
            z: 0,
        },
    );
}

fn cargo(ore_t: i64, ingots_t: i64) -> Cargo {
    Cargo {
        ore_g: ore_t * T,
        ingots_g: ingots_t * T,
    }
}

/// Alice's homeworld at 0 and an unowned planet 120 world units away (12 hours by Colony Ship); her stocks are left
/// unsolved (rate 0), so only the fleet moves.
fn outpost() -> (WorldBuilder, PlanetId, PlanetId) {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let target = b.planet(None);
    place(&mut b, home, 0);
    place(&mut b, target, 120);
    (b, home, target)
}

/// Builds the world and sends `fleet` through the engine, returning the arrival time.
fn launch(b: WorldBuilder, fleet: FleetId, target: PlanetId, mission: Mission) -> (World, i64) {
    let mut w = b.build();
    w.send_fleet(fleet, target, mission, &v0_rules()).unwrap();
    let FleetState::InTransit { arrives_at_ms, .. } = w.fleets[&fleet].state else {
        panic!("fleet {} was not sent", fleet.0)
    };
    (w, arrives_at_ms)
}

fn ship_count(w: &World, kind: ShipKind) -> usize {
    w.ships.values().filter(|s| s.kind == kind).count()
}

// --- Travel: speed, capacity, distance and travel time ---

#[test]
fn travel_a_colony_ship_covers_120_units_in_12_hours() {
    let (mut b, home, target) = outpost();
    let ship = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let (w, arrives) = launch(b, fleet, target, Mission::Colonize);
    assert_eq!(arrives, 12 * H);
    assert_eq!(
        w.fleets[&fleet].state,
        FleetState::InTransit {
            origin: home,
            target,
            mission: Mission::Colonize,
            departed_at_ms: 0,
            arrives_at_ms: 12 * H
        }
    );
}

#[test]
fn travel_a_fleet_moves_at_its_slowest_ships_speed() {
    let (mut b, home, target) = outpost();
    let colony = b.ship(ALICE, ShipKind::ColonyShip, home);
    let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
    let solo = b.ship(ALICE, ShipKind::CargoVessel, home);
    let mixed = b.fleet(&[colony, hauler], Cargo::default());
    let fast = b.fleet(&[solo], Cargo::default());
    let rules = v0_rules();
    let mut w = b.build();
    assert_eq!(w.fleet_speed(mixed, &rules), Ok(10 * U));
    assert_eq!(w.fleet_speed(fast, &rules), Ok(15 * U));
    w.send_fleet(mixed, target, Mission::Transport, &rules)
        .unwrap();
    w.send_fleet(fast, target, Mission::Transport, &rules)
        .unwrap();
    let arrival = |f: FleetId| match w.fleets[&f].state {
        FleetState::InTransit { arrives_at_ms, .. } => arrives_at_ms,
        other => panic!("{other:?}"),
    };
    // 120 units at 10 per hour, and at 15 per hour.
    assert_eq!((arrival(mixed), arrival(fast)), (12 * H, 8 * H));
}

#[test]
fn travel_distance_is_the_3d_integer_square_root_and_the_time_rounds_up() {
    // (20, −30, 60) world units is 70 units away; at a Colony Ship's speed, 7 hours exactly.
    let (mut b, home, target) = outpost();
    b.position(
        target,
        Position {
            x: 20 * U,
            y: -30 * U,
            z: 60 * U,
        },
    );
    let ship = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let (_, arrives) = launch(b, fleet, target, Mission::Colonize);
    assert_eq!(arrives, 7 * H);

    // Under a speed of 7,000 units per hour, 1 distance unit takes 514.28… ms, so 515.
    let mut rules = v0_rules();
    rules.ships.colony_ship.speed_du_per_h = 7_000;
    rules.validate().unwrap();
    let mut b = WorldBuilder::with_rules(rules.clone());
    let home = b.player(ALICE);
    let target = b.planet(None);
    b.position(home, Position { x: 0, y: 0, z: 0 });
    b.position(target, Position { x: 1, y: 0, z: 0 });
    let ship = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let mut w = b.build();
    w.send_fleet(fleet, target, Mission::Move, &rules).unwrap();
    assert!(matches!(
        w.fleets[&fleet].state,
        FleetState::InTransit {
            arrives_at_ms: 515,
            ..
        }
    ));
}

#[test]
fn travel_capacity_is_the_sum_of_every_ships_capacity() {
    let (mut b, home, _) = outpost();
    let colony = b.ship(ALICE, ShipKind::ColonyShip, home);
    let one = b.ship(ALICE, ShipKind::CargoVessel, home);
    let two = b.ship(ALICE, ShipKind::CargoVessel, home);
    let colonists = b.fleet(&[colony], Cargo::default());
    let haulers = b.fleet(&[one, two], Cargo::default());
    let rules = v0_rules();
    let w = b.build();
    assert_eq!(w.fleet_capacity(colonists, &rules), Ok(0));
    assert_eq!(w.fleet_capacity(haulers, &rules), Ok(1_000 * T));
}

// --- Send: the engine side ---

#[test]
fn send_queues_the_arrival_and_bumps_the_version() {
    let (mut b, home, target) = outpost();
    let ship = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let (w, arrives) = launch(b, fleet, target, Mission::Colonize);
    assert_eq!(w.fleets[&fleet].version, 1);
    assert!(
        w.queue().iter().any(|e| e.at_ms == arrives
            && e.kind == QueueKind::FleetArrival(fleet)
            && e.version == 1)
    );
    w.check_links().unwrap();
}

#[test]
fn send_refuses_a_fleet_not_stationed_and_changes_nothing() {
    let (mut b, home, target) = outpost();
    let ship = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let (mut w, _) = launch(b, fleet, target, Mission::Colonize);
    let before = w.clone();
    assert_eq!(
        w.send_fleet(fleet, home, Mission::Move, &v0_rules()),
        Err(EngineError::FleetNotStationed(fleet))
    );
    assert_eq!(w, before);
}

#[test]
fn send_to_a_planet_at_zero_distance_is_refused_and_changes_nothing() {
    let (mut b, home, target) = outpost();
    place(&mut b, target, 0);
    let ship = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let mut w = b.build();
    let before = w.clone();
    assert_eq!(
        w.send_fleet(fleet, target, Mission::Move, &v0_rules()),
        Err(EngineError::NotAfterNow { at_ms: 0, now: 0 })
    );
    assert_eq!(w, before);
}

#[test]
fn send_beyond_64_bits_is_a_typed_error_that_changes_nothing() {
    let (mut b, home, target) = outpost();
    let far = |v| Position { x: v, y: v, z: v };
    b.position(home, far(i64::MIN));
    b.position(target, far(i64::MAX));
    let ship = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let mut w = b.build();
    let before = w.clone();
    assert_eq!(
        w.send_fleet(fleet, target, Mission::Move, &v0_rules()),
        Err(EngineError::FleetOutOfRange {
            fleet,
            what: "distance"
        })
    );
    assert_eq!(w, before);

    // In range, but arriving after the end of time.
    b = WorldBuilder::new().at(i64::MAX - H);
    let home = b.player(ALICE);
    let target = b.planet(None);
    place(&mut b, home, 0);
    place(&mut b, target, 120);
    let ship = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let mut w = b.build();
    let before = w.clone();
    assert_eq!(
        w.send_fleet(fleet, target, Mission::Move, &v0_rules()),
        Err(EngineError::FleetOutOfRange {
            fleet,
            what: "arrival time"
        })
    );
    assert_eq!(w, before);
}

// --- Colonize ---

#[test]
fn colonize_an_unowned_planet_consumes_a_colony_ship_claims_it_and_unloads() {
    let (mut b, home, target) = outpost();
    let colony = b.ship(ALICE, ShipKind::ColonyShip, home);
    let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(&[colony, hauler], cargo(100, 400));
    let (mut w, arrives) = launch(b, fleet, target, Mission::Colonize);
    let version = w.planets[&target].version;

    let events = w.advance_to(arrives, &v0_rules()).unwrap();
    assert_eq!(
        events,
        [
            DomainEvent::FleetArrived {
                fleet,
                planet: target,
                mission: Mission::Colonize
            },
            DomainEvent::PlanetColonized {
                planet: target,
                owner: ALICE,
                fleet,
                ship: colony
            },
            DomainEvent::CargoUnloaded {
                planet: target,
                fleet,
                cargo: cargo(100, 400)
            },
        ]
    );
    let p = &w.planets[&target];
    assert_eq!(p.owner, Some(ALICE));
    assert_eq!((p.ore.value_g(), p.ingots.value_g()), (100 * T, 400 * T));
    assert!(
        !w.ships.contains_key(&colony),
        "the Colony Ship is consumed"
    );
    let f = &w.fleets[&fleet];
    assert_eq!(f.state, FleetState::Stationed { at: target });
    assert_eq!(f.ships, [hauler].into());
    assert_eq!(f.cargo, Cargo::default());
    assert!(p.version > version, "the target was re-solved");
    // A new colony has no buildings, so nothing on it moves.
    assert_eq!(p.next_event, None);
    w.check_links().unwrap();
}

#[test]
fn colonize_unloads_partially_into_free_storage_and_keeps_the_remainder() {
    let (mut b, home, target) = outpost();
    // 50 t of ore storage free; 100 t of ingot storage free, the stock carrying a remainder.
    b.stocks(
        target,
        Quantity::new(9_950 * T, 0, 0),
        Quantity::from_parts(4_900 * T, 0, 123, 0).unwrap(),
    );
    let colony = b.ship(ALICE, ShipKind::ColonyShip, home);
    let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(&[colony, hauler], cargo(100, 300));
    let (mut w, arrives) = launch(b, fleet, target, Mission::Colonize);

    let events = w.advance_to(arrives, &v0_rules()).unwrap();
    assert_eq!(
        events[2],
        DomainEvent::CargoUnloaded {
            planet: target,
            fleet,
            cargo: cargo(50, 100)
        }
    );
    let p = &w.planets[&target];
    assert_eq!(p.ore.value_g(), 10_000 * T);
    assert_eq!((p.ingots.value_g(), p.ingots.remainder()), (5_000 * T, 123));
    assert_eq!(
        w.fleets[&fleet].cargo,
        cargo(50, 200),
        "the rest stays aboard"
    );
}

#[test]
fn colonize_extra_colony_ships_stay_and_the_lowest_id_is_consumed() {
    let (mut b, home, target) = outpost();
    let first = b.ship(ALICE, ShipKind::ColonyShip, home);
    let second = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[second, first], Cargo::default());
    let (mut w, arrives) = launch(b, fleet, target, Mission::Colonize);
    w.advance_to(arrives, &v0_rules()).unwrap();
    assert!(!w.ships.contains_key(&first));
    assert_eq!(w.fleets[&fleet].ships, [second].into());
    assert_eq!(w.fleets[&fleet].state, FleetState::Stationed { at: target });
}

#[test]
fn colonize_with_a_lone_colony_ship_removes_the_emptied_fleet() {
    let (mut b, home, target) = outpost();
    let colony = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[colony], Cargo::default());
    let (mut w, arrives) = launch(b, fleet, target, Mission::Colonize);
    let events = w.advance_to(arrives, &v0_rules()).unwrap();
    assert_eq!(
        events,
        [
            DomainEvent::FleetArrived {
                fleet,
                planet: target,
                mission: Mission::Colonize
            },
            DomainEvent::PlanetColonized {
                planet: target,
                owner: ALICE,
                fleet,
                ship: colony
            },
            DomainEvent::FleetRemoved {
                fleet,
                planet: target
            },
        ]
    );
    assert!(w.fleets.is_empty() && w.ships.is_empty());
    assert_eq!(w.planets[&target].owner, Some(ALICE));
    w.check_links().unwrap();
}

#[test]
fn colonize_an_owned_planet_fails_consuming_nothing_and_keeping_cargo_aboard() {
    for owner in [BOB, ALICE] {
        let mut b = WorldBuilder::new();
        let home = b.player(ALICE);
        let target = if owner == BOB {
            b.player(BOB)
        } else {
            b.planet(Some(ALICE))
        };
        place(&mut b, home, 0);
        place(&mut b, target, 120);
        let colony = b.ship(ALICE, ShipKind::ColonyShip, home);
        let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
        let fleet = b.fleet(&[colony, hauler], cargo(0, 400));
        let (mut w, arrives) = launch(b, fleet, target, Mission::Colonize);
        let stocks = (w.planets[&target].ore, w.planets[&target].ingots);

        let events = w.advance_to(arrives, &v0_rules()).unwrap();
        assert_eq!(
            events,
            [
                DomainEvent::FleetArrived {
                    fleet,
                    planet: target,
                    mission: Mission::Colonize
                },
                DomainEvent::ColonizationFailed {
                    planet: target,
                    fleet
                },
            ],
            "owned by {owner:?}"
        );
        assert_eq!(w.planets[&target].owner, Some(owner));
        assert!(w.ships.contains_key(&colony), "nothing is consumed");
        let f = &w.fleets[&fleet];
        assert_eq!(f.ships, [colony, hauler].into());
        assert_eq!(f.cargo, cargo(0, 400), "cargo stays aboard");
        assert_eq!(f.state, FleetState::Stationed { at: target });
        let p = &w.planets[&target];
        assert_eq!(
            (
                p.ore.value_at(arrives).unwrap(),
                p.ingots.value_at(arrives).unwrap()
            ),
            (
                stocks.0.value_at(arrives).unwrap(),
                stocks.1.value_at(arrives).unwrap()
            ),
            "nothing unloads"
        );
    }
}

// --- The same-moment tie ---

/// Alice from 120 units west and Bob from 120 units east race their Colony Ships to an unowned
/// planet, arriving at the same millisecond. The fleet assembled first has the lower ID.
fn race(alice_first: bool) -> (World, FleetId, FleetId, PlanetId, Vec<DomainEvent>) {
    let mut b = WorldBuilder::new();
    let alice_home = b.player(ALICE);
    let bob_home = b.player(BOB);
    let target = b.planet(None);
    place(&mut b, alice_home, -120);
    place(&mut b, bob_home, 120);
    place(&mut b, target, 0);
    let alice_ship = b.ship(ALICE, ShipKind::ColonyShip, alice_home);
    let bob_ship = b.ship(BOB, ShipKind::ColonyShip, bob_home);
    let alice_extra = b.ship(ALICE, ShipKind::ColonyShip, alice_home);
    let bob_extra = b.ship(BOB, ShipKind::ColonyShip, bob_home);
    let (alice, bob) = if alice_first {
        let a = b.fleet(&[alice_ship, alice_extra], Cargo::default());
        (a, b.fleet(&[bob_ship, bob_extra], Cargo::default()))
    } else {
        let bob = b.fleet(&[bob_ship, bob_extra], Cargo::default());
        (b.fleet(&[alice_ship, alice_extra], Cargo::default()), bob)
    };
    let rules = v0_rules();
    let mut w = b.build();
    w.send_fleet(alice, target, Mission::Colonize, &rules)
        .unwrap();
    w.send_fleet(bob, target, Mission::Colonize, &rules)
        .unwrap();
    let events = w.advance_to(12 * H, &rules).unwrap();
    (w, alice, bob, target, events)
}

#[test]
fn tie_two_colony_fleets_at_the_same_millisecond_the_lower_fleet_id_claims_the_planet() {
    for alice_first in [true, false] {
        let (w, alice, bob, target, events) = race(alice_first);
        let (winner, loser, owner): (FleetId, FleetId, PlayerId) = if alice_first {
            (alice, bob, ALICE)
        } else {
            (bob, alice, BOB)
        };
        assert!(winner < loser);
        let outcomes: Vec<&DomainEvent> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    DomainEvent::PlanetColonized { .. } | DomainEvent::ColonizationFailed { .. }
                )
            })
            .collect();
        assert!(
            matches!(outcomes[..], [
                DomainEvent::PlanetColonized { fleet: f, owner: o, .. },
                DomainEvent::ColonizationFailed { fleet: g, .. },
            ] if *f == winner && *o == owner && *g == loser),
            "{events:?}"
        );
        assert_eq!(w.planets[&target].owner, Some(owner));
        assert_eq!(w.fleets[&winner].ships.len(), 1);
        assert_eq!(
            w.fleets[&loser].ships.len(),
            2,
            "the loser consumes nothing"
        );
        for f in [winner, loser] {
            assert_eq!(w.fleets[&f].state, FleetState::Stationed { at: target });
        }
    }
}

#[test]
fn same_moment_a_planet_event_runs_before_an_arrival() {
    // Alice's colony completes a Drill at 3 h, the instant a fleet arrives there: the planet
    // change (rank 0) runs first, then the arrival (rank 1).
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let colony = b.planet(Some(ALICE));
    place(&mut b, home, 0);
    place(&mut b, colony, 30);
    b.stocks(
        colony,
        Quantity::new(0, 0, 0),
        Quantity::new(1_000 * T, 0, 0),
    );
    let drill = b.construction(colony, BuildingKind::Drill, 0);
    let ship = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let rules = v0_rules();
    let mut w = b.build();
    w.solve_planet(colony, &rules).unwrap();
    assert_eq!(w.planets[&colony].next_event.map(|e| e.at_ms), Some(3 * H));
    w.send_fleet(fleet, colony, Mission::Move, &rules).unwrap();
    assert_eq!(
        w.advance_to(3 * H, &rules).unwrap(),
        [
            DomainEvent::BuildingCompleted {
                planet: colony,
                building: drill,
                kind: BuildingKind::Drill
            },
            DomainEvent::FleetArrived {
                fleet,
                planet: colony,
                mission: Mission::Move
            },
        ]
    );
}

// --- Transport and Move ---

#[test]
fn transport_unloads_everything_that_fits_and_the_re_solve_puts_it_to_work() {
    // Alice's colony has a Drill under construction, halted with no ingots. 100 t of ingots
    // arrive: the construction runs at its rated 50 t/h, so the stock empties in 2 hours.
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let colony = b.planet(Some(ALICE));
    place(&mut b, home, 0);
    place(&mut b, colony, 120);
    let drill = b.construction(colony, BuildingKind::Drill, 0);
    let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(&[hauler], cargo(30, 100));
    let rules = v0_rules();
    let (mut w, arrives) = launch(b, fleet, colony, Mission::Transport);
    w.solve_planet(colony, &rules).unwrap();
    assert_eq!(
        planet_flows(&w, colony, &rules).unwrap().buildings[&drill].label,
        FlowLabel::Halted
    );

    let events = w.advance_to(arrives, &rules).unwrap();
    assert_eq!(
        events,
        [
            DomainEvent::FleetArrived {
                fleet,
                planet: colony,
                mission: Mission::Transport
            },
            DomainEvent::CargoUnloaded {
                planet: colony,
                fleet,
                cargo: cargo(30, 100)
            },
        ]
    );
    assert_eq!(w.fleets[&fleet].cargo, Cargo::default());
    let flow = planet_flows(&w, colony, &rules).unwrap().buildings[&drill];
    assert_eq!(
        (flow.rate_g_per_h, flow.label),
        (50 * T, FlowLabel::Running)
    );
    assert_eq!(
        w.planets[&colony].next_event.map(|e| e.at_ms),
        Some(arrives + 2 * H)
    );
    w.check_links().unwrap();
}

#[test]
fn transport_unloads_each_resource_up_to_its_free_storage_and_the_rest_stays_aboard() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let colony = b.planet(Some(ALICE));
    place(&mut b, home, 0);
    place(&mut b, colony, 120);
    // Ore storage full; 10 t of ingot storage free.
    b.stocks(
        colony,
        Quantity::new(10_000 * T, 0, 0),
        Quantity::new(4_990 * T, 0, 0),
    );
    let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(&[hauler], cargo(200, 300));
    let (mut w, arrives) = launch(b, fleet, colony, Mission::Transport);
    let events = w.advance_to(arrives, &v0_rules()).unwrap();
    assert_eq!(
        events[1],
        DomainEvent::CargoUnloaded {
            planet: colony,
            fleet,
            cargo: cargo(0, 10)
        }
    );
    let p = &w.planets[&colony];
    assert_eq!(
        (p.ore.value_g(), p.ingots.value_g()),
        (10_000 * T, 5_000 * T)
    );
    assert_eq!(w.fleets[&fleet].cargo, cargo(200, 290));

    // Nothing fits at all: no unload event, cargo untouched.
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let colony = b.planet(Some(ALICE));
    place(&mut b, home, 0);
    place(&mut b, colony, 120);
    b.stocks(
        colony,
        Quantity::new(10_000 * T, 0, 0),
        Quantity::new(5_000 * T, 0, 0),
    );
    let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(&[hauler], cargo(200, 300));
    let (mut w, arrives) = launch(b, fleet, colony, Mission::Transport);
    let events = w.advance_to(arrives, &v0_rules()).unwrap();
    assert_eq!(
        events,
        [DomainEvent::FleetArrived {
            fleet,
            planet: colony,
            mission: Mission::Transport
        }]
    );
    assert_eq!(w.fleets[&fleet].cargo, cargo(200, 300));
}

#[test]
fn move_stations_the_fleet_and_unloads_nothing() {
    let (mut b, home, target) = outpost();
    let colony = b.ship(ALICE, ShipKind::ColonyShip, home);
    let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(&[colony, hauler], cargo(10, 20));
    let (mut w, arrives) = launch(b, fleet, target, Mission::Move);
    let events = w.advance_to(arrives, &v0_rules()).unwrap();
    assert_eq!(
        events,
        [DomainEvent::FleetArrived {
            fleet,
            planet: target,
            mission: Mission::Move
        }]
    );
    let f = &w.fleets[&fleet];
    assert_eq!(f.state, FleetState::Stationed { at: target });
    assert_eq!(f.cargo, cargo(10, 20));
    assert_eq!(f.ships, [colony, hauler].into());
    assert_eq!(w.planets[&target].owner, None, "a Move claims nothing");
    assert!(
        w.planets[&target].roster.is_empty(),
        "ships stay in the fleet"
    );
}

// --- Recall ---

#[test]
fn recall_5_hours_into_a_12_hour_trip_returns_in_5_hours_intact() {
    // The game design's example.
    let (mut b, home, target) = outpost();
    let colony = b.ship(ALICE, ShipKind::ColonyShip, home);
    let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(&[colony, hauler], cargo(0, 300));
    let (mut w, arrives) = launch(b, fleet, target, Mission::Colonize);
    assert_eq!(arrives, 12 * H);
    let rules = v0_rules();
    assert_eq!(w.advance_to(5 * H, &rules).unwrap(), []);
    w.recall_fleet(fleet).unwrap();
    assert_eq!(
        w.fleets[&fleet].state,
        FleetState::Returning {
            origin: home,
            arrives_at_ms: 10 * H
        }
    );
    assert_eq!(w.fleets[&fleet].version, 2);
    w.check_links().unwrap();

    assert_eq!(
        w.advance_to(10 * H, &rules).unwrap(),
        [DomainEvent::FleetReturned {
            fleet,
            planet: home
        }]
    );
    let f = &w.fleets[&fleet];
    assert_eq!(f.state, FleetState::Stationed { at: home });
    assert_eq!(f.cargo, cargo(0, 300));
    assert_eq!(f.ships, [colony, hauler].into());
    // The original arrival at 12 h is stale: it runs nothing and claims nothing.
    assert!(
        w.queue()
            .iter()
            .any(|e| e.at_ms == 12 * H && e.kind == QueueKind::FleetArrival(fleet))
    );
    assert_eq!(w.advance_to(13 * H, &rules).unwrap(), []);
    assert_eq!(w.planets[&target].owner, None);
    assert_eq!(ship_count(&w, ShipKind::ColonyShip), 1);
}

#[test]
fn recall_at_the_departure_instant_stations_the_fleet_at_once() {
    let (mut b, home, target) = outpost();
    let colony = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[colony], Cargo::default());
    let (mut w, _) = launch(b, fleet, target, Mission::Colonize);
    w.recall_fleet(fleet).unwrap();
    assert_eq!(w.fleets[&fleet].state, FleetState::Stationed { at: home });
    assert_eq!(w.fleets[&fleet].version, 2, "the arrival goes stale");
    w.check_links().unwrap();
    assert_eq!(w.advance_to(13 * H, &v0_rules()).unwrap(), []);
    assert_eq!(w.planets[&target].owner, None);
}

#[test]
fn recall_one_millisecond_in_returns_one_millisecond_later() {
    let (mut b, home, target) = outpost();
    let colony = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[colony], Cargo::default());
    let (mut w, _) = launch(b, fleet, target, Mission::Colonize);
    let rules = v0_rules();
    w.advance_to(1, &rules).unwrap();
    w.recall_fleet(fleet).unwrap();
    assert_eq!(
        w.fleets[&fleet].state,
        FleetState::Returning {
            origin: home,
            arrives_at_ms: 2
        }
    );
    assert_eq!(
        w.advance_to(2, &rules).unwrap(),
        [DomainEvent::FleetReturned {
            fleet,
            planet: home
        }]
    );
}

#[test]
fn recall_works_only_in_transit_and_only_once() {
    let (mut b, home, target) = outpost();
    let colony = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[colony], Cargo::default());
    let mut w = b.build();
    let not_in_transit = Err(EngineError::FleetNotInTransit(fleet));
    assert_eq!(w.recall_fleet(fleet), not_in_transit, "stationed");
    w.send_fleet(fleet, target, Mission::Colonize, &v0_rules())
        .unwrap();
    w.advance_to(H, &v0_rules()).unwrap();
    w.recall_fleet(fleet).unwrap();
    let before = w.clone();
    assert_eq!(w.recall_fleet(fleet), not_in_transit, "already recalled");
    assert_eq!(w, before);
    // A returning fleet cannot be sent either; once back, it can.
    assert_eq!(
        w.send_fleet(fleet, target, Mission::Colonize, &v0_rules()),
        Err(EngineError::FleetNotStationed(fleet))
    );
    w.advance_to(2 * H, &v0_rules()).unwrap();
    assert_eq!(w.recall_fleet(fleet), not_in_transit, "back home");
}

// --- The D-023 post-check, extended to the destination ---

/// Production planet changes, and an arrival that stations the fleet but leaves its target
/// with a next event that is not in the queue: a broken cross-entity schedule.
struct BreaksTheTarget;

impl Steps for BreaksTheTarget {
    type Event = DomainEvent;

    fn planet_change(
        &mut self,
        w: &mut World,
        p: PlanetId,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        w.solve_planet(p, &v0_rules())
    }

    fn fleet_arrival(
        &mut self,
        w: &mut World,
        f: FleetId,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        let FleetState::InTransit { target, .. } = w.fleets[&f].state else {
            panic!("fleet {} is not in transit", f.0)
        };
        w.fleets.get_mut(&f).unwrap().state = FleetState::Stationed { at: target };
        let p = w.planets.get_mut(&target).unwrap();
        p.next_event = Some(ScheduledEvent {
            at_ms: w.time_ms + 1,
            version: p.version,
        });
        Ok(vec![])
    }

    fn fleet_return(&mut self, _: &mut World, _: FleetId) -> Result<Vec<DomainEvent>, EngineError> {
        unreachable!("no fleet returns here")
    }
}

#[test]
fn post_check_an_arrival_that_breaks_its_targets_schedule_stops_and_poisons() {
    let (mut b, home, target) = outpost();
    let ship = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let (mut w, arrives) = launch(b, fleet, target, Mission::Move);
    let result = w.advance_with(arrives, &mut BreaksTheTarget);
    let Err(EngineError::State(StateError::Link(message))) = result else {
        panic!("expected the destination's schedule check to fail, got {result:?}")
    };
    assert!(
        message.contains(&format!("planet {}", target.0)) && message.contains("not in the queue"),
        "{message}"
    );
    assert!(w.is_poisoned());
}

// --- Guards on unreachable arrivals ---

#[test]
fn guard_a_colonize_arrival_without_a_colony_ship_is_a_bug_that_poisons() {
    let (mut b, home, target) = outpost();
    let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(&[hauler], cargo(0, 10));
    let (mut w, arrives) = launch(b, fleet, target, Mission::Colonize);
    assert_eq!(
        w.advance_to(arrives, &v0_rules()),
        Err(EngineError::NoColonyShip(fleet))
    );
    assert!(w.is_poisoned());
}

#[test]
fn guard_cargo_left_aboard_a_fleet_with_no_ships_is_a_bug_that_poisons() {
    // Only reachable by breaking the capacity rule: a lone Colony Ship carrying cargo onto a
    // planet with no free storage.
    let (mut b, home, target) = outpost();
    b.stocks(
        target,
        Quantity::new(0, 0, 0),
        Quantity::new(5_000 * T, 0, 0),
    );
    let colony = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[colony], cargo(0, 10));
    let (mut w, arrives) = launch(b, fleet, target, Mission::Colonize);
    assert_eq!(
        w.advance_to(arrives, &v0_rules()),
        Err(EngineError::CargoWithoutShips(fleet))
    );
    assert!(w.is_poisoned());
}

#[test]
fn guard_send_and_recall_refuse_a_poisoned_world() {
    let (mut b, home, target) = outpost();
    let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
    let other = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(&[hauler], Cargo::default());
    let spare = b.fleet(&[other], Cargo::default());
    let (mut w, arrives) = launch(b, fleet, target, Mission::Colonize);
    w.advance_to(arrives, &v0_rules()).unwrap_err();
    assert_eq!(
        w.send_fleet(spare, target, Mission::Move, &v0_rules()),
        Err(EngineError::Poisoned)
    );
    assert_eq!(w.recall_fleet(fleet), Err(EngineError::Poisoned));
}
