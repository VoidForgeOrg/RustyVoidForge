//! The API's 422 game codes are the core's reason codes verbatim (decision log D-028, D-029,
//! D-045). `tests/api_codes.rs` checks the two lists against each other (D-047 moved the mapping
//! here, for the accounts path and the routes).

use voidforge_api_types::error::ErrorCode;
use voidforge_host::Rejection;

/// The core's reason code as the API's error code: exhaustive, with no wildcard, so a new core
/// reason code fails to compile here until the API names it too.
#[must_use]
pub fn game_code(r: Rejection) -> ErrorCode {
    match r {
        Rejection::AlreadyJoined => ErrorCode::AlreadyJoined,
        Rejection::NoFreeHomeworld => ErrorCode::NoFreeHomeworld,
        Rejection::NotJoined => ErrorCode::NotJoined,
        Rejection::UnknownPlanet => ErrorCode::UnknownPlanet,
        Rejection::UnknownBuilding => ErrorCode::UnknownBuilding,
        Rejection::UnknownShipBuild => ErrorCode::UnknownShipBuild,
        Rejection::NotOwner => ErrorCode::NotOwner,
        Rejection::NoFreeSlot => ErrorCode::NoFreeSlot,
        Rejection::NotUnderConstruction => ErrorCode::NotUnderConstruction,
        Rejection::NotOperational => ErrorCode::NotOperational,
        Rejection::ShipyardHasBuilds => ErrorCode::ShipyardHasBuilds,
        Rejection::NotAShipyard => ErrorCode::NotAShipyard,
        Rejection::ShipyardNotOperational => ErrorCode::ShipyardNotOperational,
        Rejection::UnknownFleet => ErrorCode::UnknownFleet,
        Rejection::UnknownShip => ErrorCode::UnknownShip,
        Rejection::NoShips => ErrorCode::NoShips,
        Rejection::InvalidCargo => ErrorCode::InvalidCargo,
        Rejection::ShipNotOnRoster => ErrorCode::ShipNotOnRoster,
        Rejection::InsufficientStock => ErrorCode::InsufficientStock,
        Rejection::InsufficientCapacity => ErrorCode::InsufficientCapacity,
        Rejection::FleetNotStationed => ErrorCode::FleetNotStationed,
        Rejection::FleetNotInTransit => ErrorCode::FleetNotInTransit,
        Rejection::AlreadyRecalled => ErrorCode::AlreadyRecalled,
        Rejection::SamePlanet => ErrorCode::SamePlanet,
        Rejection::NoColonyShip => ErrorCode::NoColonyShip,
        Rejection::TargetOwned => ErrorCode::TargetOwned,
        Rejection::NoCargoVessel => ErrorCode::NoCargoVessel,
        Rejection::TargetNotOwned => ErrorCode::TargetNotOwned,
        Rejection::UnreachableTarget => ErrorCode::UnreachableTarget,
        Rejection::CargoAboard => ErrorCode::CargoAboard,
        Rejection::PlanetNotOwned => ErrorCode::PlanetNotOwned,
    }
}
