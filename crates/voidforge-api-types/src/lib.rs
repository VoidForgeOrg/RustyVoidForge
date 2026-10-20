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
        params(("Idempotency-Key" = String, Header, pattern = "^[!-~]{1,128}$",
            description = "1 to 128 bytes of printable ASCII without spaces, unique per player. \
                A repeat returns the first response for the key, whatever its age.")),
        responses(
            (status = 200, description = "Accepted, with the command's own events", body = CommandAccepted),
            (status = 400, description = "malformed_body, missing_idempotency_key, invalid_idempotency_key, duplicate_ship_ids", body = ErrorBody),
            (status = 401, description = "missing_token, invalid_token, with WWW-Authenticate", body = ErrorBody,
                headers(("WWW-Authenticate" = String, description = "Bearer realm=\"voidforge\", with error=\"invalid_token\" for an invalid token"))),
            (status = 422, description = "A game rule's reason code, or idempotency_key_conflict", body = ErrorBody),
            (status = 429, description = "rate_limited, with Retry-After", body = ErrorBody,
                headers(("Retry-After" = u64, description = "Whole seconds; one request after that conforms (D-054)"))),
            (status = 500, description = "internal", body = ErrorBody),
            (status = 503, description = "busy or unavailable, with Retry-After", body = ErrorBody,
                headers(("Retry-After" = u64, description = "Whole seconds before a retry is worth trying; advice, not a promise (D-046, D-047, D-049, D-051)"))),
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
            (status = 401, description = "missing_token, invalid_token, with WWW-Authenticate", body = ErrorBody,
                headers(("WWW-Authenticate" = String, description = "Bearer realm=\"voidforge\", with error=\"invalid_token\" for an invalid token"))),
            (status = 404, description = "unknown_planet", body = ErrorBody),
            (status = 422, description = "no_free_homeworld: the account could not join, because no homeworld is free (D-047)", body = ErrorBody),
            (status = 429, description = "rate_limited, with Retry-After", body = ErrorBody,
                headers(("Retry-After" = u64, description = "Whole seconds; one request after that conforms (D-054)"))),
            (status = 500, description = "internal", body = ErrorBody),
            (status = 503, description = "busy or unavailable, with Retry-After", body = ErrorBody,
                headers(("Retry-After" = u64, description = "Whole seconds before a retry is worth trying; advice, not a promise (D-046, D-047, D-049, D-051)"))),
        ),
        security(("bearer" = []))
    )]
    pub fn get_planet() {}

    /// The planets within a radius of a point, summarized.
    #[utoipa::path(
        get,
        path = "/planets",
        params(
            ("near" = String, Query, pattern = "^-?[0-9]{1,19},-?[0-9]{1,19},-?[0-9]{1,19}$",
                description = "The point, as x,y,z in distance units (each an i64; a value \
                    beyond i64 is invalid_query)"),
            ("radius" = i64, Query, description = "The radius, in distance units: 0 or more \
                (a negative radius is invalid_query). More than 1000 planets within it is \
                invalid_query, never a truncated list: narrow the radius (D-049)"),
        ),
        responses(
            (status = 200, description = "The planets", body = PlanetsNearView),
            (status = 400, description = "invalid_query", body = ErrorBody),
            (status = 401, description = "missing_token, invalid_token, with WWW-Authenticate", body = ErrorBody,
                headers(("WWW-Authenticate" = String, description = "Bearer realm=\"voidforge\", with error=\"invalid_token\" for an invalid token"))),
            (status = 422, description = "no_free_homeworld: the account could not join, because no homeworld is free (D-047)", body = ErrorBody),
            (status = 429, description = "rate_limited, with Retry-After", body = ErrorBody,
                headers(("Retry-After" = u64, description = "Whole seconds; one request after that conforms (D-054)"))),
            (status = 500, description = "internal", body = ErrorBody),
            (status = 503, description = "busy or unavailable, with Retry-After", body = ErrorBody,
                headers(("Retry-After" = u64, description = "Whole seconds before a retry is worth trying; advice, not a promise (D-046, D-047, D-049, D-051)"))),
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
            (status = 401, description = "missing_token, invalid_token, with WWW-Authenticate", body = ErrorBody,
                headers(("WWW-Authenticate" = String, description = "Bearer realm=\"voidforge\", with error=\"invalid_token\" for an invalid token"))),
            (status = 422, description = "no_free_homeworld: the account could not join, because no homeworld is free (D-047)", body = ErrorBody),
            (status = 429, description = "rate_limited, with Retry-After", body = ErrorBody,
                headers(("Retry-After" = u64, description = "Whole seconds; one request after that conforms (D-054)"))),
            (status = 500, description = "internal", body = ErrorBody),
            (status = 503, description = "busy or unavailable, with Retry-After", body = ErrorBody,
                headers(("Retry-After" = u64, description = "Whole seconds before a retry is worth trying; advice, not a promise (D-046, D-047, D-049, D-051)"))),
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
            (status = 401, description = "missing_token, invalid_token, with WWW-Authenticate", body = ErrorBody,
                headers(("WWW-Authenticate" = String, description = "Bearer realm=\"voidforge\", with error=\"invalid_token\" for an invalid token"))),
            (status = 422, description = "no_free_homeworld: the account could not join, because no homeworld is free (D-047)", body = ErrorBody),
            (status = 429, description = "rate_limited, with Retry-After", body = ErrorBody,
                headers(("Retry-After" = u64, description = "Whole seconds; one request after that conforms (D-054)"))),
            (status = 500, description = "internal", body = ErrorBody),
            (status = 503, description = "busy or unavailable, with Retry-After", body = ErrorBody,
                headers(("Retry-After" = u64, description = "Whole seconds before a retry is worth trying; advice, not a promise (D-046, D-047, D-049, D-051)"))),
        ),
        security(("bearer" = []))
    )]
    pub fn get_fleets() {}

    /// Every player ranked by score, as of its `as_of`: the board is rebuilt every 60 seconds by
    /// default. Before the first build, 503 `unavailable` with Retry-After.
    #[utoipa::path(
        get,
        path = "/leaderboard",
        responses(
            (status = 200, description = "The leaderboard", body = Leaderboard),
            (status = 401, description = "missing_token, invalid_token, with WWW-Authenticate", body = ErrorBody,
                headers(("WWW-Authenticate" = String, description = "Bearer realm=\"voidforge\", with error=\"invalid_token\" for an invalid token"))),
            (status = 422, description = "no_free_homeworld: the account could not join, because no homeworld is free (D-047)", body = ErrorBody),
            (status = 429, description = "rate_limited, with Retry-After", body = ErrorBody,
                headers(("Retry-After" = u64, description = "Whole seconds; one request after that conforms (D-054)"))),
            (status = 500, description = "internal", body = ErrorBody),
            (status = 503, description = "busy or unavailable, with Retry-After", body = ErrorBody,
                headers(("Retry-After" = u64, description = "Whole seconds before a retry is worth trying; advice, not a promise (D-046, D-047, D-049, D-051)"))),
        ),
        security(("bearer" = []))
    )]
    pub fn get_leaderboard() {}

    /// This document: the API's contract. Public: it needs no token, so reading it never joins
    /// anyone (D-049), and it's never rate limited (D-054).
    #[utoipa::path(
        get,
        path = "/openapi.json",
        responses(
            (status = 200, description = "The OpenAPI document", content_type = "application/json"),
            (status = 500, description = "internal", body = ErrorBody),
        )
    )]
    pub fn get_openapi() {}
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

/// Closes each variant of the tagged unions whose serde form refuses unknown fields (the
/// command, the event, and the per-state building and fleet views), so the schema refuses what
/// serde refuses: utoipa leaves a tagged variant's object open.
struct Closed;

impl Modify for Closed {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::RefOr;
        use utoipa::openapi::schema::{AdditionalProperties, Schema};
        let Some(components) = openapi.components.as_mut() else {
            return;
        };
        for name in ["CommandRequest", "Event", "BuildingView", "FleetView"] {
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
        paths::get_openapi,
    ),
    components(schemas(error::ErrorCode, event::Event)),
    modifiers(&Bearer, &Closed)
)]
struct Doc;

/// The OpenAPI document as JSON: every path, request, response and error, served at
/// `/openapi.json`. It's utoipa's document with every integer schema carrying its Rust type's
/// exact bounds ([`bound_integers`]), which utoipa can't express, so the schema refuses what
/// serde refuses.
///
/// # Panics
/// If an integer schema is of a kind [`bound_integers`] doesn't know: a new integer type needs
/// its bounds added there, and the tests call this.
#[must_use]
pub fn openapi() -> serde_json::Value {
    let mut doc = serde_json::to_value(Doc::openapi()).expect("the OpenAPI document serializes");
    bound_integers(&mut doc);
    // `radius` is an i64 from 0 (D-049, dev-qa's L1): a negative radius is invalid_query. utoipa's
    // bounds are f64s, which can't hold i64::MAX exactly, and a minimum of 0 would read as a u64
    // to bound_integers, so the one parameter that needs it is set here.
    let radius = doc["paths"]["/planets"]["get"]["parameters"]
        .as_array_mut()
        .and_then(|ps| ps.iter_mut().find(|p| p["name"] == "radius"))
        .expect("GET /planets has a radius parameter");
    radius["schema"]["minimum"] = serde_json::json!(0);
    doc
}

/// Sets every integer schema's `minimum` and `maximum` to its Rust type's exact range, as JSON
/// integers. utoipa marks the type only through `format` and a `minimum` of 0 for unsigned
/// types: `int64` is `i64`, `int64` from 0 is `u64` (the entity IDs, D-045's full domain), and
/// `int32` from 0 is `u32`.
fn bound_integers(value: &mut serde_json::Value) {
    use serde_json::{Value, json};
    match value {
        Value::Object(object) => {
            let integer = match object.get("type") {
                Some(Value::String(t)) => t == "integer",
                Some(Value::Array(ts)) => ts.iter().any(|t| t == "integer"),
                _ => false,
            };
            if integer {
                let format = object.get("format").and_then(Value::as_str);
                let unsigned = object.get("minimum").and_then(Value::as_u64) == Some(0);
                let (min, max) = match (format, unsigned) {
                    (Some("int64"), false) => (json!(i64::MIN), json!(i64::MAX)),
                    (Some("int64"), true) => (json!(0), json!(u64::MAX)),
                    (Some("int32"), true) => (json!(0), json!(u32::MAX)),
                    other => panic!("an integer schema of an unknown kind: {other:?}"),
                };
                object.insert("minimum".to_owned(), min);
                object.insert("maximum".to_owned(), max);
            }
            object.values_mut().for_each(bound_integers);
        }
        Value::Array(items) => items.iter_mut().for_each(bound_integers),
        _ => {}
    }
}

/// [`openapi`] as the pretty JSON checked in beside this crate, with a final newline.
///
/// # Panics
/// As [`openapi`].
#[must_use]
pub fn openapi_json() -> String {
    let mut text =
        serde_json::to_string_pretty(&openapi()).expect("the OpenAPI document serializes");
    text.push('\n');
    text
}
