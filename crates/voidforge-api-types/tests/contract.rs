//! The public contract (Technical Plan P3.1; decision log D-045). Every command, response, view,
//! event and error has an example that validates against its schema in the generated OpenAPI
//! document (draft 2020-12, formats asserted), deserializes, and serializes back to the same
//! JSON. Every tag the document declares (command types, events, states, error codes) has one.
//! Invalid payloads are refused by the schema and by serde alike.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

use boon::{Compiler, Draft, SchemaIndex, Schemas};
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

const BASE: &str = "http://voidforge.invalid/openapi.json";
const ALICE: &str = "01928c5e-7a3b-7c2d-9e4f-5a6b7c8d9e0f";
const BOB: &str = "01928c5e-7a3b-7c2d-9e4f-000000000b0b";

/// Every component schema of the generated document, compiled.
struct Contract {
    doc: Value,
    schemas: Schemas,
    index: BTreeMap<String, SchemaIndex>,
}

impl Contract {
    fn new() -> Self {
        let doc: Value = serde_json::from_str(&openapi_json()).expect("the document is JSON");
        let mut compiler = Compiler::new();
        compiler.set_default_draft(Draft::V2020_12);
        compiler.enable_format_assertions();
        compiler.add_resource(BASE, doc.clone()).unwrap();
        let mut schemas = Schemas::new();
        let index = doc["components"]["schemas"]
            .as_object()
            .expect("component schemas")
            .keys()
            .map(|name| {
                let loc = format!("{BASE}#/components/schemas/{name}");
                let idx = compiler
                    .compile(&loc, &mut schemas)
                    .unwrap_or_else(|e| panic!("{name} compiles: {e}"));
                (name.clone(), idx)
            })
            .collect();
        Self {
            doc,
            schemas,
            index,
        }
    }

    fn check(&self, schema: &str, value: &Value) -> Result<(), String> {
        let idx = self.index[schema];
        self.schemas
            .validate(value, idx)
            .map_err(|e| format!("{e:#}"))
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
    assert_eq!(
        tags_of(buildings, "state"),
        c.tags("BuildingState", "state")
    );
    let statuses: BTreeSet<String> = buildings
        .iter()
        .filter_map(|b| b["status"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(statuses, c.enum_values("FlowStatus"));
    let builds = planet["ship_builds"].as_array().unwrap();
    assert_eq!(tags_of(builds, "state"), c.enum_values("ShipBuildState"));
    let fleets = fleets["fleets"].as_array().unwrap();
    assert_eq!(tags_of(fleets, "state"), c.tags("FleetState", "state"));
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
    let doc = serde_json::to_value(openapi()).unwrap();
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
    ]
    .into_iter()
    .map(|(m, p)| (m.to_owned(), p.to_owned()))
    .collect();
    assert_eq!(paths, expected);
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
