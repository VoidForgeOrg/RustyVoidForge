//! The values every message shares: IDs, times, positions, the enums and cargo (D-045, Q1).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

macro_rules! entity_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        ///
        /// A JSON integer. Entity IDs come from one engine counter, so leaving JavaScript's safe
        /// range (2^53) would take about 9×10^15 entities (D-045).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
        #[serde(transparent)]
        #[schema(value_type = u64)]
        pub struct $name(pub u64);
    };
}

entity_id!(
    /// A planet.
    PlanetId
);
entity_id!(
    /// A building.
    BuildingId
);
entity_id!(
    /// A ship.
    ShipId
);
entity_id!(
    /// A ship build in a Shipyard.
    ShipBuildId
);
entity_id!(
    /// A fleet.
    FleetId
);

/// A player: the UUIDv7 the accounts table assigns (Decision 9), hyphenated lowercase. The
/// server converts it; this crate only carries the text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
#[schema(
    value_type = String,
    format = "uuid",
    pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
    example = "01928c5e-7a3b-7c2d-9e4f-5a6b7c8d9e0f"
)]
pub struct PlayerId(pub String);

/// A game time: the universe epoch plus game time, as RFC 3339 UTC with milliseconds, always
/// (`2026-10-01T21:00:00.000Z`). Game time is in milliseconds, so a time cut to seconds would be
/// off by up to 999 ms. The server formats it (P3.4, with D-044's stable epoch).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
#[schema(
    value_type = String,
    format = DateTime,
    pattern = "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\\.[0-9]{3}Z$",
    example = "2026-10-01T21:00:00.000Z"
)]
pub struct Timestamp(pub String);

/// A point in the universe, in distance units (D-021: 1,000 per world unit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Position {
    /// X, in distance units.
    pub x: i64,
    /// Y, in distance units.
    pub y: i64,
    /// Z, in distance units.
    pub z: i64,
}

/// What a building is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BuildingType {
    /// Mines iron ore.
    Drill,
    /// Turns iron ore into iron ingots.
    Refinery,
    /// Builds ships.
    Shipyard,
}

/// What a ship is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShipType {
    /// Colonizes an unowned planet.
    ColonyShip,
    /// Carries cargo.
    CargoVessel,
}

/// What a fleet does at its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mission {
    /// Colonize an unowned planet (needs a Colony Ship).
    Colonize,
    /// Carry cargo to one of the player's planets (needs a Cargo Vessel).
    Transport,
    /// Move to another planet.
    Move,
}

/// Grams of each resource, loaded or carried.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Cargo {
    /// Grams of iron ore.
    pub iron_ore_g: i64,
    /// Grams of iron ingots.
    pub iron_ingot_g: i64,
}
