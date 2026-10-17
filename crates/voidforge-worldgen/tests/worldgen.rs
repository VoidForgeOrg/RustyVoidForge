//! World generation's named tests (Technical Plan P1.12: "The same seed always yields the same
//! state hash, and every world generation default below holds"; decision log D-031), one per
//! default, on Rules v0 at the 100-player default and on denser rules for 1 and 7 players (D-031
//! option B: under the default 1,000-unit cube, small worlds are infeasible by design).
//! Defaults are checked by brute force here, independently of the generator's grid.

use voidforge_core::command::{Action, Command};
use voidforge_core::rules::{DISTANCE_UNITS_PER_WORLD_UNIT, Rules};
use voidforge_core::state::{PlanetId, World};
use voidforge_testkit::determinism::canonical_bytes;
use voidforge_testkit::{ALICE, v0_rules};
use voidforge_worldgen::{WorldgenError, generate};

const U: i64 = DISTANCE_UNITS_PER_WORLD_UNIT;

/// Rules v0 with the cube shrunk so `players` keep the 100-player density: side ≈ 1,000 ×
/// (players / 100)^⅓ world units (precomputed: 216 for 1 player, 413 for 7).
fn dense_rules(side_units: i64) -> Rules {
    let mut rules = v0_rules();
    rules.worldgen.cube_size_du = side_units * U;
    rules.validate().unwrap();
    rules
}

/// The worlds every default is checked on: (rules, players, seed).
fn worlds() -> Vec<(Rules, u32, World)> {
    [
        (v0_rules(), 100, 1),
        (dense_rules(216), 1, 2),
        (dense_rules(413), 7, 3),
    ]
    .into_iter()
    .map(|(rules, players, seed)| {
        let w = generate(seed, players, &rules).unwrap();
        (rules, players, w)
    })
    .collect()
}

fn d2(w: &World, a: PlanetId, b: PlanetId) -> i128 {
    let (p, q) = (w.planets[&a].position, w.planets[&b].position);
    [(p.x, q.x), (p.y, q.y), (p.z, q.z)]
        .iter()
        .map(|&(x, y)| i128::from(x - y).pow(2))
        .sum()
}

#[test]
fn default_planets_per_expected_player() {
    for (rules, players, w) in worlds() {
        let want = u64::from(players) * u64::from(rules.worldgen.planets_per_expected_player);
        assert_eq!(w.planets.len() as u64, want);
    }
}

#[test]
fn default_whole_unit_coordinates_inside_the_cube() {
    for (rules, _, w) in worlds() {
        for p in w.planets.values() {
            for c in [p.position.x, p.position.y, p.position.z] {
                assert_eq!(c % U, 0, "planet {} off whole units", p.id.0);
                assert!((0..rules.worldgen.cube_size_du).contains(&c));
            }
        }
    }
}

#[test]
fn default_every_pair_at_least_the_spacing_apart() {
    for (rules, _, w) in worlds() {
        let s = i128::from(rules.worldgen.min_planet_spacing_du);
        let ids: Vec<PlanetId> = w.planets.keys().copied().collect();
        for (i, &a) in ids.iter().enumerate() {
            for &b in &ids[i + 1..] {
                assert!(
                    d2(&w, a, b) >= s * s,
                    "planets {} and {} too close",
                    a.0,
                    b.0
                );
            }
        }
    }
}

#[test]
fn default_slots_and_caps_from_the_rules_and_nothing_else() {
    for (rules, _, w) in worlds() {
        for p in w.planets.values() {
            assert_eq!(
                (p.slots, p.ore_cap_g, p.ingot_cap_g),
                (
                    rules.planet.slots,
                    rules.planet.ore_cap_g,
                    rules.planet.ingot_cap_g
                )
            );
            assert_eq!(p.owner, None);
            assert!(p.buildings.is_empty() && p.roster.is_empty());
            assert_eq!((p.ore.value_g(), p.ingots.value_g()), (0, 0));
        }
        assert!(w.players.is_empty() && w.ships.is_empty() && w.fleets.is_empty());
    }
}

#[test]
fn default_one_candidate_per_player_far_apart_with_enough_neighbors() {
    for (rules, players, w) in worlds() {
        let g = &rules.worldgen;
        let c = &w.homeworld_candidates;
        assert_eq!(c.len(), players as usize);
        let hs = i128::from(g.homeworld_min_spacing_du);
        for (i, &a) in c.iter().enumerate() {
            for &b in &c[i + 1..] {
                assert!(
                    d2(&w, a, b) >= hs * hs,
                    "candidates {} and {} too close",
                    a.0,
                    b.0
                );
            }
            let r = i128::from(g.homeworld_neighbor_radius_du);
            let near = w
                .planets
                .keys()
                .filter(|&&p| p != a && d2(&w, a, p) <= r * r)
                .count();
            assert!(
                near >= g.homeworld_min_neighbors as usize,
                "candidate {} has {near} neighbors",
                a.0
            );
        }
    }
}

#[test]
fn default_candidates_are_ordered_by_a_seeded_shuffle() {
    // Not ID order, and a different seed gives a different order.
    let rules = v0_rules();
    let a = generate(11, 100, &rules).unwrap().homeworld_candidates;
    let mut sorted = a.clone();
    sorted.sort();
    assert_ne!(a, sorted, "candidates in ID order");
    let b = generate(12, 100, &rules).unwrap().homeworld_candidates;
    assert_ne!(a, b);
}

#[test]
fn default_the_seed_is_recorded_and_the_world_is_solved_at_time_zero() {
    for (rules, _, w) in worlds() {
        assert_eq!((w.time_ms, w.rules_version), (0, rules.version));
        w.check_links().unwrap();
        for p in w.planets.values() {
            assert_eq!(p.version, 1, "planet {} not solved", p.id.0);
        }
    }
    assert_eq!(generate(77, 100, &v0_rules()).unwrap().seed, 77);
}

#[test]
fn the_same_seed_gives_identical_bytes_and_different_seeds_differ() {
    let rules = v0_rules();
    for seed in [0, 1, u64::MAX] {
        let a = canonical_bytes(&generate(seed, 100, &rules).unwrap());
        let b = canonical_bytes(&generate(seed, 100, &rules).unwrap());
        assert!(a == b, "seed {seed}");
    }
    let a = canonical_bytes(&generate(0, 100, &rules).unwrap());
    let b = canonical_bytes(&generate(u64::MAX, 100, &rules).unwrap());
    assert!(a != b);
}

#[test]
fn small_worlds_under_the_default_cube_are_infeasible() {
    // D-031 option B (provisional pending Tom): uniform placement in the 1,000-unit cube leaves
    // a 1-player world (20 planets) no planet with 5 neighbors within 100 units.
    assert_eq!(
        generate(1, 1, &v0_rules()),
        Err(WorldgenError::Infeasible(
            "too few planets qualify as homeworld candidates"
        ))
    );
}

#[test]
fn feasibility_errors_are_typed() {
    let rules = v0_rules();
    assert_eq!(generate(1, 0, &rules), Err(WorldgenError::NoPlayers));
    // More planets than the packing bound allows in the cube.
    let mut tight = v0_rules();
    tight.worldgen.cube_size_du = 30 * U;
    tight.worldgen.homeworld_min_spacing_du = 10 * U;
    tight.worldgen.homeworld_neighbor_radius_du = 10 * U;
    assert_eq!(
        generate(1, 10, &tight),
        Err(WorldgenError::Infeasible(
            "the planets cannot keep their spacing in the cube"
        ))
    );
    // Within the bound but beyond what random placement can reach: a typed stop, not a loop.
    let mut jammed = v0_rules();
    jammed.worldgen.cube_size_du = 100 * U;
    jammed.worldgen.homeworld_min_spacing_du = 10 * U;
    jammed.worldgen.homeworld_neighbor_radius_du = 20 * U;
    assert!(matches!(
        generate(1, 50, &jammed),
        Err(WorldgenError::Exhausted { .. })
    ));
    // Invalid rules are refused through validation.
    let mut bad = v0_rules();
    bad.worldgen.homeworld_neighbor_radius_du = 5 * U;
    assert!(matches!(
        generate(1, 100, &bad),
        Err(WorldgenError::Rules(_))
    ));
}

#[test]
fn join_game_takes_the_first_candidate_of_a_generated_world() {
    let rules = v0_rules();
    let mut w = generate(5, 100, &rules).unwrap();
    let first = w.homeworld_candidates[0];
    let join = Command {
        player: ALICE,
        action: Action::JoinGame,
    };
    w.apply(&join, 0, &rules).unwrap().outcome.unwrap();
    assert_eq!(w.players[&ALICE].homeworld, first);
    assert_eq!(w.planets[&first].owner, Some(ALICE));
    w.check_links().unwrap();
}

#[test]
fn the_100_player_default_generates() {
    // Timing is reported by the test runner (PROOF records it): 2,000 planets.
    let w = generate(2026, 100, &v0_rules()).unwrap();
    assert_eq!(w.planets.len(), 2_000);
}
