//! One error body for every non-2xx status, with a closed list of stable codes (D-045, Q5).
//! These are the API's codes; the command-line tools' exit codes (D-041) are a separate
//! contract.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Every error: `{"code": "no_free_slot", "message": "..."}`. `code` is the contract; `message`
/// is for people and may change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    /// The stable code.
    pub code: ErrorCode,
    /// A human-readable explanation; not part of the contract.
    pub message: String,
}

/// Every error code the API returns, with its status ([`ErrorCode::status`]). The 31 game codes
/// ([`ErrorCode::is_game_rule`]) are the core's reason codes verbatim (D-028, D-029), and the
/// server's tests check the two lists are equal.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    // --- 400: the request is malformed.
    /// The body isn't valid JSON, names an unknown command type (`join_game` included), or has
    /// a missing, unknown or mistyped field.
    MalformedBody,
    /// `POST /commands` without an `Idempotency-Key` header.
    MissingIdempotencyKey,
    /// The `Idempotency-Key` isn't 1 to 128 bytes of printable ASCII without spaces (D-037).
    InvalidIdempotencyKey,
    /// `assemble_fleet` names a ship more than once (D-029 Q1).
    DuplicateShipIds,
    /// A query or path parameter is malformed: a bad `near` or `radius`, a non-integer ID.
    InvalidQuery,

    // --- 401: no player.
    /// No `Authorization` header.
    MissingToken,
    /// The token failed verification: expired, a foreign issuer or audience, or a bad
    /// signature. One code for all, so the API never says which check failed.
    InvalidToken,

    // --- 404.
    /// No such route. (`GET /planets/{id}` for an unknown planet answers 404 with
    /// `unknown_planet`, the game code.)
    NotFound,

    // --- 422: a game rule refused the command (D-028, D-029, in the core's order).
    /// The player has already joined.
    AlreadyJoined,
    /// No homeworld candidate is left.
    NoFreeHomeworld,
    /// The player hasn't joined.
    NotJoined,
    /// No planet has that ID.
    UnknownPlanet,
    /// No building has that ID (or it's gone: IDs are never reused).
    UnknownBuilding,
    /// No ship build has that ID.
    UnknownShipBuild,
    /// The planet isn't the player's.
    NotOwner,
    /// Every slot on the planet is taken.
    NoFreeSlot,
    /// The building isn't under construction.
    NotUnderConstruction,
    /// The building isn't operational.
    NotOperational,
    /// The Shipyard still has ship builds.
    ShipyardHasBuilds,
    /// The building isn't a Shipyard.
    NotAShipyard,
    /// The Shipyard isn't operational.
    ShipyardNotOperational,
    /// No fleet has that ID.
    UnknownFleet,
    /// No ship has that ID.
    UnknownShip,
    /// The fleet would have no ships.
    NoShips,
    /// The cargo is negative.
    InvalidCargo,
    /// A ship isn't on that planet's roster.
    ShipNotOnRoster,
    /// The planet's storage doesn't hold that much.
    InsufficientStock,
    /// The ships can't carry that much.
    InsufficientCapacity,
    /// The fleet isn't stationed.
    FleetNotStationed,
    /// The fleet isn't in transit.
    FleetNotInTransit,
    /// The fleet was already recalled.
    AlreadyRecalled,
    /// The target is the fleet's own planet.
    SamePlanet,
    /// Colonizing needs a Colony Ship.
    NoColonyShip,
    /// The target is already owned.
    TargetOwned,
    /// Transport needs a Cargo Vessel.
    NoCargoVessel,
    /// Transport needs one of the player's own planets as its target.
    TargetNotOwned,
    /// The flight time can't be computed (the target is too far).
    UnreachableTarget,
    /// The fleet still carries cargo.
    CargoAboard,
    /// The fleet's planet isn't the player's.
    PlanetNotOwned,
    /// The player used this `Idempotency-Key` for a different command (D-037). Not a game rule:
    /// a protocol error.
    IdempotencyKeyConflict,

    // --- 429.
    /// Too many requests; `Retry-After` says when to try again (P3.8).
    RateLimited,

    // --- 500.
    /// Something unexpected failed on the server.
    Internal,

    // --- 503.
    /// The engine's inbox is full; `Retry-After` says when to try again.
    Busy,
    /// The engine is stopped or recovering.
    Unavailable,
}

impl ErrorCode {
    /// Every code, in declaration order.
    pub const ALL: [Self; 44] = [
        Self::MalformedBody,
        Self::MissingIdempotencyKey,
        Self::InvalidIdempotencyKey,
        Self::DuplicateShipIds,
        Self::InvalidQuery,
        Self::MissingToken,
        Self::InvalidToken,
        Self::NotFound,
        Self::AlreadyJoined,
        Self::NoFreeHomeworld,
        Self::NotJoined,
        Self::UnknownPlanet,
        Self::UnknownBuilding,
        Self::UnknownShipBuild,
        Self::NotOwner,
        Self::NoFreeSlot,
        Self::NotUnderConstruction,
        Self::NotOperational,
        Self::ShipyardHasBuilds,
        Self::NotAShipyard,
        Self::ShipyardNotOperational,
        Self::UnknownFleet,
        Self::UnknownShip,
        Self::NoShips,
        Self::InvalidCargo,
        Self::ShipNotOnRoster,
        Self::InsufficientStock,
        Self::InsufficientCapacity,
        Self::FleetNotStationed,
        Self::FleetNotInTransit,
        Self::AlreadyRecalled,
        Self::SamePlanet,
        Self::NoColonyShip,
        Self::TargetOwned,
        Self::NoCargoVessel,
        Self::TargetNotOwned,
        Self::UnreachableTarget,
        Self::CargoAboard,
        Self::PlanetNotOwned,
        Self::IdempotencyKeyConflict,
        Self::RateLimited,
        Self::Internal,
        Self::Busy,
        Self::Unavailable,
    ];

    /// The code's wire name, as serde writes it (`"no_free_slot"`): for showing the code beside a
    /// message (phase4-ux §6.1). The contract test checks it equals serde's for every code.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MalformedBody => "malformed_body",
            Self::MissingIdempotencyKey => "missing_idempotency_key",
            Self::InvalidIdempotencyKey => "invalid_idempotency_key",
            Self::DuplicateShipIds => "duplicate_ship_ids",
            Self::InvalidQuery => "invalid_query",
            Self::MissingToken => "missing_token",
            Self::InvalidToken => "invalid_token",
            Self::NotFound => "not_found",
            Self::AlreadyJoined => "already_joined",
            Self::NoFreeHomeworld => "no_free_homeworld",
            Self::NotJoined => "not_joined",
            Self::UnknownPlanet => "unknown_planet",
            Self::UnknownBuilding => "unknown_building",
            Self::UnknownShipBuild => "unknown_ship_build",
            Self::NotOwner => "not_owner",
            Self::NoFreeSlot => "no_free_slot",
            Self::NotUnderConstruction => "not_under_construction",
            Self::NotOperational => "not_operational",
            Self::ShipyardHasBuilds => "shipyard_has_builds",
            Self::NotAShipyard => "not_a_shipyard",
            Self::ShipyardNotOperational => "shipyard_not_operational",
            Self::UnknownFleet => "unknown_fleet",
            Self::UnknownShip => "unknown_ship",
            Self::NoShips => "no_ships",
            Self::InvalidCargo => "invalid_cargo",
            Self::ShipNotOnRoster => "ship_not_on_roster",
            Self::InsufficientStock => "insufficient_stock",
            Self::InsufficientCapacity => "insufficient_capacity",
            Self::FleetNotStationed => "fleet_not_stationed",
            Self::FleetNotInTransit => "fleet_not_in_transit",
            Self::AlreadyRecalled => "already_recalled",
            Self::SamePlanet => "same_planet",
            Self::NoColonyShip => "no_colony_ship",
            Self::TargetOwned => "target_owned",
            Self::NoCargoVessel => "no_cargo_vessel",
            Self::TargetNotOwned => "target_not_owned",
            Self::UnreachableTarget => "unreachable_target",
            Self::CargoAboard => "cargo_aboard",
            Self::PlanetNotOwned => "planet_not_owned",
            Self::IdempotencyKeyConflict => "idempotency_key_conflict",
            Self::RateLimited => "rate_limited",
            Self::Internal => "internal",
            Self::Busy => "busy",
            Self::Unavailable => "unavailable",
        }
    }

    /// Whether this is one of the core's 31 game-rule reason codes (D-028, D-029).
    #[must_use]
    pub fn is_game_rule(self) -> bool {
        self.status() == 422 && self != Self::IdempotencyKeyConflict
    }

    /// The HTTP status the API answers with. `unknown_planet` is 422 for a command; only
    /// `GET /planets/{id}` answers it with 404.
    #[must_use]
    pub fn status(self) -> u16 {
        match self {
            Self::MalformedBody
            | Self::MissingIdempotencyKey
            | Self::InvalidIdempotencyKey
            | Self::DuplicateShipIds
            | Self::InvalidQuery => 400,
            Self::MissingToken | Self::InvalidToken => 401,
            Self::NotFound => 404,
            Self::RateLimited => 429,
            Self::Internal => 500,
            Self::Busy | Self::Unavailable => 503,
            Self::AlreadyJoined
            | Self::NoFreeHomeworld
            | Self::NotJoined
            | Self::UnknownPlanet
            | Self::UnknownBuilding
            | Self::UnknownShipBuild
            | Self::NotOwner
            | Self::NoFreeSlot
            | Self::NotUnderConstruction
            | Self::NotOperational
            | Self::ShipyardHasBuilds
            | Self::NotAShipyard
            | Self::ShipyardNotOperational
            | Self::UnknownFleet
            | Self::UnknownShip
            | Self::NoShips
            | Self::InvalidCargo
            | Self::ShipNotOnRoster
            | Self::InsufficientStock
            | Self::InsufficientCapacity
            | Self::FleetNotStationed
            | Self::FleetNotInTransit
            | Self::AlreadyRecalled
            | Self::SamePlanet
            | Self::NoColonyShip
            | Self::TargetOwned
            | Self::NoCargoVessel
            | Self::TargetNotOwned
            | Self::UnreachableTarget
            | Self::CargoAboard
            | Self::PlanetNotOwned
            | Self::IdempotencyKeyConflict => 422,
        }
    }
}
