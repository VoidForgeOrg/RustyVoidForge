//! The replay CLI driven as a process (Technical Plan P2.7; decision log D-041): every golden
//! verifies, a tampered golden is pinpointed to its record with exit 1, a build or rules
//! mismatch is refused with exit 3, inspection is labeled unverified, and the same input gives
//! byte-identical output.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_voidforge-replay");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn goldens() -> PathBuf {
    repo().join("tests/golden")
}

/// Rules v0, passed explicitly for golden copies under the target directory, which may be outside
/// the repository (P2.7 QA L1): the default search looks only above the golden.
fn v0() -> String {
    repo().join("rules/v0.toml").to_str().unwrap().to_owned()
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str]) -> Run {
    let out = Command::new(BIN)
        .args(args)
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .env_remove("DATABASE_URL")
        .output()
        .expect("the binary runs");
    Run {
        code: out.status.code().expect("an exit code"),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn json(args: &[&str]) -> (i32, Value) {
    let mut all = args.to_vec();
    all.push("--json");
    let r = run(&all);
    let v = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", r.stdout, r.stderr));
    (r.code, v)
}

/// A scratch copy of a golden, to tamper with.
fn copy_golden(name: &str, case: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("replay-cli")
        .join(case)
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["start.vfsnap", "records.jsonl", "manifest.json"] {
        std::fs::copy(goldens().join(name).join(f), dir.join(f)).unwrap();
    }
    dir
}

/// The record lines, as text: a `serde_json::Value` cannot hold a 128-bit player ID exactly
/// (D-036), so records are edited as text.
fn lines(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("records.jsonl"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn write_lines(dir: &Path, records: &[String]) {
    let text: String = records.iter().map(|r| format!("{r}\n")).collect();
    std::fs::write(dir.join("records.jsonl"), text).unwrap();
}

/// A record line's events, as the text of each event object.
fn events(line: &str) -> Vec<String> {
    let start = line.find("\"events\":[").unwrap() + "\"events\":[".len();
    let (mut out, mut depth, mut from) = (Vec::new(), 0, start);
    for (i, c) in line[start..].char_indices() {
        let at = start + i;
        match c {
            '{' | '[' => depth += 1,
            '}' => depth -= 1,
            ']' if depth == 0 => {
                if at > from {
                    out.push(line[from..at].to_owned());
                }
                return out;
            }
            ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(line[from..at].to_owned());
                from = at + 1;
            }
            _ => {}
        }
    }
    panic!("unterminated events: {line}")
}

/// The line with its events replaced.
fn with_events(line: &str, events: &[String]) -> String {
    let old = format!("\"events\":[{}]", self::events(line).join(","));
    assert!(line.contains(&old));
    line.replacen(&old, &format!("\"events\":[{}]", events.join(",")), 1)
}

fn manifest(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap()
}

fn write_manifest(dir: &Path, m: &Value) {
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(m).unwrap(),
    )
    .unwrap();
}

/// The first divergence of a golden run: (exit, result, seq, what).
fn divergence(dir: &Path) -> (i32, String, Option<i64>, String) {
    let (code, v) = json(&["golden", dir.to_str().unwrap(), "--rules", &v0()]);
    let s = &v["sources"][0];
    let d = &s["divergence"];
    (
        code,
        s["result"].as_str().unwrap().to_owned(),
        d["seq"].as_i64(),
        d["what"].as_str().unwrap_or_default().to_owned(),
    )
}

#[test]
fn every_golden_verifies_through_the_binary() {
    let r = run(&["golden", goldens().to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}{}", r.stdout, r.stderr);
    let verified = r
        .stdout
        .lines()
        .filter(|l| l.starts_with("verified "))
        .count();
    let on_disk = std::fs::read_dir(goldens())
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().join("manifest.json").is_file())
        .count();
    assert!(on_disk >= 6, "{on_disk} goldens");
    assert_eq!(verified, on_disk, "{}", r.stdout);
    let (code, v) = json(&["golden", goldens().to_str().unwrap()]);
    assert_eq!(code, 0);
    assert_eq!(v["result"], "verified");
    for s in v["sources"].as_array().unwrap() {
        assert_eq!(s["result"], "verified", "{s}");
        assert_eq!(s["build"], "0.1.0+golden");
        assert_eq!(s["build_from_flag"], false);
        assert_eq!(s["records"].as_u64(), s["last_seq"].as_u64());
    }
}

/// The tamper matrix (SPEC 7): each edit is pinpointed to its record, with exit 1.
#[test]
fn a_tampered_golden_is_pinpointed_to_the_record() {
    // whole-loop: record 10 holds three events (FleetArrived, PlanetColonized, CargoUnloaded).
    let name = "whole-loop";

    let dir = copy_golden(name, "event");
    let mut records = lines(&dir);
    let mut ev = events(&records[9]);
    assert!(ev.len() >= 2);
    ev.swap(0, 1);
    records[9] = with_events(&records[9], &ev);
    write_lines(&dir, &records);
    assert_eq!(
        divergence(&dir),
        (1, "diverged".into(), Some(10), "events".into())
    );

    let dir = copy_golden(name, "event-value");
    let mut records = lines(&dir);
    let mut ev = events(&records[9]);
    assert!(ev[2].contains("\"ingots_g\":150000000"), "{}", ev[2]);
    ev[2] = ev[2].replace("\"ingots_g\":150000000", "\"ingots_g\":1");
    records[9] = with_events(&records[9], &ev);
    write_lines(&dir, &records);
    assert_eq!(
        divergence(&dir),
        (1, "diverged".into(), Some(10), "events".into())
    );

    let dir = copy_golden(name, "outcome");
    let mut records = lines(&dir);
    let accepted = "\"outcome\":{\"result\":\"accepted\",\"own_events\":0}";
    assert!(records[1].contains(accepted), "{}", records[1]);
    records[1] = records[1].replace(
        accepted,
        "\"outcome\":{\"result\":\"rejected\",\"reason\":\"no_free_slot\"}",
    );
    write_lines(&dir, &records);
    assert_eq!(
        divergence(&dir),
        (1, "diverged".into(), Some(2), "outcome".into())
    );

    let dir = copy_golden(name, "hash");
    let mut m = manifest(&dir);
    m["records"][6]["state_hash"] = Value::from("00".repeat(32));
    write_manifest(&dir, &m);
    assert_eq!(
        divergence(&dir),
        (1, "diverged".into(), Some(7), "state_hash".into())
    );

    let dir = copy_golden(name, "removed");
    let mut records = lines(&dir);
    records.remove(4);
    write_lines(&dir, &records);
    assert_eq!(
        divergence(&dir),
        (1, "diverged".into(), Some(5), "gap".into())
    );

    let dir = copy_golden(name, "removed-last");
    let mut records = lines(&dir);
    records.pop();
    write_lines(&dir, &records);
    assert_eq!(
        divergence(&dir),
        (1, "diverged".into(), Some(12), "gap".into())
    );

    let dir = copy_golden(name, "swapped");
    let mut records = lines(&dir);
    records.swap(5, 6);
    write_lines(&dir, &records);
    assert_eq!(
        divergence(&dir),
        (1, "diverged".into(), Some(6), "gap".into())
    );

    let dir = copy_golden(name, "end-hash");
    let mut m = manifest(&dir);
    m["end_hash"] = Value::from("00".repeat(32));
    write_manifest(&dir, &m);
    assert_eq!(
        divergence(&dir),
        (1, "diverged".into(), None, "end_hash".into())
    );

    let dir = copy_golden(name, "score");
    let mut m = manifest(&dir);
    m["scores"][0]["score"] = Value::from(3_240);
    write_manifest(&dir, &m);
    assert_eq!(
        divergence(&dir),
        (1, "diverged".into(), None, "scores".into())
    );
}

#[test]
fn a_divergence_reports_what_the_record_holds_and_what_replay_found() {
    let dir = copy_golden("whole-loop", "report");
    let mut records = lines(&dir);
    let mut ev = events(&records[9]);
    ev.swap(0, 1);
    records[9] = with_events(&records[9], &ev);
    write_lines(&dir, &records);
    let (_, v) = json(&["golden", dir.to_str().unwrap(), "--rules", &v0()]);
    let d = &v["sources"][0]["divergence"];
    assert_ne!(d["expected"], d["found"]);
    assert!(
        d["expected"]
            .as_str()
            .unwrap()
            .starts_with("[{\"PlanetColonized\"")
    );
    assert!(
        d["found"]
            .as_str()
            .unwrap()
            .starts_with("[{\"FleetArrived\"")
    );
    let r = run(&["golden", dir.to_str().unwrap(), "--rules", &v0()]);
    assert!(r.stdout.starts_with("DIVERGED "), "{}", r.stdout);
    assert!(r.stdout.contains("events at seq 10"), "{}", r.stdout);
}

#[test]
fn a_build_or_rules_mismatch_is_refused() {
    let dir = copy_golden("start-and-build", "build");
    let mut m = manifest(&dir);
    m["engine_build"] = Value::from("0.1.0+someothercommit");
    write_manifest(&dir, &m);
    let (code, v) = json(&["golden", dir.to_str().unwrap(), "--rules", &v0()]);
    assert_eq!(code, 3);
    assert_eq!(v["result"], "refused");
    assert_eq!(v["sources"][0]["divergence"]["what"], "build");

    let rules = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-cli/v1.toml");
    let text = std::fs::read_to_string(repo().join("rules/v0.toml")).unwrap();
    assert!(
        text.contains("version = 0"),
        "rules/v0.toml names its version"
    );
    std::fs::write(&rules, text.replacen("version = 0", "version = 1", 1)).unwrap();
    let (code, v) = json(&[
        "golden",
        goldens().join("start-and-build").to_str().unwrap(),
        "--rules",
        rules.to_str().unwrap(),
    ]);
    assert_eq!(code, 3);
    assert_eq!(v["sources"][0]["divergence"]["what"], "rules_version");
}

/// A scoring error diverges with exit 1, never verifies as a score (phase 2 gate, R2 L1): under
/// rules whose planet is worth `i64::MAX`, a manifest naming `i64::MIN` (the sentinel the verifier
/// once substituted) fails at the scores, with the core's error as what was found.
#[test]
fn a_scoring_error_diverges_and_is_never_a_score() {
    let dir = copy_golden("whole-loop", "score-error");
    let mut m = manifest(&dir);
    for s in m["scores"].as_array_mut().unwrap() {
        s["score"] = Value::from(i64::MIN);
    }
    write_manifest(&dir, &m);
    let rules = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-cli/score-max.toml");
    let text = std::fs::read_to_string(repo().join("rules/v0.toml")).unwrap();
    assert!(
        text.contains("planet = 1_000"),
        "rules/v0.toml names the planet's score"
    );
    let max = text.replacen("planet = 1_000", &format!("planet = {}", i64::MAX), 1);
    std::fs::write(&rules, max).unwrap();
    let args = [
        "golden",
        dir.to_str().unwrap(),
        "--rules",
        rules.to_str().unwrap(),
    ];
    let (code, v) = json(&args);
    let s = &v["sources"][0];
    assert_eq!((code, v["result"].as_str()), (1, Some("diverged")), "{v}");
    assert_eq!(s["divergence"]["what"], "scores");
    assert!(
        s["divergence"]["expected"]
            .as_str()
            .unwrap()
            .starts_with("a score for player "),
        "{v}"
    );
    assert!(
        s["divergence"]["found"].as_str().unwrap().contains("score"),
        "the core's error: {v}"
    );
}

#[test]
fn the_same_input_gives_byte_identical_output() {
    let dir = copy_golden("ships-and-colonize", "identical");
    let mut records = lines(&dir);
    let mut ev = events(&records[11]);
    ev.pop();
    records[11] = with_events(&records[11], &ev);
    write_lines(&dir, &records);
    let rules = v0();
    for args in [
        vec!["golden", goldens().to_str().unwrap()],
        vec!["golden", goldens().to_str().unwrap(), "--json"],
        vec!["golden", dir.to_str().unwrap(), "--rules", &rules, "--json"],
        vec!["inspect", "golden", goldens().to_str().unwrap()],
    ] {
        let (a, b) = (run(&args), run(&args));
        assert_eq!((a.code, &a.stdout), (b.code, &b.stdout), "{args:?}");
    }
}

#[test]
fn inspect_reports_metadata_labeled_unverified() {
    let r = run(&[
        "inspect",
        "golden",
        goldens().join("whole-loop").to_str().unwrap(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.starts_with("UNVERIFIED "), "{}", r.stdout);
    assert!(r.stdout.contains("build 0.1.0+golden"));
    assert!(r.stdout.contains("records: 12 (seq 1..12)"));
    let (code, v) = json(&["inspect", "golden", goldens().to_str().unwrap()]);
    assert_eq!(code, 0);
    assert_eq!(v["result"], "unverified");
    for s in v["sources"].as_array().unwrap() {
        assert_eq!(s["verified"], false);
        assert_eq!(s["snapshots"][0]["format"], 1);
    }
    // Inspection does not verify: a tampered golden still inspects.
    let dir = copy_golden("whole-loop", "inspect");
    let mut m = manifest(&dir);
    m["end_hash"] = Value::from("00".repeat(32));
    write_manifest(&dir, &m);
    assert_eq!(run(&["inspect", "golden", dir.to_str().unwrap()]).code, 0);
}

#[test]
fn exit_codes_for_usage_and_input_errors() {
    assert_eq!(run(&[]).code, 2);
    assert_eq!(run(&["golden"]).code, 2);
    assert_eq!(
        run(&["postgres", "--rules", "x"]).code,
        2,
        "no database URL"
    );
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-cli/nothing-here");
    let (code, v) = json(&["golden", missing.to_str().unwrap()]);
    assert_eq!(code, 4);
    assert_eq!(v["result"], "error");
    let r = run(&[
        "postgres",
        "--database-url",
        "not a url",
        "--rules",
        repo().join("rules/v0.toml").to_str().unwrap(),
    ]);
    assert_eq!(r.code, 4);
    assert!(!r.stdout.contains("not a url"), "the URL is never echoed");
}

/// A directory of goldens with one that can't be looked into is an input error, never a pass over
/// the readable rest (phase 2 gate sweep: the search once skipped what it couldn't read).
#[cfg(unix)]
#[test]
fn an_unreadable_golden_is_an_input_error_not_skipped() {
    use std::os::unix::fs::PermissionsExt;
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-cli/unreadable");
    if root.exists() {
        // A previous run's locked directory: unlock it so it can be removed.
        let _ = std::fs::set_permissions(root.join("b"), std::fs::Permissions::from_mode(0o755));
        std::fs::remove_dir_all(&root).unwrap();
    }
    std::fs::create_dir_all(&root).unwrap();
    for name in ["a", "b"] {
        let dir = root.join(name);
        std::fs::create_dir(&dir).unwrap();
        for f in ["start.vfsnap", "records.jsonl", "manifest.json"] {
            std::fs::copy(goldens().join("whole-loop").join(f), dir.join(f)).unwrap();
        }
    }
    assert_eq!(
        json(&["golden", root.to_str().unwrap(), "--rules", &v0()]).0,
        0
    );
    let locked = root.join("b");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::metadata(locked.join("manifest.json")).is_ok();
    let (code, v) = json(&["golden", root.to_str().unwrap(), "--rules", &v0()]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        // Root ignores permissions, so there is nothing unreadable to find.
        eprintln!("skipped: permissions are not enforced for this user");
        return;
    }
    assert_eq!((code, v["result"].as_str()), (4, Some("error")), "{v}");
}

/// The lead's Q3 requirement: --help says what a pass under --build means.
#[test]
fn help_says_what_a_pass_under_build_means() {
    let r = run(&["postgres", "--help"]);
    assert_eq!(r.code, 0);
    let help = r.stdout.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        help.contains("A pass under --build means this binary's code reproduces that log, not that the named build's code was verified"),
        "{help}"
    );
    assert!(
        run(&["--help"])
            .stdout
            .contains("Exit codes: 0 verified, 1 diverged")
    );
}
