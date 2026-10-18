//! Rules files and a pure parser into the core's typed specs (decision log D-020).
//!
//! [`parse`] turns the text of a rules file (`rules/v0.toml`, `rules/v0-fast.toml`) into
//! [`voidforge_core::rules::Rules`] and validates it. It reads no files, clocks or network: the
//! caller supplies the text. Every field is required, unknown fields are rejected, and every
//! number must be an integer, so a typo or a float fails loudly instead of falling back to a
//! default.

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

use serde::Deserialize;
use voidforge_core::rules::{
    BuildCost, BuildingSpecs, DrillSpec, HomeworldSpec, MassPoints, PlanetSpec, RefinerySpec,
    Rules, RulesError, ScoringSpecs, ShipSpec, ShipSpecs, ShipyardSpec, WorldgenSpec,
};

/// Why a rules file was rejected.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Not valid TOML, or not the rules schema: a missing, unknown or mistyped field. The
    /// message names the field and the line.
    #[error("invalid rules file: {0}")]
    Schema(String),
    /// Well-formed, but a value breaks a rules invariant.
    #[error("invalid rules: {0}")]
    Invalid(#[from] RulesError),
}

/// Parses and validates the text of a rules file.
pub fn parse(text: &str) -> Result<Rules, Error> {
    let file: File = toml::from_str(text).map_err(|error| Error::Schema(error.to_string()))?;
    let rules = file.into_rules();
    rules.validate()?;
    Ok(rules)
}

// The file schema. It mirrors the core types field for field; keeping it separate lets the core
// stay free of serde until it needs serde for snapshots.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    name: String,
    planet: Planet,
    homeworld: Homeworld,
    buildings: Buildings,
    ships: Ships,
    scoring: Scoring,
    worldgen: Worldgen,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Planet {
    slots: u32,
    ore_cap_g: i64,
    ingot_cap_g: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Homeworld {
    drills: u32,
    refineries: u32,
    ore_g: i64,
    ingots_g: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Buildings {
    drill: Drill,
    refinery: Refinery,
    shipyard: Shipyard,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Drill {
    cost_g: i64,
    build_time_ms: i64,
    demolition_time_ms: i64,
    output_g_per_h: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Refinery {
    cost_g: i64,
    build_time_ms: i64,
    demolition_time_ms: i64,
    intake_g_per_h: i64,
    ingots_per_ore: i64,
    output_g_per_h: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Shipyard {
    cost_g: i64,
    build_time_ms: i64,
    demolition_time_ms: i64,
    parallel_builds: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ships {
    colony_ship: Ship,
    cargo_vessel: Ship,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ship {
    cost_g: i64,
    build_time_ms: i64,
    speed_du_per_h: i64,
    cargo_capacity_g: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scoring {
    planet: i64,
    drill: i64,
    refinery: i64,
    shipyard: i64,
    colony_ship: i64,
    cargo_vessel: i64,
    ore: Mass,
    ingots: Mass,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mass {
    points: i64,
    per_g: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Worldgen {
    planets_per_expected_player: u32,
    cube_size_du: i64,
    min_planet_spacing_du: i64,
    homeworld_min_spacing_du: i64,
    homeworld_neighbor_radius_du: i64,
    homeworld_min_neighbors: u32,
}

impl File {
    fn into_rules(self) -> Rules {
        let b = self.buildings;
        let s = self.ships;
        let ship = |ship: Ship| ShipSpec {
            build: BuildCost {
                cost_g: ship.cost_g,
                build_time_ms: ship.build_time_ms,
            },
            speed_du_per_h: ship.speed_du_per_h,
            cargo_capacity_g: ship.cargo_capacity_g,
        };
        let mass = |mass: Mass| MassPoints {
            points: mass.points,
            per_g: mass.per_g,
        };
        Rules {
            version: self.version,
            name: self.name,
            planet: PlanetSpec {
                slots: self.planet.slots,
                ore_cap_g: self.planet.ore_cap_g,
                ingot_cap_g: self.planet.ingot_cap_g,
            },
            homeworld: HomeworldSpec {
                drills: self.homeworld.drills,
                refineries: self.homeworld.refineries,
                ore_g: self.homeworld.ore_g,
                ingots_g: self.homeworld.ingots_g,
            },
            buildings: BuildingSpecs {
                drill: DrillSpec {
                    build: BuildCost {
                        cost_g: b.drill.cost_g,
                        build_time_ms: b.drill.build_time_ms,
                    },
                    demolition_time_ms: b.drill.demolition_time_ms,
                    output_g_per_h: b.drill.output_g_per_h,
                },
                refinery: RefinerySpec {
                    build: BuildCost {
                        cost_g: b.refinery.cost_g,
                        build_time_ms: b.refinery.build_time_ms,
                    },
                    demolition_time_ms: b.refinery.demolition_time_ms,
                    intake_g_per_h: b.refinery.intake_g_per_h,
                    ingots_per_ore: b.refinery.ingots_per_ore,
                    output_g_per_h: b.refinery.output_g_per_h,
                },
                shipyard: ShipyardSpec {
                    build: BuildCost {
                        cost_g: b.shipyard.cost_g,
                        build_time_ms: b.shipyard.build_time_ms,
                    },
                    demolition_time_ms: b.shipyard.demolition_time_ms,
                    parallel_builds: b.shipyard.parallel_builds,
                },
            },
            ships: ShipSpecs {
                colony_ship: ship(s.colony_ship),
                cargo_vessel: ship(s.cargo_vessel),
            },
            scoring: ScoringSpecs {
                planet: self.scoring.planet,
                drill: self.scoring.drill,
                refinery: self.scoring.refinery,
                shipyard: self.scoring.shipyard,
                colony_ship: self.scoring.colony_ship,
                cargo_vessel: self.scoring.cargo_vessel,
                ore: mass(self.scoring.ore),
                ingots: mass(self.scoring.ingots),
            },
            worldgen: WorldgenSpec {
                planets_per_expected_player: self.worldgen.planets_per_expected_player,
                cube_size_du: self.worldgen.cube_size_du,
                min_planet_spacing_du: self.worldgen.min_planet_spacing_du,
                homeworld_min_spacing_du: self.worldgen.homeworld_min_spacing_du,
                homeworld_neighbor_radius_du: self.worldgen.homeworld_neighbor_radius_du,
                homeworld_min_neighbors: self.worldgen.homeworld_min_neighbors,
            },
        }
    }
}

#[cfg(test)]
mod tests;
