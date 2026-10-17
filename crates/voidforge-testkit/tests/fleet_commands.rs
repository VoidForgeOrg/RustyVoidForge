//! Commands, part 2: the fleet commands' named tests (Technical Plan P1.10: "Every accept and
//! reject rule for these five commands has a test"; Architectural Design, Commands and Fleets;
//! Game Design, Player actions and Ships and fleets; decision log D-029), on Rules v0 worlds.
//! Names start with the command they cover: `assemble_*`, `send_*`, `recall_*`, `disband_*`,
//! `unload_*`, then `fleet_commands_*` for the check order. The mechanics behind send, recall
//! and unload are P1.8's, tested in `tests/fleets.rs`.

use std::collections::BTreeSet;

use voidforge_core::command::{Action, Applied, Command, Rejection};
use voidforge_core::quantity::{MS_PER_HOUR, Quantity};
use voidforge_core::rules::GRAMS_PER_TON;
use voidforge_core::state::{
    Cargo, DomainEvent, FleetId, FleetState, Mission, PlanetId, PlayerId, Position, ShipId,
    ShipKind, ShipLocation, World,
};
use voidforge_testkit::determinism::{CAROL, canonical_bytes};
use voidforge_testkit::{ALICE, BOB, WorldBuilder, v0_rules};

const T: i64 = GRAMS_PER_TON;
const H: i64 = MS_PER_HOUR;

fn apply(w: &mut World, player: PlayerId, action: &Action, t: i64) -> Applied {
    let command = Command {
        player,
        action: action.clone(),
    };
    w.apply(&command, t, &v0_rules()).unwrap()
}

fn accept(w: &mut World, player: PlayerId, action: &Action, t: i64) -> Vec<DomainEvent> {
    let applied = apply(w, player, action, t);
    applied
        .outcome
        .unwrap_or_else(|r| panic!("{action:?} by {player:?} was rejected: {}", r.code()))
}

/// The command is rejected with `reason`, and the world equals one that only advanced to `t`.
fn reject(w: &mut World, player: PlayerId, action: &Action, t: i64, reason: Rejection) {
    let mut advanced = w.clone();
    let events = advanced.advance_to(t, &v0_rules()).unwrap();
    let applied = apply(w, player, action, t);
    assert_eq!(
        applied,
        Applied {
            advanced: events,
            outcome: Err(reason)
        },
        "{action:?} by {player:?}"
    );
    assert_eq!(canonical_bytes(&*w), canonical_bytes(&advanced));
}

fn tons(ore: i64, ingots: i64) -> Cargo {
    Cargo {
        ore_g: ore * T,
        ingots_g: ingots * T,
    }
}

fn assemble(planet: PlanetId, ships: &[ShipId], cargo: Cargo) -> Action {
    Action::AssembleFleet {
        planet,
        ships: ships.iter().copied().collect(),
        cargo,
    }
}

fn send(fleet: FleetId, target: PlanetId, mission: Mission) -> Action {
    Action::SendFleet {
        fleet,
        target,
        mission,
    }
}

/// Alice's homeworld (starting stocks: 2,000 t ore, 1,500 t ingots) with a Colony Ship and two
/// Cargo Vessels on its roster, solved at 0; Bob's homeworld with a Cargo Vessel; an unowned
/// planet. Planets sit one world unit apart along x (testkit's default).
struct Setup {
    w: World,
    home: PlanetId,
    bob_home: PlanetId,
    free: PlanetId,
    colony_ship: ShipId,
    haulers: [ShipId; 2],
    bob_ship: ShipId,
}

fn setup() -> Setup {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let bob_home = b.player(BOB);
    let free = b.planet(None);
    let colony_ship = b.ship(ALICE, ShipKind::ColonyShip, home);
    let haulers = [
        b.ship(ALICE, ShipKind::CargoVessel, home),
        b.ship(ALICE, ShipKind::CargoVessel, home),
    ];
    let bob_ship = b.ship(BOB, ShipKind::CargoVessel, bob_home);
    let mut w = b.build();
    for p in [home, bob_home] {
        w.solve_planet(p, &v0_rules()).unwrap();
    }
    Setup {
        w,
        home,
        bob_home,
        free,
        colony_ship,
        haulers,
        bob_ship,
    }
}

/// Assembles Alice's `ships` at `home` with `cargo` at time 0 and returns the fleet.
fn fleet_of(w: &mut World, home: PlanetId, ships: &[ShipId], cargo: Cargo) -> FleetId {
    let id = FleetId(w.next_id);
    accept(w, ALICE, &assemble(home, ships, cargo), 0);
    id
}

fn arrival(w: &World, fleet: FleetId) -> i64 {
    match w.fleets[&fleet].state {
        FleetState::InTransit { arrives_at_ms, .. } => arrives_at_ms,
        other => panic!("fleet {} is {other:?}", fleet.0),
    }
}

// --- Assemble fleet ---

#[test]
fn assemble_without_cargo_takes_ships_off_the_roster_into_a_stationed_fleet() {
    let mut s = setup();
    let version = s.w.planets[&s.home].version;
    let fleet = FleetId(s.w.next_id);
    let ships = [s.colony_ship, s.haulers[0]];
    assert_eq!(
        accept(
            &mut s.w,
            ALICE,
            &assemble(s.home, &ships, Cargo::default()),
            H
        ),
        []
    );
    let f = &s.w.fleets[&fleet];
    assert_eq!(
        (f.owner, f.state, f.cargo, f.version),
        (
            ALICE,
            FleetState::Stationed { at: s.home },
            Cargo::default(),
            0
        )
    );
    assert_eq!(f.ships, ships.into_iter().collect::<BTreeSet<_>>());
    for ship in ships {
        assert_eq!(s.w.ships[&ship].location, ShipLocation::Fleet(fleet));
        assert!(!s.w.planets[&s.home].roster.contains(&ship));
    }
    assert!(s.w.planets[&s.home].roster.contains(&s.haulers[1]));
    // No storage changed, so the planet was not re-solved.
    assert_eq!(s.w.planets[&s.home].version, version);
    s.w.check_links().unwrap();
}

#[test]
fn assemble_with_cargo_takes_it_out_of_storage_exactly_and_re_solves() {
    let mut s = setup();
    // A remainder on the ingot stock, to show it is kept.
    let p = s.w.planets.get_mut(&s.home).unwrap();
    p.ingots = Quantity::from_parts(p.ingots.value_g(), 0, 777, 0).unwrap();
    let version = s.w.planets[&s.home].version;
    let fleet = fleet_of(
        &mut s.w,
        s.home,
        &[s.haulers[0], s.haulers[1]],
        tons(300, 700),
    );
    let p = &s.w.planets[&s.home];
    assert_eq!(
        (p.ore.value_g(), p.ingots.value_g(), p.ingots.remainder()),
        (1_700 * T, 800 * T, 777)
    );
    assert_eq!(s.w.fleets[&fleet].cargo, tons(300, 700));
    assert!(
        p.version > version,
        "storage changed, so the planet was re-solved"
    );
    s.w.check_links().unwrap();
}

#[test]
fn assemble_fills_capacity_exactly_and_refuses_a_gram_more() {
    // Two Cargo Vessels hold 1,000 t; the Colony Ship adds nothing (D-027).
    let mut s = setup();
    let ships = [s.colony_ship, s.haulers[0], s.haulers[1]];
    let over = Cargo {
        ore_g: 400 * T,
        ingots_g: 600 * T + 1,
    };
    reject(
        &mut s.w,
        ALICE,
        &assemble(s.home, &ships, over),
        0,
        Rejection::InsufficientCapacity,
    );
    accept(
        &mut s.w,
        ALICE,
        &assemble(s.home, &ships, tons(400, 600)),
        0,
    );
    // A lone Colony Ship carries nothing at all.
    let mut s = setup();
    let one_gram = Cargo {
        ore_g: 1,
        ingots_g: 0,
    };
    let colony = assemble(s.home, &[s.colony_ship], one_gram);
    reject(&mut s.w, ALICE, &colony, 0, Rejection::InsufficientCapacity);
}

#[test]
fn assemble_refuses_cargo_beyond_either_stock() {
    let mut s = setup();
    let ore = Cargo {
        ore_g: 2_000 * T + 1,
        ingots_g: 0,
    };
    let ingots = Cargo {
        ore_g: 0,
        ingots_g: 1_500 * T + 1,
    };
    let ships = [s.haulers[0], s.haulers[1]];
    for cargo in [ore, ingots] {
        let action = assemble(s.home, &ships, cargo);
        reject(&mut s.w, ALICE, &action, 0, Rejection::InsufficientStock);
    }
    // The stock is read at t: an hour of Refinery output later, more ingots can be loaded.
    let more = tons(0, 500);
    let at_home = assemble(s.home, &[s.haulers[0]], more);
    accept(&mut s.w, ALICE, &at_home, H);
}

#[test]
fn assemble_on_an_unowned_planet_needs_no_ownership_without_cargo() {
    let mut b = WorldBuilder::new();
    b.player(ALICE);
    let free = b.planet(None);
    let ship = b.ship(ALICE, ShipKind::CargoVessel, free);
    let mut w = b.build();
    accept(&mut w, ALICE, &assemble(free, &[ship], Cargo::default()), 0);
    // With cargo, the planet must be the player's.
    let mut b = WorldBuilder::new();
    b.player(ALICE);
    let free = b.planet(None);
    let ship = b.ship(ALICE, ShipKind::CargoVessel, free);
    b.stocks(free, Quantity::new(100 * T, 0, 0), Quantity::new(0, 0, 0));
    let mut w = b.build();
    let action = assemble(free, &[ship], tons(10, 0));
    reject(&mut w, ALICE, &action, 0, Rejection::NotOwner);
}

#[test]
fn assemble_rejects_unknown_planets_and_ships_no_ships_and_negative_cargo() {
    let mut s = setup();
    let ghost = s.w.next_id + 7;
    let ship = [s.haulers[0]];
    let cases = [
        (
            assemble(PlanetId(ghost), &ship, Cargo::default()),
            Rejection::UnknownPlanet,
        ),
        (
            assemble(s.home, &[ShipId(ghost)], Cargo::default()),
            Rejection::UnknownShip,
        ),
        (assemble(s.home, &[], Cargo::default()), Rejection::NoShips),
        (
            assemble(
                s.home,
                &ship,
                Cargo {
                    ore_g: -1,
                    ingots_g: 0,
                },
            ),
            Rejection::InvalidCargo,
        ),
    ];
    for (action, reason) in cases {
        reject(&mut s.w, ALICE, &action, 0, reason);
    }
}

#[test]
fn assemble_rejects_another_players_ship_and_ships_not_on_that_roster() {
    let mut s = setup();
    // Bob's ship (on his roster) is not Alice's.
    let action = assemble(s.bob_home, &[s.bob_ship], Cargo::default());
    reject(&mut s.w, ALICE, &action, 0, Rejection::NotOwner);
    // Alice's ship named at the wrong planet, and one already in a fleet.
    let wrong = assemble(s.free, &[s.haulers[0]], Cargo::default());
    reject(&mut s.w, ALICE, &wrong, 0, Rejection::ShipNotOnRoster);
    fleet_of(&mut s.w, s.home, &[s.haulers[0]], Cargo::default());
    let again = assemble(s.home, &[s.haulers[0], s.haulers[1]], Cargo::default());
    reject(&mut s.w, ALICE, &again, 0, Rejection::ShipNotOnRoster);
}

// --- Send fleet ---

#[test]
fn send_every_mission_departs_with_its_arrival_fixed() {
    // Colonize the unowned planet, and Move to another player's planet (allowed).
    let mut s = setup();
    let colonizers = fleet_of(&mut s.w, s.home, &[s.colony_ship], Cargo::default());
    let movers = fleet_of(&mut s.w, s.home, &[s.haulers[1]], Cargo::default());
    accept(
        &mut s.w,
        ALICE,
        &send(colonizers, s.free, Mission::Colonize),
        H,
    );
    accept(&mut s.w, ALICE, &send(movers, s.bob_home, Mission::Move), H);
    // The colonizers fly at a Colony Ship's 10 units per hour, so 6 minutes per world unit,
    // from their departure at 1 h (testkit puts planet n at x = n units).
    let units = (s.w.planets[&s.free].position.x - s.w.planets[&s.home].position.x) / 1_000;
    assert_eq!(arrival(&s.w, colonizers), H + units * 6 * 60_000);
    assert!(matches!(
        s.w.fleets[&movers].state,
        FleetState::InTransit {
            mission: Mission::Move,
            ..
        }
    ));
    // Transport to another planet of the player's own.
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let own = b.planet(Some(ALICE));
    let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
    let mut w = b.build();
    let fleet = fleet_of(&mut w, home, &[hauler], tons(5, 5));
    accept(&mut w, ALICE, &send(fleet, own, Mission::Transport), 0);
    // A Cargo Vessel's 15 units per hour: 4 minutes per world unit.
    let units = (w.planets[&own].position.x - w.planets[&home].position.x) / 1_000;
    assert_eq!(
        w.fleets[&fleet].state,
        FleetState::InTransit {
            origin: home,
            target: own,
            mission: Mission::Transport,
            departed_at_ms: 0,
            arrives_at_ms: units * 4 * 60_000,
        }
    );
}

#[test]
fn send_rejects_an_unknown_fleet_or_target_another_players_fleet_and_one_not_stationed() {
    let mut s = setup();
    let fleet = fleet_of(&mut s.w, s.home, &[s.haulers[0]], Cargo::default());
    let ghost = s.w.next_id + 3;
    reject(
        &mut s.w,
        ALICE,
        &send(FleetId(ghost), s.free, Mission::Move),
        0,
        Rejection::UnknownFleet,
    );
    reject(
        &mut s.w,
        ALICE,
        &send(fleet, PlanetId(ghost), Mission::Move),
        0,
        Rejection::UnknownPlanet,
    );
    reject(
        &mut s.w,
        BOB,
        &send(fleet, s.free, Mission::Move),
        0,
        Rejection::NotOwner,
    );
    accept(&mut s.w, ALICE, &send(fleet, s.free, Mission::Move), 0);
    reject(
        &mut s.w,
        ALICE,
        &send(fleet, s.bob_home, Mission::Move),
        1,
        Rejection::FleetNotStationed,
    );
}

#[test]
fn send_rejects_the_planet_the_fleet_is_at_and_an_unreachable_target() {
    let mut s = setup();
    let fleet = fleet_of(&mut s.w, s.home, &[s.haulers[0]], Cargo::default());
    reject(
        &mut s.w,
        ALICE,
        &send(fleet, s.home, Mission::Move),
        0,
        Rejection::SamePlanet,
    );
    // Another planet at the very same coordinates: a zero-length trip (D-029).
    let at = s.w.planets[&s.home].position;
    s.w.planets.get_mut(&s.free).unwrap().position = at;
    reject(
        &mut s.w,
        ALICE,
        &send(fleet, s.free, Mission::Move),
        0,
        Rejection::UnreachableTarget,
    );
    // One whose arrival would be past the end of time.
    s.w.planets.get_mut(&s.free).unwrap().position = Position {
        x: i64::MAX / 2,
        y: i64::MAX / 2,
        z: 0,
    };
    reject(
        &mut s.w,
        ALICE,
        &send(fleet, s.free, Mission::Move),
        0,
        Rejection::UnreachableTarget,
    );
    assert!(!s.w.is_poisoned());
}

#[test]
fn send_colonize_needs_a_colony_ship_and_an_unowned_target() {
    let mut s = setup();
    let haulers = fleet_of(&mut s.w, s.home, &[s.haulers[0]], Cargo::default());
    let colonizers = fleet_of(&mut s.w, s.home, &[s.colony_ship], Cargo::default());
    reject(
        &mut s.w,
        ALICE,
        &send(haulers, s.free, Mission::Colonize),
        0,
        Rejection::NoColonyShip,
    );
    reject(
        &mut s.w,
        ALICE,
        &send(colonizers, s.bob_home, Mission::Colonize),
        0,
        Rejection::TargetOwned,
    );
}

#[test]
fn send_transport_needs_a_cargo_vessel_and_a_target_the_player_owns() {
    let mut s = setup();
    let colonizers = fleet_of(&mut s.w, s.home, &[s.colony_ship], Cargo::default());
    let haulers = fleet_of(&mut s.w, s.home, &[s.haulers[0]], Cargo::default());
    reject(
        &mut s.w,
        ALICE,
        &send(colonizers, s.free, Mission::Transport),
        0,
        Rejection::NoCargoVessel,
    );
    for target in [s.free, s.bob_home] {
        reject(
            &mut s.w,
            ALICE,
            &send(haulers, target, Mission::Transport),
            0,
            Rejection::TargetNotOwned,
        );
    }
}

// --- Recall fleet ---

#[test]
fn recall_turns_a_fleet_in_transit_around() {
    let mut s = setup();
    let fleet = fleet_of(&mut s.w, s.home, &[s.colony_ship], Cargo::default());
    accept(&mut s.w, ALICE, &send(fleet, s.free, Mission::Colonize), 0);
    accept(&mut s.w, ALICE, &Action::RecallFleet { fleet }, 5 * 60_000);
    assert_eq!(
        s.w.fleets[&fleet].state,
        FleetState::Returning {
            origin: s.home,
            arrives_at_ms: 10 * 60_000
        }
    );
}

#[test]
fn recall_at_the_departure_instant_is_back_at_once() {
    let mut s = setup();
    let fleet = fleet_of(&mut s.w, s.home, &[s.colony_ship], Cargo::default());
    accept(&mut s.w, ALICE, &send(fleet, s.free, Mission::Colonize), H);
    accept(&mut s.w, ALICE, &Action::RecallFleet { fleet }, H);
    assert_eq!(
        s.w.fleets[&fleet].state,
        FleetState::Stationed { at: s.home }
    );
}

#[test]
fn recall_rejects_unknown_foreign_stationed_arrived_and_already_recalled_fleets() {
    let mut s = setup();
    let fleet = fleet_of(&mut s.w, s.home, &[s.colony_ship], Cargo::default());
    let recall = Action::RecallFleet { fleet };
    let ghost = Action::RecallFleet {
        fleet: FleetId(s.w.next_id + 2),
    };
    reject(&mut s.w, ALICE, &ghost, 0, Rejection::UnknownFleet);
    reject(&mut s.w, ALICE, &recall, 0, Rejection::FleetNotInTransit);
    accept(&mut s.w, ALICE, &send(fleet, s.free, Mission::Colonize), 0);
    reject(&mut s.w, BOB, &recall, 1, Rejection::NotOwner);
    accept(&mut s.w, ALICE, &recall, 2 * 60_000);
    reject(
        &mut s.w,
        ALICE,
        &recall,
        3 * 60_000,
        Rejection::AlreadyRecalled,
    );
    // A recall at the arrival instant comes after the arrival (events at t first).
    let mut s = setup();
    let fleet = fleet_of(&mut s.w, s.home, &[s.haulers[0]], Cargo::default());
    accept(&mut s.w, ALICE, &send(fleet, s.free, Mission::Move), 0);
    let lands = arrival(&s.w, fleet);
    reject(
        &mut s.w,
        ALICE,
        &Action::RecallFleet { fleet },
        lands,
        Rejection::FleetNotInTransit,
    );
}

// --- Disband fleet ---

#[test]
fn disband_returns_the_ships_to_the_roster_and_removes_the_fleet_even_unowned() {
    let mut s = setup();
    let fleet = fleet_of(
        &mut s.w,
        s.home,
        &[s.colony_ship, s.haulers[0]],
        Cargo::default(),
    );
    accept(&mut s.w, ALICE, &send(fleet, s.free, Mission::Move), 0);
    let lands = arrival(&s.w, fleet);
    assert_eq!(
        accept(&mut s.w, ALICE, &Action::DisbandFleet { fleet }, lands),
        []
    );
    assert!(!s.w.fleets.contains_key(&fleet));
    for ship in [s.colony_ship, s.haulers[0]] {
        assert_eq!(s.w.ships[&ship].location, ShipLocation::Roster(s.free));
        assert!(s.w.planets[&s.free].roster.contains(&ship));
    }
    s.w.check_links().unwrap();
}

#[test]
fn disband_rejects_unknown_foreign_moving_and_loaded_fleets() {
    let mut s = setup();
    let loaded = fleet_of(&mut s.w, s.home, &[s.haulers[0]], tons(0, 10));
    let moving = fleet_of(&mut s.w, s.home, &[s.haulers[1]], Cargo::default());
    accept(&mut s.w, ALICE, &send(moving, s.free, Mission::Move), 0);
    let ghost = FleetId(s.w.next_id + 1);
    reject(
        &mut s.w,
        ALICE,
        &Action::DisbandFleet { fleet: ghost },
        0,
        Rejection::UnknownFleet,
    );
    reject(
        &mut s.w,
        BOB,
        &Action::DisbandFleet { fleet: loaded },
        0,
        Rejection::NotOwner,
    );
    reject(
        &mut s.w,
        ALICE,
        &Action::DisbandFleet { fleet: moving },
        0,
        Rejection::FleetNotStationed,
    );
    reject(
        &mut s.w,
        ALICE,
        &Action::DisbandFleet { fleet: loaded },
        0,
        Rejection::CargoAboard,
    );
}

#[test]
fn disband_after_a_partial_unload_is_refused_until_the_rest_is_unloaded() {
    // Alice's home ingot storage has 3,500 t free at 0; 1,000 t aboard fits, so fill storage
    // first to leave only 600 t free.
    let mut s = setup();
    let fleet = fleet_of(
        &mut s.w,
        s.home,
        &[s.haulers[0], s.haulers[1]],
        tons(0, 1_000),
    );
    let p = s.w.planets.get_mut(&s.home).unwrap();
    p.ingots = Quantity::new(4_400 * T, 0, 0);
    p.ore = Quantity::new(10_000 * T, 0, 0);
    s.w.solve_planet(s.home, &v0_rules()).unwrap();
    accept(&mut s.w, ALICE, &Action::TriggerUnload { fleet }, 0);
    assert_eq!(s.w.fleets[&fleet].cargo, tons(0, 400));
    reject(
        &mut s.w,
        ALICE,
        &Action::DisbandFleet { fleet },
        0,
        Rejection::CargoAboard,
    );
}

// --- Trigger unload ---

#[test]
fn unload_moves_what_fits_into_storage_and_re_solves() {
    let mut s = setup();
    let fleet = fleet_of(&mut s.w, s.home, &[s.haulers[0]], tons(100, 200));
    let version = s.w.planets[&s.home].version;
    let before = (
        s.w.planets[&s.home].ore.value_at(H).unwrap(),
        s.w.planets[&s.home].ingots.value_at(H).unwrap(),
    );
    assert_eq!(
        accept(&mut s.w, ALICE, &Action::TriggerUnload { fleet }, H),
        []
    );
    let p = &s.w.planets[&s.home];
    assert_eq!(
        (p.ore.value_g(), p.ingots.value_g()),
        (before.0 + 100 * T, before.1 + 200 * T)
    );
    assert_eq!(s.w.fleets[&fleet].cargo, Cargo::default());
    assert!(p.version > version);
    accept(&mut s.w, ALICE, &Action::DisbandFleet { fleet }, H);
}

#[test]
fn unload_with_nothing_aboard_or_no_room_is_accepted_and_moves_nothing() {
    let mut s = setup();
    let empty = fleet_of(&mut s.w, s.home, &[s.haulers[0]], Cargo::default());
    accept(&mut s.w, ALICE, &Action::TriggerUnload { fleet: empty }, 0);
    let full = fleet_of(&mut s.w, s.home, &[s.haulers[1]], tons(0, 50));
    let p = s.w.planets.get_mut(&s.home).unwrap();
    p.ingots = Quantity::new(5_000 * T, 0, 0);
    s.w.solve_planet(s.home, &v0_rules()).unwrap();
    accept(&mut s.w, ALICE, &Action::TriggerUnload { fleet: full }, 0);
    assert_eq!(s.w.fleets[&full].cargo, tons(0, 50));
}

#[test]
fn unload_rejects_unknown_foreign_moving_fleets_and_a_planet_not_the_players() {
    let mut s = setup();
    let fleet = fleet_of(&mut s.w, s.home, &[s.haulers[0]], tons(0, 10));
    let ghost = FleetId(s.w.next_id + 1);
    reject(
        &mut s.w,
        ALICE,
        &Action::TriggerUnload { fleet: ghost },
        0,
        Rejection::UnknownFleet,
    );
    reject(
        &mut s.w,
        BOB,
        &Action::TriggerUnload { fleet },
        0,
        Rejection::NotOwner,
    );
    accept(&mut s.w, ALICE, &send(fleet, s.free, Mission::Move), 0);
    reject(
        &mut s.w,
        ALICE,
        &Action::TriggerUnload { fleet },
        1,
        Rejection::FleetNotStationed,
    );
    let lands = arrival(&s.w, fleet);
    reject(
        &mut s.w,
        ALICE,
        &Action::TriggerUnload { fleet },
        lands,
        Rejection::PlanetNotOwned,
    );
    // At another player's planet too.
    accept(
        &mut s.w,
        ALICE,
        &send(fleet, s.bob_home, Mission::Move),
        lands,
    );
    let lands = arrival(&s.w, fleet);
    reject(
        &mut s.w,
        ALICE,
        &Action::TriggerUnload { fleet },
        lands,
        Rejection::PlanetNotOwned,
    );
}

// --- What holds for every fleet command ---

#[test]
fn fleet_commands_reject_a_player_who_has_not_joined() {
    let mut s = setup();
    let fleet = fleet_of(&mut s.w, s.home, &[s.haulers[0]], Cargo::default());
    let actions = [
        assemble(s.home, &[s.haulers[1]], Cargo::default()),
        send(fleet, s.free, Mission::Move),
        Action::RecallFleet { fleet },
        Action::DisbandFleet { fleet },
        Action::TriggerUnload { fleet },
    ];
    for action in &actions {
        reject(&mut s.w, CAROL, action, 0, Rejection::NotJoined);
    }
}

#[test]
fn fleet_commands_check_ownership_before_state() {
    // Bob's fleets, each in a state that would also be refused: another player always gets
    // not_owner, never the state code (D-028's order; R1's carry).
    let mut s = setup();
    let id = FleetId(s.w.next_id);
    accept(
        &mut s.w,
        BOB,
        &assemble(s.bob_home, &[s.bob_ship], tons(0, 10)),
        0,
    );
    let checks = [
        (
            Action::RecallFleet { fleet: id },
            "stationed: fleet_not_in_transit",
        ),
        (Action::DisbandFleet { fleet: id }, "loaded: cargo_aboard"),
        (
            send(id, s.bob_home, Mission::Move),
            "same planet: same_planet",
        ),
    ];
    for (action, why) in &checks {
        let applied = apply(&mut s.w, ALICE, action, 0);
        assert_eq!(applied.outcome, Err(Rejection::NotOwner), "{why}");
    }
    accept(&mut s.w, BOB, &send(id, s.free, Mission::Move), 0);
    for (action, why) in [
        (
            send(id, s.home, Mission::Move),
            "moving: fleet_not_stationed",
        ),
        (
            Action::TriggerUnload { fleet: id },
            "moving: fleet_not_stationed",
        ),
    ] {
        let applied = apply(&mut s.w, ALICE, &action, 1);
        assert_eq!(applied.outcome, Err(Rejection::NotOwner), "{why}");
    }
    // A ship of Bob's already in his fleet: not_owner before ship_not_on_roster.
    let action = assemble(s.bob_home, &[s.bob_ship], Cargo::default());
    let applied = apply(&mut s.w, ALICE, &action, 1);
    assert_eq!(applied.outcome, Err(Rejection::NotOwner));
}
