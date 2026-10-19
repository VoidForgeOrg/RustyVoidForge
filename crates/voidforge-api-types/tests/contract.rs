//! The public contract (Technical Plan P3.1; decision log D-045). Every command, response, view,
//! event and error has an example that validates against its schema in the generated OpenAPI
//! document (draft 2020-12, formats asserted, jsonschema), deserializes, and serializes back to the same
//! JSON. Every tag the document declares (command types, events, states, error codes) has one.
//! Invalid payloads are refused by the schema and by serde alike.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

use jsonschema::{Draft, Validator};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use voidforge_api_types::command::{
    AssembleFleet, CommandAccepted, CommandRequest, IdempotencyKey,
};
use voidforge_api_types::error::{ErrorBody, ErrorCode};
use voidforge_api_types::view::{FleetsView, Leaderboard, PlanetView, PlanetsNearView, PlayerView};
use voidforge_api_types::wire::{PlanetId, ShipId};
use voidforge_api_types::{openapi, openapi_json};

const ALICE: &str = "01928c5e-7a3b-7c2d-9e4f-5a6b7c8d9e0f";
const BOB: &str = "01928c5e-7a3b-7c2d-9e4f-000000000b0b";

/// Every component schema of the generated document, compiled as draft 2020-12 with format
/// assertions. jsonschema compares integers exactly (not through f64), which the bounds at 2^63
/// and 2^64 need.
struct Contract {
    doc: Value,
    validators: BTreeMap<String, Validator>,
}

/// A validator for `schema` against the document's components.
fn compile(doc: &Value, schema: Value) -> Validator {
    let root = json!({"allOf": [schema], "components": doc["components"]});
    jsonschema::options()
        .with_draft(Draft::Draft202012)
        .should_validate_formats(true)
        .build(&root)
        .unwrap_or_else(|e| panic!("the schema compiles: {e}"))
}

impl Contract {
    fn new() -> Self {
        let doc: Value = serde_json::from_str(&openapi_json()).expect("the document is JSON");
        let validators = doc["components"]["schemas"]
            .as_object()
            .expect("component schemas")
            .keys()
            .map(|name| {
                let schema = json!({"$ref": format!("#/components/schemas/{name}")});
                (name.clone(), compile(&doc, schema))
            })
            .collect();
        Self { doc, validators }
    }

    fn check(&self, schema: &str, value: &Value) -> Result<(), String> {
        self.validators[schema]
            .validate(value)
            .map_err(|e| e.to_string())
    }

    /// The schema of a path's parameter, compiled the same way.
    fn parameter(&self, path: &str, method: &str, name: &str) -> Validator {
        let schema = self.doc["paths"][path][method]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("{method} {path} has {name}"))["schema"]
            .clone();
        compile(&self.doc, schema)
    }

    /// The example validates, deserializes to `T`, and serializes back to itself.
    fn round_trip<T: Serialize + DeserializeOwned + Debug>(&self, schema: &str, value: &Value) {
        self.check(schema, value)
            .unwrap_or_else(|e| panic!("{schema} refuses {value}:\n{e}"));
        let typed: T = serde_json::from_value(value.clone())
            .unwrap_or_else(|e| panic!("{schema}: serde refuses {value}: {e}"));
        assert_eq!(
            &serde_json::to_value(&typed).unwrap(),
            value,
            "{schema} round trip"
        );
    }

    /// The tags a schema's `oneOf` declares under `key` (`type` or `state`).
    fn tags(&self, schema: &str, key: &str) -> BTreeSet<String> {
        self.doc["components"]["schemas"][schema]["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("{schema} is a oneOf"))
            .iter()
            .map(|v| v["properties"][key]["enum"][0].as_str().unwrap().to_owned())
            .collect()
    }

    fn enum_values(&self, schema: &str) -> BTreeSet<String> {
        self.doc["components"]["schemas"][schema]["enum"]
            .as_array()
            .unwrap_or_else(|| panic!("{schema} is an enum"))
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    }
}

fn tags_of(values: &[Value], key: &str) -> BTreeSet<String> {
    values
        .iter()
        .map(|v| v[key].as_str().unwrap().to_owned())
        .collect()
}

fn t(hms: &str) -> Value {
    json!(format!("2026-10-01T{hms}.000Z"))
}

/// One example per command type.
fn commands() -> Vec<Value> {
    vec![
        // The architecture's own example (Read side and API).
        json!({"type": "build_building", "payload": {"planet_id": 1042, "building_type": "refinery"}}),
        json!({"type": "cancel_construction", "payload": {"building_id": 5007}}),
        json!({"type": "demolish_building", "payload": {"building_id": 5001}}),
        json!({"type": "queue_ship", "payload": {"shipyard_id": 5003, "ship_type": "colony_ship"}}),
        json!({"type": "cancel_ship_build", "payload": {"ship_build_id": 6001}}),
        json!({"type": "assemble_fleet", "payload": {"planet_id": 1042, "ship_ids": [7001, 7002],
            "cargo": {"iron_ore_g": 0, "iron_ingot_g": 2000000000}}}),
        json!({"type": "send_fleet", "payload": {"fleet_id": 8001, "target_planet_id": 1077, "mission": "colonize"}}),
        json!({"type": "recall_fleet", "payload": {"fleet_id": 8001}}),
        json!({"type": "disband_fleet", "payload": {"fleet_id": 8001}}),
        json!({"type": "trigger_unload", "payload": {"fleet_id": 8001}}),
    ]
}

/// One example per event type.
fn events() -> Vec<Value> {
    vec![
        json!({"type": "building_completed", "planet_id": 1042, "building_id": 5007, "building_type": "refinery"}),
        json!({"type": "ship_completed", "planet_id": 1042, "shipyard_id": 5003, "ship_id": 7001, "ship_type": "colony_ship"}),
        json!({"type": "demolition_completed", "planet_id": 1042, "building_id": 5001, "building_type": "drill"}),
        json!({"type": "fleet_arrived", "fleet_id": 8001, "planet_id": 1077, "mission": "colonize"}),
        json!({"type": "planet_colonized", "planet_id": 1077, "owner": ALICE, "fleet_id": 8001, "ship_id": 7001}),
        json!({"type": "colonization_failed", "planet_id": 1077, "fleet_id": 8002}),
        json!({"type": "cargo_unloaded", "planet_id": 1077, "fleet_id": 8001,
            "cargo": {"iron_ore_g": 0, "iron_ingot_g": 1500000000}}),
        json!({"type": "fleet_removed", "fleet_id": 8001, "planet_id": 1077}),
        json!({"type": "fleet_returned", "fleet_id": 8003, "planet_id": 1042}),
    ]
}

fn stock(amount: i64, rate: i64, cap: i64) -> Value {
    json!({"amount_g": amount, "rate_g_per_h": rate, "cap_g": cap})
}

/// The architecture's example planet view, carrying the full contract: milliseconds in times,
/// `rate_g_per_h` on progress, and the planet's public state.
fn planet_view() -> Value {
    json!({
        "planet_id": 1042,
        "owner": ALICE,
        "as_of": t("12:00:00"),
        "valid_until": t("21:00:00"),
        "stocks": {
            "iron_ore": stock(8_400_000_000, -100_000_000, 10_000_000_000_i64),
            "iron_ingot": stock(1_200_000_000, 40_000_000, 5_000_000_000)
        },
        "buildings": [
            {"id": 5001, "type": "drill", "state": "operational", "status": "running"},
            {"id": 5007, "type": "refinery", "state": "under_construction", "delivered_g": 360_000_000,
                "cost_g": 900_000_000, "rate_g_per_h": 60_000_000, "eta": t("21:00:00")},
            {"id": 5002, "type": "refinery", "state": "demolishing", "demolition_ends_at": t("14:30:00")},
            {"id": 5003, "type": "shipyard", "state": "operational", "status": "throttled"},
            {"id": 5004, "type": "drill", "state": "under_construction", "delivered_g": 0,
                "cost_g": 600_000_000, "rate_g_per_h": 0, "eta": null, "status": "halted"}
        ],
        "position": {"x": 120_000, "y": -45_500, "z": 3_000},
        "slots": 6,
        "ship_builds": [
            {"id": 6001, "shipyard_id": 5003, "ship_type": "colony_ship", "state": "active",
                "status": "throttled", "delivered_g": 100_000_000, "cost_g": 400_000_000,
                "rate_g_per_h": 20_000_000, "eta": t("15:00:00")},
            {"id": 6002, "shipyard_id": 5003, "ship_type": "cargo_vessel", "state": "waiting",
                "delivered_g": 0, "cost_g": 150_000_000, "rate_g_per_h": 0, "eta": null}
        ],
        "roster": [{"ship_id": 7003, "ship_type": "cargo_vessel", "owner": ALICE}]
    })
}

fn fleets_view() -> Value {
    let cargo = json!({"iron_ore_g": 0, "iron_ingot_g": 2_000_000_000});
    json!({
        "as_of": t("12:00:00"),
        "valid_until": null,
        "fleets": [
            {"fleet_id": 8001, "ships": [{"ship_id": 7001, "ship_type": "colony_ship"}],
                "cargo": cargo, "capacity_g": 0, "speed_du_per_h": 10_000,
                "state": "stationed", "at_planet_id": 1042},
            {"fleet_id": 8002, "ships": [{"ship_id": 7002, "ship_type": "cargo_vessel"}],
                "cargo": cargo, "capacity_g": 5_000_000_000_i64, "speed_du_per_h": 12_000,
                "state": "in_transit", "origin_planet_id": 1042, "target_planet_id": 1077,
                "mission": "transport", "departed_at": t("10:00:00"), "arrives_at": t("16:15:30")},
            {"fleet_id": 8003, "ships": [{"ship_id": 7004, "ship_type": "cargo_vessel"}],
                "cargo": {"iron_ore_g": 0, "iron_ingot_g": 0}, "capacity_g": 5_000_000_000_i64,
                "speed_du_per_h": 12_000, "state": "returning", "origin_planet_id": 1042,
                "arrives_at": t("13:00:00")}
        ]
    })
}

#[test]
fn every_command_example_validates_and_round_trips() {
    let c = Contract::new();
    let examples = commands();
    for e in &examples {
        c.round_trip::<CommandRequest>("CommandRequest", e);
    }
    assert_eq!(
        tags_of(&examples, "type"),
        c.tags("CommandRequest", "type"),
        "one example per command type"
    );
    assert_eq!(examples.len(), 10, "the ten player actions");
}

#[test]
fn an_accepted_command_carries_every_event_type() {
    let c = Contract::new();
    let examples = events();
    let accepted = json!({"seq": 17, "game_time": t("12:00:00"), "events": examples});
    c.round_trip::<CommandAccepted>("CommandAccepted", &accepted);
    c.round_trip::<CommandAccepted>(
        "CommandAccepted",
        &json!({"seq": 18, "game_time": t("12:00:01"), "events": []}),
    );
    assert_eq!(tags_of(&examples, "type"), c.tags("Event", "type"));
}

#[test]
fn every_view_example_validates_and_round_trips() {
    let c = Contract::new();
    let planet = planet_view();
    c.round_trip::<PlanetView>("PlanetView", &planet);
    let unowned = json!({
        "planet_id": 1077, "owner": null, "as_of": t("12:00:00"), "valid_until": null,
        "stocks": {"iron_ore": stock(0, 0, 10_000_000_000_i64), "iron_ingot": stock(0, 0, 5_000_000_000)},
        "buildings": [], "position": {"x": 0, "y": 0, "z": 0}, "slots": 4,
        "ship_builds": [], "roster": []
    });
    c.round_trip::<PlanetView>("PlanetView", &unowned);
    c.round_trip::<PlanetsNearView>(
        "PlanetsNearView",
        &json!({"as_of": t("12:00:00"), "valid_until": t("12:30:00"), "planets": [
            {"planet_id": 1042, "position": {"x": 120_000, "y": -45_500, "z": 3_000}, "owner": ALICE, "distance": 0},
            {"planet_id": 1077, "position": {"x": 130_000, "y": -45_500, "z": 3_000}, "owner": null, "distance": 10_000}
        ]}),
    );
    c.round_trip::<PlayerView>(
        "PlayerView",
        &json!({"player_id": ALICE, "homeworld_id": 1042, "planet_ids": [1042, 1077],
            "score": 12_345, "as_of": t("12:00:00"), "valid_until": t("21:00:00")}),
    );
    let fleets = fleets_view();
    c.round_trip::<FleetsView>("FleetsView", &fleets);
    c.round_trip::<Leaderboard>(
        "Leaderboard",
        &json!({"as_of": t("12:00:00"), "entries": [
            {"rank": 1, "player_id": ALICE, "score": 900},
            {"rank": 2, "player_id": BOB, "score": 500},
            {"rank": 2, "player_id": "01928c5e-7a3b-7c2d-9e4f-00000000c0de", "score": 500},
            {"rank": 4, "player_id": "01928c5e-7a3b-7c2d-9e4f-00000000d00d", "score": 10}
        ]}),
    );
    // Every state and status the document declares has an example.
    let buildings = planet["buildings"].as_array().unwrap();
    assert_eq!(tags_of(buildings, "state"), c.tags("BuildingView", "state"));
    let statuses: BTreeSet<String> = buildings
        .iter()
        .filter_map(|b| b["status"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(statuses, c.enum_values("FlowStatus"));
    let builds = planet["ship_builds"].as_array().unwrap();
    assert_eq!(tags_of(builds, "state"), c.enum_values("ShipBuildState"));
    let fleets = fleets["fleets"].as_array().unwrap();
    assert_eq!(tags_of(fleets, "state"), c.tags("FleetView", "state"));
}

#[test]
fn every_error_code_validates_in_the_one_error_body() {
    let c = Contract::new();
    let mut seen = BTreeSet::new();
    for code in ErrorCode::ALL {
        let body = ErrorBody {
            code,
            message: "for people".to_owned(),
        };
        let value = serde_json::to_value(&body).unwrap();
        c.round_trip::<ErrorBody>("ErrorBody", &value);
        seen.insert(value["code"].as_str().unwrap().to_owned());
    }
    assert_eq!(seen.len(), ErrorCode::ALL.len(), "codes are distinct");
    assert_eq!(seen, c.enum_values("ErrorCode"), "ALL is every code");
}

/// The code list per status, as ruled (D-045, Q5): the status is part of the contract.
#[test]
fn every_code_has_its_ruled_status() {
    let by_status = |status: u16| -> BTreeSet<String> {
        ErrorCode::ALL
            .iter()
            .filter(|c| c.status() == status)
            .map(|c| {
                serde_json::to_value(c)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    };
    let set = |codes: &[&str]| {
        codes
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        by_status(400),
        set(&[
            "malformed_body",
            "missing_idempotency_key",
            "invalid_idempotency_key",
            "duplicate_ship_ids",
            "invalid_query"
        ])
    );
    assert_eq!(by_status(401), set(&["missing_token", "invalid_token"]));
    assert_eq!(by_status(404), set(&["not_found"]));
    assert_eq!(by_status(429), set(&["rate_limited"]));
    assert_eq!(by_status(500), set(&["internal"]));
    assert_eq!(by_status(503), set(&["busy", "unavailable"]));
    let game: BTreeSet<ErrorCode> = ErrorCode::ALL
        .into_iter()
        .filter(|c| c.is_game_rule())
        .collect();
    assert_eq!(game.len(), 31, "the core's 31 reason codes (D-028, D-029)");
    assert_eq!(
        by_status(422).len(),
        32,
        "the game codes and idempotency_key_conflict"
    );
    assert!(by_status(422).contains("idempotency_key_conflict"));
    assert!(!ErrorCode::IdempotencyKeyConflict.is_game_rule());
}

/// Each invalid payload is refused by the schema and by serde (or, for duplicates, by the edge
/// check the schema describes).
#[test]
fn invalid_payloads_are_refused() {
    let c = Contract::new();
    let refused = |schema: &str, value: Value| {
        assert!(c.check(schema, &value).is_err(), "{schema} accepts {value}");
    };
    let serde_refuses = |value: &Value| {
        assert!(
            serde_json::from_value::<CommandRequest>(value.clone()).is_err(),
            "serde accepts {value}"
        );
    };
    for bad in [
        // Join game isn't public (D-045 Q2).
        json!({"type": "join_game", "payload": {}}),
        json!({"type": "join_game"}),
        json!({"type": "launch_missiles", "payload": {"planet_id": 1}}),
        // A missing, unknown or mistyped field.
        json!({"type": "build_building", "payload": {"planet_id": 1042}}),
        json!({"type": "build_building", "payload": {"planet_id": 1042, "building_type": "refinery", "player": ALICE}}),
        json!({"type": "build_building", "payload": {"planet_id": "1042", "building_type": "refinery"}}),
        json!({"type": "build_building", "payload": {"planet_id": -1, "building_type": "refinery"}}),
        json!({"type": "build_building", "payload": {"planet_id": 1042, "building_type": "Refinery"}}),
        // The player never comes from the body.
        json!({"type": "recall_fleet", "payload": {"fleet_id": 8001}, "player": ALICE}),
        json!({"payload": {"fleet_id": 8001}}),
    ] {
        refused("CommandRequest", bad.clone());
        serde_refuses(&bad);
    }
    // Duplicate ship IDs: the schema's uniqueItems and the edge check (D-029 Q1).
    let dup = json!({"type": "assemble_fleet", "payload": {"planet_id": 1042, "ship_ids": [7001, 7001],
        "cargo": {"iron_ore_g": 0, "iron_ingot_g": 0}}});
    refused("CommandRequest", dup.clone());
    let CommandRequest::AssembleFleet(fleet) = serde_json::from_value(dup).unwrap() else {
        panic!("an assemble_fleet")
    };
    assert_eq!(fleet.check_unique_ships(), Err(ErrorCode::DuplicateShipIds));
    let fine = AssembleFleet {
        planet_id: PlanetId(1042),
        ship_ids: vec![ShipId(7001), ShipId(7002)],
        cargo: Default::default(),
    };
    assert_eq!(fine.check_unique_ships(), Ok(()));
    // Rules the core owns stay the core's: an empty fleet and negative cargo are well-formed
    // here, so they reach the core's 422 no_ships and invalid_cargo.
    for core_rule in [
        json!({"type": "assemble_fleet", "payload": {"planet_id": 1042, "ship_ids": [],
            "cargo": {"iron_ore_g": 0, "iron_ingot_g": 0}}}),
        json!({"type": "assemble_fleet", "payload": {"planet_id": 1042, "ship_ids": [7001],
            "cargo": {"iron_ore_g": -1, "iron_ingot_g": 0}}}),
    ] {
        c.round_trip::<CommandRequest>("CommandRequest", &core_rule);
    }
    // Times carry milliseconds, always, in UTC; player IDs are lowercase hyphenated UUIDs.
    let mut player = json!({"player_id": ALICE, "homeworld_id": 1042, "planet_ids": [1042],
        "score": 1, "as_of": t("12:00:00"), "valid_until": null});
    c.round_trip::<PlayerView>("PlayerView", &player);
    for (field, bad) in [
        ("as_of", json!("2026-10-01T12:00:00Z")),
        ("as_of", json!("2026-10-01T12:00:00.000+02:00")),
        ("as_of", json!(1_759_320_000_000_i64)),
        ("player_id", json!(ALICE.to_uppercase())),
        ("player_id", json!("01928c5e7a3b7c2d9e4f5a6b7c8d9e0f")),
        ("player_id", json!(42)),
    ] {
        let good = player[field].clone();
        player[field] = bad;
        refused("PlayerView", player.clone());
        player[field] = good;
    }
    // A view field dropped, an unknown error code, an unknown field in a view.
    let mut planet = planet_view();
    planet.as_object_mut().unwrap().remove("valid_until");
    refused("PlanetView", planet);
    refused("ErrorBody", json!({"code": "teapot", "message": "no"}));
    let mut event = events()[0].clone();
    event["owner"] = json!(BOB);
    refused("Event", event.clone());
    assert!(serde_json::from_value::<voidforge_api_types::event::Event>(event).is_err());
    refused("ErrorBody", json!({"code": "no_free_slot"}));
    let mut planet = planet_view();
    planet["secret"] = json!(1);
    refused("PlanetView", planet);
}

#[test]
fn the_idempotency_key_is_one_to_128_printable_ascii_bytes() {
    assert_eq!(
        IdempotencyKey::parse(None),
        Err(ErrorCode::MissingIdempotencyKey)
    );
    for bad in ["", "has space", "tab\there", "naïve", &"k".repeat(129)] {
        assert_eq!(
            IdempotencyKey::parse(Some(bad)),
            Err(ErrorCode::InvalidIdempotencyKey),
            "{bad:?}"
        );
    }
    for good in ["k", "a1-B2_c3.~!", &"k".repeat(128)] {
        assert_eq!(IdempotencyKey::parse(Some(good)).unwrap().as_str(), good);
    }
}

/// The six endpoints of the Architectural Design, each with its method, and the security scheme.
#[test]
fn the_document_has_every_endpoint() {
    let doc = openapi();
    let paths: BTreeSet<(String, String)> = doc["paths"]
        .as_object()
        .unwrap()
        .iter()
        .flat_map(|(path, item)| {
            item.as_object()
                .unwrap()
                .keys()
                .map(move |method| (method.clone(), path.clone()))
        })
        .collect();
    let expected: BTreeSet<(String, String)> = [
        ("post", "/commands"),
        ("get", "/planets/{id}"),
        ("get", "/planets"),
        ("get", "/players/me"),
        ("get", "/players/me/fleets"),
        ("get", "/leaderboard"),
        ("get", "/openapi.json"),
    ]
    .into_iter()
    .map(|(m, p)| (m.to_owned(), p.to_owned()))
    .collect();
    assert_eq!(paths, expected);
    // The radius is an i64 from 0 (D-049, dev-qa's L1).
    let radius = doc["paths"]["/planets"]["get"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "radius")
        .unwrap();
    assert_eq!(radius["schema"]["minimum"], serde_json::json!(0));
    assert_eq!(radius["schema"]["maximum"], serde_json::json!(i64::MAX));
    // The contract is public (D-049, T2); every other path needs a dex token.
    for (method, path) in &paths {
        let security = &doc["paths"][path][method]["security"];
        if path == "/openapi.json" {
            assert!(security.is_null(), "{path} is public: {security}");
        } else {
            assert_eq!(
                security[0]["bearer"],
                serde_json::json!([]),
                "{method} {path}"
            );
        }
    }
    assert_eq!(
        doc["components"]["securitySchemes"]["bearer"]["scheme"],
        "bearer"
    );
    let header = &doc["paths"]["/commands"]["post"]["parameters"][0];
    assert_eq!(
        (
            header["name"].as_str(),
            header["in"].as_str(),
            header["required"].as_bool()
        ),
        (Some("Idempotency-Key"), Some("header"), Some(true))
    );
}

/// The schema's verdict, serde's verdict, and the expected one, for one value of `T`.
fn agree<T: DeserializeOwned>(c: &Contract, schema: &str, value: &Value, valid: bool) {
    let by_schema = c.check(schema, value).is_ok();
    let by_serde = serde_json::from_value::<T>(value.clone()).is_ok();
    assert_eq!(
        (by_schema, by_serde),
        (valid, valid),
        "{schema} on {value}: (schema, serde) should both be {valid}"
    );
}

/// `value` with the JSON pointer `at` set to `to`.
fn with(value: &Value, at: &str, to: Value) -> Value {
    let mut v = value.clone();
    *v.pointer_mut(at)
        .unwrap_or_else(|| panic!("{at} in {value}")) = to;
    v
}

/// `value` with an object field added, or removed when `to` is None.
fn field(value: &Value, name: &str, to: Option<Value>) -> Value {
    let mut v = value.clone();
    let object = v.as_object_mut().unwrap();
    match to {
        Some(x) => object.insert(name.to_owned(), x),
        None => object.remove(name),
    };
    v
}

/// Repair round 1, F1: a view composed from a flattened state (a building, a fleet) refuses a
/// misspelled field and another state's field, in the schema (`unevaluatedProperties: false`)
/// and in serde (the state unions deny unknown fields), and every legitimate state still
/// validates.
#[test]
fn composed_views_refuse_typos_and_other_states_fields() {
    use voidforge_api_types::view::{BuildingView, FleetView};
    let c = Contract::new();
    let planet = planet_view();
    let buildings = planet["buildings"].as_array().unwrap();
    for b in buildings {
        agree::<BuildingView>(&c, "BuildingView", b, true);
    }
    let operational = &buildings[0];
    let constructing = &buildings[1];
    let demolishing = &buildings[2];
    for bad in [
        field(operational, "stauts", Some(json!("running"))),
        field(operational, "delivered_g", Some(json!(1))),
        field(demolishing, "eta", Some(t("13:00:00"))),
        field(constructing, "demolition_ends_at", Some(t("13:00:00"))),
        field(constructing, "cost", Some(json!(1))),
    ] {
        agree::<BuildingView>(&c, "BuildingView", &bad, false);
    }
    let fleets = fleets_view();
    let fleets = fleets["fleets"].as_array().unwrap();
    for f in fleets {
        agree::<FleetView>(&c, "FleetView", f, true);
    }
    let (stationed, in_transit, returning) = (&fleets[0], &fleets[1], &fleets[2]);
    for bad in [
        field(stationed, "capcity_g", Some(json!(0))),
        field(stationed, "arrives_at", Some(t("13:00:00"))),
        field(returning, "target_planet_id", Some(json!(1077))),
        field(in_transit, "at_planet_id", Some(json!(1042))),
    ] {
        agree::<FleetView>(&c, "FleetView", &bad, false);
    }
    // In a whole view too.
    let mut planet = planet_view();
    planet["buildings"][0]["delivered_g"] = json!(1);
    agree::<PlanetView>(&c, "PlanetView", &planet, false);
}

/// Repair round 1, L1: every integer kind at its valid edge and one past it, in the schema and in
/// serde alike: i64, u32 and u64 (D-045 keeps the IDs' full u64 domain), also through a path and
/// a query parameter. JSON integers past i64's minimum or u64's maximum parse as floats (no
/// arbitrary precision), so below i64::MIN the first value past is the nearest float,
/// -2^63 - 2048; -2^63 - 1 would round to -2^63 itself.
#[test]
fn integers_are_bounded_by_their_rust_type() {
    let c = Contract::new();
    let i64_past_min: Value = serde_json::from_str("-9223372036854777856").unwrap();
    let u64_past_max: Value = serde_json::from_str("18446744073709551616").unwrap();
    let planet = planet_view();
    // i64: a stock's amount.
    for (value, valid) in [
        (json!(i64::MAX), true),
        (json!(1_u64 << 63), false),
        (json!(i64::MIN), true),
        (i64_past_min.clone(), false),
    ] {
        let v = with(&planet, "/stocks/iron_ore/amount_g", value);
        agree::<PlanetView>(&c, "PlanetView", &v, valid);
    }
    // u32: slots.
    for (value, valid) in [
        (json!(u32::MAX), true),
        (json!(u64::from(u32::MAX) + 1), false),
        (json!(0), true),
        (json!(-1), false),
    ] {
        agree::<PlanetView>(&c, "PlanetView", &with(&planet, "/slots", value), valid);
    }
    // u64: an ID, referenced.
    for (value, valid) in [
        (json!(u64::MAX), true),
        (u64_past_max.clone(), false),
        (json!(0), true),
        (json!(-1), false),
    ] {
        agree::<PlanetView>(&c, "PlanetView", &with(&planet, "/planet_id", value), valid);
    }
    // Through the parameters: the planet's path ID (u64) and the radius (an i64 from 0, D-049).
    let id = c.parameter("/planets/{id}", "get", "id");
    assert!(id.is_valid(&json!(u64::MAX)));
    assert!(!id.is_valid(&u64_past_max));
    assert!(!id.is_valid(&json!(-1)));
    let radius = c.parameter("/planets", "get", "radius");
    assert!(radius.is_valid(&json!(i64::MAX)));
    assert!(!radius.is_valid(&json!(1_u64 << 63)));
    assert!(radius.is_valid(&json!(0)));
    assert!(!radius.is_valid(&json!(-1)));
    assert!(!radius.is_valid(&json!(i64::MIN)));
    assert!(!radius.is_valid(&i64_past_min));
}

/// The recurring class behind F1 and L1: anywhere the schema and serde could disagree, they give
/// the same verdict. The deliberate exceptions are two: an integral float (pinned below) and a
/// duplicate ship ID, which serde reads into
/// a list so `check_unique_ships` can answer 400 `duplicate_ship_ids` (covered above).
#[test]
fn the_schema_and_serde_agree() {
    let c = Contract::new();
    let player = json!({"player_id": ALICE, "homeworld_id": 1042, "planet_ids": [1042],
        "score": 1, "as_of": t("12:00:00"), "valid_until": null});
    agree::<PlayerView>(&c, "PlayerView", &player, true);
    for (id, valid) in [
        (ALICE.to_owned(), true),
        (ALICE.to_uppercase(), false),
        (ALICE.replace('-', ""), false),
        (format!("{ALICE}0"), false),
        (ALICE.replacen('5', "g", 1), false),
    ] {
        agree::<PlayerView>(
            &c,
            "PlayerView",
            &with(&player, "/player_id", json!(id)),
            valid,
        );
    }
    for (time, valid) in [
        ("2026-10-01T12:00:00.000Z", true),
        ("2028-02-29T23:59:59.999Z", true),
        ("2025-02-29T12:00:00.000Z", false),
        ("2026-04-31T12:00:00.000Z", false),
        ("2026-13-01T12:00:00.000Z", false),
        ("2026-10-01T24:00:00.000Z", false),
        ("2026-10-01T12:60:00.000Z", false),
        ("2026-12-31T23:59:60.000Z", false),
        ("2026-10-01T12:00:00Z", false),
        ("2026-10-01T12:00:00.000+00:00", false),
        ("2026-10-01 12:00:00.000Z", false),
    ] {
        agree::<PlayerView>(
            &c,
            "PlayerView",
            &with(&player, "/as_of", json!(time)),
            valid,
        );
    }
    // A nullable field that's always sent: null is fine, missing isn't, in both.
    agree::<PlayerView>(
        &c,
        "PlayerView",
        &field(&player, "valid_until", None),
        false,
    );
    let planet = planet_view();
    agree::<PlanetView>(&c, "PlanetView", &field(&planet, "owner", None), false);
    let builds = with(&planet, "/ship_builds/1/eta", json!(null));
    agree::<PlanetView>(&c, "PlanetView", &builds, true);
    let mut no_eta = planet.clone();
    no_eta["buildings"][1]
        .as_object_mut()
        .unwrap()
        .remove("eta");
    agree::<PlanetView>(&c, "PlanetView", &no_eta, false);
    // An optional field: absent and null are both fine in both.
    let mut statusless = planet.clone();
    statusless["buildings"][0]
        .as_object_mut()
        .unwrap()
        .remove("status");
    agree::<PlanetView>(&c, "PlanetView", &statusless, true);
    // Header and query forms the schema states and the parsers apply.
    let key = c.parameter("/commands", "post", "Idempotency-Key");
    for (k, valid) in [
        ("k".to_owned(), true),
        ("a1-B2_c3.~!".to_owned(), true),
        ("k".repeat(128), true),
        ("k".repeat(129), false),
        (String::new(), false),
        ("has space".to_owned(), false),
        ("naïve".to_owned(), false),
    ] {
        assert_eq!(key.is_valid(&json!(k)), valid, "schema on {k:?}");
        assert_eq!(
            IdempotencyKey::parse(Some(&k)).is_ok(),
            valid,
            "parse on {k:?}"
        );
    }
    // The second deliberate difference (D-045): an integral float is an integer to JSON Schema
    // 2020-12, while serde refuses it; a request carrying one is 400 malformed_body.
    let float_id = json!({"type": "recall_fleet", "payload": {"fleet_id": 8001.0}});
    assert!(
        c.check("CommandRequest", &float_id).is_ok(),
        "the schema accepts 8001.0"
    );
    assert!(
        serde_json::from_value::<CommandRequest>(float_id).is_err(),
        "serde refuses 8001.0"
    );
    let near = c.parameter("/planets", "get", "near");
    for (n, valid) in [
        ("120000,-45500,3000", true),
        ("0,0,0", true),
        ("1,2", false),
        ("1, 2, 3", false),
        ("x,y,z", false),
    ] {
        assert_eq!(near.is_valid(&json!(n)), valid, "near {n:?}");
    }
}

/// Repair round 2 (dev-qa's Q1): the cases that caught QA's schema/serde drift mutants, so this
/// suite catches them too. Times: century leap years, every month's last day and the next in a
/// common and a leap year, and a wrong byte at each separator.
#[test]
fn times_agree_on_the_calendar_and_every_separator() {
    let c = Contract::new();
    let player = json!({"player_id": ALICE, "homeworld_id": 1042, "planet_ids": [1042],
        "score": 1, "as_of": t("12:00:00"), "valid_until": null});
    let at = |time: &str| with(&player, "/as_of", json!(time));
    for (date, valid) in [
        ("2000-02-29", true),
        ("1900-02-29", false),
        ("2100-02-29", false),
    ] {
        agree::<PlayerView>(
            &c,
            "PlayerView",
            &at(&format!("{date}T00:00:00.000Z")),
            valid,
        );
    }
    for (year, february) in [(2026, 28), (2028, 29)] {
        let lengths = [31, february, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        for (month, last) in (1..=12).zip(lengths) {
            let day = |d: u32| format!("{year}-{month:02}-{d:02}T12:00:00.000Z");
            agree::<PlayerView>(&c, "PlayerView", &at(&day(last)), true);
            agree::<PlayerView>(&c, "PlayerView", &at(&day(last + 1)), false);
        }
    }
    let good = "2026-10-01T12:00:00.000Z";
    for position in [4, 7, 10, 13, 16, 19, 23] {
        for wrong in ['0', 'x', ' '] {
            let mut bytes = good.as_bytes().to_vec();
            bytes[position] = wrong as u8;
            let time = String::from_utf8(bytes).unwrap();
            agree::<PlayerView>(&c, "PlayerView", &at(&time), false);
        }
    }
}

/// Every nullable field that's always sent, missing: refused by both (null is fine, covered
/// above). One case per field, so a field that drifts back to optional in serde is caught.
#[test]
fn every_always_sent_field_is_required_by_both() {
    let c = Contract::new();
    let planet = planet_view();
    let near = json!({"as_of": t("12:00:00"), "valid_until": null, "planets": [
        {"planet_id": 1077, "position": {"x": 0, "y": 0, "z": 0}, "owner": null, "distance": 1}]});
    let player = json!({"player_id": ALICE, "homeworld_id": 1042, "planet_ids": [1042],
        "score": 1, "as_of": t("12:00:00"), "valid_until": null});
    let fleets = fleets_view();
    agree::<PlanetsNearView>(&c, "PlanetsNearView", &near, true);
    let drop = |value: &Value, at: &str, name: &str| {
        let mut v = value.clone();
        v.pointer_mut(at)
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("{at} is an object"))
            .remove(name)
            .unwrap_or_else(|| panic!("{at}/{name} exists"));
        v
    };
    agree::<PlanetView>(&c, "PlanetView", &drop(&planet, "", "owner"), false);
    agree::<PlanetView>(&c, "PlanetView", &drop(&planet, "", "valid_until"), false);
    agree::<PlanetView>(
        &c,
        "PlanetView",
        &drop(&planet, "/buildings/1", "eta"),
        false,
    );
    agree::<PlanetView>(
        &c,
        "PlanetView",
        &drop(&planet, "/ship_builds/0", "eta"),
        false,
    );
    agree::<PlanetView>(
        &c,
        "PlanetView",
        &drop(&planet, "/ship_builds/1", "eta"),
        false,
    );
    agree::<PlanetsNearView>(
        &c,
        "PlanetsNearView",
        &drop(&near, "", "valid_until"),
        false,
    );
    agree::<PlanetsNearView>(
        &c,
        "PlanetsNearView",
        &drop(&near, "/planets/0", "owner"),
        false,
    );
    agree::<PlayerView>(&c, "PlayerView", &drop(&player, "", "valid_until"), false);
    agree::<FleetsView>(&c, "FleetsView", &drop(&fleets, "", "valid_until"), false);
}

/// The fields of each state, as the contract (D-045). Pinned here rather than read from the
/// document, so a field added to a state (an optional `mission` on a returning fleet, say)
/// fails even when serde and the schema agree on it.
fn state_fields() -> [(&'static str, &'static str, &'static [&'static str]); 6] {
    [
        (
            "BuildingView",
            "under_construction",
            &["delivered_g", "cost_g", "rate_g_per_h", "eta"],
        ),
        ("BuildingView", "operational", &[]),
        ("BuildingView", "demolishing", &["demolition_ends_at"]),
        ("FleetView", "stationed", &["at_planet_id"]),
        (
            "FleetView",
            "in_transit",
            &[
                "origin_planet_id",
                "target_planet_id",
                "mission",
                "departed_at",
                "arrives_at",
            ],
        ),
        (
            "FleetView",
            "returning",
            &["origin_planet_id", "arrives_at"],
        ),
    ]
}

/// A well-typed value for a state field, so a refusal is about the field's presence.
fn sample(field: &str) -> Value {
    match field {
        "delivered_g" | "cost_g" | "rate_g_per_h" => json!(1),
        "eta" | "demolition_ends_at" | "departed_at" | "arrives_at" => t("13:00:00"),
        "at_planet_id" | "origin_planet_id" | "target_planet_id" => json!(1042),
        "mission" => json!("move"),
        other => panic!("no sample for {other}"),
    }
}

/// Every other state's field, injected into every state of both composed views, is refused by
/// both, directly and inside the whole view; and each state's fields are exactly the contract's.
#[test]
fn every_other_states_field_is_refused_in_every_state() {
    use voidforge_api_types::view::{BuildingView, FleetView};
    let c = Contract::new();
    let common: BTreeMap<&str, BTreeSet<&str>> = [
        (
            "BuildingView",
            BTreeSet::from(["id", "type", "status", "state"]),
        ),
        (
            "FleetView",
            BTreeSet::from([
                "fleet_id",
                "ships",
                "cargo",
                "capacity_g",
                "speed_du_per_h",
                "state",
            ]),
        ),
    ]
    .into_iter()
    .collect();
    for (view, state, fields) in state_fields() {
        let branch = c.doc["components"]["schemas"][view]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["properties"]["state"]["enum"][0] == state)
            .unwrap_or_else(|| panic!("{view} has {state}"));
        let declared: BTreeSet<&str> = branch["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let expected: BTreeSet<&str> = common[view]
            .iter()
            .copied()
            .chain(fields.iter().copied())
            .collect();
        assert_eq!(
            declared, expected,
            "{view} {state}'s fields are the contract's"
        );
    }
    let planet = planet_view();
    let fleets = fleets_view();
    let examples: BTreeMap<(&str, &str), Value> = planet["buildings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| ("BuildingView", b))
        .chain(
            fleets["fleets"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| ("FleetView", f)),
        )
        .map(|(view, v)| ((view, v["state"].as_str().unwrap()), v.clone()))
        .collect();
    for (view, state, own) in state_fields() {
        let example = &examples[&(view, state)];
        let foreign: BTreeSet<&str> = state_fields()
            .into_iter()
            .filter(|(v, s, _)| *v == view && *s != state)
            .flat_map(|(_, _, fields)| fields.iter().copied())
            .filter(|f| !own.contains(f))
            .collect();
        for f in foreign {
            let bad = field(example, f, Some(sample(f)));
            if view == "BuildingView" {
                agree::<BuildingView>(&c, view, &bad, false);
                let mut whole = planet.clone();
                whole["buildings"] = json!([bad]);
                agree::<PlanetView>(&c, "PlanetView", &whole, false);
            } else {
                agree::<FleetView>(&c, view, &bad, false);
                let mut whole = fleets.clone();
                whole["fleets"] = json!([bad]);
                agree::<FleetsView>(&c, "FleetsView", &whole, false);
            }
        }
    }
}
