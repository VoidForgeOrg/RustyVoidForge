//! `voidforge-server seed` and `serve` without a database (P2.9, D-043; P3.4s, D-050): the
//! world-size cap, an epoch out of range and bad input are refused before anything is generated
//! or connected, with D-041's exit codes.

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
    let help = Command::new(BIN).args(["seed", "--help"]).output().unwrap();
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("Exit codes: 0 seeded")
    );
}

/// The epoch window (D-050) is checked before the URL is even parsed: with a URL that isn't a
/// Postgres URL, an out-of-range epoch is still a refusal (3), not an input error (4).
#[test]
fn seed_refuses_an_epoch_outside_the_window_before_connecting() {
    for epoch in ["-1", "4102444800000"] {
        let out = Command::new(BIN)
            .args(["seed", "--seed", "1", "--players", "1", "--rules", &rules()])
            .args(["--epoch-unix-ms", epoch])
            .env("DATABASE_URL", "mysql://u:secret-password@127.0.0.1/none")
            .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
            .output()
            .unwrap();
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert_eq!(out.status.code(), Some(3), "{epoch}: {stdout}");
        assert!(
            stdout.contains("outside 1970-01-01 to 2100-01-01"),
            "{stdout}"
        );
    }
}

/// `serve` without a database: its help lists the exit codes; no `--database-url` is a usage
/// error even with `DATABASE_URL` set (it is deliberately not read); settings out of range are
/// usage errors before any connection; a bad URL is an input error and is never printed.
#[test]
fn serve_usage_settings_and_a_bad_url() {
    let help = Command::new(BIN)
        .args(["serve", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("Exit codes: 0 stopped cleanly"), "{help}");
    assert!(help.contains("6 stopped uncleanly"), "{help}");
    assert!(help.contains("VOIDFORGE_DATABASE_URL"), "{help}");

    let serve = |args: &[&str], url: Option<&str>| {
        let mut c = Command::new(BIN);
        c.arg("serve").args(["--rules", &rules()]).args(args);
        c.env(
            "DATABASE_URL",
            "postgres://u:secret-password@127.0.0.1/voidforge",
        )
        .env_remove("VOIDFORGE_DATABASE_URL")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT");
        if let Some(url) = url {
            c.args(["--database-url", url]);
        }
        let out = c.output().unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8(out.stderr).unwrap() + &String::from_utf8(out.stdout).unwrap(),
        )
    };
    let (code, out) = serve(&[], None);
    assert_eq!(code, 2, "DATABASE_URL is never read: {out}");
    assert!(out.contains("--database-url"), "{out}");
    // Port 1: nothing listens, so no server is reached.
    let bad = Some("postgres://u:secret-password@127.0.0.1:1/none");
    let (code, out) = serve(&["--drain-secs", "61"], bad);
    assert_eq!(code, 2, "{out}");
    let (code, out) = serve(&["--token-leeway-secs", "301"], bad);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("token-leeway-secs"), "{out}");
    let (code, out) = serve(&[], bad);
    assert_eq!(code, 4, "{out}");
    assert!(!out.contains("secret-password"), "{out}");
    let (code, out) = serve(&[], Some("not a url"));
    assert_eq!(code, 4, "{out}");
    assert!(out.contains("not a Postgres URL"), "{out}");
}
