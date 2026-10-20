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
