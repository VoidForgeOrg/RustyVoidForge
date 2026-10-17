//! Commands, part 1: named tests (Technical Plan P1.9: "Every accept and reject rule for these
//! six commands has a test"; Architectural Design, Engine core and Commands; Game Design,
//! Player actions; decision log D-028), on Rules v0 worlds. Names start with the command they
//! cover: `join_*`, `build_*`, `cancel_construction_*`, `demolish_*`, `queue_*`,
//! `cancel_ship_build_*`, then `apply_*` for what holds for every command (order of checks, the
//! instant, rejections changing nothing, errors). Homeworld candidates are built with testkit
//! until P1.12.

use voidforge_core::command::{Action, Applied, Command, Rejection};
use voidforge_core::quantity::MS_PER_HOUR;
use voidforge_core::rules::GRAMS_PER_TON;
use voidforge_core::solver::planet_flows;
use voidforge_core::state::{
    BuildingId, BuildingKind, BuildingState, Cargo, DomainEvent, EngineError, FleetState, Mission,
    PlanetId, PlayerId, ShipBuildId, ShipBuildStatus, ShipKind, World,
};
use voidforge_testkit::determinism::canonical_bytes;
use voidforge_testkit::{ALICE, BOB, WorldBuilder, v0_rules};

const T: i64 = GRAMS_PER_TON;
const H: i64 = MS_PER_HOUR;
const CAROL: PlayerId = PlayerId(0x0192_8c5e_7a3b_7c2d_9e4f_5a6b_7c8d_9e11);

fn command(player: PlayerId, action: Action) -> Command {
    Command { player, action }
}

/// Applies an accepted command at `t` and returns its events.
fn accept(w: &mut World, player: PlayerId, action: Action, t: i64) -> Vec<DomainEvent> {
    let applied = w.apply(&command(player, action), t, &v0_rules()).unwrap();
    applied
        .outcome
        .unwrap_or_else(|r| panic!("{action:?} by {player:?} was rejected: {}", r.code()))
}

/// Applies a command at `t` that must be rejected with `reason`, and checks the world equals
/// one that only advanced to `t`, byte for byte.
fn reject(w: &mut World, player: PlayerId, action: Action, t: i64, reason: Rejection) {
    let rules = v0_rules();
    let mut advanced = w.clone();
    let events = advanced.advance_to(t, &rules).unwrap();
    let applied = w.apply(&command(player, action), t, &rules).unwrap();
    assert_eq!(
        applied,
        Applied {
            advanced: events,
            outcome: Err(reason)
        },
        "{action:?} by {player:?}"
    );
    assert_eq!(
        canonical_bytes(&*w),
        canonical_bytes(&advanced),
        "{action:?} changed the world"
    );
    assert_eq!(*w, advanced);
}

/// Alice at home (1 Drill, 1 Refinery, starting stocks), solved at time 0, and Bob's homeworld,
/// plus an unowned planet.
fn two_players() -> (World, PlanetId, PlanetId, PlanetId) {
    let mut b = WorldBuilder::new();
    let alice = b.player(ALICE);
    let bob = b.player(BOB);
    let free = b.planet(None);
    let mut w = b.build();
    for p in [alice, bob] {
        w.solve_planet(p, &v0_rules()).unwrap();
    }
    (w, alice, bob, free)
}

fn buildings_of(w: &World, planet: PlanetId, kind: BuildingKind) -> Vec<BuildingId> {
    w.planets[&planet]
        .buildings
        .iter()
        .copied()
        .filter(|id| w.buildings[id].kind == kind)
        .collect()
}

/// Alice's home with a Shipyard and `active` active and `waiting` waiting Cargo Vessel builds.
fn shipyard(active: u32, waiting: u32) -> (World, PlanetId, BuildingId, Vec<ShipBuildId>) {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    b.player(BOB);
    let yard = b.building(home, BuildingKind::Shipyard, BuildingState::Operational);
    let mut builds = Vec::new();
    for k in 0..active + waiting {
        let status = if k < active {
            ShipBuildStatus::Active
        } else {
            ShipBuildStatus::Waiting
        };
        builds.push(b.ship_build(yard, ShipKind::CargoVessel, status));
    }
    let mut w = b.build();
    w.solve_planet(home, &v0_rules()).unwrap();
    (w, home, yard, builds)
}

// --- Join game ---

#[test]
fn join_takes_the_first_free_candidate_and_sets_up_the_homeworld() {
    let mut b = WorldBuilder::new().at(H);
    let first = b.planet(None);
    let second = b.planet(None);
    b.candidates(&[first, second]);
    let mut w = b.build();
    let next_id = w.next_id;

    assert_eq!(accept(&mut w, ALICE, Action::JoinGame, 2 * H), []);
    let rules = v0_rules();
    let p = &w.planets[&first];
    assert_eq!(p.owner, Some(ALICE));
    assert_eq!(w.players[&ALICE].homeworld, first);
    // Drills first, then Refineries, on fresh IDs, all operational.
    let kinds: Vec<_> = p
        .buildings
        .iter()
        .map(|id| (id.0, w.buildings[id].kind, w.buildings[id].state))
        .collect();
    assert_eq!(
        kinds,
        [
            (next_id, BuildingKind::Drill, BuildingState::Operational),
            (
                next_id + 1,
                BuildingKind::Refinery,
                BuildingState::Operational
            ),
        ]
    );
    assert_eq!(
        (p.ore.value_g(), p.ingots.value_g(), p.ore.remainder()),
        (rules.homeworld.ore_g, rules.homeworld.ingots_g, 0)
    );
    // Re-solved: the Drill and Refinery run, and the planet has its next event.
    let flows = planet_flows(&w, first, &rules).unwrap();
    assert_eq!((flows.drill_actual, flows.refinery_intake), (10 * T, 8 * T));
    assert!(p.next_event.is_some_and(|e| e.at_ms > 2 * H));
    assert_eq!(w.planets[&second].owner, None);
    w.check_links().unwrap();
}

#[test]
fn join_skips_a_colonized_candidate_and_the_next_player_takes_the_next_one() {
    let mut b = WorldBuilder::new();
    let colonized = b.player(BOB); // owned, so not free
    let free = b.planet(None);
    let last = b.planet(None);
    b.candidates(&[colonized, free, last]);
    let mut w = b.build();
    accept(&mut w, ALICE, Action::JoinGame, 0);
    assert_eq!(w.players[&ALICE].homeworld, free);
    accept(&mut w, CAROL, Action::JoinGame, 0);
    assert_eq!(w.players[&CAROL].homeworld, last);
}

#[test]
fn join_takes_a_candidate_where_other_players_ships_and_fleets_wait() {
    // The advisor's case: Bob's ships reached the unowned candidate by Move. Taking the planet
    // leaves them where they are; there is no combat in the MVP.
    let mut b = WorldBuilder::new();
    let bob_home = b.player(BOB);
    let candidate = b.planet(None);
    let parked = b.ship(BOB, ShipKind::CargoVessel, candidate);
    let mover = b.ship(BOB, ShipKind::ColonyShip, bob_home);
    let fleet = b.fleet(&[mover], Cargo::default());
    b.candidates(&[candidate]);
    let mut w = b.build();
    w.send_fleet(fleet, candidate, Mission::Move, &v0_rules())
        .unwrap();
    let FleetState::InTransit { arrives_at_ms, .. } = w.fleets[&fleet].state else {
        unreachable!()
    };
    w.advance_to(arrives_at_ms, &v0_rules()).unwrap();

    accept(&mut w, ALICE, Action::JoinGame, arrives_at_ms);
    assert_eq!(w.planets[&candidate].owner, Some(ALICE));
    assert!(w.planets[&candidate].roster.contains(&parked));
    assert_eq!(w.ships[&parked].owner, BOB);
    assert_eq!(
        w.fleets[&fleet].state,
        FleetState::Stationed { at: candidate }
    );
    w.check_links().unwrap();
}

#[test]
fn join_rejects_a_player_who_has_already_joined() {
    let mut b = WorldBuilder::new();
    b.player(ALICE);
    let free = b.planet(None);
    b.candidates(&[free]);
    let mut w = b.build();
    reject(&mut w, ALICE, Action::JoinGame, H, Rejection::AlreadyJoined);
}

#[test]
fn join_rejects_when_no_candidate_is_free() {
    // None at all, then every one owned.
    let mut w = WorldBuilder::new().build();
    reject(
        &mut w,
        ALICE,
        Action::JoinGame,
        0,
        Rejection::NoFreeHomeworld,
    );
    let mut b = WorldBuilder::new();
    let only = b.planet(None);
    b.candidates(&[only]);
    let mut w = b.build();
    accept(&mut w, ALICE, Action::JoinGame, 0);
    reject(&mut w, BOB, Action::JoinGame, H, Rejection::NoFreeHomeworld);
}

// --- Build building ---

#[test]
fn build_starts_a_construction_on_the_rules_terms_and_it_draws_ingots() {
    let (mut w, alice, _, _) = two_players();
    let id = w.next_id;
    let action = Action::BuildBuilding {
        planet: alice,
        kind: BuildingKind::Shipyard,
    };
    assert_eq!(accept(&mut w, ALICE, action, H), []);
    let b = &w.buildings[&BuildingId(id)];
    let rules = v0_rules();
    let cost = rules.buildings.shipyard.build;
    let BuildingState::UnderConstruction {
        delivered,
        cost_g,
        rated_g_per_h,
    } = b.state
    else {
        panic!("{b:?}")
    };
    assert_eq!((b.kind, b.planet), (BuildingKind::Shipyard, alice));
    assert_eq!((cost_g, rated_g_per_h), (cost.cost_g, 100 * T));
    assert_eq!((delivered.value_g(), delivered.checkpoint_ms()), (0, H));
    // The re-solve feeds it from the 1,500 t of ingots at its rated 100 t/h.
    assert_eq!(delivered.rate_g_per_h(), 100 * T);
    w.check_links().unwrap();
}

#[test]
fn build_fills_the_last_slot_and_then_finds_none_counting_demolitions() {
    // 2 homeworld buildings + 1 being demolished + 4 more = 7 of 8; the 8th is accepted, the
    // 9th is no_free_slot.
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    b.demolition(home, BuildingKind::Drill);
    for _ in 0..4 {
        b.construction(home, BuildingKind::Drill, 0);
    }
    let mut w = b.build();
    w.solve_planet(home, &v0_rules()).unwrap();
    let build = Action::BuildBuilding {
        planet: home,
        kind: BuildingKind::Refinery,
    };
    assert_eq!(w.planets[&home].buildings.len(), 7);
    accept(&mut w, ALICE, build, 0);
    reject(&mut w, ALICE, build, 0, Rejection::NoFreeSlot);
}

#[test]
fn build_rejects_an_unknown_planet() {
    let (mut w, _, _, _) = two_players();
    let action = Action::BuildBuilding {
        planet: PlanetId(w.next_id + 5),
        kind: BuildingKind::Drill,
    };
    reject(&mut w, ALICE, action, 0, Rejection::UnknownPlanet);
}

#[test]
fn build_rejects_a_planet_that_is_not_the_players() {
    let (mut w, _, bob, free) = two_players();
    for planet in [bob, free] {
        let action = Action::BuildBuilding {
            planet,
            kind: BuildingKind::Drill,
        };
        reject(&mut w, ALICE, action, 0, Rejection::NotOwner);
    }
}

// --- Cancel construction ---

#[test]
fn cancel_construction_removes_it_loses_its_ingots_and_frees_the_slot_at_once() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    for _ in 0..5 {
        b.building(home, BuildingKind::Drill, BuildingState::Operational);
    }
    let site = b.construction(home, BuildingKind::Refinery, 0);
    let mut w = b.build();
    w.solve_planet(home, &v0_rules()).unwrap();
    assert_eq!(w.planets[&home].buildings.len(), 8);
    let ingots_before = w.planets[&home].ingots.value_at(2 * H).unwrap();
    accept(
        &mut w,
        ALICE,
        Action::CancelConstruction { building: site },
        2 * H,
    );
    assert!(!w.buildings.contains_key(&site));
    assert!(!w.planets[&home].buildings.contains(&site));
    // Two hours of delivery (125 t) are gone: nothing comes back to storage.
    assert_eq!(w.planets[&home].ingots.value_g(), ingots_before);
    let build = Action::BuildBuilding {
        planet: home,
        kind: BuildingKind::Shipyard,
    };
    accept(&mut w, ALICE, build, 2 * H); // the slot is free at the same instant
}

#[test]
fn cancel_construction_rejects_an_unknown_or_already_cancelled_building() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let site = b.construction(home, BuildingKind::Drill, 0);
    let mut w = b.build();
    let cancel = Action::CancelConstruction { building: site };
    accept(&mut w, ALICE, cancel, 0);
    reject(&mut w, ALICE, cancel, 0, Rejection::UnknownBuilding);
}

#[test]
fn cancel_construction_rejects_a_building_on_another_players_planet() {
    let mut b = WorldBuilder::new();
    b.player(ALICE);
    let bob = b.player(BOB);
    let site = b.construction(bob, BuildingKind::Drill, 0);
    let mut w = b.build();
    let cancel = Action::CancelConstruction { building: site };
    reject(&mut w, ALICE, cancel, 0, Rejection::NotOwner);
}

#[test]
fn cancel_construction_rejects_an_operational_or_demolishing_building() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let going = b.demolition(home, BuildingKind::Refinery);
    let mut w = b.build();
    let drill = buildings_of(&w, home, BuildingKind::Drill)[0];
    for building in [drill, going] {
        let cancel = Action::CancelConstruction { building };
        reject(&mut w, ALICE, cancel, 0, Rejection::NotUnderConstruction);
    }
}

// --- Demolish building ---

#[test]
fn demolish_stops_the_building_now_and_frees_the_slot_when_it_ends() {
    let (mut w, alice, _, _) = two_players();
    let drill = buildings_of(&w, alice, BuildingKind::Drill)[0];
    accept(
        &mut w,
        ALICE,
        Action::DemolishBuilding { building: drill },
        H,
    );
    let rules = v0_rules();
    assert_eq!(
        w.buildings[&drill].state,
        BuildingState::Demolishing {
            ends_at_ms: H + rules.buildings.drill.demolition_time_ms
        }
    );
    // It stops working at once: no Drill output.
    assert_eq!(planet_flows(&w, alice, &rules).unwrap().drill_output, 0);
    assert_eq!(w.planets[&alice].buildings.len(), 2, "the slot stays taken");
    w.advance_to(2 * H, &rules).unwrap();
    assert!(!w.buildings.contains_key(&drill));
    assert_eq!(w.planets[&alice].buildings.len(), 1);
}

#[test]
fn demolish_accepts_a_shipyard_with_no_builds() {
    let (mut w, _, yard, _) = shipyard(0, 0);
    accept(
        &mut w,
        ALICE,
        Action::DemolishBuilding { building: yard },
        0,
    );
    assert!(matches!(
        w.buildings[&yard].state,
        BuildingState::Demolishing { .. }
    ));
}

#[test]
fn demolish_rejects_a_shipyard_with_ships_building_or_queued() {
    for (active, waiting) in [(1, 0), (3, 2)] {
        let (mut w, _, yard, _) = shipyard(active, waiting);
        let demolish = Action::DemolishBuilding { building: yard };
        reject(&mut w, ALICE, demolish, 0, Rejection::ShipyardHasBuilds);
    }
    // Only a waiting build, so a check that counts active builds alone would let it through.
    let (mut w, _, yard, _) = shipyard(0, 1);
    let demolish = Action::DemolishBuilding { building: yard };
    reject(&mut w, ALICE, demolish, 0, Rejection::ShipyardHasBuilds);
}

#[test]
fn demolish_rejects_an_unknown_building_one_not_the_players_and_one_not_operational() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let bob = b.player(BOB);
    let site = b.construction(home, BuildingKind::Drill, 0);
    let going = b.demolition(home, BuildingKind::Drill);
    let mut w = b.build();
    let bobs = buildings_of(&w, bob, BuildingKind::Drill)[0];
    let demolish = |building| Action::DemolishBuilding { building };
    let unknown = BuildingId(w.next_id);
    reject(
        &mut w,
        ALICE,
        demolish(unknown),
        0,
        Rejection::UnknownBuilding,
    );
    reject(&mut w, ALICE, demolish(bobs), 0, Rejection::NotOwner);
    reject(&mut w, ALICE, demolish(site), 0, Rejection::NotOperational);
    reject(&mut w, ALICE, demolish(going), 0, Rejection::NotOperational);
}

// --- Queue ship ---

#[test]
fn queue_starts_at_once_below_the_parallel_limit_and_waits_at_it() {
    let (mut w, _, yard, _) = shipyard(2, 0);
    let queue = |kind| Action::QueueShip {
        shipyard: yard,
        kind,
    };
    let third = ShipBuildId(w.next_id);
    accept(&mut w, ALICE, queue(ShipKind::ColonyShip), H);
    let fourth = ShipBuildId(w.next_id);
    accept(&mut w, ALICE, queue(ShipKind::CargoVessel), H);
    let rules = v0_rules();
    let b = &w.ship_builds[&third];
    let colony = rules.ships.colony_ship.build;
    assert_eq!(
        (b.status, b.kind, b.cost_g, b.rated_g_per_h),
        (
            ShipBuildStatus::Active,
            ShipKind::ColonyShip,
            colony.cost_g,
            80 * T
        )
    );
    assert_eq!((b.delivered.value_g(), b.delivered.checkpoint_ms()), (0, H));
    assert_eq!(w.ship_builds[&fourth].status, ShipBuildStatus::Waiting);
    w.check_links().unwrap();
}

#[test]
fn queue_rejects_an_unknown_building_and_one_not_the_players() {
    let (mut w, _, _, _) = shipyard(0, 0);
    let mut b = WorldBuilder::new();
    b.player(ALICE);
    let bob = b.player(BOB);
    let bob_yard = b.building(bob, BuildingKind::Shipyard, BuildingState::Operational);
    let mut other = b.build();
    let queue = |shipyard| Action::QueueShip {
        shipyard,
        kind: ShipKind::CargoVessel,
    };
    let unknown = BuildingId(w.next_id);
    reject(&mut w, ALICE, queue(unknown), 0, Rejection::UnknownBuilding);
    reject(&mut other, ALICE, queue(bob_yard), 0, Rejection::NotOwner);
}

#[test]
fn queue_rejects_a_building_that_is_not_a_shipyard() {
    let (mut w, alice, _, _) = two_players();
    let drill = buildings_of(&w, alice, BuildingKind::Drill)[0];
    let queue = Action::QueueShip {
        shipyard: drill,
        kind: ShipKind::ColonyShip,
    };
    reject(&mut w, ALICE, queue, 0, Rejection::NotAShipyard);
}

#[test]
fn queue_rejects_a_shipyard_under_construction_or_being_demolished() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let site = b.construction(home, BuildingKind::Shipyard, 0);
    let going = b.demolition(home, BuildingKind::Shipyard);
    let mut w = b.build();
    for shipyard in [site, going] {
        let queue = Action::QueueShip {
            shipyard,
            kind: ShipKind::CargoVessel,
        };
        reject(&mut w, ALICE, queue, 0, Rejection::ShipyardNotOperational);
    }
}

// --- Cancel ship build ---

#[test]
fn cancel_ship_build_of_an_active_build_starts_the_next_waiting_one_with_no_refund() {
    let (mut w, home, _, builds) = shipyard(3, 2);
    let ingots = w.planets[&home].ingots.value_at(H).unwrap();
    accept(
        &mut w,
        ALICE,
        Action::CancelShipBuild { build: builds[1] },
        H,
    );
    assert!(!w.ship_builds.contains_key(&builds[1]));
    let status = |i: usize| w.ship_builds[&builds[i]].status;
    assert_eq!(
        (status(0), status(2), status(3), status(4)),
        (
            ShipBuildStatus::Active,
            ShipBuildStatus::Active,
            ShipBuildStatus::Active,
            ShipBuildStatus::Waiting
        )
    );
    let started = &w.ship_builds[&builds[3]];
    assert_eq!(
        (
            started.delivered.value_g(),
            started.delivered.checkpoint_ms()
        ),
        (0, H)
    );
    assert_eq!(started.delivered.rate_g_per_h(), started.rated_g_per_h);
    assert_eq!(w.planets[&home].ingots.value_g(), ingots, "no refund");
}

#[test]
fn cancel_ship_build_of_a_waiting_build_starts_nothing() {
    let (mut w, _, _, builds) = shipyard(3, 2);
    accept(
        &mut w,
        ALICE,
        Action::CancelShipBuild { build: builds[3] },
        H,
    );
    assert!(!w.ship_builds.contains_key(&builds[3]));
    assert_eq!(w.ship_builds[&builds[4]].status, ShipBuildStatus::Waiting);
    let active = w
        .ship_builds
        .values()
        .filter(|b| b.status == ShipBuildStatus::Active)
        .count();
    assert_eq!(active, 3);
}

#[test]
fn cancel_ship_build_rejects_an_unknown_or_already_cancelled_build_and_one_not_the_players() {
    let (mut w, _, _, builds) = shipyard(1, 0);
    let cancel = Action::CancelShipBuild { build: builds[0] };
    reject(&mut w, BOB, cancel, 0, Rejection::NotOwner);
    accept(&mut w, ALICE, cancel, 0);
    reject(&mut w, ALICE, cancel, 0, Rejection::UnknownShipBuild);
}

// --- What holds for every command ---

#[test]
fn apply_rejects_every_command_from_a_player_who_has_not_joined() {
    let (mut w, home, yard, builds) = shipyard(1, 0);
    let drill = buildings_of(&w, home, BuildingKind::Drill)[0];
    let actions = [
        Action::BuildBuilding {
            planet: home,
            kind: BuildingKind::Drill,
        },
        Action::CancelConstruction { building: drill },
        Action::DemolishBuilding { building: drill },
        Action::QueueShip {
            shipyard: yard,
            kind: ShipKind::CargoVessel,
        },
        Action::CancelShipBuild { build: builds[0] },
    ];
    for action in actions {
        reject(&mut w, CAROL, action, 0, Rejection::NotJoined);
    }
}

#[test]
fn apply_checks_who_then_what_then_whose_then_state() {
    // Bob's full planet: not_owner before no_free_slot; an unknown planet before ownership; a
    // stranger before anything.
    let mut b = WorldBuilder::new();
    b.player(ALICE);
    let bob = b.player(BOB);
    for _ in 0..6 {
        b.construction(bob, BuildingKind::Drill, 0);
    }
    let mut w = b.build();
    let build = |planet| Action::BuildBuilding {
        planet,
        kind: BuildingKind::Drill,
    };
    reject(&mut w, ALICE, build(bob), 0, Rejection::NotOwner);
    reject(&mut w, BOB, build(bob), 0, Rejection::NoFreeSlot);
    reject(&mut w, CAROL, build(PlanetId(999)), 0, Rejection::NotJoined);
    reject(
        &mut w,
        ALICE,
        build(PlanetId(999)),
        0,
        Rejection::UnknownPlanet,
    );
    // A Drill of Bob's under construction: not_owner before not_a_shipyard.
    let site = w.planets[&bob].buildings.iter().copied().last().unwrap();
    let queue = Action::QueueShip {
        shipyard: site,
        kind: ShipKind::CargoVessel,
    };
    reject(&mut w, ALICE, queue, 0, Rejection::NotOwner);
    reject(&mut w, BOB, queue, 0, Rejection::NotAShipyard);
}

#[test]
fn apply_runs_events_at_t_before_the_command() {
    // A Drill completes at exactly 3 h; demolishing it at 3 h is accepted because the
    // completion ran first. Before 3 h it is not operational.
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let site = b.construction(home, BuildingKind::Drill, 0);
    let mut w = b.build();
    w.solve_planet(home, &v0_rules()).unwrap();
    let demolish = Action::DemolishBuilding { building: site };
    reject(
        &mut w,
        ALICE,
        demolish,
        3 * H - 1,
        Rejection::NotOperational,
    );
    let applied = w
        .apply(&command(ALICE, demolish), 3 * H, &v0_rules())
        .unwrap();
    assert_eq!(
        applied.advanced,
        [DomainEvent::BuildingCompleted {
            planet: home,
            building: site,
            kind: BuildingKind::Drill
        }]
    );
    assert_eq!(applied.outcome, Ok(vec![]));
}

#[test]
fn apply_returns_a_rejected_commands_advance_events_too() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let site = b.construction(home, BuildingKind::Drill, 0);
    let mut w = b.build();
    w.solve_planet(home, &v0_rules()).unwrap();
    // reject() checks the advance's events are returned with the rejection.
    reject(
        &mut w,
        ALICE,
        Action::JoinGame,
        3 * H,
        Rejection::AlreadyJoined,
    );
    assert_eq!(w.buildings[&site].state, BuildingState::Operational);
}

#[test]
fn apply_going_back_in_time_changes_nothing_and_does_not_poison() {
    let (mut w, alice, _, _) = two_players();
    w.advance_to(H, &v0_rules()).unwrap();
    let before = w.clone();
    let build = Action::BuildBuilding {
        planet: alice,
        kind: BuildingKind::Drill,
    };
    assert_eq!(
        w.apply(&command(ALICE, build), H - 1, &v0_rules()),
        Err(EngineError::TimeBeforeNow { t: H - 1, now: H })
    );
    assert_eq!(w, before);
    assert!(!w.is_poisoned());
}

#[test]
fn apply_an_effect_out_of_range_is_an_error_that_poisons() {
    // A demolition ending after the end of time: valid rules, extreme instant.
    let mut b = WorldBuilder::new().at(i64::MAX - 1);
    let home = b.player(ALICE);
    let mut w = b.build();
    let drill = buildings_of(&w, home, BuildingKind::Drill)[0];
    let demolish = Action::DemolishBuilding { building: drill };
    assert_eq!(
        w.apply(&command(ALICE, demolish), i64::MAX - 1, &v0_rules()),
        Err(EngineError::CommandOutOfRange {
            what: "demolition end"
        })
    );
    assert!(w.is_poisoned());
}

#[test]
fn apply_reason_codes_are_their_serialized_names() {
    // The codes are the API contract (D-028): the code and the serialized form never drift.
    let all = [
        Rejection::AlreadyJoined,
        Rejection::NoFreeHomeworld,
        Rejection::NotJoined,
        Rejection::UnknownPlanet,
        Rejection::UnknownBuilding,
        Rejection::UnknownShipBuild,
        Rejection::NotOwner,
        Rejection::NoFreeSlot,
        Rejection::NotUnderConstruction,
        Rejection::NotOperational,
        Rejection::ShipyardHasBuilds,
        Rejection::NotAShipyard,
        Rejection::ShipyardNotOperational,
    ];
    for r in all {
        assert_eq!(
            serde_json::to_value(r).unwrap(),
            serde_json::Value::String(r.code().into())
        );
    }
    let codes: std::collections::BTreeSet<_> = all.iter().map(|r| r.code()).collect();
    assert_eq!(codes.len(), all.len());
}

#[test]
fn apply_commands_round_trip_in_their_snake_case_form() {
    let c = command(
        ALICE,
        Action::BuildBuilding {
            planet: PlanetId(1042),
            kind: BuildingKind::Refinery,
        },
    );
    // A string, not a serde_json Value: the player ID is 128 bits (the API's UUID text form is
    // Phase 3's).
    let json = serde_json::to_string(&c).unwrap();
    assert!(
        json.ends_with(r#""action":{"build_building":{"planet":1042,"kind":"Refinery"}}}"#),
        "{json}"
    );
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), c);
    let join = serde_json::to_string(&command(ALICE, Action::JoinGame)).unwrap();
    assert!(join.ends_with(r#""action":"join_game"}"#), "{join}");
    assert_eq!(
        canonical_bytes(&serde_json::from_str::<Command>(&join).unwrap()),
        canonical_bytes(&command(ALICE, Action::JoinGame))
    );
}
