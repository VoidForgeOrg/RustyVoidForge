//! The API's 422 game codes are the core's reason codes verbatim (decision log D-028, D-029,
//! D-045). The API types can't see the core, so the server's tests, which may (D-043), check the
//! two lists against each other. A new core reason code fails to compile here until the API
//! names it too.

use voidforge_api_types::error::ErrorCode;
use voidforge_core::command::Rejection;

/// The core's reason code as the API's error code: exhaustive, with no wildcard.
fn to_api(r: Rejection) -> ErrorCode {
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

#[test]
fn every_game_code_is_a_core_reason_code_verbatim() {
    let game: Vec<ErrorCode> = ErrorCode::ALL
        .into_iter()
        .filter(|c| c.is_game_rule())
        .collect();
    assert_eq!(game.len(), 31);
    for code in game {
        let wire = serde_json::to_value(code).unwrap();
        let core: Rejection = serde_json::from_value(wire.clone())
            .unwrap_or_else(|e| panic!("{wire} is not a core reason code: {e}"));
        assert_eq!(wire.as_str(), Some(core.code()), "the same text");
        assert_eq!(to_api(core), code, "the same code both ways");
    }
}

#[test]
fn every_core_reason_code_maps_to_a_game_code() {
    let all: Vec<Rejection> = ErrorCode::ALL
        .into_iter()
        .filter(|c| c.is_game_rule())
        .map(|c| serde_json::from_value(serde_json::to_value(c).unwrap()).unwrap())
        .collect();
    for r in all {
        let api = to_api(r);
        assert!(api.is_game_rule(), "{r:?} maps to {api:?}");
        assert_eq!(serde_json::to_value(api).unwrap().as_str(), Some(r.code()));
    }
}
