//! The one record→core function (Technical Plan, Project layering; decision log D-036, D-038).
//!
//! Host recovery and replay map a log record to `advance_to` or `apply` through [`step`], so they
//! cannot drift apart: the host's live path builds every record from [`step`]'s result, and
//! [`replay_record`] runs the same function on a logged record and checks that it produces the
//! logged events and outcome. The replay tool (P2.7) reuses both. It lives in store, the crate
//! that both the host and the replay tool may depend on and that defines the record.

use voidforge_core::command::{Command, Rejection};
use voidforge_core::rules::Rules;
use voidforge_core::state::{DomainEvent, EngineError, World};

use crate::log::{LogRecord, Outcome, Payload, RecordKind};
use crate::snapshot::EngineBuild;

/// What a step runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step<'a> {
    /// `apply(command, t)`.
    Command(&'a Command),
    /// `advance_to(t)`.
    Advance,
}

/// What a step produced, in the record's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stepped {
    /// Every event, flattened: an apply's advance, then the command's own (D-036).
    pub events: Vec<DomainEvent>,
    /// The record's outcome: `Empty` for an advance.
    pub outcome: Outcome,
    /// For a command: its own events, or its rejection. For an advance: its events.
    pub result: Result<Vec<DomainEvent>, Rejection>,
}

/// Runs one step at `t`: the only way the host and replay change a World.
///
/// # Errors
/// The core's [`EngineError`], as `apply` or `advance_to` returns it (a poisoning error leaves
/// the World poisoned, D-023).
pub fn step(
    world: &mut World,
    step: &Step<'_>,
    t: i64,
    rules: &Rules,
) -> Result<Stepped, EngineError> {
    match step {
        Step::Advance => {
            let events = world.advance_to(t, rules)?;
            Ok(Stepped {
                events: events.clone(),
                outcome: Outcome::Empty,
                result: Ok(events),
            })
        }
        Step::Command(command) => {
            let applied = world.apply(command, t, rules)?;
            let mut events = applied.advanced;
            Ok(match applied.outcome {
                Ok(own) => {
                    events.extend_from_slice(&own);
                    // A command produces a handful of events; u32 is far beyond any.
                    let own_events = u32::try_from(own.len()).expect("own events fit u32");
                    Stepped {
                        events,
                        outcome: Outcome::Accepted { own_events },
                        result: Ok(own),
                    }
                }
                Err(reason) => Stepped {
                    events,
                    outcome: Outcome::Rejected(reason),
                    result: Err(reason),
                },
            })
        }
    }
}

/// Why a logged record could not be replayed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplayError {
    /// The record was written by another build: replays never cross builds.
    #[error("record {seq} was written by build {found}, not {expected}")]
    CrossBuildRecord {
        /// The record.
        seq: i64,
        /// The running build.
        expected: String,
        /// The record's build.
        found: String,
    },
    /// A record is missing: the tail must continue exactly after the last one replayed.
    #[error("expected record {expected}, found {found}")]
    Gap {
        /// The next seq.
        expected: i64,
        /// The seq found.
        found: i64,
    },
    /// The record was written under another rules version than the rules supplied: replay
    /// refuses a mismatch (Architectural Design, Determinism rules).
    #[error("record {seq} is for rules version {found}, replaying with {expected}")]
    RulesMismatch {
        /// The record.
        seq: i64,
        /// The rules supplied.
        expected: u32,
        /// The record's.
        found: u32,
    },
    /// A record kind this build does not replay yet (`rules_change`, `system`).
    #[error("record {seq}: {kind} records are not replayed by this build")]
    Unsupported {
        /// The record.
        seq: i64,
        /// Its kind.
        kind: &'static str,
    },
    /// The record's kind and payload disagree.
    #[error("record {seq} is malformed: {what}")]
    Malformed {
        /// The record.
        seq: i64,
        /// What is wrong.
        what: &'static str,
    },
    /// Replaying the record produced other events or another outcome than it holds.
    #[error("record {seq} diverged: replay produced different {what}")]
    Diverged {
        /// The record.
        seq: i64,
        /// "events" or "outcome".
        what: &'static str,
        /// What the record holds, in its log JSON form.
        recorded: String,
        /// What replay produced, in the same form.
        replayed: String,
    },
    /// The engine failed on the record.
    #[error("record {seq}: {error}")]
    Engine {
        /// The record.
        seq: i64,
        /// The engine's error.
        error: EngineError,
    },
}

/// Replays one logged record on `world` through [`step`] and checks that it produces exactly the
/// record's events and outcome. The record must come from `build`.
///
/// # Errors
/// [`ReplayError`]: another build's record, an unsupported kind, a malformed record, an engine
/// error, or a divergence.
pub fn replay_record(
    world: &mut World,
    record: &LogRecord,
    rules: &Rules,
    build: &EngineBuild,
) -> Result<(), ReplayError> {
    let seq = record.seq;
    if &record.engine_build != build {
        return Err(ReplayError::CrossBuildRecord {
            seq,
            expected: build.as_str().to_owned(),
            found: record.engine_build.as_str().to_owned(),
        });
    }
    if record.rules_version != rules.version {
        return Err(ReplayError::RulesMismatch {
            seq,
            expected: rules.version,
            found: record.rules_version,
        });
    }
    let what = match (record.kind, &record.payload) {
        (RecordKind::Command, Payload::Command(c)) if record.player != Some(c.player) => {
            return Err(ReplayError::Malformed {
                seq,
                what: "the player column differs from the command's player",
            });
        }
        (RecordKind::Command, Payload::Command(c)) => Step::Command(c),
        (RecordKind::Advance, Payload::Empty) => Step::Advance,
        (RecordKind::Command | RecordKind::Advance, _) => {
            return Err(ReplayError::Malformed {
                seq,
                what: "the kind and payload disagree",
            });
        }
        (kind, _) => {
            return Err(ReplayError::Unsupported {
                seq,
                kind: kind.as_str(),
            });
        }
    };
    let stepped = step(world, &what, record.game_time, rules)
        .map_err(|error| ReplayError::Engine { seq, error })?;
    if stepped.events != record.events {
        return Err(ReplayError::Diverged {
            seq,
            what: "events",
            recorded: crate::log::events_json(&record.events),
            replayed: crate::log::events_json(&stepped.events),
        });
    }
    if stepped.outcome != record.outcome {
        return Err(ReplayError::Diverged {
            seq,
            what: "outcome",
            recorded: crate::log::outcome_text(record.outcome),
            replayed: crate::log::outcome_text(stepped.outcome),
        });
    }
    Ok(())
}

/// Replays a tail of records, in order and gap-free, starting right after `after` (the
/// snapshot's seq). Returns the last seq replayed (`after` for an empty tail).
///
/// # Errors
/// The first [`ReplayError`]; the World must then be discarded.
pub fn replay(
    world: &mut World,
    after: i64,
    records: &[LogRecord],
    rules: &Rules,
    build: &EngineBuild,
) -> Result<i64, ReplayError> {
    let mut last = after;
    for record in records {
        let expected = last.saturating_add(1);
        if record.seq != expected {
            return Err(ReplayError::Gap {
                expected,
                found: record.seq,
            });
        }
        replay_record(world, record, rules, build)?;
        last = record.seq;
    }
    Ok(last)
}
