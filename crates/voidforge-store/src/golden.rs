//! The golden log format and its verification (Technical Plan P2.6 and P2.7; decision log D-040,
//! D-041). The host's tests record goldens and the replay tool verifies them, both through
//! [`verify_golden`], so there is one path. Everything here works on bytes; reading and writing
//! files is the caller's.
//!
//! A golden is a directory with three files:
//! - `start.vfsnap`: the start World as a snapshot envelope (D-035), under [`FIXTURE`];
//! - `records.jsonl`: the log records, one per line ([`LogRecord::to_line`]);
//! - `manifest.json`: the scenario, the rules version, the build, the start hash, the state hash
//!   after every record, the end time and hash, and every player's score at the end.
//!
//! Goldens are recorded and replayed under [`FIXTURE`], a fixed test-only build, so they replay
//! on every commit; production recovery keeps the real build's rule (D-038).

use serde::{Deserialize, Serialize};
use voidforge_core::rules::Rules;

use crate::log::{LogRecord, player_uuid};
use crate::snapshot::{self, EngineBuild, Expected};
use crate::verify::{self, Report, VerifyError, hash_hex, snapshot_error};

/// The fixture build every golden is recorded and replayed under (D-040).
pub const FIXTURE: &str = "0.1.0+golden";

/// The fixture build.
///
/// # Panics
/// Never: [`FIXTURE`] is a valid build identity.
#[must_use]
pub fn fixture() -> EngineBuild {
    EngineBuild::new(FIXTURE).expect("the fixture build is valid")
}

/// A golden's file names.
pub const FILES: [&str; 3] = ["start.vfsnap", "records.jsonl", "manifest.json"];

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The scenario's name (its directory).
    pub scenario: String,
    /// What it covers.
    pub about: String,
    /// The rules version it was recorded under.
    pub rules_version: u32,
    /// [`FIXTURE`].
    pub engine_build: String,
    /// The start World's state hash.
    pub start_hash: String,
    /// The state hash after each record, in order.
    pub records: Vec<Checkpoint>,
    /// The game time the run ends at.
    pub end_ms: i64,
    /// The state hash at the end.
    pub end_hash: String,
    /// Every player's score at the end.
    pub scores: Vec<Score>,
}

/// The state hash after a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    /// The record.
    pub seq: i64,
    /// Its game time.
    pub game_time: i64,
    /// The state hash after it.
    pub state_hash: String,
}

/// A player's score at the end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Score {
    /// The player, as a UUID.
    pub player: String,
    /// The score.
    pub score: i64,
}

/// A golden's three files, as bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    /// `start.vfsnap`.
    pub start: Vec<u8>,
    /// `records.jsonl`.
    pub records: Vec<u8>,
    /// `manifest.json`.
    pub manifest: Vec<u8>,
}

impl Files {
    /// A file by name, one of [`FILES`].
    ///
    /// # Panics
    /// On any other name.
    #[must_use]
    pub fn get(&self, name: &str) -> &[u8] {
        match name {
            "start.vfsnap" => &self.start,
            "records.jsonl" => &self.records,
            "manifest.json" => &self.manifest,
            _ => panic!("no golden file {name}"),
        }
    }

    /// The manifest.
    ///
    /// # Errors
    /// [`VerifyError::Input`] if it is not a manifest.
    pub fn manifest(&self) -> Result<Manifest, VerifyError> {
        serde_json::from_slice(&self.manifest)
            .map_err(|e| VerifyError::Input(format!("manifest.json: {e}")))
    }

    /// The records, one per line.
    ///
    /// # Errors
    /// [`VerifyError::Input`] if a line is not a record.
    pub fn records(&self) -> Result<Vec<LogRecord>, VerifyError> {
        let text = std::str::from_utf8(&self.records)
            .map_err(|e| VerifyError::Input(format!("records.jsonl: {e}")))?;
        text.lines()
            .enumerate()
            .map(|(i, line)| {
                LogRecord::from_line(line).map_err(|e| {
                    VerifyError::Input(format!("records.jsonl line {}: {e}", i.saturating_add(1)))
                })
            })
            .collect()
    }

    /// `records.jsonl` for `records`.
    ///
    /// # Errors
    /// [`VerifyError::Input`] if a record cannot be written (not expected).
    pub fn records_text(records: &[LogRecord]) -> Result<Vec<u8>, VerifyError> {
        let mut text = String::new();
        for r in records {
            text.push_str(&r.to_line().map_err(|e| VerifyError::Input(e.to_string()))?);
            text.push('\n');
        }
        Ok(text.into_bytes())
    }
}

/// Verifies a golden under [`FIXTURE`] and `rules`: the start decoded strictly, every record
/// replayed through the shared function with the state hash after each, the end (no events due)
/// and its hash, and every player's score (a scoring error is a divergence, never a score). The
/// first failure names its record; for a missing or reordered record that is the seq expected
/// there.
///
/// # Errors
/// [`VerifyError`]: the first divergence, a refusal (another build or rules version), or a file
/// that cannot be read.
pub fn verify_golden(files: &Files, rules: &Rules) -> Result<Report, VerifyError> {
    let manifest = files.manifest()?;
    if manifest.engine_build != FIXTURE {
        return Err(VerifyError::refused(
            None,
            "build",
            FIXTURE,
            manifest.engine_build,
        ));
    }
    if manifest.rules_version != rules.version {
        return Err(VerifyError::refused(
            None,
            "rules_version",
            manifest.rules_version.to_string(),
            rules.version.to_string(),
        ));
    }
    let build = fixture();
    let mut world = snapshot::decode(
        &files.start,
        &Expected {
            rules_version: rules.version,
            engine_build: &build,
        },
    )
    .map_err(|e| snapshot_error(None, e))?
    .world;
    let start_hash = hash_hex(&world, None)?;
    if start_hash != manifest.start_hash {
        return Err(VerifyError::diverged(
            None,
            "state_hash",
            manifest.start_hash,
            start_hash,
        ));
    }
    let records = files.records()?;
    let mut checkpoints = manifest.records.iter();
    let (last, checked) = verify::replay_checked(&mut world, 0, &records, rules, &build, |r| {
        let c = checkpoints.next().ok_or_else(|| {
            VerifyError::diverged(Some(r.seq), "checkpoints", "a checkpoint", "none")
        })?;
        if c.seq != r.seq || c.game_time != r.game_time {
            return Err(VerifyError::diverged(
                Some(r.seq),
                "checkpoints",
                format!("seq {} at {}", c.seq, c.game_time),
                format!("seq {} at {}", r.seq, r.game_time),
            ));
        }
        Ok(Some(c.state_hash.clone()))
    })?;
    // Checkpoints left over: records are missing at the end, from the first one expected.
    if let Some(c) = checkpoints.next() {
        return Err(VerifyError::diverged(
            Some(c.seq),
            "gap",
            c.seq.to_string(),
            "the end of the records",
        ));
    }
    let events = world
        .advance_to(manifest.end_ms, rules)
        .map_err(|e| VerifyError::diverged(None, "end_events", "no engine error", e.to_string()))?;
    if !events.is_empty() {
        return Err(VerifyError::diverged(
            None,
            "end_events",
            "no events",
            crate::log::events_json(&events),
        ));
    }
    let end_hash = hash_hex(&world, None)?;
    if end_hash != manifest.end_hash {
        return Err(VerifyError::diverged(
            None,
            "end_hash",
            manifest.end_hash,
            end_hash,
        ));
    }
    // A scoring error is a failure, never a score: no sentinel value can stand in for it, since
    // a manifest could name that value (phase 2 gate, R2 L1).
    let scores = world
        .players
        .keys()
        .map(|&p| {
            let player = player_uuid(p);
            match world.score(p, manifest.end_ms, rules) {
                Ok(score) => Ok(Score { player, score }),
                Err(e) => Err(VerifyError::diverged(
                    None,
                    "scores",
                    format!("a score for player {player}"),
                    e.to_string(),
                )),
            }
        })
        .collect::<Result<Vec<Score>, VerifyError>>()?;
    if scores != manifest.scores {
        return Err(VerifyError::diverged(
            None,
            "scores",
            format!("{:?}", manifest.scores),
            format!("{scores:?}"),
        ));
    }
    Ok(Report {
        build: FIXTURE.to_owned(),
        rules_version: rules.version,
        start_seq: 0,
        records: records.len(),
        first_seq: records.first().map(|r| r.seq),
        last_seq: last,
        checkpoints: checked.saturating_add(1),
        end_hash,
        world,
    })
}
