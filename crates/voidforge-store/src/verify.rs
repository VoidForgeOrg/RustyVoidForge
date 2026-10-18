//! Verifying a log offline (Technical Plan P2.7; decision log D-041): a start snapshot decoded
//! strictly, every later record replayed through [`replay::replay_record`] (the one record→core
//! path the host's recovery uses), and the state hash checked wherever the source records one.
//! [`verify_log`] reads any [`LogStore`], whose later snapshots are its hash checkpoints;
//! [`crate::golden::verify_golden`] reads a golden, which records a hash after every record.
//!
//! A failure is one of three kinds, which the replay tool maps to its exit codes:
//! - [`VerifyError::Diverged`]: the source and a replay disagree, at the first divergent record;
//! - [`VerifyError::Refused`]: the source is pinned to another build or rules version (replays
//!   never cross either);
//! - [`VerifyError::Input`]: the source cannot be read.

use voidforge_core::rules::Rules;
use voidforge_core::state::World;

use crate::log::{LogError, LogRecord, LogStore, SnapshotRow};
use crate::replay::{self, ReplayError};
use crate::snapshot::{self, EngineBuild, Expected, SnapshotError};

/// Where and how a source failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The record (or snapshot seq) it is at, if any. For a gap, the seq that was expected: the
    /// first position that is not as it should be.
    pub seq: Option<i64>,
    /// What differed: `events`, `outcome`, `state_hash`, `gap`, `engine`, `malformed`,
    /// `snapshot`, `checkpoints`, `end_events`, `end_hash`, `scores`; for a refusal, `build`,
    /// `rules_version` or `kind`.
    pub what: &'static str,
    /// What the source holds (or the pin expects, for a refusal).
    pub expected: String,
    /// What replay found.
    pub found: String,
}

/// Why a source does not verify.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    /// The source and a replay disagree.
    #[error("diverged at {}: {} (expected {}, found {})", seq_text(.0.seq), .0.what, .0.expected, .0.found)]
    Diverged(Finding),
    /// The source is pinned to another build or rules version.
    #[error("refused at {}: {} (expected {}, found {})", seq_text(.0.seq), .0.what, .0.expected, .0.found)]
    Refused(Finding),
    /// The source cannot be read.
    #[error("input error: {0}")]
    Input(String),
}

fn seq_text(seq: Option<i64>) -> String {
    seq.map_or_else(|| "the start".to_owned(), |s| format!("seq {s}"))
}

impl VerifyError {
    /// The finding, unless the source could not be read.
    #[must_use]
    pub fn finding(&self) -> Option<&Finding> {
        match self {
            Self::Diverged(f) | Self::Refused(f) => Some(f),
            Self::Input(_) => None,
        }
    }

    pub(crate) fn diverged(
        seq: Option<i64>,
        what: &'static str,
        expected: impl Into<String>,
        found: impl Into<String>,
    ) -> Self {
        Self::Diverged(Finding {
            seq,
            what,
            expected: expected.into(),
            found: found.into(),
        })
    }

    pub(crate) fn refused(
        seq: Option<i64>,
        what: &'static str,
        expected: impl Into<String>,
        found: impl Into<String>,
    ) -> Self {
        Self::Refused(Finding {
            seq,
            what,
            expected: expected.into(),
            found: found.into(),
        })
    }
}

impl From<ReplayError> for VerifyError {
    fn from(e: ReplayError) -> Self {
        match e {
            ReplayError::CrossBuildRecord {
                seq,
                expected,
                found,
            } => Self::refused(Some(seq), "build", expected, found),
            ReplayError::RulesMismatch {
                seq,
                expected,
                found,
            } => Self::refused(
                Some(seq),
                "rules_version",
                expected.to_string(),
                found.to_string(),
            ),
            ReplayError::Unsupported { seq, kind } => {
                Self::refused(Some(seq), "kind", "command or advance", kind)
            }
            ReplayError::Gap { expected, found } => Self::diverged(
                Some(expected),
                "gap",
                expected.to_string(),
                found.to_string(),
            ),
            ReplayError::Malformed { seq, what } => {
                Self::diverged(Some(seq), "malformed", "a well-formed record", what)
            }
            ReplayError::Diverged {
                seq,
                what,
                recorded,
                replayed,
            } => Self::diverged(Some(seq), what, recorded, replayed),
            ReplayError::Engine { seq, error } => {
                Self::diverged(Some(seq), "engine", "no engine error", error.to_string())
            }
        }
    }
}

impl From<LogError> for VerifyError {
    fn from(e: LogError) -> Self {
        Self::Input(e.to_string())
    }
}

/// A snapshot decoding failure at `seq`: a pin mismatch is a refusal, anything else a
/// divergence of the snapshot from its own header.
pub(crate) fn snapshot_error(seq: Option<i64>, e: SnapshotError) -> VerifyError {
    match e {
        SnapshotError::BuildMismatch { expected, found } => {
            VerifyError::refused(seq, "build", expected, found)
        }
        SnapshotError::RulesMismatch { expected, found } => VerifyError::refused(
            seq,
            "rules_version",
            expected.to_string(),
            found.to_string(),
        ),
        other => VerifyError::diverged(seq, "snapshot", "a valid snapshot", other.to_string()),
    }
}

/// A World's state hash as lowercase hex.
///
/// # Errors
/// [`VerifyError::Diverged`] (`state_hash`) if the World cannot be hashed (it is poisoned).
pub fn hash_hex(world: &World, seq: Option<i64>) -> Result<String, VerifyError> {
    snapshot::state_hash(world)
        .map(|h| hex(&h))
        .map_err(|e| VerifyError::diverged(seq, "state_hash", "a hashable World", e.to_string()))
}

/// Bytes as lowercase hex.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// What a verified source holds.
#[derive(Debug, Clone)]
pub struct Report {
    /// The build replayed under.
    pub build: String,
    /// The rules version replayed under.
    pub rules_version: u32,
    /// The start snapshot's seq (0 for a golden).
    pub start_seq: i64,
    /// How many records were replayed.
    pub records: usize,
    /// The first record replayed, if any.
    pub first_seq: Option<i64>,
    /// The last record replayed (the start's seq when none).
    pub last_seq: i64,
    /// How many recorded state hashes were checked, the start's included.
    pub checkpoints: usize,
    /// The state hash at the end.
    pub end_hash: String,
    /// The World at the end.
    pub world: World,
}

/// Replays `records` on `world` from `after`, each through [`replay::replay`] (gap-free), and
/// checks the state hash after every record that `checkpoint` names one for. Returns the last
/// seq and the number of hashes checked.
pub(crate) fn replay_checked(
    world: &mut World,
    after: i64,
    records: &[LogRecord],
    rules: &Rules,
    build: &EngineBuild,
    mut checkpoint: impl FnMut(&LogRecord) -> Result<Option<String>, VerifyError>,
) -> Result<(i64, usize), VerifyError> {
    let (mut last, mut checked) = (after, 0_usize);
    for r in records {
        replay::replay(world, last, std::slice::from_ref(r), rules, build)?;
        last = r.seq;
        if let Some(expected) = checkpoint(r)? {
            let found = hash_hex(world, Some(r.seq))?;
            if found != expected {
                return Err(VerifyError::diverged(
                    Some(r.seq),
                    "state_hash",
                    expected,
                    found,
                ));
            }
            checked = checked.saturating_add(1);
        }
    }
    Ok((last, checked))
}

/// Verifies a store's log offline under `build` and `rules`: the snapshot at `from` (the
/// earliest when `None`) decoded strictly and cross-checked against its row, then every later
/// record up to `to` (the head when `None`) replayed through the shared function, with the state
/// hash checked at every later snapshot row.
///
/// # Errors
/// [`VerifyError`]: the first divergence, a refusal (another build or rules version), or an
/// unreadable source (no snapshot at `from`, or none at all).
pub async fn verify_log<S: LogStore>(
    store: &mut S,
    rules: &Rules,
    build: &EngineBuild,
    from: Option<i64>,
    to: Option<i64>,
) -> Result<Report, VerifyError> {
    let index = store.snapshot_index().await?;
    // A snapshot past the log head claims records the log does not hold (as recovery refuses,
    // D-038), wherever it is in the index.
    let head = store.last().await?.map_or(0, |(seq, _)| seq);
    if let Some(past) = index.iter().find(|s| s.seq > head) {
        return Err(VerifyError::diverged(
            Some(past.seq),
            "snapshot",
            format!("a snapshot at or before the log head (seq {head})"),
            format!("a snapshot at seq {}", past.seq),
        ));
    }
    let start = match from {
        Some(seq) => index
            .iter()
            .find(|s| s.seq == seq)
            .ok_or_else(|| VerifyError::Input(format!("no snapshot at seq {seq}")))?,
        None => index
            .first()
            .ok_or_else(|| VerifyError::Input("the store holds no snapshot".to_owned()))?,
    };
    let row = store
        .snapshot_at(start.seq)
        .await?
        .ok_or_else(|| VerifyError::Input(format!("the snapshot at seq {} vanished", start.seq)))?;
    let seq = row.seq;
    let mut world = decode_row(&row, rules, build)?;
    let records: Vec<LogRecord> = store
        .records_after(seq)
        .await?
        .into_iter()
        .filter(|r| to.is_none_or(|t| r.seq <= t))
        .collect();
    // Every later snapshot is a checkpoint: decoded strictly and cross-checked like the start,
    // then compared with the replayed World brought to the snapshot's own game time. A snapshot
    // may follow unlogged eventless advances (D-036), which bringing the World there reproduces;
    // events in between would have been logged, so any is a divergence.
    let mut later = Vec::new();
    for meta in index.iter().filter(|s| s.seq > seq) {
        if records.last().is_some_and(|r| meta.seq <= r.seq) {
            let row = store.snapshot_at(meta.seq).await?.ok_or_else(|| {
                VerifyError::Input(format!("the snapshot at seq {} vanished", meta.seq))
            })?;
            // Strict decoding refuses a corrupt payload; the columns must match its header.
            decode_row(&row, rules, build)?;
            later.push(row);
        }
    }
    let (mut last, mut checked) = (seq, 0_usize);
    for r in &records {
        replay::replay(&mut world, last, std::slice::from_ref(r), rules, build)?;
        last = r.seq;
        for row in later.iter().filter(|row| row.seq == r.seq) {
            let at = Some(row.seq);
            let mut brought = world.clone();
            let events = brought.advance_to(row.game_time, rules).map_err(|e| {
                VerifyError::diverged(at, "snapshot", "no engine error", e.to_string())
            })?;
            if !events.is_empty() {
                return Err(VerifyError::diverged(
                    at,
                    "snapshot",
                    format!("no events before the snapshot's time {}", row.game_time),
                    crate::log::events_json(&events),
                ));
            }
            let (expected, found) = (hex(&row.state_hash), hash_hex(&brought, at)?);
            if found != expected {
                return Err(VerifyError::diverged(at, "state_hash", expected, found));
            }
            checked = checked.saturating_add(1);
        }
    }
    Ok(Report {
        build: build.as_str().to_owned(),
        rules_version: rules.version,
        start_seq: seq,
        records: records.len(),
        first_seq: records.first().map(|r| r.seq),
        last_seq: last,
        checkpoints: checked.saturating_add(1),
        end_hash: hash_hex(&world, Some(last))?,
        world,
    })
}

/// Decodes a snapshot row strictly under the pin and checks its columns against its header.
fn decode_row(row: &SnapshotRow, rules: &Rules, build: &EngineBuild) -> Result<World, VerifyError> {
    let seq = Some(row.seq);
    let decoded = snapshot::decode(
        &row.payload,
        &Expected {
            rules_version: rules.version,
            engine_build: build,
        },
    )
    .map_err(|e| snapshot_error(seq, e))?;
    let h = &decoded.header;
    let columns = [
        (
            "game_time",
            h.game_time_ms.to_string(),
            row.game_time.to_string(),
        ),
        (
            "rules_version",
            h.rules_version.to_string(),
            row.rules_version.to_string(),
        ),
        (
            "engine_build",
            h.engine_build.clone(),
            row.engine_build.clone(),
        ),
        ("state_hash", hex(&h.state_hash), hex(&row.state_hash)),
    ];
    for (column, header, found) in columns {
        if header != found {
            return Err(VerifyError::diverged(
                seq,
                "snapshot",
                format!("{column} {header}"),
                found,
            ));
        }
    }
    Ok(decoded.world)
}
