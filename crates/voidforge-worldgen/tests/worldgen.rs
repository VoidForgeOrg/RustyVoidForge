//! World generation's named tests (Technical Plan P1.12: "The same seed always yields the same
//! state hash, and every world generation default below holds"; decision log D-031), one per
//! default, on Rules v0 at 1, 7 and 100 players (D-031 option A: any player count generates in
//! the fixed cube) and on altered rules.
//! Defaults are checked by brute force here, independently of the generator's grid.

use voidforge_core::command::{Action, Command};
use voidforge_core::rules::{DISTANCE_UNITS_PER_WORLD_UNIT, Rules};
use voidforge_core::state::{PlanetId, World};
use voidforge_testkit::determinism::{canonical_bytes, mix};
use voidforge_testkit::{ALICE, v0_rules};
use voidforge_worldgen::{MAX_PLANETS, WorldgenError, generate};

const U: i64 = DISTANCE_UNITS_PER_WORLD_UNIT;

/// Rules with every worldgen number changed from Rules v0, so a default hard-coded in the
/// generator shows up: 25 planets per player, 12-unit spacing, candidates 160 units apart with
/// 4 neighbors within 110 units in a 450-unit cube, slots and caps changed too.
fn altered_rules() -> Rules {
    let mut rules = v0_rules();
    let g = &mut rules.worldgen;
    g.cube_size_du = 450 * U;
    g.planets_per_expected_player = 25;
    g.min_planet_spacing_du = 12 * U;
    g.homeworld_min_spacing_du = 160 * U;
    g.homeworld_neighbor_radius_du = 110 * U;
    g.homeworld_min_neighbors = 4;
    rules.planet.slots = 6;
    rules.planet.ore_cap_g = 7_000_000_000;
    rules.validate().unwrap();
    rules
}

/// The worlds every default is checked on: (rules, players, seed).
fn worlds() -> Vec<(Rules, u32, World)> {
    [
        (v0_rules(), 100, 1),
        (v0_rules(), 1, 2),
        (v0_rules(), 7, 3),
        (v0_rules(), 1, 0),
        (v0_rules(), 7, u64::MAX),
        (altered_rules(), 7, 4),
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
fn one_seven_and_a_hundred_players_generate_under_the_default_rules() {
    // D-031 option A (Tom, 2026-10-18): any player count generates in the fixed 1,000-unit cube.
    // The guarantees themselves are checked on these sizes by the `default_*` tests above.
    for players in [1, 7, 100] {
        let w = generate(1, players, &v0_rules()).unwrap();
        assert_eq!(w.homeworld_candidates.len(), players as usize);
        assert_eq!(w.planets.len(), 20 * players as usize);
    }
}

/// The real player limit of the default cube: candidates 150 units apart by random placement
/// fill it at about 240 to 255 (measured over seeds 0 to 11; the packing bound allows 901).
/// Past it, generation stops with a typed error naming the candidates, never a loop.
#[test]
fn the_player_limit_is_a_typed_stop_at_the_candidates() {
    let rules = v0_rules();
    assert_eq!(
        generate(0, 241, &rules).unwrap().homeworld_candidates.len(),
        241
    );
    assert_eq!(
        generate(0, 242, &rules),
        Err(WorldgenError::Exhausted {
            what: "homeworld candidate",
            placed: 241
        })
    );
}

#[test]
fn planet_ids_do_not_reveal_the_candidates() {
    // Candidates are placed first, but IDs follow coordinate order, so they are not the lowest
    // IDs, and ID order is coordinate order.
    let w = generate(9, 7, &v0_rules()).unwrap();
    let mut ids: Vec<u64> = w.homeworld_candidates.iter().map(|p| p.0).collect();
    ids.sort();
    let lowest: Vec<u64> = w.planets.keys().take(7).map(|p| p.0).collect();
    assert_ne!(ids, lowest);
    let positions: Vec<(i64, i64, i64)> = w
        .planets
        .values()
        .map(|p| (p.position.x, p.position.y, p.position.z))
        .collect();
    assert!(positions.windows(2).all(|p| p[0] < p[1]));
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
        Err(WorldgenError::Exhausted { what: "planet", .. })
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

#[test]
fn a_fixed_seed_gives_the_recorded_world() {
    // A golden: seed 8 at 7 players under Rules v0. Any change to the sampling, the RNG's use or
    // the shuffle changes it; refresh only on a deliberate change (D-031; option A refreshed it).
    let w = generate(8, 7, &v0_rules()).unwrap();
    let first: Vec<(i64, i64, i64)> = w
        .planets
        .values()
        .take(3)
        .map(|p| (p.position.x / U, p.position.y / U, p.position.z / U))
        .collect();
    let candidates: Vec<u64> = w.homeworld_candidates.iter().map(|p| p.0).collect();
    assert_eq!(first, GOLDEN_FIRST);
    assert_eq!(candidates, GOLDEN_CANDIDATES);
}

const GOLDEN_FIRST: [(i64, i64, i64); 3] = [(7, 227, 327), (13, 947, 274), (28, 21, 660)];
const GOLDEN_CANDIDATES: [u64; 7] = [91, 84, 126, 118, 80, 58, 61];

/// A golden at 100 players, where neighborhoods overlap: a fingerprint of every planet's
/// position (in ID order) and the candidate order. It pins what the guarantees alone can't see,
/// such as step 2 counting planets already near a candidate before adding more (the mutation
/// that skips the count keeps every guarantee). Refresh only on a deliberate change (D-031).
#[test]
fn a_fixed_seed_gives_the_recorded_100_player_world() {
    let w = generate(8, 100, &v0_rules()).unwrap();
    let positions = w.planets.values().fold(0u64, |acc, p| {
        let (x, y, z) = (p.position.x / U, p.position.y / U, p.position.z / U);
        mix(acc ^ (x as u64) ^ ((y as u64) << 20) ^ ((z as u64) << 40))
    });
    let candidates = w
        .homeworld_candidates
        .iter()
        .fold(0u64, |acc, p| mix(acc ^ p.0));
    assert_eq!((positions, candidates), GOLDEN_100);
}

const GOLDEN_100: (u64, u64) = (12_890_825_601_051_078_430, 18_307_943_236_284_880_597);

/// review-r2's M2 witness: a cube of 160 units, spacing 11, and 4,000 planets for one player.
/// The lattice (8x, 8y, 8z), x, y, z in 0..20 with x + y + z even, holds 4,000 points pairwise
/// at least √128 > 11 apart, with 12 neighbors within 12 units of (80, 80, 80). So these rules
/// are feasible, and generation must not call them impossible. Random placement may honestly
/// run out of attempts at this density (`Exhausted`).
#[test]
fn feasibility_a_dense_but_possible_packing_is_not_called_impossible() {
    let lattice: Vec<[i64; 3]> = (0..20)
        .flat_map(|x| (0..20).flat_map(move |y| (0..20).map(move |z| [x, y, z])))
        .filter(|[x, y, z]| (x + y + z) % 2 == 0)
        .map(|[x, y, z]| [8 * x, 8 * y, 8 * z])
        .collect();
    assert_eq!(lattice.len(), 4_000);
    for (i, a) in lattice.iter().enumerate() {
        for b in &lattice[i + 1..] {
            let d2: i64 = (0..3).map(|k| (a[k] - b[k]).pow(2)).sum();
            assert!(d2 >= 121, "{a:?} {b:?}");
        }
    }
    let mut rules = v0_rules();
    let g = &mut rules.worldgen;
    g.cube_size_du = 160 * U;
    g.min_planet_spacing_du = 11 * U;
    g.homeworld_min_spacing_du = 11 * U;
    g.homeworld_neighbor_radius_du = 12 * U;
    g.homeworld_min_neighbors = 5;
    g.planets_per_expected_player = 4_000;
    rules.validate().unwrap();
    match generate(1, 1, &rules) {
        Ok(_) | Err(WorldgenError::Exhausted { .. }) => {}
        other => panic!("a feasible packing was rejected: {other:?}"),
    }
}

/// review-r2's M1 on the generation side: products of extreme but valid values are decided,
/// not overflowed. u32::MAX players of u32::MAX planets are refused by the size cap first
/// (`TooLarge`, D-043); within the cap, the largest distances at a half-cube spacing cannot fit,
/// and the answer is the typed `Infeasible` in every build profile, never a panic or a wrapped
/// decision.
#[test]
fn feasibility_extreme_counts_and_distances_are_decided_without_overflow() {
    let mut rules = v0_rules();
    let g = &mut rules.worldgen;
    g.cube_size_du = 1_000_000_000_000;
    g.min_planet_spacing_du = 500_000_000_000;
    g.homeworld_min_spacing_du = 500_000_000_000;
    g.homeworld_neighbor_radius_du = 500_000_000_000;
    g.homeworld_min_neighbors = 0;
    g.planets_per_expected_player = u32::MAX;
    rules.validate().unwrap();
    assert_eq!(
        generate(0, u32::MAX, &rules),
        Err(WorldgenError::TooLarge {
            planets: u64::from(u32::MAX) * u64::from(u32::MAX),
            max: MAX_PLANETS
        })
    );
    rules.worldgen.planets_per_expected_player = 100;
    assert_eq!(
        generate(0, 1_000, &rules),
        Err(WorldgenError::Infeasible(
            "the planets cannot keep their spacing in the cube"
        ))
    );
}

/// D-033's carry (P1.12 QA LOW), D-043: the world size an operator can ask for is capped at
/// MAX_PLANETS, refused before anything is generated, with the limit in the error. The edge
/// itself generates in a cube large enough to hold it.
#[test]
fn the_world_size_is_capped_at_max_planets() {
    let rules = v0_rules();
    let players = (MAX_PLANETS / 20) as u32;
    assert_eq!(
        generate(0, players + 1, &rules),
        Err(WorldgenError::TooLarge {
            planets: MAX_PLANETS + 20,
            max: MAX_PLANETS
        })
    );
    let message = generate(0, u32::MAX, &rules).unwrap_err().to_string();
    assert!(message.contains("the limit is 100000 planets"), "{message}");
    let mut big = v0_rules();
    big.worldgen.cube_size_du = 6_000 * U;
    let w = generate(1, players, &big).unwrap();
    assert_eq!(w.planets.len() as u64, MAX_PLANETS);
}
