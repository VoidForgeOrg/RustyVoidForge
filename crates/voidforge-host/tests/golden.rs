//! Golden logs (Technical Plan P2.6; decision log D-040): the goldens in `tests/golden/` replay
//! exactly, they are what a fresh recording gives (so they change only through `just bless`),
//! a tampered golden is pinpointed, and together they cover every command, event and outcome.
//! `.github/scripts/determinism.sh` runs this binary; `just bless` runs [`bless`].

mod common;

use std::collections::BTreeSet;

use common::golden::{on_disk, read, record, root, write};
use voidforge_core::command::Action;
use voidforge_core::state::DomainEvent;
use voidforge_store::golden::{FILES, FIXTURE, Files, Manifest, fixture, verify_golden};
use voidforge_store::log::{LogRecord, Outcome, Payload, player_uuid};
use voidforge_store::snapshot::ENGINE_BUILD;
use voidforge_store::verify::{Finding, VerifyError};
use voidforge_testkit::golden::scenarios;
use voidforge_testkit::{ALICE, v0_rules};

/// Writes every golden from a fresh recording. Only through `just bless`, which sets
/// `VOIDFORGE_BLESS=1`; a reviewer checks every change (Technical Plan, Agent factory rules).
#[test]
#[ignore = "writes tests/golden: just bless"]
fn bless() {
    assert_eq!(
        std::env::var("VOIDFORGE_BLESS").as_deref(),
        Ok("1"),
        "goldens are written only by `just bless`"
    );
    let names: BTreeSet<&str> = scenarios().iter().map(|s| s.name).collect();
    let stale: Vec<String> = on_disk()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|n| !names.contains(n.as_str()))
        .collect();
    assert!(
        stale.is_empty(),
        "tests/golden holds goldens with no scenario; remove them by hand: {stale:?}"
    );
    for s in scenarios() {
        let files = record(&s);
        let dir = root().join(s.name);
        let before = read(&dir).ok();
        write(&files, &dir).unwrap();
        let state = match before {
            None => "new",
            Some(b) if b == files => "unchanged",
            Some(_) => "CHANGED",
        };
        println!(
            "bless: tests/golden/{}: {state} ({} records)",
            s.name,
            files.records().unwrap().len()
        );
    }
}

#[test]
fn every_golden_replays_exactly() {
    let rules = v0_rules();
    let goldens = on_disk();
    assert!(!goldens.is_empty(), "no goldens in tests/golden");
    for (name, files) in goldens {
        if let Err(f) = verify_golden(&files, &rules) {
            panic!("tests/golden/{name} does not replay: {f:?}");
        }
    }
}

/// The freshness check (D-040): every golden on disk is byte for byte what a fresh recording
/// gives, and every scenario has its golden. A hand edit, or a behavior change that was not
/// blessed, fails here.
#[test]
fn the_goldens_are_what_a_fresh_recording_gives() {
    let disk = on_disk();
    let on_disk_names: Vec<&str> = disk.iter().map(|(n, _)| n.as_str()).collect();
    let mut wanted: Vec<&str> = scenarios().iter().map(|s| s.name).collect();
    wanted.sort_unstable();
    assert_eq!(
        on_disk_names, wanted,
        "tests/golden must hold exactly one golden per scenario: run `just bless`"
    );
    for s in scenarios() {
        let (_, kept) = disk.iter().find(|(n, _)| n == s.name).unwrap();
        if let Some((file, line)) = stale(&record(&s), kept) {
            panic!(
                "tests/golden/{}/{file} differs from a fresh recording (first at line {line}). \
                 Goldens change only through `just bless`, and the PR body must call out the \
                 change for review",
                s.name
            );
        }
    }
}

/// The first golden file that differs from a fresh recording, and the line of the first
/// difference.
fn stale(fresh: &Files, kept: &Files) -> Option<(&'static str, usize)> {
    FILES
        .into_iter()
        .find(|&f| fresh.get(f) != kept.get(f))
        .map(|f| (f, first_difference(fresh.get(f), kept.get(f))))
}

/// A hand edit anywhere in a golden, even one replay cannot see (the manifest's description),
/// fails the freshness check.
#[test]
fn a_hand_edit_fails_the_freshness_check() {
    let s = &scenarios()[0];
    let fresh = record(s);
    assert_eq!(stale(&fresh, &fresh), None);
    let mut m = fresh.manifest().unwrap();
    m.about.push('.');
    let mut edited = serde_json::to_string_pretty(&m).unwrap();
    edited.push('\n');
    let kept = Files {
        manifest: edited.into_bytes(),
        ..fresh.clone()
    };
    assert_eq!(stale(&fresh, &kept), Some(("manifest.json", 3)));
    let kept = Files {
        start: [&fresh.start[..], b"\0"].concat(),
        ..fresh.clone()
    };
    assert_eq!(stale(&fresh, &kept).map(|(f, _)| f), Some("start.vfsnap"));
}

fn first_difference(a: &[u8], b: &[u8]) -> usize {
    let at = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    a[..at].iter().filter(|&&c| c == b'\n').count() + 1
}

/// D-040: the fixture build is a test-only identity. A real build is `<version>+<commit>`
/// (D-035), never the fixture, so production recovery never treats a golden as its own.
#[test]
fn the_fixture_build_is_never_a_real_build() {
    assert_ne!(ENGINE_BUILD, FIXTURE);
    assert!(!ENGINE_BUILD.ends_with("+golden"), "{ENGINE_BUILD}");
    assert_eq!(fixture().as_str(), FIXTURE);
}

/// A golden and the index of its first record with several events that is not its last record.
fn a_golden_with_events() -> (String, Files, usize) {
    on_disk()
        .into_iter()
        .find_map(|(name, files)| {
            let records = files.records().unwrap();
            let at = records
                .iter()
                .take(records.len().saturating_sub(1))
                .position(|r| r.events.len() > 1)?;
            Some((name, files, at))
        })
        .expect("a golden record with several events, before the last")
}

fn with_records(files: &Files, records: &[LogRecord]) -> Files {
    Files {
        records: Files::records_text(records).unwrap(),
        ..files.clone()
    }
}

/// Where a golden fails: its seq and what differed.
fn failure(files: &Files) -> (Option<i64>, &'static str) {
    let e = verify_golden(files, &v0_rules()).unwrap_err();
    let f = e.finding().expect("a finding, not an input error");
    (f.seq, f.what)
}

fn with_manifest(files: &Files, edit: impl FnOnce(&mut Manifest)) -> Files {
    let mut m = files.manifest().unwrap();
    edit(&mut m);
    Files {
        manifest: serde_json::to_vec_pretty(&m).unwrap(),
        ..files.clone()
    }
}

/// A tampered golden is pinpointed at its record (the replay tool's matrix is in its own tests;
/// these keep the host's goldens honest on their own).
#[test]
fn tampered_goldens_are_pinpointed() {
    let (name, files, at) = a_golden_with_events();
    let records = files.records().unwrap();
    let seq = records[at].seq;
    // Two events of a record swapped: the same events, in another order.
    let mut swapped = records.clone();
    swapped[at].events.swap(0, 1);
    assert_eq!(
        failure(&with_records(&files, &swapped)),
        (Some(seq), "events"),
        "{name}"
    );
    // A state hash edited.
    let tampered = with_manifest(&files, |m| m.records[at].state_hash = "00".repeat(32));
    assert_eq!(failure(&tampered), (Some(seq), "state_hash"));
    // A record dropped: the seq expected there is reported.
    let mut dropped = records.clone();
    dropped.remove(at);
    let mut m = files.manifest().unwrap();
    m.records.remove(at);
    let tampered = Files {
        manifest: serde_json::to_vec_pretty(&m).unwrap(),
        ..with_records(&files, &dropped)
    };
    assert_eq!(failure(&tampered), (Some(seq), "gap"));
    // The end hash, a checkpoint's time, an extra checkpoint and a score.
    let tampered = with_manifest(&files, |m| m.end_hash = "00".repeat(32));
    assert_eq!(failure(&tampered), (None, "end_hash"));
    let tampered = with_manifest(&files, |m| m.records[at].game_time += 1);
    assert_eq!(failure(&tampered), (Some(seq), "checkpoints"));
    let tampered = with_manifest(&files, |m| {
        let mut extra = m.records.last().unwrap().clone();
        extra.seq += 1;
        m.records.push(extra);
    });
    let missing = records.last().unwrap().seq + 1;
    assert_eq!(failure(&tampered), (Some(missing), "gap"));
    let tampered = with_manifest(&files, |m| m.scores[0].score += 1);
    assert_eq!(failure(&tampered), (None, "scores"));
}

/// A scoring error is a divergence, never a score (phase 2 gate, R2 L1): with a planet worth
/// `i64::MAX`, every player's score is out of range, and a manifest naming the sentinel the
/// verifier once substituted (`i64::MIN`) still fails, naming the player and the core's error.
#[test]
fn a_scoring_error_is_never_verified_as_a_score() {
    let (name, files) = on_disk().into_iter().next().unwrap();
    let mut rules = v0_rules();
    rules.scoring.planet = i64::MAX;
    rules.validate().unwrap();
    // Scoring weights don't change the simulation: the records and hashes still replay, and only
    // the end's scores fail.
    let end = files.manifest().unwrap().end_ms;
    let world = verify_golden(&files, &v0_rules()).unwrap().world;
    let player = *world.players.keys().next().unwrap();
    let error = world.score(player, end, &rules).unwrap_err();
    let sentinel = with_manifest(&files, |m| {
        for s in &mut m.scores {
            s.score = i64::MIN;
        }
    });
    let e = verify_golden(&sentinel, &rules).unwrap_err();
    let VerifyError::Diverged(f) = e else {
        panic!("{name}: a divergence, not {e:?}")
    };
    assert_eq!((f.seq, f.what), (None, "scores"), "{name}");
    assert_eq!(
        f.expected,
        format!("a score for player {}", player_uuid(player))
    );
    assert_eq!(f.found, error.to_string());
}

/// The goldens replay only under the fixture build and the rules they were recorded with.
#[test]
fn a_golden_replays_only_under_its_build_and_rules() {
    let (_, files) = on_disk().into_iter().next().unwrap();
    let other = with_manifest(&files, |m| m.engine_build = ENGINE_BUILD.to_owned());
    assert!(matches!(
        verify_golden(&other, &v0_rules()),
        Err(VerifyError::Refused(Finding { what: "build", .. }))
    ));
    let mut rules = v0_rules();
    rules.version += 1;
    assert!(matches!(
        verify_golden(&files, &rules),
        Err(VerifyError::Refused(Finding {
            what: "rules_version",
            ..
        }))
    ));
}

fn action_index(a: &Action) -> usize {
    // Exhaustive: a new command fails to compile here, then fails the coverage check until a
    // golden covers it.
    match a {
        Action::JoinGame => 0,
        Action::BuildBuilding { .. } => 1,
        Action::CancelConstruction { .. } => 2,
        Action::DemolishBuilding { .. } => 3,
        Action::QueueShip { .. } => 4,
        Action::CancelShipBuild { .. } => 5,
        Action::AssembleFleet { .. } => 6,
        Action::SendFleet { .. } => 7,
        Action::RecallFleet { .. } => 8,
        Action::DisbandFleet { .. } => 9,
        Action::TriggerUnload { .. } => 10,
    }
}
const ACTIONS: usize = 11;

fn event_index(e: &DomainEvent) -> usize {
    match e {
        DomainEvent::BuildingCompleted { .. } => 0,
        DomainEvent::ShipCompleted { .. } => 1,
        DomainEvent::DemolitionCompleted { .. } => 2,
        DomainEvent::FleetArrived { .. } => 3,
        DomainEvent::PlanetColonized { .. } => 4,
        DomainEvent::ColonizationFailed { .. } => 5,
        DomainEvent::CargoUnloaded { .. } => 6,
        DomainEvent::FleetRemoved { .. } => 7,
        DomainEvent::FleetReturned { .. } => 8,
    }
}
const EVENTS: usize = 9;

/// Every command was accepted at least once, every event occurs, at least one command was
/// rejected, and advance records occur, across the goldens.
#[test]
fn the_goldens_cover_every_command_event_and_outcome() {
    let mut accepted = BTreeSet::new();
    let mut events = BTreeSet::new();
    let (mut rejected, mut advances) = (0, 0);
    for (_, files) in on_disk() {
        for r in files.records().unwrap() {
            events.extend(r.events.iter().map(event_index));
            match (&r.payload, r.outcome) {
                (Payload::Command(c), Outcome::Accepted { .. }) => {
                    accepted.insert(action_index(&c.action));
                }
                (Payload::Command(_), Outcome::Rejected(_)) => rejected += 1,
                (Payload::Empty, Outcome::Empty) => advances += 1,
                other => panic!("an odd record: {other:?}"),
            }
        }
    }
    assert_eq!(accepted, (0..ACTIONS).collect(), "commands accepted");
    assert_eq!(events, (0..EVENTS).collect(), "events");
    assert!(
        rejected > 0 && advances > 0,
        "{rejected} rejected, {advances} advances"
    );
}

/// The Phase 1 gate's whole loop: its golden's score at the end is QA's hand derivation
/// (`crates/voidforge-testkit/tests/scenario.rs`): 2,000 + 750 + 100 + 150 + 213 + 26.
#[test]
fn the_whole_loop_golden_scores_as_derived_by_hand() {
    let (_, files) = on_disk()
        .into_iter()
        .find(|(n, _)| n == "whole-loop")
        .expect("the whole-loop golden");
    let m = files.manifest().unwrap();
    assert_eq!(m.end_ms, 675 * voidforge_core::quantity::MS_PER_HOUR / 10);
    assert_eq!(m.scores.len(), 1);
    assert_eq!(m.scores[0].player, player_uuid(ALICE));
    assert_eq!(m.scores[0].score, 3_239);
}
