//! `GET /rules`'s view against the rules files (P4.1r, D-058), field by field.
//!
//! Each file is read twice: by `voidforge-rules`, as `serve` loads it, to build the answer; and
//! as a plain TOML table, never through the server's mapping, to say what the answer must be.
//! The view's sections and names are the file's, so the answer must equal the file minus
//! worldgen, key for key and value for value: a swapped field, the wrong kind, a missing or an
//! extra field all fail.

use std::path::Path;

use serde_json::Value;

fn rules_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../rules")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The answer `serve` would give for this file.
fn answer(text: &str) -> Value {
    let rules = voidforge_rules::parse(text).expect("the rules file parses");
    serde_json::from_slice(&voidforge_server::rules_view::body(&rules)).expect("the body is JSON")
}

/// The file as plain TOML, without worldgen (the advisor's A1).
fn expected(text: &str) -> Value {
    let mut table: toml::Table = toml::from_str(text).expect("the file is TOML");
    assert!(
        table.remove("worldgen").is_some(),
        "the file has a worldgen section"
    );
    serde_json::to_value(table).expect("TOML converts to JSON")
}

fn equal_field_by_field(file: &str) {
    let text = rules_file(file);
    let (got, want) = (answer(&text), expected(&text));
    // A whole-value comparison, plus a walk that names the first differing path.
    fn walk(path: &str, got: &Value, want: &Value) {
        match (got, want) {
            (Value::Object(g), Value::Object(w)) => {
                let (gk, wk): (Vec<_>, Vec<_>) = (g.keys().collect(), w.keys().collect());
                let mut gs = gk.clone();
                let mut ws = wk.clone();
                gs.sort();
                ws.sort();
                assert_eq!(gs, ws, "the keys at {path}");
                for k in ws {
                    walk(&format!("{path}.{k}"), &g[k], &w[k]);
                }
            }
            _ => assert_eq!(got, want, "{path}"),
        }
    }
    walk(file, &got, &want);
    assert_eq!(got, want, "{file}");
}

#[test]
fn v0_is_the_file_field_by_field() {
    equal_field_by_field("v0.toml");
}

#[test]
fn v0_fast_is_the_file_field_by_field() {
    equal_field_by_field("v0-fast.toml");
}

#[test]
fn the_two_files_differ_where_the_view_shows_it() {
    // A guard on the test itself: the files differ, so a view built from the wrong one fails.
    let (v0, fast) = (
        answer(&rules_file("v0.toml")),
        answer(&rules_file("v0-fast.toml")),
    );
    assert_ne!(v0, fast);
    assert_eq!(
        (v0["name"].as_str(), v0["version"].as_u64()),
        (Some("v0"), Some(0))
    );
    assert_eq!(
        (fast["name"].as_str(), fast["version"].as_u64()),
        (Some("v0-fast"), Some(1))
    );
}

#[test]
fn the_answer_is_the_contracts_rules_view() {
    // The body deserializes as the api-types view, whose fields deny anything unknown.
    let body = voidforge_server::rules_view::body(
        &voidforge_rules::parse(&rules_file("v0.toml")).unwrap(),
    );
    let view: voidforge_api_types::rules::RulesView = serde_json::from_slice(&body).unwrap();
    assert_eq!(view.ships.cargo_vessel.cargo_capacity_g, 500_000_000);
    assert_eq!(view.buildings.shipyard.parallel_builds, 3);
}

// --- dev-qa's ready test (P4.1r QA F1): equal values in the shipped files hide swaps ---

/// A rules file whose every viewed number is distinct (dev-qa's P4.1r ready test). In v0.toml and
/// v0-fast.toml several fields share a value (homeworld drills and refineries are both 1; scoring's
/// refinery and cargo vessel both 150; ore and ingots both per 10 t; drill and refinery both
/// demolish in 1 h), so swapping such a pair in the mapping gives byte-identical answers for both
/// files and no comparison against them can see it. Here every pair differs. Builds are whole
/// hours with a rate divisible by ingots_per_ore, as validation requires (D-025).
const DISTINCT: &str = r#"
version = 7
name = "qa-distinct"

[planet]
slots = 9
ore_cap_g = 10_000_000_011
ingot_cap_g = 5_000_000_012

[homeworld]
drills = 2
refineries = 3
ore_g = 2_000_000_013
ingots_g = 1_500_000_014

[buildings.drill]
cost_g = 150_000_012
build_time_ms = 10_800_000
demolition_time_ms = 3_600_017
output_g_per_h = 10_000_018

[buildings.refinery]
cost_g = 250_000_060
build_time_ms = 18_000_000
demolition_time_ms = 3_600_021
intake_g_per_h = 8_000_022
ingots_per_ore = 4
output_g_per_h = 32_000_088

[buildings.shipyard]
cost_g = 1_200_000_100
build_time_ms = 46_800_000
demolition_time_ms = 7_200_025
parallel_builds = 5

[ships.colony_ship]
cost_g = 800_000_036
build_time_ms = 39_600_000
speed_du_per_h = 10_028
cargo_capacity_g = 0

[ships.cargo_vessel]
cost_g = 300_000_036
build_time_ms = 25_200_000
speed_du_per_h = 15_031
cargo_capacity_g = 500_000_032

[scoring]
planet = 1_033
drill = 134
refinery = 135
shipyard = 536
colony_ship = 437
cargo_vessel = 138
ore = { points = 6, per_g = 10_000_039 }
ingots = { points = 10, per_g = 20_000_040 }

[worldgen]
planets_per_expected_player = 20
cube_size_du = 1_000_000
min_planet_spacing_du = 10_000
homeworld_min_spacing_du = 150_000
homeworld_neighbor_radius_du = 100_000
homeworld_min_neighbors = 5
"#;

#[test]
fn a_file_with_every_value_distinct_is_mapped_field_by_field() {
    fn leaves(v: &Value, out: &mut Vec<u64>) {
        match v {
            Value::Object(m) => m.values().for_each(|v| leaves(v, out)),
            Value::Number(n) => out.push(n.as_u64().expect("a non-negative integer")),
            _ => {}
        }
    }
    let want = expected(DISTINCT);
    // A guard on the fixture: no two numbers are equal, except the colony ship's required 0.
    let mut all = Vec::new();
    leaves(&want, &mut all);
    let n = all.len();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), n, "every number in the fixture is distinct");
    assert_eq!(answer(DISTINCT), want);
}
