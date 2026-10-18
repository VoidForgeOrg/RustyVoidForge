//! Seeded world generation (Technical Plan P1.12 and its world generation defaults; Architectural
//! Design, State model and Determinism rules; decision log D-031).
//!
//! [`generate`] turns a 64-bit universe seed, an expected player count and the rules into the
//! universe's first `World`: planets on whole world units in the rules' cube (D-021), every pair
//! at least the rules' spacing apart, and an ordered list of homeworld candidates, each far
//! enough from the others and with enough planets near it by construction (D-031, option A:
//! candidates first, then their neighborhoods, then a uniform fill). Every number comes from the
//! rules' `worldgen` section, never from a constant here.
//!
//! The only randomness is `ChaCha8Rng` seeded from the universe seed. Sampling is integer-exact
//! rejection sampling. Each placement has a hard attempt bound, and running out of attempts is
//! a typed error, so generation never loops forever.

// Determinism guardrails (Technical Plan P0.4). Forbidden, so no item can opt out; the banned
// paths are in this crate's clippy.toml.
// Every integer operation in the library is checked or strict (D-032), so nothing wraps
// silently in release; unit tests are exempt.
#![cfg_attr(not(test), forbid(clippy::arithmetic_side_effects))]
#![forbid(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::float_arithmetic
)]

use std::collections::BTreeMap;

use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use voidforge_core::quantity::Quantity;
use voidforge_core::rules::{DISTANCE_UNITS_PER_WORLD_UNIT, Rules, RulesError};
use voidforge_core::state::{EngineError, Planet, PlanetId, Position, StateError, World};

/// Draws for one planet before generation gives up with [`WorldgenError::Exhausted`].
pub const MAX_ATTEMPTS: u32 = 10_000;

/// Why a world could not be generated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorldgenError {
    /// A universe needs at least one expected player.
    #[error("a universe needs at least one expected player")]
    NoPlayers,
    /// The rules are not valid.
    #[error(transparent)]
    Rules(#[from] RulesError),
    /// The parameters cannot hold for this many players: a necessary packing bound fails.
    #[error("infeasible world generation: {0}")]
    Infeasible(&'static str),
    /// A homeworld candidate or a planet found no free spot within [`MAX_ATTEMPTS`] draws. For
    /// candidates, this is the real player limit of the cube (D-031).
    #[error("no room for another {what} after {MAX_ATTEMPTS} draws ({placed} placed)")]
    Exhausted {
        /// "homeworld candidate" or "planet".
        what: &'static str,
        /// How many of them were placed so far.
        placed: usize,
    },
    /// Building the world failed (exhausted IDs, a solve error): an engine bug, not a parameter.
    #[error(transparent)]
    Engine(#[from] EngineError),
    /// The generated world's links do not hold: a bug.
    #[error(transparent)]
    State(#[from] StateError),
}

/// Generates the first `World` of a universe from `seed` for `players` expected players
/// (D-031, option A, Tom's decision of 2026-10-18).
///
/// 1. `players` homeworld candidates, uniform in the cube on whole world units, each at least
///    `homeworld_min_spacing_du` from the others.
/// 2. Around each candidate in turn, planets uniform within `homeworld_neighbor_radius_du`
///    (inclusive, clipped to the cube) until it has `homeworld_min_neighbors`, counting any
///    already there.
/// 3. The rest uniform in the cube, up to `players × planets_per_expected_player`.
///
/// Every planet is at least `min_planet_spacing_du` from every other. The candidate order is a
/// seeded shuffle. Planet IDs follow coordinate order, so an ID says nothing about which planets
/// are candidates. Each placement has [`MAX_ATTEMPTS`] draws, then [`WorldgenError::Exhausted`].
///
/// The world is at time 0 with the seed recorded, every planet unowned with the rules' slots
/// and caps and empty stocks, and solved (D-025).
pub fn generate(seed: u64, players: u32, rules: &Rules) -> Result<World, WorldgenError> {
    rules.validate()?;
    if players == 0 {
        return Err(WorldgenError::NoPlayers);
    }
    let g = &rules.worldgen;
    let unit = DISTANCE_UNITS_PER_WORLD_UNIT;
    let side = g.cube_size_du / unit;
    let spacing = g.min_planet_spacing_du / unit;
    let home_spacing = g.homeworld_min_spacing_du / unit;
    let radius = g.homeworld_neighbor_radius_du / unit;
    // Two u32 values in u64 (D-032).
    let total = u64::from(players).strict_mul(u64::from(g.planets_per_expected_player));

    // Necessary packing bounds. Balls of radius s/2 around n points pairwise s apart are
    // disjoint and lie in the cube widened by s/2 on each side, so n × (π/6) × s³ ≤ (L + s)³.
    // With π > 3 that gives the weaker but still necessary, integer-exact n × s³ ≤ 2 × (L + s)³
    // (review-r2's M2: separation does not imply disjoint s-cubes, so the cube bound
    // n × s³ ≤ (L + s)³ was not necessary). Rules validation caps the distances (D-031), so the
    // right side is exact; a left side beyond i128 is larger still.
    let packs = |n: u64, s: i64| {
        i128::from(n)
            .checked_mul(cube(s))
            .is_some_and(|need| need <= cube(side.strict_add(s)).strict_mul(2))
    };
    if !packs(total, spacing) {
        return Err(WorldgenError::Infeasible(
            "the planets cannot keep their spacing in the cube",
        ));
    }
    if !packs(u64::from(players), home_spacing) {
        return Err(WorldgenError::Infeasible(
            "the homeworld candidates cannot keep their spacing in the cube",
        ));
    }

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    // 1. The candidates. Validation keeps their spacing at least the planets', so they are the
    // first planets as they are.
    let mut sites = Placed::new(home_spacing);
    while sites.points.len() < players as usize {
        sites.place(&mut rng, side, "homeworld candidate")?;
    }
    let mut placed = Placed::new(spacing);
    for &p in &sites.points {
        placed.insert(p);
    }

    // 2. Each candidate's neighborhood, counting planets already within the radius (earlier
    // neighborhoods can overlap it). Later placements only add neighbors.
    let min_neighbors = g.homeworld_min_neighbors as usize;
    for (k, &c) in sites.points.iter().enumerate() {
        let mut near = placed
            .points
            .iter()
            .enumerate()
            .filter(|&(i, &q)| i != k && within(q, c, radius))
            .count();
        while near < min_neighbors {
            placed.place_near(&mut rng, side, c, radius)?;
            near = near.strict_add(1);
        }
    }

    // 3. The uniform fill. Validation gives planets_per_expected_player ≥ 1 + min_neighbors, so
    // steps 1 and 2 never exceed the total.
    while (placed.points.len() as u64) < total {
        placed.place(&mut rng, side, "planet")?;
    }

    // 4. The candidate order: a seeded shuffle (Fisher–Yates). Candidates are the first
    // `players` points.
    let mut chosen: Vec<usize> = (0..players as usize).collect();
    for i in (1..chosen.len()).rev() {
        let j = below(&mut rng, (i as u64).strict_add(1)) as usize;
        chosen.swap(i, j);
    }

    let mut world = World::new(0, rules.version, seed);
    // IDs in coordinate order (a stable sort), so candidates are not the lowest IDs.
    let mut by_position: Vec<usize> = (0..placed.points.len()).collect();
    by_position.sort_by_key(|&i| placed.points[i]);
    let mut ids = vec![PlanetId(0); placed.points.len()];
    for i in by_position {
        let p = &placed.points[i];
        let id = PlanetId(world.allocate_id()?);
        world.planets.insert(
            id,
            Planet {
                id,
                position: Position {
                    // A coordinate below the side, times 1,000, is below the cube_size_du it came
                    // from (D-032).
                    x: p[0].strict_mul(unit),
                    y: p[1].strict_mul(unit),
                    z: p[2].strict_mul(unit),
                },
                owner: None,
                slots: rules.planet.slots,
                ore_cap_g: rules.planet.ore_cap_g,
                ingot_cap_g: rules.planet.ingot_cap_g,
                ore: Quantity::new(0, 0, 0),
                ingots: Quantity::new(0, 0, 0),
                buildings: Default::default(),
                roster: Default::default(),
                version: 0,
                next_event: None,
            },
        );
        ids[i] = id;
    }
    world.homeworld_candidates = chosen.into_iter().map(|i| ids[i]).collect();
    for id in world.planets.keys().copied().collect::<Vec<_>>() {
        world.solve_planet(id, rules)?;
    }
    world.check_links()?;
    Ok(world)
}

/// Points placed so far, with a grid of cell size `spacing` for the spacing check: two points
/// closer than `spacing` are in the same or adjacent cells.
struct Placed {
    spacing: i64,
    points: Vec<[i64; 3]>,
    grid: BTreeMap<[i64; 3], Vec<usize>>,
}

impl Placed {
    fn new(spacing: i64) -> Self {
        Self {
            spacing,
            points: Vec::new(),
            grid: BTreeMap::new(),
        }
    }

    fn cell(&self, p: [i64; 3]) -> [i64; 3] {
        let c = self.spacing.max(1);
        [p[0].strict_div(c), p[1].strict_div(c), p[2].strict_div(c)]
    }

    /// Whether `p` keeps at least the spacing from every placed point.
    fn fits(&self, p: [i64; 3]) -> bool {
        adjacent(&self.grid, self.cell(p)).all(|i| dist2(self.points[i], p) >= sq(self.spacing))
    }

    /// Adds `p` (the caller has checked it fits).
    fn insert(&mut self, p: [i64; 3]) {
        let key = self.cell(p);
        self.grid.entry(key).or_default().push(self.points.len());
        self.points.push(p);
    }

    /// Places one point uniformly in `[0, side)³` that keeps the spacing, within
    /// [`MAX_ATTEMPTS`] draws.
    fn place(
        &mut self,
        rng: &mut ChaCha8Rng,
        side: i64,
        what: &'static str,
    ) -> Result<(), WorldgenError> {
        for _ in 0..MAX_ATTEMPTS {
            let p = uniform(rng, side);
            if self.fits(p) {
                self.insert(p);
                return Ok(());
            }
        }
        Err(WorldgenError::Exhausted {
            what,
            placed: self.points.len(),
        })
    }

    /// Places one point uniformly within `radius` of `c` (inclusive) and inside `[0, side)³`
    /// that keeps the spacing, within [`MAX_ATTEMPTS`] draws: uniform in the bounding box
    /// clipped to the cube, rejecting points outside the ball.
    fn place_near(
        &mut self,
        rng: &mut ChaCha8Rng,
        side: i64,
        c: [i64; 3],
        radius: i64,
    ) -> Result<(), WorldgenError> {
        // c is inside the cube and the radius is validated (D-031), so these stay in range.
        let lo = c.map(|x| x.strict_sub(radius).max(0));
        let hi = c.map(|x| x.strict_add(radius).min(side.strict_sub(1).max(0)));
        for _ in 0..MAX_ATTEMPTS {
            let p = [0, 1, 2].map(|i| {
                let span = hi[i].strict_sub(lo[i]).strict_add(1).max(1) as u64;
                lo[i].strict_add(below(rng, span) as i64)
            });
            if within(p, c, radius) && self.fits(p) {
                self.insert(p);
                return Ok(());
            }
        }
        Err(WorldgenError::Exhausted {
            what: "planet",
            placed: self.points.len(),
        })
    }
}

/// Points in the 27 cells around `c` (its own and the adjacent ones) of a grid.
fn adjacent(
    grid: &BTreeMap<[i64; 3], Vec<usize>>,
    c: [i64; 3],
) -> impl Iterator<Item = usize> + '_ {
    (0..27)
        // Cells are coordinates divided by a positive size, so ±1 cannot overflow (D-032).
        .filter_map(move |k: i64| {
            let d = [
                k.strict_div(9),
                k.strict_div(3).strict_rem(3),
                k.strict_rem(3),
            ];
            grid.get(&[0, 1, 2].map(|i| c[i].strict_add(d[i]).strict_sub(1)))
        })
        .flatten()
        .copied()
}

/// Whether `a` is within `radius` of `b`, inclusive ("within 100 units").
fn within(a: [i64; 3], b: [i64; 3], radius: i64) -> bool {
    dist2(a, b) <= sq(radius)
}

/// A uniform integer in `0..n` (`n > 0`), without modulo bias: draws in the top partial range
/// are rejected and redrawn. Each draw is accepted with probability above 1/2, so the expected
/// number of draws is below 2 and k rejections in a row have probability below 2^-k.
fn below(rng: &mut ChaCha8Rng, n: u64) -> u64 {
    let n = u128::from(n.max(1));
    // n ≥ 1, and ⌊2^64 / n⌋ × n ≤ 2^64 (D-032).
    let accept = (1u128 << 64).strict_div(n).strict_mul(n);
    loop {
        let v = u128::from(rng.next_u64());
        if v < accept {
            return v.strict_rem(n) as u64;
        }
    }
}

/// A point uniform in the cube `[0, side)³`, in whole world units.
fn uniform(rng: &mut ChaCha8Rng, side: i64) -> [i64; 3] {
    let side = side.max(1) as u64;
    [0, 0, 0].map(|_| below(rng, side) as i64)
}

fn dist2(a: [i64; 3], b: [i64; 3]) -> i128 {
    // Coordinates are below 10^9 units (the validated range), so differences, squares and the
    // sum of three fit easily (D-032).
    (0..3)
        .map(|i| sq(a[i].strict_sub(b[i])))
        .fold(0, i128::strict_add)
}

fn sq(x: i64) -> i128 {
    i128::from(x).strict_mul(i128::from(x))
}

fn cube(x: i64) -> i128 {
    i128::from(x)
        .strict_mul(i128::from(x))
        .strict_mul(i128::from(x))
}

#[cfg(test)]
mod tests;
