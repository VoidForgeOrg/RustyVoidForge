//! The checked-in `openapi.json` (decision log D-045, Q6): byte for byte what [`openapi_json`]
//! generates, so the public contract changes only visibly, through `just openapi`, and a
//! reviewer checks every change, as with goldens (D-040).

use std::path::PathBuf;

use voidforge_api_types::openapi_json;

fn file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("openapi.json")
}

/// Writes `openapi.json` from the types. Only through `just openapi`, which sets
/// `VOIDFORGE_BLESS_OPENAPI=1`; call the change out in the PR body.
#[test]
#[ignore = "writes openapi.json: just openapi"]
fn bless() {
    assert_eq!(
        std::env::var("VOIDFORGE_BLESS_OPENAPI").as_deref(),
        Ok("1"),
        "openapi.json is written only by `just openapi`"
    );
    let fresh = openapi_json();
    let state = match std::fs::read_to_string(file()) {
        Err(_) => "new",
        Ok(kept) if kept == fresh => "unchanged",
        Ok(_) => "CHANGED",
    };
    std::fs::write(file(), fresh).unwrap();
    println!("openapi.json: {state}");
}

/// The freshness check: a type change that wasn't blessed, or a hand edit, fails here.
#[test]
fn openapi_json_is_what_the_types_generate() {
    let kept = std::fs::read_to_string(file())
        .expect("crates/voidforge-api-types/openapi.json exists: run `just openapi`");
    let fresh = openapi_json();
    if kept != fresh {
        let line = kept
            .lines()
            .zip(fresh.lines())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| kept.lines().count().min(fresh.lines().count()))
            .saturating_add(1);
        panic!(
            "openapi.json differs from the types (first at line {line}). The public contract \
             changes only through `just openapi`, and the PR body must call the change out \
             for review"
        );
    }
}
