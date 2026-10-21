//! The read side: planets, planets near a point, the player, the player's fleets and the
//! leaderboard (D-045, Q4). Every view has `as_of` and `valid_until` (null when nothing is
//! scheduled), every continuous value its `rate_g_per_h`, and every stock its `cap_g`
//! (Architectural Design, Read side and API).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::wire::{
    BuildingId, BuildingType, Cargo, FleetId, Mission, PlanetId, PlayerId, Position, ShipBuildId,
    ShipId, ShipType, Timestamp,
};

/// One resource in a planet's storage, valid from `as_of` until `valid_until`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Stock {
    /// Grams in storage at `as_of`.
    pub amount_g: i64,
    /// Its rate until `valid_until`, in grams per hour; negative while it falls.
    pub rate_g_per_h: i64,
    /// The storage cap, in grams.
    pub cap_g: i64,
}

/// A planet's storage, per resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Stocks {
    /// Iron ore.
    pub iron_ore: Stock,
    /// Iron ingots.
    pub iron_ingot: Stock,
}

/// How a working building or a build is doing: running at its rated speed, throttled by its
/// supply, or halted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FlowStatus {
    /// At its rated speed.
    Running,
    /// Slowed by its supply.
    Throttled,
    /// Stopped.
    Halted,
}

/// A building on a planet, tagged by its `state`. Each state lists every field it has and no
/// other, so a misspelled field or another state's field is refused by serde and by the schema
/// alike (repair round 1, F1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum BuildingView {
    /// Receiving ingots until it reaches its cost.
    UnderConstruction {
        /// The building.
        id: BuildingId,
        /// What it is.
        #[serde(rename = "type")]
        building_type: BuildingType,
        /// How its flow is doing; absent when it has none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<FlowStatus>,
        /// Grams of ingots delivered at `as_of`.
        delivered_g: i64,
        /// Grams it costs.
        cost_g: i64,
        /// Grams per hour being delivered until `valid_until`.
        rate_g_per_h: i64,
        /// When it completes at the current rate, exactly; null while nothing flows.
        #[schema(required = true)]
        #[serde(deserialize_with = "Option::deserialize")]
        eta: Option<Timestamp>,
    },
    /// Working.
    Operational {
        /// The building.
        id: BuildingId,
        /// What it is.
        #[serde(rename = "type")]
        building_type: BuildingType,
        /// How its flow is doing; absent when it has none. An operational Shipyard with no active
        /// builds is `halted` (D-025): it's idle, not short of ingots.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<FlowStatus>,
    },
    /// Stopped and being demolished; the slot frees at `demolition_ends_at`.
    Demolishing {
        /// The building.
        id: BuildingId,
        /// What it is.
        #[serde(rename = "type")]
        building_type: BuildingType,
        /// How its flow is doing; absent when it has none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<FlowStatus>,
        /// When demolition ends.
        demolition_ends_at: Timestamp,
    },
}

/// Whether a ship build is being built or waiting its turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShipBuildState {
    /// One of the Shipyard's (at most three) active builds.
    Active,
    /// Waiting, in order.
    Waiting,
}

/// A ship build in one of the planet's Shipyards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ShipBuildView {
    /// The build.
    pub id: ShipBuildId,
    /// The Shipyard.
    pub shipyard_id: BuildingId,
    /// What it builds.
    pub ship_type: ShipType,
    /// Active or waiting.
    pub state: ShipBuildState,
    /// How its flow is doing; absent when it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<FlowStatus>,
    /// Grams of ingots delivered at `as_of`.
    pub delivered_g: i64,
    /// Grams it costs.
    pub cost_g: i64,
    /// Grams per hour being delivered until `valid_until`.
    pub rate_g_per_h: i64,
    /// When it completes at the current rate, exactly; null while nothing flows.
    #[schema(required = true)]
    #[serde(deserialize_with = "Option::deserialize")]
    pub eta: Option<Timestamp>,
}

/// A ship on a planet's roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RosterShip {
    /// The ship.
    pub ship_id: ShipId,
    /// What it is.
    pub ship_type: ShipType,
    /// Whose it is.
    pub owner: PlayerId,
}

/// `GET /planets/{id}`: one planet, public to every player, projected to now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanetView {
    /// The planet.
    pub planet_id: PlanetId,
    /// Its owner; null while unowned.
    #[schema(required = true)]
    #[serde(deserialize_with = "Option::deserialize")]
    pub owner: Option<PlayerId>,
    /// When the view is computed for.
    pub as_of: Timestamp,
    /// When it next changes (the planet's next event); null when nothing is scheduled.
    #[schema(required = true)]
    #[serde(deserialize_with = "Option::deserialize")]
    pub valid_until: Option<Timestamp>,
    /// Its storage.
    pub stocks: Stocks,
    /// Its buildings.
    pub buildings: Vec<BuildingView>,
    /// Where it is.
    pub position: Position,
    /// How many buildings it can hold.
    pub slots: u32,
    /// Its Shipyards' builds.
    pub ship_builds: Vec<ShipBuildView>,
    /// The ships stationed on its roster.
    pub roster: Vec<RosterShip>,
}

/// A planet in a `GET /planets?near=` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanetSummary {
    /// The planet.
    pub planet_id: PlanetId,
    /// Where it is.
    pub position: Position,
    /// Its owner; null while unowned.
    #[schema(required = true)]
    #[serde(deserialize_with = "Option::deserialize")]
    pub owner: Option<PlayerId>,
    /// Its distance from the query's point, in distance units.
    pub distance: i64,
}

/// `GET /planets?near=x,y,z&radius=r`: the planets within a radius of a point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanetsNearView {
    /// When the view is computed for.
    pub as_of: Timestamp,
    /// When it next changes; null when nothing is scheduled.
    #[schema(required = true)]
    #[serde(deserialize_with = "Option::deserialize")]
    pub valid_until: Option<Timestamp>,
    /// The planets.
    pub planets: Vec<PlanetSummary>,
}

/// `GET /players/me`: the player's homeworld, planets and score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PlayerView {
    /// The player.
    pub player_id: PlayerId,
    /// The planet the player started on.
    pub homeworld_id: PlanetId,
    /// Every planet the player owns.
    pub planet_ids: Vec<PlanetId>,
    /// The score at `as_of`.
    pub score: i64,
    /// When the view is computed for.
    pub as_of: Timestamp,
    /// When it next changes; null when nothing is scheduled.
    #[schema(required = true)]
    #[serde(deserialize_with = "Option::deserialize")]
    pub valid_until: Option<Timestamp>,
}

/// A ship in a fleet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FleetShip {
    /// The ship.
    pub ship_id: ShipId,
    /// What it is.
    pub ship_type: ShipType,
}

/// One of the player's fleets, tagged by its `state`. Each state lists every field it has and no
/// other, so a misspelled field or another state's field is refused by serde and by the schema
/// alike (repair round 1, F1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum FleetView {
    /// At a planet.
    Stationed {
        /// The fleet.
        fleet_id: FleetId,
        /// Its ships.
        ships: Vec<FleetShip>,
        /// What it carries.
        cargo: Cargo,
        /// How many grams it can carry.
        capacity_g: i64,
        /// Its speed, in distance units per hour.
        speed_du_per_h: i64,
        /// The planet.
        at_planet_id: PlanetId,
    },
    /// Flying a mission.
    InTransit {
        /// The fleet.
        fleet_id: FleetId,
        /// Its ships.
        ships: Vec<FleetShip>,
        /// What it carries.
        cargo: Cargo,
        /// How many grams it can carry.
        capacity_g: i64,
        /// Its speed, in distance units per hour.
        speed_du_per_h: i64,
        /// Where it left from.
        origin_planet_id: PlanetId,
        /// Where it goes.
        target_planet_id: PlanetId,
        /// What it will do there.
        mission: Mission,
        /// When it left.
        departed_at: Timestamp,
        /// When it arrives.
        arrives_at: Timestamp,
    },
    /// Recalled, flying back.
    Returning {
        /// The fleet.
        fleet_id: FleetId,
        /// Its ships.
        ships: Vec<FleetShip>,
        /// What it carries.
        cargo: Cargo,
        /// How many grams it can carry.
        capacity_g: i64,
        /// Its speed, in distance units per hour.
        speed_du_per_h: i64,
        /// Where it returns to.
        origin_planet_id: PlanetId,
        /// When it gets back.
        arrives_at: Timestamp,
    },
}

/// `GET /players/me/fleets`: the player's own fleets. Fleets are visible only to their owner
/// (D-030); the server takes the viewer from the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FleetsView {
    /// When the view is computed for.
    pub as_of: Timestamp,
    /// When it next changes; null when nothing is scheduled.
    #[schema(required = true)]
    #[serde(deserialize_with = "Option::deserialize")]
    pub valid_until: Option<Timestamp>,
    /// The fleets.
    pub fleets: Vec<FleetView>,
}

/// A leaderboard row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaderboardEntry {
    /// Competition ranking: 1 plus the number of players with a strictly higher score, so ties
    /// share a rank.
    pub rank: u32,
    /// The player.
    pub player_id: PlayerId,
    /// The score at the leaderboard's `as_of`.
    pub score: i64,
}

/// `GET /leaderboard`: every player ranked by score, rebuilt every 60 seconds (P3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Leaderboard {
    /// When it was rebuilt.
    pub as_of: Timestamp,
    /// The rows, by score descending, then player ID ascending.
    pub entries: Vec<LeaderboardEntry>,
}
