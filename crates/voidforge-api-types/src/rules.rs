//! `GET /rules` (P4.1r, D-058): the rules the universe runs, so a client can say what a choice
//! costs. The sections and field names are the rules file's own (`rules/v0.toml`), so a reader
//! of either knows the other; units are D-045's (grams, milliseconds, distance units per hour).
//! Worldgen is left out: it's a seed-time detail (the advisor's A1).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// `GET /rules`: the rules the running universe was loaded with. Public, and the same for every
/// player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RulesView {
    /// The rules version the universe was seeded with.
    pub version: u32,
    /// The rules' name, such as `v0` or `v0-fast` (the fast test rules).
    pub name: String,
    /// Every planet's slots and stock caps.
    pub planet: PlanetRules,
    /// What every player starts with on their homeworld.
    pub homeworld: HomeworldRules,
    /// Each building kind.
    pub buildings: BuildingRules,
    /// Each ship kind.
    pub ships: ShipRules,
    /// The score's weights.
    pub scoring: ScoringRules,
}

/// A planet's slots and caps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanetRules {
    /// Building slots per planet.
    pub slots: u32,
    /// The most Iron Ore a planet holds, in grams.
    pub ore_cap_g: i64,
    /// The most Iron Ingots a planet holds, in grams.
    pub ingot_cap_g: i64,
}

/// A homeworld at the start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HomeworldRules {
    /// Operational drills.
    pub drills: u32,
    /// Operational refineries.
    pub refineries: u32,
    /// Iron Ore, in grams.
    pub ore_g: i64,
    /// Iron Ingots, in grams.
    pub ingots_g: i64,
}

/// Each building kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildingRules {
    /// The drill.
    pub drill: DrillRules,
    /// The refinery.
    pub refinery: RefineryRules,
    /// The shipyard.
    pub shipyard: ShipyardRules,
}

/// A drill: mines Iron Ore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DrillRules {
    /// The ingots it costs to build, in grams.
    pub cost_g: i64,
    /// How long it takes to build, in milliseconds.
    pub build_time_ms: i64,
    /// How long it takes to demolish, in milliseconds.
    pub demolition_time_ms: i64,
    /// The ore it mines, in grams per hour.
    pub output_g_per_h: i64,
}

/// A refinery: turns Iron Ore into Iron Ingots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RefineryRules {
    /// The ingots it costs to build, in grams.
    pub cost_g: i64,
    /// How long it takes to build, in milliseconds.
    pub build_time_ms: i64,
    /// How long it takes to demolish, in milliseconds.
    pub demolition_time_ms: i64,
    /// The ore it takes, in grams per hour.
    pub intake_g_per_h: i64,
    /// Grams of ingots per gram of ore.
    pub ingots_per_ore: i64,
    /// The ingots it makes, in grams per hour.
    pub output_g_per_h: i64,
}

/// A shipyard: builds ships.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ShipyardRules {
    /// The ingots it costs to build, in grams.
    pub cost_g: i64,
    /// How long it takes to build, in milliseconds.
    pub build_time_ms: i64,
    /// How long it takes to demolish, in milliseconds.
    pub demolition_time_ms: i64,
    /// Ships it builds at once.
    pub parallel_builds: u32,
}

/// Each ship kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ShipRules {
    /// The colony ship.
    pub colony_ship: ShipKindRules,
    /// The cargo vessel.
    pub cargo_vessel: ShipKindRules,
}

/// A ship kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ShipKindRules {
    /// The ingots it costs to build, in grams.
    pub cost_g: i64,
    /// How long it takes to build, in milliseconds.
    pub build_time_ms: i64,
    /// Its speed, in distance units per hour.
    pub speed_du_per_h: i64,
    /// The cargo it carries, in grams.
    pub cargo_capacity_g: i64,
}

/// The score's weights: points per planet, building and ship owned, and per mass of stock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ScoringRules {
    /// Per planet.
    pub planet: i64,
    /// Per drill.
    pub drill: i64,
    /// Per refinery.
    pub refinery: i64,
    /// Per shipyard.
    pub shipyard: i64,
    /// Per colony ship.
    pub colony_ship: i64,
    /// Per cargo vessel.
    pub cargo_vessel: i64,
    /// For Iron Ore.
    pub ore: MassPointsRules,
    /// For Iron Ingots.
    pub ingots: MassPointsRules,
}

/// Points for a stock: `points` for every whole `per_g` grams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MassPointsRules {
    /// The points.
    pub points: i64,
    /// Per this many grams.
    pub per_g: i64,
}
