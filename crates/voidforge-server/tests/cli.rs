//! `voidforge-server seed` without a database (P2.9, D-043): the world-size cap and bad input
//! are refused before anything is generated or connected, with D-041's exit codes.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_voidforge-server");

fn rules() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../rules/v0.toml")
        .to_str()
        .unwrap()
        .to_owned()
}

/// Runs `seed` with a URL that is not a Postgres URL, so nothing reaches a database.
fn seed(players: &str) -> (i32, String) {
    let out = Command::new(BIN)
        .args([
            "seed",
            "--seed",
            "1",
            "--players",
            players,
            "--rules",
            &rules(),
        ])
        .env("DATABASE_URL", "mysql://u:secret-password@127.0.0.1/none")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
    )
}

#[test]
fn a_universe_beyond_the_cap_is_refused_before_anything_else() {
    // 5,001 players × 20 planets is 100,020 planets, past MAX_PLANETS.
    let (code, out) = seed("5001");
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("the limit is 100000 planets"), "{out}");
    let (code, out) = seed("4294967295");
    assert_eq!(code, 3, "{out}");
    let (code, _) = seed("0");
    assert_eq!(code, 3, "no players");
}

#[test]
fn a_bad_database_url_is_an_input_error_and_is_never_printed() {
    let (code, out) = seed("1");
    assert_eq!(code, 4, "{out}");
    assert!(!out.contains("secret-password"), "{out}");
}

#[test]
fn usage_errors_exit_2() {
    let out = Command::new(BIN).arg("seed").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let help = Command::new(BIN).arg("--help").output().unwrap();
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("Exit codes: 0 seeded")
    );
}
