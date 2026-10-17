//! Builder tests: every builder yields a linked world, defaults come from the rules, overrides
//! apply, IDs are deterministic, and the busy world covers every variant and round-trips.

use voidforge_core::rules::DISTANCE_UNITS_PER_WORLD_UNIT;
use voidforge_core::state::{EngineError, QueueKind, Steps};

use super::*;

/// Counts the variants present. The matches are exhaustive, so a new variant fails to compile
/// here until the busy world (and this census) covers it.
#[derive(Default, Debug)]
struct Census {
    building_states: [usize; 3],
    building_kinds: [usize; 3],
    ship_build_status: [usize; 2],
    ship_kinds: [usize; 2],
    ship_locations: [usize; 2],
    fleet_states: [usize; 3],
    missions: [usize; 3],
    queue_kinds: [usize; 3],
    stale_entries: usize,
    owned: usize,
    unowned: usize,
}

fn census(w: &World) -> Census {
    let mut c = Census::default();
    for b in w.buildings.values() {
        c.building_states[match b.state {
            BuildingState::UnderConstruction { .. } => 0,
            BuildingState::Operational => 1,
            BuildingState::Demolishing { .. } => 2,
        }] += 1;
        c.building_kinds[match b.kind {
            BuildingKind::Drill => 0,
            BuildingKind::Refinery => 1,
            BuildingKind::Shipyard => 2,
        }] += 1;
    }
    for sb in w.ship_builds.values() {
        c.ship_build_status[match sb.status {
            ShipBuildStatus::Active => 0,
            ShipBuildStatus::Waiting => 1,
        }] += 1;
    }
    for s in w.ships.values() {
        c.ship_kinds[match s.kind {
            ShipKind::ColonyShip => 0,
            ShipKind::CargoVessel => 1,
        }] += 1;
        c.ship_locations[match s.location {
            ShipLocation::Roster(_) => 0,
            ShipLocation::Fleet(_) => 1,
        }] += 1;
    }
    for f in w.fleets.values() {
        c.fleet_states[match f.state {
            FleetState::Stationed { .. } => 0,
            FleetState::InTransit { .. } => 1,
            FleetState::Returning { .. } => 2,
        }] += 1;
        if let FleetState::InTransit { mission, .. } = f.state {
            c.missions[match mission {
                Mission::Colonize => 0,
                Mission::Transport => 1,
                Mission::Move => 2,
            }] += 1;
        }
    }
    for e in w.queue() {
        let (index, current) = match e.kind {
            QueueKind::PlanetChange(p) => (0, w.planets[&p].version),
            QueueKind::FleetArrival(f) => (1, w.fleets[&f].version),
            QueueKind::FleetReturn(f) => (2, w.fleets[&f].version),
        };
        c.queue_kinds[index] += 1;
        if e.version != current {
            c.stale_entries += 1;
        }
    }
    for p in w.planets.values() {
        if p.owner.is_some() {
            c.owned += 1
        } else {
            c.unowned += 1
        }
    }
    c
}

#[test]
fn the_busy_world_covers_every_variant() {
    let w = busy_world();
    let c = census(&w);
    let all = [
        &c.building_states[..],
        &c.building_kinds[..],
        &c.ship_build_status[..],
        &c.ship_kinds[..],
        &c.ship_locations[..],
        &c.fleet_states[..],
        &c.missions[..],
        &c.queue_kinds[..],
    ];
    assert!(
        all.iter().all(|counts| counts.iter().all(|&n| n > 0)),
        "{c:?}"
    );
    assert!(c.stale_entries > 0 && c.owned > 0 && c.unowned > 0, "{c:?}");
    assert_eq!(w.players.len(), 2);
    assert_eq!(w.check_links(), Ok(()));
}

#[test]
fn the_busy_world_round_trips_and_is_deterministic() {
    let w = busy_world();
    let text = serde_json::to_string(&w).unwrap();
    let back: World = serde_json::from_str(&text).unwrap();
    assert_eq!(back, w);
    assert_eq!(
        serde_json::to_string(&busy_world()).unwrap(),
        text,
        "building twice gives the same bytes"
    );
}

#[test]
fn a_player_gets_the_rules_homeworld() {
    let mut b = WorldBuilder::new().at(77);
    let home = b.player(ALICE);
    let rules = b.rules().clone();
    let w = b.build();
    let p = &w.planets[&home];
    assert_eq!(w.players[&ALICE].homeworld, home);
    assert_eq!(p.owner, Some(ALICE));
    assert_eq!(
        (p.slots, p.ore_cap_g, p.ingot_cap_g),
        (
            rules.planet.slots,
            rules.planet.ore_cap_g,
            rules.planet.ingot_cap_g
        )
    );
    assert_eq!(
        (p.ore.value_g(), p.ingots.value_g()),
        (rules.homeworld.ore_g, rules.homeworld.ingots_g)
    );
    assert_eq!((p.ore.checkpoint_ms(), p.ingots.checkpoint_ms()), (77, 77));
    let kinds: Vec<_> = p
        .buildings
        .iter()
        .map(|id| (w.buildings[id].kind, w.buildings[id].state))
        .collect();
    assert_eq!(
        kinds,
        [
            (BuildingKind::Drill, BuildingState::Operational),
            (BuildingKind::Refinery, BuildingState::Operational)
        ]
    );
}

#[test]
fn builds_take_cost_and_rated_rate_from_the_rules() {
    let mut b = WorldBuilder::new().at(1_000);
    let home = b.player(ALICE);
    let drill = b.construction(home, BuildingKind::Drill, 25_000_000);
    let yard = b.building(home, BuildingKind::Shipyard, BuildingState::Operational);
    let active = b.ship_build(yard, ShipKind::CargoVessel, ShipBuildStatus::Active);
    let waiting = b.ship_build(yard, ShipKind::ColonyShip, ShipBuildStatus::Waiting);
    let gone = b.demolition(home, BuildingKind::Refinery);
    let rules = b.rules().clone();
    let w = b.build();
    let BuildingState::UnderConstruction {
        delivered,
        cost_g,
        rated_g_per_h,
    } = w.buildings[&drill].state
    else {
        panic!("under construction")
    };
    assert_eq!(
        (
            delivered.value_g(),
            delivered.rate_g_per_h(),
            delivered.checkpoint_ms()
        ),
        (25_000_000, rated_g_per_h, 1_000)
    );
    assert_eq!(
        (cost_g, Some(rated_g_per_h)),
        (
            rules.buildings.drill.build.cost_g,
            rules.buildings.drill.build.rate_g_per_h()
        )
    );
    let a = w.ship_builds[&active];
    assert_eq!(
        (a.cost_g, a.delivered.rate_g_per_h()),
        (rules.ships.cargo_vessel.build.cost_g, a.rated_g_per_h)
    );
    let q = w.ship_builds[&waiting];
    assert_eq!(
        (q.delivered.rate_g_per_h(), q.rated_g_per_h),
        (0, rules.ships.colony_ship.build.rate_g_per_h().unwrap())
    );
    assert!(active < waiting, "queue order is ID order");
    assert_eq!(
        w.buildings[&gone].state,
        BuildingState::Demolishing {
            ends_at_ms: 1_000 + rules.buildings.refinery.demolition_time_ms
        }
    );
}

#[test]
fn fast_rules_give_fast_builds() {
    let fast = voidforge_rules::parse(include_str!("../../../rules/v0-fast.toml")).unwrap();
    let mut b = WorldBuilder::with_rules(fast.clone());
    let home = b.player(ALICE);
    let drill = b.construction(home, BuildingKind::Drill, 0);
    let w = b.build();
    assert_eq!(w.rules_version, 1);
    let BuildingState::UnderConstruction { rated_g_per_h, .. } = w.buildings[&drill].state else {
        panic!()
    };
    assert_eq!(
        Some(rated_g_per_h),
        fast.buildings.drill.build.rate_g_per_h()
    );
}

#[test]
fn fleets_move_ships_off_the_roster_and_recall_rewinds() {
    let mut b = WorldBuilder::new().at(10);
    let home = b.player(ALICE);
    let target = b.planet(None);
    let ship = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(
        &[ship],
        Cargo {
            ore_g: 5,
            ingots_g: 0,
        },
    );
    assert!(b.world().planets[&home].roster.is_empty());
    b.send(fleet, target, Mission::Move, 1_000);
    let mut b = b.at(400);
    b.recall(fleet);
    let w = b.build();
    let f = &w.fleets[&fleet];
    assert_eq!(
        f.state,
        FleetState::Returning {
            origin: home,
            arrives_at_ms: 790
        }
    );
    // Each schedule bumps the version: 1 at send, 2 at recall.
    assert_eq!(f.version, 2);
    assert_eq!(w.ships[&ship].location, ShipLocation::Fleet(fleet));
    let kinds: Vec<_> = w
        .queue()
        .iter()
        .map(|e| (e.at_ms, e.kind, e.version))
        .collect();
    assert_eq!(
        kinds,
        [
            (790, QueueKind::FleetReturn(fleet), 2),
            (1_000, QueueKind::FleetArrival(fleet), 1)
        ]
    );
}

#[test]
fn ids_are_dense_and_overridable() {
    let mut b = WorldBuilder::new().next_id(1_000);
    let home = b.player(ALICE);
    let w = b.build();
    assert_eq!(home, PlanetId(1_000));
    let ids: Vec<u64> = w.buildings.keys().map(|k| k.0).collect();
    assert_eq!(ids, [1_001, 1_002]);
    assert_eq!(w.next_id, 1_003);
    assert_eq!(
        w.planets[&home].position.x,
        1_000 * DISTANCE_UNITS_PER_WORLD_UNIT
    );
}

#[test]
fn try_build_reports_a_missing_player() {
    let mut b = WorldBuilder::new();
    b.planet(Some(BOB));
    let error = b
        .try_build()
        .expect_err("Bob owns a planet but is not a player");
    assert!(error.to_string().contains("is not a player"), "{error}");
}

#[test]
#[should_panic(expected = "ship builds need an operational Shipyard")]
fn ship_builds_need_a_shipyard() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let drill = *b.world().planets[&home].buildings.first().unwrap();
    b.ship_build(drill, ShipKind::ColonyShip, ShipBuildStatus::Active);
}

#[test]
#[should_panic(expected = "is elsewhere")]
fn a_fleet_takes_ships_from_one_roster() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let other = b.planet(Some(ALICE));
    let a = b.ship(ALICE, ShipKind::CargoVessel, home);
    let c = b.ship(ALICE, ShipKind::CargoVessel, other);
    b.fleet(&[a, c], Cargo::default());
}

/// Stands in for P1.8's return step: the fleet is stationed at its origin. Nothing else is due.
struct Returns;

impl Steps for Returns {
    type Event = FleetId;

    fn planet_change(
        &mut self,
        _: &mut World,
        planet: PlanetId,
    ) -> Result<Vec<FleetId>, EngineError> {
        panic!("no planet change is due, but planet {} was", planet.0)
    }

    fn fleet_arrival(
        &mut self,
        _: &mut World,
        fleet: FleetId,
    ) -> Result<Vec<FleetId>, EngineError> {
        panic!("no arrival is live, but fleet {} arrived", fleet.0)
    }

    fn fleet_return(
        &mut self,
        world: &mut World,
        fleet: FleetId,
    ) -> Result<Vec<FleetId>, EngineError> {
        let f = world.fleets.get_mut(&fleet).unwrap();
        let FleetState::Returning { origin, .. } = f.state else {
            panic!("fleet {} is not returning", fleet.0)
        };
        f.state = FleetState::Stationed { at: origin };
        Ok(vec![fleet])
    }
}

/// review-r1's repro: sent at 0 to arrive at 10 s, recalled at 1 s, back at 2 s, disbanded at
/// 3 s. The stale 10 s arrival entry stays queued for a fleet that no longer exists.
#[test]
fn a_world_with_a_disbanded_fleets_stale_arrival_loads() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let target = b.planet(None);
    let ship = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    b.send(fleet, target, Mission::Move, 10_000);
    let mut b = b.at(1_000);
    b.recall(fleet);
    let mut w = b.build();
    assert_eq!(w.advance_with(3_000, &mut Returns).unwrap(), [fleet]);
    // Disband at 3 s puts the ship back on the roster and removes the fleet, without searching
    // the queue.
    w.fleets.remove(&fleet);
    w.ships.get_mut(&ship).unwrap().location = ShipLocation::Roster(home);
    let _ = w.planets.get_mut(&home).unwrap().roster.insert(ship);

    assert_eq!(
        w.queue()
            .iter()
            .map(|e| (e.at_ms, e.kind))
            .collect::<Vec<_>>(),
        [(10_000, QueueKind::FleetArrival(fleet))]
    );
    w.check_links().expect("the stale entry is allowed");
    let text = serde_json::to_string(&w).unwrap();
    assert_eq!(serde_json::from_str::<World>(&text).unwrap(), w);
    // And the loop drops it when its time comes.
    assert_eq!(w.advance_with(20_000, &mut Returns).unwrap(), []);
    assert!(w.queue().is_empty());
}
