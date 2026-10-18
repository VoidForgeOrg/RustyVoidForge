//! Domain events on the wire: the core's nine events, tagged by `type` (D-045, Q3).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::wire::{
    BuildingId, BuildingType, Cargo, FleetId, Mission, PlanetId, PlayerId, ShipId, ShipType,
};

/// Something that happened, as `{"type": "building_completed", ...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    /// A construction reached its cost and is operational.
    BuildingCompleted {
        /// Where.
        planet_id: PlanetId,
        /// The building.
        building_id: BuildingId,
        /// What it is.
        building_type: BuildingType,
    },
    /// A ship build reached its cost; the ship joined the planet's roster.
    ShipCompleted {
        /// Where.
        planet_id: PlanetId,
        /// The Shipyard that built it.
        shipyard_id: BuildingId,
        /// The ship.
        ship_id: ShipId,
        /// What it is.
        ship_type: ShipType,
    },
    /// A demolition ended; the building is gone and its slot free.
    DemolitionCompleted {
        /// Where.
        planet_id: PlanetId,
        /// The building, now gone.
        building_id: BuildingId,
        /// What it was.
        building_type: BuildingType,
    },
    /// A fleet reached its target and is stationed there; first of an arrival's events.
    FleetArrived {
        /// The fleet.
        fleet_id: FleetId,
        /// Where.
        planet_id: PlanetId,
        /// Its mission.
        mission: Mission,
    },
    /// A Colony Ship was consumed and the planet is the fleet owner's.
    PlanetColonized {
        /// The planet.
        planet_id: PlanetId,
        /// Its new owner.
        owner: PlayerId,
        /// The fleet that carried the Colony Ship.
        fleet_id: FleetId,
        /// The Colony Ship, used up.
        ship_id: ShipId,
    },
    /// A colony fleet found its target owned: nothing consumed, cargo still aboard.
    ColonizationFailed {
        /// The planet.
        planet_id: PlanetId,
        /// The fleet.
        fleet_id: FleetId,
    },
    /// Cargo went into the planet's storage, up to free storage; only when something did.
    CargoUnloaded {
        /// The planet.
        planet_id: PlanetId,
        /// The fleet.
        fleet_id: FleetId,
        /// What was unloaded.
        cargo: Cargo,
    },
    /// Colonizing used the fleet's last ship, so the fleet is gone.
    FleetRemoved {
        /// The fleet.
        fleet_id: FleetId,
        /// Where.
        planet_id: PlanetId,
    },
    /// A recalled fleet is back, stationed at its origin.
    FleetReturned {
        /// The fleet.
        fleet_id: FleetId,
        /// Where.
        planet_id: PlanetId,
    },
}
