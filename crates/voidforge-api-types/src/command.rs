//! `POST /commands`: the request body, the accepted response and the idempotency key (D-045,
//! Q2 and Q3).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ErrorCode;
use crate::event::Event;
use crate::wire::{
    BuildingId, BuildingType, Cargo, FleetId, Mission, PlanetId, ShipBuildId, ShipId, ShipType,
    Timestamp,
};

/// A command: one of the ten player actions, as `{"type": ..., "payload": {...}}`. It never
/// names the player, who comes from the auth token. Join game isn't here: the server issues it
/// on a player's first call (Decision 9), so `"type": "join_game"` is a 400 `malformed_body`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CommandRequest {
    /// Start a building on one of the player's planets.
    BuildBuilding(BuildBuilding),
    /// Remove a building under construction; delivered ingots are lost.
    CancelConstruction(BuildingRef),
    /// Start demolishing an operational building.
    DemolishBuilding(BuildingRef),
    /// Queue a ship at an operational Shipyard.
    QueueShip(QueueShip),
    /// Remove an active or waiting ship build; nothing is refunded.
    CancelShipBuild(CancelShipBuild),
    /// Form a fleet from ships on one planet's roster, optionally loading cargo.
    AssembleFleet(AssembleFleet),
    /// Send a stationed fleet on a mission.
    SendFleet(SendFleet),
    /// Turn a fleet in transit around, once.
    RecallFleet(FleetRef),
    /// Return a stationed, empty fleet's ships to the planet's roster.
    DisbandFleet(FleetRef),
    /// Unload a stationed fleet's cargo into its planet's storage, up to free storage.
    TriggerUnload(FleetRef),
}

/// The `build_building` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildBuilding {
    /// The planet.
    pub planet_id: PlanetId,
    /// What to build.
    pub building_type: BuildingType,
}

/// A payload naming one building.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildingRef {
    /// The building.
    pub building_id: BuildingId,
}

/// The `queue_ship` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct QueueShip {
    /// The Shipyard.
    pub shipyard_id: BuildingId,
    /// What to build.
    pub ship_type: ShipType,
}

/// The `cancel_ship_build` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelShipBuild {
    /// The build.
    pub ship_build_id: ShipBuildId,
}

/// The `assemble_fleet` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AssembleFleet {
    /// The planet whose roster the ships are on.
    pub planet_id: PlanetId,
    /// The ships, each once. A list, not a set, so a duplicate is refused at the API's edge
    /// (400 `duplicate_ship_ids`, D-029 Q1) rather than collapsing silently in the core. An
    /// empty list reaches the core, which answers 422 `no_ships`.
    #[schema(schema_with = unique_ship_ids)]
    pub ship_ids: Vec<ShipId>,
    /// Grams of each resource to load; zero for none. A negative amount reaches the core, which
    /// answers 422 `invalid_cargo`.
    pub cargo: Cargo,
}

/// `ship_ids`'s schema: an array of ship IDs, each at most once (`uniqueItems`).
fn unique_ship_ids() -> utoipa::openapi::Array {
    utoipa::openapi::ArrayBuilder::new()
        .items(utoipa::openapi::Ref::from_schema_name("ShipId"))
        .unique_items(true)
        .description(Some("The ships, each once"))
        .build()
}

impl AssembleFleet {
    /// Checks the ship list for duplicates, as the API does before the core sees it.
    ///
    /// # Errors
    /// [`ErrorCode::DuplicateShipIds`] if a ship is named twice.
    pub fn check_unique_ships(&self) -> Result<(), ErrorCode> {
        let mut seen = BTreeSet::new();
        if self.ship_ids.iter().all(|id| seen.insert(*id)) {
            Ok(())
        } else {
            Err(ErrorCode::DuplicateShipIds)
        }
    }
}

/// The `send_fleet` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SendFleet {
    /// The fleet.
    pub fleet_id: FleetId,
    /// Where to.
    pub target_planet_id: PlanetId,
    /// What to do there.
    pub mission: Mission,
}

/// A payload naming one fleet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FleetRef {
    /// The fleet.
    pub fleet_id: FleetId,
}

/// An accepted command (200). A repeated `Idempotency-Key` returns the first response for that
/// key, whatever its age (D-037).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandAccepted {
    /// The command's log sequence number.
    pub seq: i64,
    /// The game time the command was applied at.
    pub game_time: Timestamp,
    /// **The command's own events only**, never the events of advancing time to it: those can
    /// be other players' owner-only information (D-030), so returning them would leak it. A
    /// privacy requirement (D-045); the log's `own_events` count rebuilds the same list for a
    /// repeated key.
    pub events: Vec<Event>,
}

/// The most bytes an `Idempotency-Key` may have (D-037).
pub const IDEMPOTENCY_KEY_MAX: usize = 128;

/// An `Idempotency-Key` header value: 1 to 128 bytes of printable ASCII without spaces, unique
/// per player (D-037).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Parses a header value.
    ///
    /// # Errors
    /// [`ErrorCode::MissingIdempotencyKey`] when there's none, [`ErrorCode::InvalidIdempotencyKey`]
    /// when it isn't 1 to 128 bytes of printable ASCII without spaces.
    pub fn parse(header: Option<&str>) -> Result<Self, ErrorCode> {
        let key = header.ok_or(ErrorCode::MissingIdempotencyKey)?;
        let valid = !key.is_empty()
            && key.len() <= IDEMPOTENCY_KEY_MAX
            && key.bytes().all(|b| b.is_ascii_graphic());
        if valid {
            Ok(Self(key.to_owned()))
        } else {
            Err(ErrorCode::InvalidIdempotencyKey)
        }
    }

    /// The key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
