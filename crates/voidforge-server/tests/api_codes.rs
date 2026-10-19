//! The API's 422 game codes are the core's reason codes verbatim (decision log D-028, D-029,
//! D-045). The API types can't see the core, so the server's tests, which may (D-043), check the
//! two lists against each other, through the server's own mapping (`voidforge_server::codes`).

use voidforge_api_types::error::ErrorCode;
use voidforge_core::command::Rejection;
use voidforge_server::codes::game_code as to_api;

#[test]
fn every_game_code_is_a_core_reason_code_verbatim() {
    let game: Vec<ErrorCode> = ErrorCode::ALL
        .into_iter()
        .filter(|c| c.is_game_rule())
        .collect();
    assert_eq!(game.len(), 31);
    for code in game {
        let wire = serde_json::to_value(code).unwrap();
        let core: Rejection = serde_json::from_value(wire.clone())
            .unwrap_or_else(|e| panic!("{wire} is not a core reason code: {e}"));
        assert_eq!(wire.as_str(), Some(core.code()), "the same text");
        assert_eq!(to_api(core), code, "the same code both ways");
    }
}

#[test]
fn every_core_reason_code_maps_to_a_game_code() {
    let all: Vec<Rejection> = ErrorCode::ALL
        .into_iter()
        .filter(|c| c.is_game_rule())
        .map(|c| serde_json::from_value(serde_json::to_value(c).unwrap()).unwrap())
        .collect();
    for r in all {
        let api = to_api(r);
        assert!(api.is_game_rule(), "{r:?} maps to {api:?}");
        assert_eq!(serde_json::to_value(api).unwrap().as_str(), Some(r.code()));
    }
}
