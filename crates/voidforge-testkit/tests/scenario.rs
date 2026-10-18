//! The Phase 1 end-to-end scenario (QA's gate verdict, adopted by review-r1's addendum C):
//! the whole Game Design loop through the public API (join, Shipyard, ships, colonize, a colony
//! Drill, score) on Rules v0. **The expected values were derived by hand by dev-qa** from the
//! Game Design and Rules v0 (proof/phase1-qa/qa_e2e.rs), independently of the engine's code;
//! change them only from a new hand derivation, never from the engine's output.

use voidforge_core::command::{Action, Command};
use voidforge_core::state::*;
use voidforge_testkit::{ALICE, WorldBuilder};
const H: i64 = 3_600_000;
const T: i64 = 1_000_000;
fn ok(w: &mut World, a: Action, t: i64, r: &voidforge_core::rules::Rules) {
    let o = w
        .apply(
            &Command {
                player: ALICE,
                action: a,
            },
            t,
            r,
        )
        .unwrap();
    assert!(o.outcome.is_ok(), "{:?} at {t}", o.outcome);
    w.check_links().unwrap();
}
#[test]
fn qa_hand_derived_homeworld_to_colony() {
    let r = voidforge_testkit::v0_rules();
    let mut b = WorldBuilder::with_rules(r.clone());
    let home = b.planet(None);
    let colony = b.planet(None);
    b.position(home, Position { x: 0, y: 0, z: 0 });
    b.position(
        colony,
        Position {
            x: 45_000,
            y: 0,
            z: 0,
        },
    );
    b.candidates(&[home]);
    let mut w = b.build();
    ok(&mut w, Action::JoinGame, 0, &r);
    let ing = |w: &World, p: PlanetId, t: i64| w.planets[&p].ingots.value_at(t).unwrap();
    assert_eq!(
        (w.planets[&home].ore.value_g(), ing(&w, home, 0)),
        (2_000 * T, 1_500 * T)
    );
    ok(
        &mut w,
        Action::BuildBuilding {
            planet: home,
            kind: BuildingKind::Shipyard,
        },
        0,
        &r,
    );
    let yard = *w.planets[&home]
        .buildings
        .iter()
        .find(|b| w.buildings[b].kind == BuildingKind::Shipyard)
        .unwrap();
    assert_eq!(
        w.planets[&home].next_event.map(|n| n.at_ms),
        Some(12 * H),
        "Shipyard 1,200 t at 100 t/h completes at 12 h"
    );
    let ev = w.advance_to(12 * H, &r).unwrap();
    assert_eq!(ev.len(), 1);
    assert_eq!(w.buildings[&yard].state, BuildingState::Operational);
    assert_eq!(ing(&w, home, 12 * H), 492 * T, "1,500 - 84 x 12");
    assert_eq!(ing(&w, home, 50 * H), 1_100 * T, "492 + 16 x 38");
    ok(
        &mut w,
        Action::QueueShip {
            shipyard: yard,
            kind: ShipKind::ColonyShip,
        },
        50 * H,
        &r,
    );
    ok(
        &mut w,
        Action::QueueShip {
            shipyard: yard,
            kind: ShipKind::CargoVessel,
        },
        50 * H,
        &r,
    );
    w.advance_to(54 * H, &r).unwrap();
    assert_eq!(
        w.ships
            .values()
            .filter(|s| s.kind == ShipKind::CargoVessel)
            .count(),
        1,
        "Cargo Vessel at 54 h"
    );
    assert_eq!(ing(&w, home, 54 * H), 544 * T, "1,100 - 139 x 4");
    w.advance_to(60 * H, &r).unwrap();
    assert_eq!(w.ships.len(), 2, "Colony Ship at 60 h");
    assert_eq!(ing(&w, home, 60 * H), 160 * T, "544 - 64 x 6");
    let ships = w.planets[&home].roster.clone();
    ok(
        &mut w,
        Action::AssembleFleet {
            planet: home,
            ships,
            cargo: Cargo {
                ore_g: 0,
                ingots_g: 150 * T,
            },
        },
        60 * H,
        &r,
    );
    let f = *w.fleets.keys().max().unwrap();
    assert_eq!(ing(&w, home, 60 * H), 10 * T);
    ok(
        &mut w,
        Action::SendFleet {
            fleet: f,
            target: colony,
            mission: Mission::Colonize,
        },
        60 * H,
        &r,
    );
    assert!(
        matches!(w.fleets[&f].state, FleetState::InTransit { arrives_at_ms, .. } if arrives_at_ms == 60 * H + 45 * H / 10),
        "45 units at 10 u/h = 4.5 h"
    );
    let ev = w.advance_to(645 * H / 10, &r).unwrap();
    assert!(
        ev.iter()
            .any(|e| matches!(e, DomainEvent::PlanetColonized { .. })),
        "{ev:?}"
    );
    assert_eq!(w.planets[&colony].owner, Some(ALICE));
    assert_eq!(ing(&w, colony, 645 * H / 10), 150 * T);
    ok(
        &mut w,
        Action::BuildBuilding {
            planet: colony,
            kind: BuildingKind::Drill,
        },
        645 * H / 10,
        &r,
    );
    assert_eq!(
        w.planets[&colony].next_event.map(|n| n.at_ms),
        Some(675 * H / 10),
        "150 t at 50 t/h: 3 h, ingots reach 0 at the same instant"
    );
    let ev = w.advance_to(675 * H / 10, &r).unwrap();
    assert!(ev.iter().any(|e| matches!(e, DomainEvent::BuildingCompleted { planet, kind: BuildingKind::Drill, .. } if *planet == colony)), "{ev:?}");
    assert_eq!(ing(&w, colony, 675 * H / 10), 0);
    assert_eq!(
        w.planets[&colony].ore.rate_g_per_h(),
        10 * T,
        "the colony's Drill now extracts 10 t/h"
    );
    let t = 675 * H / 10;
    assert_eq!(w.planets[&home].ore.value_at(t).unwrap(), 2_135 * T);
    assert_eq!(ing(&w, home, t), 130 * T);
    assert_eq!(
        w.score(ALICE, t, &r).unwrap(),
        3_239,
        "2,000 + 750 + 100 + 150 + 213 + 26"
    );
}
