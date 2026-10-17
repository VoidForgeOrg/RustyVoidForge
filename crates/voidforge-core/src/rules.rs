//! Typed rules: the game's numbers as data (Architectural Design, State model: "Rules are data
//! in one versioned table"; decision log D-020).
//!
//! Every mass is in grams, every duration in milliseconds, every rate in grams per hour, and
//! every distance in distance units, fixed-point thousandths of a world unit (speeds in distance
//! units per hour). `voidforge-rules` parses rules
//! files into these types and calls [`Rules::validate`]; code that builds rules by hand, such as
//! tests, should validate them too.

use crate::quantity::MS_PER_HOUR;

/// Grams per metric ton; the game design writes masses in tons.
pub const GRAMS_PER_TON: i64 = 1_000_000;

/// Distance units per world unit: coordinates, distances and speeds are fixed-point with three
/// decimals (decision log D-021). The game design's "10 units per hour" is 10,000 per hour here.
pub const DISTANCE_UNITS_PER_WORLD_UNIT: i64 = 1_000;

/// One complete, versioned rule set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    /// Identifies the rule set in log records and snapshots; replay refuses a mismatch.
    pub version: u32,
    /// A human-readable name such as `v0` or `v0-fast`.
    pub name: String,
    /// Values every planet has.
    pub planet: PlanetSpec,
    /// What a new player's homeworld starts with.
    pub homeworld: HomeworldSpec,
    /// Drill, Refinery and Shipyard.
    pub buildings: BuildingSpecs,
    /// Colony Ship and Cargo Vessel.
    pub ships: ShipSpecs,
    /// Points for the leaderboard score.
    pub scoring: ScoringSpecs,
    /// Parameters for world generation (Technical Plan, P1.12 defaults).
    pub worldgen: WorldgenSpec,
}

/// Values every planet has (game design, Universe and planets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanetSpec {
    /// Identical building slots.
    pub slots: u32,
    /// Iron Ore storage capacity.
    pub ore_cap_g: i64,
    /// Iron Ingot storage capacity.
    pub ingot_cap_g: i64,
}

/// A new homeworld: these operational buildings and starting stocks (game design, Starting
/// conditions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HomeworldSpec {
    /// Operational Drills.
    pub drills: u32,
    /// Operational Refineries.
    pub refineries: u32,
    /// Iron Ore in storage.
    pub ore_g: i64,
    /// Iron Ingots in storage.
    pub ingots_g: i64,
}

/// A total ingot cost drawn evenly over a build time (game design, Construction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildCost {
    /// Total Iron Ingots consumed.
    pub cost_g: i64,
    /// Build duration.
    pub build_time_ms: i64,
}

impl BuildCost {
    /// The rate at which a build draws ingots: its cost spread evenly over its build time.
    /// `None` unless that is an exact whole number of grams per hour within 64 bits, so a build
    /// fed at this rate takes exactly its build time. Validated rules always have one.
    pub fn rate_g_per_h(&self) -> Option<i64> {
        if self.build_time_ms <= 0 {
            return None;
        }
        let scaled = i128::from(self.cost_g) * i128::from(MS_PER_HOUR);
        let time = i128::from(self.build_time_ms);
        if scaled % time != 0 {
            return None;
        }
        i64::try_from(scaled / time).ok()
    }
}

/// The three buildings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildingSpecs {
    /// Extracts Iron Ore.
    pub drill: DrillSpec,
    /// Turns Iron Ore into Iron Ingots.
    pub refinery: RefinerySpec,
    /// Builds ships.
    pub shipyard: ShipyardSpec,
}

/// A Drill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrillSpec {
    /// Construction cost and time.
    pub build: BuildCost,
    /// Time from starting demolition until the slot frees.
    pub demolition_time_ms: i64,
    /// Ore extracted at the rated rate.
    pub output_g_per_h: i64,
}

/// A Refinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefinerySpec {
    /// Construction cost and time.
    pub build: BuildCost,
    /// Time from starting demolition until the slot frees.
    pub demolition_time_ms: i64,
    /// Ore taken in at the rated rate.
    pub intake_g_per_h: i64,
    /// Grams of ingots made from each gram of ore (game design: 1 t of ore gives 2 t).
    pub ingots_per_ore: i64,
    /// Ingots made at the rated rate: intake times `ingots_per_ore`.
    pub output_g_per_h: i64,
}

/// A Shipyard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipyardSpec {
    /// Construction cost and time.
    pub build: BuildCost,
    /// Time from starting demolition until the slot frees.
    pub demolition_time_ms: i64,
    /// Ships built at once; more wait in the queue.
    pub parallel_builds: u32,
}

/// The two ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipSpecs {
    /// Colonizes an unowned planet; consumed on use.
    pub colony_ship: ShipSpec,
    /// Carries cargo.
    pub cargo_vessel: ShipSpec,
}

/// A ship type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipSpec {
    /// Shipyard cost and time.
    pub build: BuildCost,
    /// Travel speed in distance units per hour; a fleet moves at its slowest ship's speed.
    pub speed_du_per_h: i64,
    /// Cargo capacity; zero for ships that carry none.
    pub cargo_capacity_g: i64,
}

/// Score points (game design, Scoring and Rules v0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoringSpecs {
    /// Per colonized planet.
    pub planet: i64,
    /// Per finished Drill.
    pub drill: i64,
    /// Per finished Refinery.
    pub refinery: i64,
    /// Per finished Shipyard.
    pub shipyard: i64,
    /// Per finished Colony Ship.
    pub colony_ship: i64,
    /// Per finished Cargo Vessel.
    pub cargo_vessel: i64,
    /// Iron Ore in storage or cargo.
    pub ore: MassPoints,
    /// Iron Ingots in storage or cargo.
    pub ingots: MassPoints,
}

/// `points` for every `per_g` grams of a resource, such as 1 point per 10 t.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MassPoints {
    /// Points per `per_g` grams.
    pub points: i64,
    /// The mass that earns `points`.
    pub per_g: i64,
}

/// World generation parameters (Technical Plan, P1.12 defaults, stored as rules data).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldgenSpec {
    /// Planets generated per expected player.
    pub planets_per_expected_player: u32,
    /// Edge of the cube the planets are spread through, in distance units.
    pub cube_size_du: i64,
    /// Minimum distance between any two planets, in distance units.
    pub min_planet_spacing_du: i64,
    /// Minimum distance between homeworld candidates, in distance units.
    pub homeworld_min_spacing_du: i64,
    /// Radius within which a homeworld candidate needs neighbors, in distance units.
    pub homeworld_neighbor_radius_du: i64,
    /// Planets a homeworld candidate needs within that radius.
    pub homeworld_min_neighbors: u32,
}

/// A rules value that breaks an invariant, naming the field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("rules field `{field}`: {problem}")]
pub struct RulesError {
    /// The field's path, as in the rules file, for example `buildings.drill.cost_g`.
    pub field: &'static str,
    /// What is wrong with it.
    pub problem: String,
}

impl Rules {
    /// Checks every invariant the engine relies on. Fails on the first broken one.
    pub fn validate(&self) -> Result<(), RulesError> {
        if self.name.trim().is_empty() {
            return Err(problem("name", "must not be empty"));
        }
        let planet = &self.planet;
        positive("planet.slots", i64::from(planet.slots))?;
        positive("planet.ore_cap_g", planet.ore_cap_g)?;
        positive("planet.ingot_cap_g", planet.ingot_cap_g)?;

        let home = &self.homeworld;
        let buildings = i64::from(home.drills) + i64::from(home.refineries);
        if buildings > i64::from(planet.slots) {
            return Err(problem(
                "homeworld",
                format!(
                    "{buildings} starting buildings do not fit in {} slots",
                    planet.slots
                ),
            ));
        }
        within(
            "homeworld.ore_g",
            home.ore_g,
            planet.ore_cap_g,
            "planet.ore_cap_g",
        )?;
        within(
            "homeworld.ingots_g",
            home.ingots_g,
            planet.ingot_cap_g,
            "planet.ingot_cap_g",
        )?;

        let drill = &self.buildings.drill;
        build_cost(
            "buildings.drill.cost_g",
            "buildings.drill.build_time_ms",
            drill.build,
        )?;
        positive(
            "buildings.drill.demolition_time_ms",
            drill.demolition_time_ms,
        )?;
        positive("buildings.drill.output_g_per_h", drill.output_g_per_h)?;

        let refinery = &self.buildings.refinery;
        build_cost(
            "buildings.refinery.cost_g",
            "buildings.refinery.build_time_ms",
            refinery.build,
        )?;
        positive(
            "buildings.refinery.demolition_time_ms",
            refinery.demolition_time_ms,
        )?;
        positive("buildings.refinery.intake_g_per_h", refinery.intake_g_per_h)?;
        positive("buildings.refinery.ingots_per_ore", refinery.ingots_per_ore)?;
        let expected = i128::from(refinery.intake_g_per_h) * i128::from(refinery.ingots_per_ore);
        if i128::from(refinery.output_g_per_h) != expected {
            return Err(problem(
                "buildings.refinery.output_g_per_h",
                format!(
                    "must be intake_g_per_h times ingots_per_ore ({} x {} = {expected}), got {}",
                    refinery.intake_g_per_h, refinery.ingots_per_ore, refinery.output_g_per_h
                ),
            ));
        }

        let shipyard = &self.buildings.shipyard;
        build_cost(
            "buildings.shipyard.cost_g",
            "buildings.shipyard.build_time_ms",
            shipyard.build,
        )?;
        positive(
            "buildings.shipyard.demolition_time_ms",
            shipyard.demolition_time_ms,
        )?;
        positive(
            "buildings.shipyard.parallel_builds",
            i64::from(shipyard.parallel_builds),
        )?;

        let colony = &self.ships.colony_ship;
        build_cost(
            "ships.colony_ship.cost_g",
            "ships.colony_ship.build_time_ms",
            colony.build,
        )?;
        positive("ships.colony_ship.speed_du_per_h", colony.speed_du_per_h)?;
        // A Colony Ship carries nothing: fleet capacity is the plain sum of every ship's
        // capacity, so a nonzero value here would let colony fleets carry cargo (D-027).
        if colony.cargo_capacity_g != 0 {
            return Err(problem(
                "ships.colony_ship.cargo_capacity_g",
                format!("must be 0, got {}", colony.cargo_capacity_g),
            ));
        }
        let cargo = &self.ships.cargo_vessel;
        build_cost(
            "ships.cargo_vessel.cost_g",
            "ships.cargo_vessel.build_time_ms",
            cargo.build,
        )?;
        positive("ships.cargo_vessel.speed_du_per_h", cargo.speed_du_per_h)?;
        positive(
            "ships.cargo_vessel.cargo_capacity_g",
            cargo.cargo_capacity_g,
        )?;

        // Every ingot demand must divide by the refining ratio, so a full ingot stock is held
        // exactly at its cap (decision log D-025).
        for (field, build) in [
            ("buildings.drill.cost_g", drill.build),
            ("buildings.refinery.cost_g", refinery.build),
            ("buildings.shipyard.cost_g", shipyard.build),
            ("ships.colony_ship.cost_g", colony.build),
            ("ships.cargo_vessel.cost_g", cargo.build),
        ] {
            let rate = build.rate_g_per_h().unwrap_or_default();
            if rate % refinery.ingots_per_ore != 0 {
                return Err(problem(
                    field,
                    format!(
                        "the build's rate of {rate} g/h is not a multiple of ingots_per_ore ({})",
                        refinery.ingots_per_ore
                    ),
                ));
            }
        }

        let score = &self.scoring;
        not_negative("scoring.planet", score.planet)?;
        not_negative("scoring.drill", score.drill)?;
        not_negative("scoring.refinery", score.refinery)?;
        not_negative("scoring.shipyard", score.shipyard)?;
        not_negative("scoring.colony_ship", score.colony_ship)?;
        not_negative("scoring.cargo_vessel", score.cargo_vessel)?;
        not_negative("scoring.ore.points", score.ore.points)?;
        positive("scoring.ore.per_g", score.ore.per_g)?;
        not_negative("scoring.ingots.points", score.ingots.points)?;
        positive("scoring.ingots.per_g", score.ingots.per_g)?;

        let world = &self.worldgen;
        positive(
            "worldgen.planets_per_expected_player",
            i64::from(world.planets_per_expected_player),
        )?;
        positive("worldgen.cube_size_du", world.cube_size_du)?;
        positive(
            "worldgen.min_planet_spacing_du",
            world.min_planet_spacing_du,
        )?;
        if world.min_planet_spacing_du >= world.cube_size_du {
            return Err(problem(
                "worldgen.min_planet_spacing_du",
                format!("must be less than cube_size_du ({})", world.cube_size_du),
            ));
        }
        positive(
            "worldgen.homeworld_min_spacing_du",
            world.homeworld_min_spacing_du,
        )?;
        positive(
            "worldgen.homeworld_neighbor_radius_du",
            world.homeworld_neighbor_radius_du,
        )?;
        // Feasibility that does not depend on the player count (D-031); generation checks the
        // rest. World generation places planets on whole world units (D-021).
        for (field, value) in [
            ("worldgen.cube_size_du", world.cube_size_du),
            (
                "worldgen.min_planet_spacing_du",
                world.min_planet_spacing_du,
            ),
            (
                "worldgen.homeworld_min_spacing_du",
                world.homeworld_min_spacing_du,
            ),
            (
                "worldgen.homeworld_neighbor_radius_du",
                world.homeworld_neighbor_radius_du,
            ),
        ] {
            if value % DISTANCE_UNITS_PER_WORLD_UNIT != 0 {
                return Err(problem(
                    field,
                    format!(
                        "must be whole world units (a multiple of {DISTANCE_UNITS_PER_WORLD_UNIT}), got {value}"
                    ),
                ));
            }
        }
        let (side, spacing) = (
            i128::from(world.cube_size_du),
            i128::from(world.min_planet_spacing_du),
        );
        let (home, radius) = (
            i128::from(world.homeworld_min_spacing_du),
            i128::from(world.homeworld_neighbor_radius_du),
        );
        if home < spacing {
            return Err(problem(
                "worldgen.homeworld_min_spacing_du",
                format!("must be at least min_planet_spacing_du ({spacing})"),
            ));
        }
        if home * home > 3 * side * side {
            return Err(problem(
                "worldgen.homeworld_min_spacing_du",
                format!("must not exceed the cube's diagonal (side {side})"),
            ));
        }
        let neighbors = i128::from(world.homeworld_min_neighbors);
        if neighbors > 0 {
            if radius < spacing {
                return Err(problem(
                    "worldgen.homeworld_neighbor_radius_du",
                    format!(
                        "must be at least min_planet_spacing_du ({spacing}) when neighbors are required"
                    ),
                ));
            }
            // Necessary packing bound: the candidate and its neighbors, pairwise `spacing` apart,
            // fit in a ball of `radius` only if (n + 1) × s³ ≤ (2r + s)³.
            let reach = 2 * radius + spacing;
            if (neighbors + 1) * spacing * spacing * spacing > reach * reach * reach {
                return Err(problem(
                    "worldgen.homeworld_min_neighbors",
                    format!(
                        "{neighbors} neighbors cannot keep min_planet_spacing_du within homeworld_neighbor_radius_du"
                    ),
                ));
            }
        }
        if i128::from(world.planets_per_expected_player) < 1 + neighbors {
            return Err(problem(
                "worldgen.planets_per_expected_player",
                format!(
                    "must be at least 1 + homeworld_min_neighbors ({})",
                    1 + neighbors
                ),
            ));
        }
        Ok(())
    }
}

/// A valid rules set for core tests, which cannot read rules files. Its numbers are round test
/// values, not Rules v0; tests of the real files live in voidforge-rules and testkit.
#[cfg(test)]
pub(crate) fn test_rules() -> Rules {
    let build = |tons: i64, hours: i64| BuildCost {
        cost_g: tons * GRAMS_PER_TON,
        build_time_ms: hours * MS_PER_HOUR,
    };
    Rules {
        version: 0,
        name: "core test rules".to_string(),
        planet: PlanetSpec {
            slots: 8,
            ore_cap_g: 10_000 * GRAMS_PER_TON,
            ingot_cap_g: 5_000 * GRAMS_PER_TON,
        },
        homeworld: HomeworldSpec {
            drills: 1,
            refineries: 1,
            ore_g: 2_000 * GRAMS_PER_TON,
            ingots_g: 1_500 * GRAMS_PER_TON,
        },
        buildings: BuildingSpecs {
            drill: DrillSpec {
                build: build(150, 3),
                demolition_time_ms: MS_PER_HOUR,
                output_g_per_h: 10 * GRAMS_PER_TON,
            },
            refinery: RefinerySpec {
                build: build(240, 4),
                demolition_time_ms: MS_PER_HOUR,
                intake_g_per_h: 8 * GRAMS_PER_TON,
                ingots_per_ore: 2,
                output_g_per_h: 16 * GRAMS_PER_TON,
            },
            shipyard: ShipyardSpec {
                build: build(1_200, 12),
                demolition_time_ms: 2 * MS_PER_HOUR,
                parallel_builds: 3,
            },
        },
        ships: ShipSpecs {
            colony_ship: ShipSpec {
                build: build(800, 10),
                speed_du_per_h: 10 * DISTANCE_UNITS_PER_WORLD_UNIT,
                cargo_capacity_g: 0,
            },
            cargo_vessel: ShipSpec {
                build: build(300, 4),
                speed_du_per_h: 15 * DISTANCE_UNITS_PER_WORLD_UNIT,
                cargo_capacity_g: 500 * GRAMS_PER_TON,
            },
        },
        scoring: ScoringSpecs {
            planet: 1_000,
            drill: 100,
            refinery: 150,
            shipyard: 500,
            colony_ship: 400,
            cargo_vessel: 150,
            ore: MassPoints {
                points: 1,
                per_g: 10 * GRAMS_PER_TON,
            },
            ingots: MassPoints {
                points: 2,
                per_g: 10 * GRAMS_PER_TON,
            },
        },
        worldgen: WorldgenSpec {
            planets_per_expected_player: 20,
            cube_size_du: 1_000 * DISTANCE_UNITS_PER_WORLD_UNIT,
            min_planet_spacing_du: 10 * DISTANCE_UNITS_PER_WORLD_UNIT,
            homeworld_min_spacing_du: 150 * DISTANCE_UNITS_PER_WORLD_UNIT,
            homeworld_neighbor_radius_du: 100 * DISTANCE_UNITS_PER_WORLD_UNIT,
            homeworld_min_neighbors: 5,
        },
    }
}

fn problem(field: &'static str, problem: impl Into<String>) -> RulesError {
    RulesError {
        field,
        problem: problem.into(),
    }
}

fn positive(field: &'static str, value: i64) -> Result<(), RulesError> {
    if value > 0 {
        Ok(())
    } else {
        Err(problem(field, format!("must be positive, got {value}")))
    }
}

fn not_negative(field: &'static str, value: i64) -> Result<(), RulesError> {
    if value >= 0 {
        Ok(())
    } else {
        Err(problem(field, format!("must not be negative, got {value}")))
    }
}

fn within(field: &'static str, value: i64, cap: i64, cap_field: &str) -> Result<(), RulesError> {
    not_negative(field, value)?;
    if value > cap {
        return Err(problem(
            field,
            format!("{value} exceeds {cap_field} ({cap})"),
        ));
    }
    Ok(())
}

/// A positive cost over a positive time whose even rate is an exact whole number of grams per
/// hour, so a build takes exactly its build time.
fn build_cost(
    cost_field: &'static str,
    time_field: &'static str,
    build: BuildCost,
) -> Result<(), RulesError> {
    positive(cost_field, build.cost_g)?;
    positive(time_field, build.build_time_ms)?;
    let scaled = i128::from(build.cost_g) * i128::from(MS_PER_HOUR);
    let time = i128::from(build.build_time_ms);
    if build.rate_g_per_h().is_some() {
        return Ok(());
    }
    if scaled % time != 0 {
        return Err(problem(
            cost_field,
            format!(
                "{} g over {} ms is not a whole number of grams per hour; the build would not take exactly its build time",
                build.cost_g, build.build_time_ms
            ),
        ));
    }
    Err(problem(
        cost_field,
        "the build rate does not fit in 64 bits",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_core_test_rules_are_valid() {
        test_rules().validate().unwrap();
    }

    #[test]
    fn a_build_rate_exists_only_when_exact_and_in_range() {
        let drill = BuildCost {
            cost_g: 150 * GRAMS_PER_TON,
            build_time_ms: 3 * MS_PER_HOUR,
        };
        assert_eq!(drill.rate_g_per_h(), Some(50 * GRAMS_PER_TON));
        assert_eq!(
            BuildCost {
                cost_g: 1,
                build_time_ms: 7
            }
            .rate_g_per_h(),
            None
        );
        assert_eq!(
            BuildCost {
                cost_g: 1,
                build_time_ms: 0
            }
            .rate_g_per_h(),
            None
        );
        assert_eq!(
            BuildCost {
                cost_g: i64::MAX,
                build_time_ms: 1
            }
            .rate_g_per_h(),
            None
        );
    }
}
