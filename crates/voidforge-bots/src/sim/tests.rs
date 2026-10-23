//! The simulation's checks on views and answers, each against a case that breaks exactly its rule
//! beside one that keeps it, so a check that passes on anything is caught.

use serde_json::{Value, json};
use voidforge_api_types::command::CommandAccepted;
use voidforge_api_types::error::{ErrorBody, ErrorCode};
use voidforge_api_types::view::{FleetsView, PlanetView, PlayerView};

use super::*;
use crate::client::Refused;

const AS_OF: &str = "2026-10-23T12:00:00.000Z";
const LATER: &str = "2026-10-23T12:00:05.000Z";
const EARLIER: &str = "2026-10-23T11:59:55.000Z";

fn stock(amount: i64, cap: i64) -> Value {
    json!({ "amount_g": amount, "rate_g_per_h": 0, "cap_g": cap })
}

fn planet(edit: impl FnOnce(&mut Value)) -> PlanetView {
    let mut v = json!({
        "planet_id": 7, "owner": null, "as_of": AS_OF, "valid_until": LATER,
        "stocks": { "iron_ore": stock(10, 100), "iron_ingot": stock(0, 50) },
        "buildings": [
            { "state": "under_construction", "id": 1, "type": "drill", "delivered_g": 0, "cost_g": 10, "rate_g_per_h": 1, "eta": LATER },
            { "state": "operational", "id": 2, "type": "shipyard" }
        ],
        "position": { "x": 0, "y": 0, "z": 0 }, "slots": 2,
        "ship_builds": [],
        "roster": [ { "ship_id": 5, "ship_type": "colony_ship", "owner": "00000000-0000-7000-8000-000000000001" } ]
    });
    edit(&mut v);
    serde_json::from_value(v).expect("a planet view")
}

fn fleets(edit: impl FnOnce(&mut Value)) -> FleetsView {
    let mut v = json!({
        "as_of": AS_OF, "valid_until": null,
        "fleets": [
            { "state": "stationed", "fleet_id": 1, "ships": [ { "ship_id": 5, "ship_type": "cargo_vessel" } ],
              "cargo": { "iron_ore_g": 0, "iron_ingot_g": 40 }, "capacity_g": 50, "speed_du_per_h": 1, "at_planet_id": 7 },
            { "state": "in_transit", "fleet_id": 2, "ships": [ { "ship_id": 6, "ship_type": "colony_ship" } ],
              "cargo": { "iron_ore_g": 0, "iron_ingot_g": 0 }, "capacity_g": 0, "speed_du_per_h": 1,
              "origin_planet_id": 7, "target_planet_id": 8, "mission": "colonize",
              "departed_at": EARLIER, "arrives_at": LATER }
        ]
    });
    edit(&mut v);
    serde_json::from_value(v).expect("a fleets view")
}

#[test]
fn the_fixtures_are_the_api_s_shapes_and_keep_every_rule() {
    // A fixture the API couldn't send would make every case below hollow.
    let p = planet(|_| {});
    assert_eq!(p.buildings.len(), 2);
    check_planet(&p).unwrap();
    let f = fleets(|_| {});
    assert_eq!(f.fleets.len(), 2);
    check_fleets(&f).unwrap();
}

#[test]
fn a_stock_outside_zero_to_cap_breaks_the_planet() {
    assert!(check_planet(&planet(|v| v["stocks"]["iron_ore"] = stock(101, 100))).is_err());
    assert!(check_planet(&planet(|v| v["stocks"]["iron_ingot"] = stock(-1, 50))).is_err());
    check_planet(&planet(|v| v["stocks"]["iron_ore"] = stock(100, 100))).unwrap();
}

#[test]
fn more_buildings_than_slots_breaks_the_planet() {
    assert!(check_planet(&planet(|v| v["slots"] = json!(1))).is_err());
}

#[test]
fn a_time_before_the_view_s_as_of_breaks_the_planet() {
    assert!(check_planet(&planet(|v| v["buildings"][0]["eta"] = json!(EARLIER))).is_err());
    assert!(check_planet(&planet(|v| v["valid_until"] = json!(EARLIER))).is_err());
    assert!(
        check_planet(&planet(|v| v["buildings"][1] = json!(
            { "state": "demolishing", "id": 2, "type": "shipyard", "demolition_ends_at": EARLIER }
        )))
        .is_err()
    );
    assert!(
        check_planet(&planet(|v| v["ship_builds"] = json!([{
            "id": 3, "shipyard_id": 2, "ship_type": "colony_ship", "state": "active",
            "delivered_g": 0, "cost_g": 1, "rate_g_per_h": 1, "eta": EARLIER
        }])))
        .is_err()
    );
    check_planet(&planet(|v| v["buildings"][0]["eta"] = json!(AS_OF))).unwrap();
}

#[test]
fn a_ship_twice_on_a_roster_breaks_the_planet() {
    assert!(
        check_planet(&planet(|v| {
            let ship = v["roster"][0].clone();
            v["roster"].as_array_mut().unwrap().push(ship);
        }))
        .is_err()
    );
}

#[test]
fn a_ship_in_two_fleets_breaks_the_fleets() {
    assert!(check_fleets(&fleets(|v| v["fleets"][1]["ships"][0]["ship_id"] = json!(5))).is_err());
}

#[test]
fn cargo_over_capacity_or_negative_breaks_the_fleets() {
    assert!(
        check_fleets(&fleets(
            |v| v["fleets"][0]["cargo"]["iron_ingot_g"] = json!(51)
        ))
        .is_err()
    );
    assert!(
        check_fleets(&fleets(
            |v| v["fleets"][0]["cargo"]["iron_ore_g"] = json!(-1)
        ))
        .is_err()
    );
    check_fleets(&fleets(|v| {
        v["fleets"][0]["cargo"]["iron_ingot_g"] = json!(50)
    }))
    .unwrap();
}

#[test]
fn a_fleet_s_times_must_bracket_the_view() {
    assert!(check_fleets(&fleets(|v| v["fleets"][1]["arrives_at"] = json!(EARLIER))).is_err());
    assert!(check_fleets(&fleets(|v| v["fleets"][1]["departed_at"] = json!(LATER))).is_err());
}

#[test]
fn a_fleet_without_ships_breaks_the_fleets() {
    assert!(check_fleets(&fleets(|v| v["fleets"][0]["ships"] = json!([]))).is_err());
}

#[test]
fn a_player_without_its_homeworld_breaks_the_player() {
    let player = |ids: Value| -> PlayerView {
        serde_json::from_value(json!({
            "player_id": "00000000-0000-7000-8000-000000000001", "homeworld_id": 7,
            "planet_ids": ids, "score": 0, "as_of": AS_OF, "valid_until": null
        }))
        .unwrap()
    };
    check_player(&player(json!([7, 9]))).unwrap();
    assert!(check_player(&player(json!([9]))).is_err());
}

fn accepted(seq: i64) -> Reply<CommandAccepted> {
    Ok(serde_json::from_value(json!({ "seq": seq, "game_time": AS_OF, "events": [] })).unwrap())
}

fn refused(status: u16, code: ErrorCode) -> Reply<CommandAccepted> {
    Err(Refused {
        status,
        body: ErrorBody {
            code,
            message: "m".to_owned(),
        },
    })
}

#[test]
fn a_valid_command_is_accepted_or_refused_by_a_game_rule_and_nothing_else() {
    assert_eq!(judge_valid(&accepted(3)), Ok(None));
    assert_eq!(
        judge_valid(&refused(422, ErrorCode::InsufficientStock)),
        Ok(Some("insufficient_stock".to_owned()))
    );
    assert!(judge_valid(&refused(404, ErrorCode::NotFound)).is_err());
    assert!(judge_valid(&refused(400, ErrorCode::MalformedBody)).is_err());
    assert!(judge_valid(&refused(503, ErrorCode::NotFound)).is_err());
}

#[test]
fn an_invalid_command_gets_exactly_its_status_and_code() {
    let want = (422, ErrorCode::SamePlanet);
    judge_invalid(want, &refused(422, ErrorCode::SamePlanet)).unwrap();
    assert!(judge_invalid(want, &refused(422, ErrorCode::UnknownFleet)).is_err());
    assert!(judge_invalid(want, &refused(400, ErrorCode::SamePlanet)).is_err());
    assert!(judge_invalid(want, &accepted(4)).is_err());
}

#[test]
fn a_retry_gets_exactly_the_stored_answer() {
    judge_retry(&accepted(3), &accepted(3)).unwrap();
    assert!(judge_retry(&accepted(3), &accepted(4)).is_err());
    judge_retry(
        &refused(422, ErrorCode::NoFreeSlot),
        &refused(422, ErrorCode::NoFreeSlot),
    )
    .unwrap();
    assert!(
        judge_retry(
            &refused(422, ErrorCode::NoFreeSlot),
            &refused(422, ErrorCode::InsufficientStock)
        )
        .is_err()
    );
    assert!(judge_retry(&refused(422, ErrorCode::NoFreeSlot), &accepted(3)).is_err());
}

#[test]
fn the_generator_is_reproducible_from_its_seed() {
    let draw = |seed| {
        let mut r = Rng::new(seed);
        (0..8).map(|_| r.next()).collect::<Vec<_>>()
    };
    assert_eq!(draw(42), draw(42));
    assert_ne!(draw(42), draw(43));
}

#[test]
fn the_players_are_the_ci_dex_config_s_static_users() {
    let config = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/dex/config.ci.yaml"
    ))
    .unwrap();
    let mut names = BTreeSet::new();
    for user in PLAYERS {
        assert!(names.insert(user.name), "{} twice", user.name);
        assert!(
            config.contains(&format!("email: {}", user.email)),
            "{} not in config.ci.yaml",
            user.email
        );
        assert!(
            config.contains(&format!("# password: {}", user.password)),
            "{}'s password not in config.ci.yaml",
            user.name
        );
    }
    assert_eq!(names.len(), 8);
}
