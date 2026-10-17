//! Even-split tests: the solver's pure arithmetic (Flows, "Even split"). The named solver
//! tests, on real Rules v0 worlds, are in testkit's `tests/solver.rs`.

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

use super::*;
use crate::quantity::Quantity;
use crate::rules::test_rules;
use crate::state::{
    Building, BuildingKind, BuildingState, Planet, Player, PlayerId, Position, ShipBuild, ShipKind,
    World,
};

const T: i64 = 1_000_000; // grams per ton

#[test]
fn even_split_equal_demands_get_equal_shares() {
    // The game design's example: 10 t/h shared by two Refineries wanting 8 t/h gives 5 t/h each.
    assert_eq!(
        even_split(10 * T, &[(4, 8 * T), (5, 8 * T)]),
        [5 * T, 5 * T]
    );
}

#[test]
fn even_split_a_small_demand_passes_the_rest_evenly_to_the_others() {
    // 12 among 2, 9 and 9: the first takes 2, the others split the remaining 10.
    assert_eq!(even_split(12, &[(1, 2), (2, 9), (3, 9)]), [2, 5, 5]);
    // A second round: 30 among 20, 4, 20 and 6 gives 4 and 6, then 10 each.
    assert_eq!(
        even_split(30, &[(7, 20), (3, 4), (9, 20), (5, 6)]),
        [10, 4, 10, 6]
    );
}

#[test]
fn even_split_leftover_grams_go_one_at_a_time_to_the_lowest_ids() {
    // 11 among three that each want more: 3 each, then 2 left over to IDs 2 and 5.
    assert_eq!(even_split(11, &[(9, 100), (2, 100), (5, 100)]), [3, 4, 4]);
    // Fewer grams than consumers: the lowest IDs get one each.
    assert_eq!(even_split(2, &[(9, 100), (2, 100), (5, 100)]), [0, 1, 1]);
}

#[test]
fn even_split_a_demand_equal_to_the_share_is_met_and_the_rest_goes_on() {
    // 10 among 3, 5 and 5: the share is 3, which exactly meets the first; the other two split
    // the remaining 7 as 3 each, and the leftover gram goes to the lower ID.
    assert_eq!(even_split(10, &[(1, 3), (2, 5), (3, 5)]), [3, 4, 3]);
}

#[test]
fn even_split_one_consumer_takes_up_to_its_demand() {
    assert_eq!(even_split(7, &[(3, 10)]), [7]);
    assert_eq!(even_split(70, &[(3, 10)]), [10]);
}

#[test]
fn even_split_supply_beyond_demand_meets_every_demand() {
    assert_eq!(even_split(100, &[(1, 3), (2, 0), (3, 40)]), [3, 0, 40]);
}

#[test]
fn even_split_of_nothing_gives_nothing() {
    assert_eq!(even_split(0, &[(1, 3), (2, 5)]), [0, 0]);
    assert_eq!(even_split(5, &[]), Vec::<i64>::new());
}

fn config() -> Config {
    Config {
        cases: 512,
        rng_seed: RngSeed::Fixed(0x0f10_5e7e_0000_0001),
        failure_persistence: None,
        ..Config::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// Max-min fairness, checked against its definition rather than the algorithm: nobody gets
    /// more than they want; the supply is used up unless every demand is met; and while anyone
    /// is short, nobody else has more than one gram per hour over them, that extra gram going
    /// only to a lower ID.
    #[test]
    fn even_split_is_max_min_fair(
        supply in 0i64..1_000,
        demands in prop::collection::vec((0u64..50, 0i64..200), 0..8),
    ) {
        let mut seen = BTreeSet::new();
        let demands: Vec<(u64, i64)> = demands.into_iter().filter(|d| seen.insert(d.0)).collect();
        let given = even_split(supply, &demands);
        let total: i64 = given.iter().sum();
        let wanted: i64 = demands.iter().map(|d| d.1).sum();
        prop_assert_eq!(total, supply.min(wanted));
        for (g, d) in given.iter().zip(&demands) {
            prop_assert!(*g >= 0 && *g <= d.1);
        }
        for (i, (gi, di)) in given.iter().zip(&demands).enumerate() {
            if *gi < di.1 {
                for (j, (gj, dj)) in given.iter().zip(&demands).enumerate() {
                    if i != j {
                        prop_assert!(*gj <= gi + 1, "{:?} -> {:?}", demands, given);
                        if *gj == gi + 1 {
                            prop_assert!(dj.0 < di.0, "the extra gram goes to the lower ID: {:?} -> {:?}", demands, given);
                        }
                    }
                }
            }
        }
    }
}

// --- The loop guard (Next event; D-026) ---

/// A homeworld with a Shipyard (2), an active build already at its cost (3) and a waiting build
/// already at its cost too (4): completing 3 starts 4, which is due at once, so step 2 needs two
/// rounds. No command makes such a state; it is built to reach the guard.
fn two_round_world() -> (World, PlanetId) {
    let alice = PlayerId(7);
    let p = PlanetId(1);
    let mut w = World::new(0, 0, 0);
    w.players.insert(
        alice,
        Player {
            id: alice,
            homeworld: p,
        },
    );
    w.planets.insert(
        p,
        Planet {
            id: p,
            position: Position { x: 0, y: 0, z: 0 },
            owner: Some(alice),
            slots: 8,
            ore_cap_g: 10_000 * T,
            ingot_cap_g: 5_000 * T,
            ore: Quantity::new(0, 0, 0),
            ingots: Quantity::new(0, 0, 0),
            buildings: [BuildingId(2)].into(),
            roster: BTreeSet::new(),
            version: 0,
            next_event: None,
        },
    );
    w.buildings.insert(
        BuildingId(2),
        Building {
            id: BuildingId(2),
            planet: p,
            kind: BuildingKind::Shipyard,
            state: BuildingState::Operational,
        },
    );
    for (id, status) in [(3, ShipBuildStatus::Active), (4, ShipBuildStatus::Waiting)] {
        w.ship_builds.insert(
            ShipBuildId(id),
            ShipBuild {
                id: ShipBuildId(id),
                shipyard: BuildingId(2),
                kind: ShipKind::CargoVessel,
                delivered: Quantity::new(300 * T, 0, 0),
                cost_g: 300 * T,
                rated_g_per_h: 75 * T,
                status,
            },
        );
    }
    w.next_id = 5;
    w.check_links().unwrap();
    (w, p)
}

#[test]
fn loop_guard_too_many_transition_rounds_stop_the_engine() {
    let rules = test_rules();
    let (mut w, p) = two_round_world();
    assert_eq!(
        w.settle_transitions(p, &rules, 1),
        Err(EngineError::TransitionLoop {
            planet: p,
            rounds: 2
        })
    );
    // At the bound itself, two rounds are allowed.
    let (mut w, p) = two_round_world();
    let events = w.settle_transitions(p, &rules, 2).unwrap();
    assert_eq!(events.len(), 2);
}

#[test]
fn loop_guard_the_production_bound_is_buildings_plus_builds() {
    let (w, p) = two_round_world();
    assert_eq!(w.transition_bound(p), 1 + 2);
    // So the two-round state solves in production.
    let (mut w, p) = two_round_world();
    assert_eq!(w.solve_planet(p, &test_rules()).unwrap().len(), 2);
}

#[test]
fn loop_guard_a_next_event_not_strictly_after_now_stops_the_engine() {
    assert_eq!(
        after_now(Some(10), 10),
        Err(EngineError::NotAfterNow { at_ms: 10, now: 10 })
    );
    assert_eq!(
        after_now(Some(9), 10),
        Err(EngineError::NotAfterNow { at_ms: 9, now: 10 })
    );
    assert_eq!(after_now(Some(11), 10), Ok(()));
    assert_eq!(after_now(None, 10), Ok(()));
}
