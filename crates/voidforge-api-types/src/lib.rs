//! Wire types and OpenAPI schemas: the public contract of the game's API (Technical Plan P3.1;
//! decision log D-045). Commands, views, events and errors as plain serde types, with no
//! workspace dependency, so the web client and the bots link them without the engine. The
//! server maps between them and the core (P3.4).
//!
//! [`openapi`] builds the OpenAPI document, served at `/openapi.json`; `openapi.json` beside
//! this crate is that document checked in, kept fresh by its test (`just openapi` rewrites it).

pub mod command;
pub mod error;
pub mod event;
pub mod view;
pub mod wire;

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

/// The paths, described here so the whole contract is in one crate; the server's handlers
/// (P3.4) must match them. These functions exist only for their documentation.
#[expect(
    dead_code,
    reason = "each function exists only for its utoipa::path description"
)]
mod paths {
    use crate::command::{CommandAccepted, CommandRequest};
    use crate::error::ErrorBody;
    use crate::view::{FleetsView, Leaderboard, PlanetView, PlanetsNearView, PlayerView};

    /// Submit a command. Every write goes through here, one command per log record.
    #[utoipa::path(
        post,
        path = "/commands",
        request_body = CommandRequest,
        params(("Idempotency-Key" = String, Header,
            description = "1 to 128 bytes of printable ASCII without spaces, unique per player. \
                A repeat returns the first response for the key, whatever its age.")),
        responses(
            (status = 200, description = "Accepted, with the command's own events", body = CommandAccepted),
            (status = 400, description = "malformed_body, missing_idempotency_key, invalid_idempotency_key, duplicate_ship_ids", body = ErrorBody),
            (status = 401, description = "missing_token, invalid_token", body = ErrorBody),
            (status = 422, description = "A game rule's reason code, or idempotency_key_conflict", body = ErrorBody),
            (status = 429, description = "rate_limited, with Retry-After", body = ErrorBody),
            (status = 500, description = "internal", body = ErrorBody),
            (status = 503, description = "busy (with Retry-After) or unavailable", body = ErrorBody),
        ),
        security(("bearer" = []))
    )]
    pub fn post_commands() {}

    /// One planet, projected to now. Planets are public.
    #[utoipa::path(
        get,
        path = "/planets/{id}",
        params(("id" = u64, Path, description = "The planet")),
        responses(
            (status = 200, description = "The planet", body = PlanetView),
            (status = 400, description = "invalid_query", body = ErrorBody),
            (status = 401, description = "missing_token, invalid_token", body = ErrorBody),
            (status = 404, description = "unknown_planet", body = ErrorBody),
            (status = 429, description = "rate_limited", body = ErrorBody),
            (status = 500, description = "internal", body = ErrorBody),
            (status = 503, description = "unavailable", body = ErrorBody),
        ),
        security(("bearer" = []))
    )]
    pub fn get_planet() {}

    /// The planets within a radius of a point, summarized.
    #[utoipa::path(
        get,
        path = "/planets",
        params(
            ("near" = String, Query, description = "The point, as x,y,z in distance units"),
            ("radius" = i64, Query, description = "The radius, in distance units"),
        ),
        responses(
            (status = 200, description = "The planets", body = PlanetsNearView),
            (status = 400, description = "invalid_query", body = ErrorBody),
            (status = 401, description = "missing_token, invalid_token", body = ErrorBody),
            (status = 429, description = "rate_limited", body = ErrorBody),
            (status = 500, description = "internal", body = ErrorBody),
            (status = 503, description = "unavailable", body = ErrorBody),
        ),
        security(("bearer" = []))
    )]
    pub fn get_planets_near() {}

    /// The player: homeworld, planets and score.
    #[utoipa::path(
        get,
        path = "/players/me",
        responses(
            (status = 200, description = "The player", body = PlayerView),
            (status = 401, description = "missing_token, invalid_token", body = ErrorBody),
            (status = 429, description = "rate_limited", body = ErrorBody),
            (status = 500, description = "internal", body = ErrorBody),
            (status = 503, description = "unavailable", body = ErrorBody),
        ),
        security(("bearer" = []))
    )]
    pub fn get_player() {}

    /// The player's own fleets; no one sees another player's.
    #[utoipa::path(
        get,
        path = "/players/me/fleets",
        responses(
            (status = 200, description = "The fleets", body = FleetsView),
            (status = 401, description = "missing_token, invalid_token", body = ErrorBody),
            (status = 429, description = "rate_limited", body = ErrorBody),
            (status = 500, description = "internal", body = ErrorBody),
            (status = 503, description = "unavailable", body = ErrorBody),
        ),
        security(("bearer" = []))
    )]
    pub fn get_fleets() {}

    /// Every player ranked by score, rebuilt every 60 seconds.
    #[utoipa::path(
        get,
        path = "/leaderboard",
        responses(
            (status = 200, description = "The leaderboard", body = Leaderboard),
            (status = 401, description = "missing_token, invalid_token", body = ErrorBody),
            (status = 429, description = "rate_limited", body = ErrorBody),
            (status = 500, description = "internal", body = ErrorBody),
            (status = 503, description = "unavailable", body = ErrorBody),
        ),
        security(("bearer" = []))
    )]
    pub fn get_leaderboard() {}
}

/// The bearer scheme: a dex ID token (P3.2).
struct Bearer;

impl Modify for Bearer {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .build(),
            ),
        );
    }
}

/// Closes each variant of the tagged unions whose serde form refuses unknown fields, so the
/// schema refuses what serde refuses: utoipa leaves a tagged variant's object open.
struct Closed;

impl Modify for Closed {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::RefOr;
        use utoipa::openapi::schema::{AdditionalProperties, Schema};
        let Some(components) = openapi.components.as_mut() else {
            return;
        };
        for name in ["CommandRequest", "Event"] {
            if let Some(RefOr::T(Schema::OneOf(one_of))) = components.schemas.get_mut(name) {
                for item in &mut one_of.items {
                    if let RefOr::T(Schema::Object(object)) = item {
                        object.additional_properties =
                            Some(Box::new(AdditionalProperties::FreeForm(false)));
                    }
                }
            }
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Voidforge",
        description = "The Voidforge game API (decision log D-045). Game times are RFC 3339 UTC \
            with milliseconds; amounts are integer grams.",
    ),
    paths(
        paths::post_commands,
        paths::get_planet,
        paths::get_planets_near,
        paths::get_player,
        paths::get_fleets,
        paths::get_leaderboard,
    ),
    components(schemas(error::ErrorCode, event::Event)),
    modifiers(&Bearer, &Closed)
)]
struct Doc;

/// The OpenAPI document: every path, request, response and error, served at `/openapi.json`.
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Doc::openapi()
}

/// [`openapi`] as the pretty JSON checked in beside this crate, with a final newline.
///
/// # Panics
/// Never in practice: the document is plain data, which serde_json always serializes.
#[must_use]
pub fn openapi_json() -> String {
    let mut text = openapi()
        .to_pretty_json()
        .expect("the OpenAPI document serializes");
    text.push('\n');
    text
}
