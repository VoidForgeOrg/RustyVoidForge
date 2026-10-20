//! What the player reads when an API call fails (P4.2, D-059): phase4-ux §6.1's copy for every
//! `ErrorCode`, §6.2's for the web's own states, and what the page does with each.
//!
//! [`copy`] matches every code with no wildcard arm, so a new code fails to compile until it
//! has its sentence. The code is shown beside the copy (`Not done: … (no_free_slot)`); the
//! API's `message` never is (D-045).

use voidforge_api_types::error::ErrorCode;

use crate::api::ApiError;

/// Where the failure happened: a page's read, or a submitted order. The retry-class copy
/// differs: an order's says whether it was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// A page's read.
    Page,
    /// `POST /commands`.
    Command,
}

/// The numbers some copy names, when the page or form has them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fill {
    /// The planet acted on: `{p}`.
    pub planet: Option<u64>,
    /// The fleet's target: `{t}`.
    pub target: Option<u64>,
}

/// What the page does with a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The copy and the code at the top of the page.
    Banner,
    /// §5.5's retry page, answered with this status (and Retry-After when there is one).
    Retry {
        /// The web's status: the API's for 429, 500 and 503; 502 when it gave no usable answer.
        status: u16,
        /// Whole seconds, passed on.
        retry_after: Option<u64>,
    },
    /// The login is over: the session ends and the expired notice shows (D-057, A4).
    Login,
    /// §4.1's page: the universe is full, so this account can't join; Log out only.
    UniverseFull,
}

/// A failure, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The sentence.
    pub copy: String,
    /// The API's code, shown beside it; None for the web's own states (§6.2).
    pub code: Option<&'static str>,
    /// What the page does.
    pub kind: Kind,
}

/// §6.2: the API can't be reached.
pub const UNREACHABLE_PAGE: &str = "Can't reach the game server. Try again in a moment.";
/// §6.2: the API can't be reached, after an order.
pub const UNREACHABLE_COMMAND: &str = "Can't reach the game server. Your order may or may not have gone through. Trying again is safe: it won't run twice.";
/// §6.2 (P4.2's D2): an unexpected status or an unreadable body.
pub const UNEXPECTED_PAGE: &str =
    "The game server answered something unexpected. This is a bug; please report it.";
/// §6.2 (P4.2's D2): the same after an order, which may have been applied.
pub const UNEXPECTED_COMMAND: &str = "The game server answered something unexpected, so your order may or may not have gone through. Trying again is safe: it won't run twice. This is a bug; please report it.";

/// The message for a failed call.
#[must_use]
pub fn for_error(e: &ApiError, ctx: Context, fill: Fill) -> Message {
    match *e {
        ApiError::Refused {
            code,
            status,
            retry_after,
        } => {
            let kind = match code {
                ErrorCode::MissingToken | ErrorCode::InvalidToken => Kind::Login,
                ErrorCode::NoFreeHomeworld if ctx == Context::Page => Kind::UniverseFull,
                ErrorCode::RateLimited
                | ErrorCode::Internal
                | ErrorCode::Busy
                | ErrorCode::Unavailable => Kind::Retry {
                    status,
                    retry_after,
                },
                _ => Kind::Banner,
            };
            Message {
                copy: with_wait(copy(code, ctx, fill), retry_after),
                code: Some(code.as_str()),
                kind,
            }
        }
        ApiError::Unreachable(_) => Message {
            copy: match ctx {
                Context::Page => UNREACHABLE_PAGE,
                Context::Command => UNREACHABLE_COMMAND,
            }
            .to_owned(),
            code: None,
            kind: Kind::Retry {
                status: 502,
                retry_after: None,
            },
        },
        ApiError::UnexpectedStatus(_) | ApiError::Undecodable { .. } => Message {
            copy: match ctx {
                Context::Page => UNEXPECTED_PAGE,
                Context::Command => UNEXPECTED_COMMAND,
            }
            .to_owned(),
            code: None,
            kind: Kind::Retry {
                status: 502,
                retry_after: None,
            },
        },
    }
}

/// §5.5: "in a moment" when Retry-After is at most 2 s or absent, "in N seconds" otherwise.
fn with_wait(copy: String, retry_after: Option<u64>) -> String {
    match retry_after {
        Some(n) if n > 2 => copy.replace("in a moment", &format!("in {n} seconds")),
        _ => copy,
    }
}

fn planet(n: Option<u64>, known: impl FnOnce(u64) -> String, unknown: &str) -> String {
    n.map_or_else(|| unknown.to_owned(), known)
}

/// §6.1's sentence for `code`. Codes with `{p}` or `{t}` read without the number when the page
/// doesn't have it (P4.2's D1).
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one arm per code, with no wildcard, is the point"
)]
pub fn copy(code: ErrorCode, ctx: Context, fill: Fill) -> String {
    let command = ctx == Context::Command;
    let s = match code {
        // 400.
        ErrorCode::MalformedBody => {
            "The game didn't understand that order. This is a bug in the web client, not something you did; please report it."
        }
        ErrorCode::MissingIdempotencyKey => {
            "The form was missing its submission key. This is a bug in the web client; reload the page and try again."
        }
        ErrorCode::InvalidIdempotencyKey => {
            "The form's submission key wasn't valid. This is a bug in the web client; reload the page and try again."
        }
        ErrorCode::DuplicateShipIds => {
            "A ship was picked twice. Reload the page and pick each ship once."
        }
        ErrorCode::InvalidQuery => {
            "Those numbers couldn't be used for a search. Check them, or use a smaller radius (at most 1,000 planets per search)."
        }
        // 401.
        ErrorCode::MissingToken => "You're not logged in. Log in to continue.",
        ErrorCode::InvalidToken => "Your session ended. Log in again to continue.",
        // 404.
        ErrorCode::NotFound => {
            "That page doesn't exist on the game server. This is a bug in the web client; please report it."
        }
        // 422, the game's rules.
        ErrorCode::AlreadyJoined => "You've already joined this universe.",
        ErrorCode::NoFreeHomeworld => {
            "The universe is full: there's no free homeworld for a new player, so your account can't join."
        }
        ErrorCode::NotJoined => "Your account hasn't joined the game yet. Reload the page to join.",
        ErrorCode::UnknownPlanet => {
            return planet(
                fill.planet,
                |p| format!("There's no planet #{p}. Check the number on the Universe page."),
                "There's no such planet. Check the number on the Universe page.",
            );
        }
        ErrorCode::UnknownBuilding => {
            "That building no longer exists. It may have just been demolished or cancelled."
        }
        ErrorCode::UnknownShipBuild => {
            "That ship build no longer exists. It may have just finished or been cancelled."
        }
        ErrorCode::NotOwner => {
            return planet(
                fill.planet,
                |p| {
                    format!(
                        "Planet #{p} isn't yours. You can build, and load cargo, only on your own planets."
                    )
                },
                "That planet isn't yours. You can build, and load cargo, only on your own planets.",
            );
        }
        ErrorCode::NoFreeSlot => {
            "Every building slot on this planet is taken. Demolish or cancel a building to free one."
        }
        ErrorCode::NotUnderConstruction => {
            "That building isn't under construction any more. It may have just been completed."
        }
        ErrorCode::NotOperational => {
            "That building isn't working: it's under construction or being demolished."
        }
        ErrorCode::ShipyardHasBuilds => {
            "That Shipyard still has ships building or waiting. Cancel them before demolishing it."
        }
        ErrorCode::NotAShipyard => "That building isn't a Shipyard.",
        ErrorCode::ShipyardNotOperational => {
            "That Shipyard isn't working: it's under construction or being demolished."
        }
        ErrorCode::UnknownFleet => {
            "That fleet no longer exists. It may have been disbanded, or used its last ship to colonize."
        }
        ErrorCode::UnknownShip => {
            "One of those ships isn't available any more. Reload the page and pick again."
        }
        ErrorCode::NoShips => "Pick at least one ship.",
        ErrorCode::InvalidCargo => "Cargo can't be negative.",
        ErrorCode::ShipNotOnRoster => {
            "One of those ships isn't on this planet's roster any more. Reload the page and pick again."
        }
        ErrorCode::InsufficientStock => "This planet doesn't have that much in storage. Load less.",
        ErrorCode::InsufficientCapacity => {
            "Those ships can't carry that much. Load less, or add Cargo Vessels."
        }
        ErrorCode::FleetNotStationed => {
            "That fleet is flying, so it can't do that now. Wait until it's stationed."
        }
        ErrorCode::FleetNotInTransit => {
            "That fleet isn't flying a mission, so there's nothing to turn back. It may have just arrived."
        }
        ErrorCode::AlreadyRecalled => {
            "That fleet has already turned back once, and it can't again."
        }
        ErrorCode::SamePlanet => {
            return planet(
                fill.target,
                |t| format!("The fleet is already at Planet #{t}. Choose another target."),
                "The fleet is already at that planet. Choose another target.",
            );
        }
        ErrorCode::NoColonyShip => "Colonizing needs a Colony Ship in the fleet.",
        ErrorCode::TargetOwned => {
            return planet(
                fill.target,
                |t| {
                    format!(
                        "Planet #{t} is already owned. Colonize needs an unowned planet; Move goes anywhere."
                    )
                },
                "That planet is already owned. Colonize needs an unowned planet; Move goes anywhere.",
            );
        }
        ErrorCode::NoCargoVessel => "Transport needs a Cargo Vessel in the fleet.",
        ErrorCode::TargetNotOwned => {
            "Transport goes only to your own planets. To go elsewhere, use Move."
        }
        ErrorCode::UnreachableTarget => {
            return planet(
                fill.target,
                |t| format!("Planet #{t} is too far: the trip would take more than 1,000 years."),
                "That planet is too far: the trip would take more than 1,000 years.",
            );
        }
        ErrorCode::CargoAboard => {
            "The fleet still carries cargo. Unload it at one of your planets before disbanding."
        }
        ErrorCode::PlanetNotOwned => {
            "The fleet isn't at one of your planets, so it can't unload there. Move it to one of your planets first."
        }
        // 422, the protocol.
        ErrorCode::IdempotencyKeyConflict => {
            "That form was already used for a different order, so this one wasn't sent. The page has fresh forms now: submit it again."
        }
        // 429, 500, 503: an order's copy says whether it was sent.
        ErrorCode::RateLimited if command => {
            "Too many requests at once. Try again in a moment; your order wasn't sent."
        }
        ErrorCode::RateLimited => "Too many requests at once. Try again in a moment.",
        ErrorCode::Internal if command => {
            "Something went wrong on the game server. Your order may or may not have gone through. Trying again is safe: it won't run twice."
        }
        ErrorCode::Internal => "Something went wrong on the game server. Try again.",
        ErrorCode::Busy if command => {
            "The game is busy. Try again in a moment; your order wasn't sent."
        }
        ErrorCode::Busy => "The game is busy. Try again in a moment.",
        ErrorCode::Unavailable if command => {
            "The game is starting up or catching up. Try again in a moment; your order wasn't sent."
        }
        ErrorCode::Unavailable => "The game is starting up or catching up. Try again in a moment.",
    };
    s.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Unreachable;

    fn refused(code: ErrorCode, retry_after: Option<u64>) -> ApiError {
        ApiError::Refused {
            code,
            status: code.status(),
            retry_after,
        }
    }

    #[test]
    fn each_failure_has_its_kind() {
        let page = |e: &ApiError| for_error(e, Context::Page, Fill::default()).kind;
        assert_eq!(page(&refused(ErrorCode::InvalidToken, None)), Kind::Login);
        assert_eq!(page(&refused(ErrorCode::MissingToken, None)), Kind::Login);
        assert_eq!(
            page(&refused(ErrorCode::NoFreeHomeworld, None)),
            Kind::UniverseFull
        );
        assert_eq!(
            for_error(
                &refused(ErrorCode::NoFreeHomeworld, None),
                Context::Command,
                Fill::default()
            )
            .kind,
            Kind::Banner
        );
        for (code, status) in [
            (ErrorCode::RateLimited, 429),
            (ErrorCode::Internal, 500),
            (ErrorCode::Busy, 503),
            (ErrorCode::Unavailable, 503),
        ] {
            assert_eq!(
                page(&refused(code, Some(4))),
                Kind::Retry {
                    status,
                    retry_after: Some(4)
                },
                "{code:?}"
            );
        }
        assert_eq!(page(&refused(ErrorCode::NoFreeSlot, None)), Kind::Banner);
        let gone = Kind::Retry {
            status: 502,
            retry_after: None,
        };
        assert_eq!(page(&ApiError::Unreachable(Unreachable::Timeout)), gone);
        assert_eq!(page(&ApiError::UnexpectedStatus(302)), gone);
        assert_eq!(page(&ApiError::Undecodable { status: 200 }), gone);
    }

    #[test]
    fn the_web_s_own_states_have_no_code_and_differ_for_an_order() {
        for e in [
            ApiError::Unreachable(Unreachable::Connect),
            ApiError::UnexpectedStatus(418),
            ApiError::Undecodable { status: 200 },
        ] {
            let page = for_error(&e, Context::Page, Fill::default());
            let order = for_error(&e, Context::Command, Fill::default());
            assert_eq!((page.code, order.code), (None, None));
            assert!(
                order.copy.contains("may or may not have gone through"),
                "{e:?}"
            );
            assert!(!page.copy.contains("order"), "{e:?}");
        }
    }

    #[test]
    fn a_wait_over_two_seconds_is_named() {
        let copy = |n| for_error(&refused(ErrorCode::Busy, n), Context::Page, Fill::default()).copy;
        assert_eq!(copy(None), "The game is busy. Try again in a moment.");
        assert_eq!(copy(Some(2)), "The game is busy. Try again in a moment.");
        assert_eq!(copy(Some(3)), "The game is busy. Try again in 3 seconds.");
    }

    #[test]
    fn numbers_are_filled_when_known() {
        let fill = Fill {
            planet: Some(4),
            target: Some(9),
        };
        assert_eq!(
            copy(ErrorCode::NotOwner, Context::Command, fill),
            "Planet #4 isn't yours. You can build, and load cargo, only on your own planets."
        );
        assert_eq!(
            copy(ErrorCode::TargetOwned, Context::Command, fill),
            "Planet #9 is already owned. Colonize needs an unowned planet; Move goes anywhere."
        );
    }
}
