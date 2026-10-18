//! The append-only log: typed records, the store interface, an in-memory store for tests and the
//! Postgres store (Architectural Design, Persistence; decision log D-034, D-036).
//!
//! Records are typed, never `serde_json::Value`: a `PlayerId` is a `u128`, which a `Value`
//! cannot hold exactly. The Postgres store writes the JSON columns as text and reads them back
//! as text into the types, and jsonb keeps big integers exactly.
//!
//! The JSON forms (D-034: JSON values, never SQL NULL):
//! - `payload`: the command's serde JSON, or `null` for a record without one;
//! - `outcome`: `{"result":"accepted"}`, `{"result":"rejected","reason":"<code>"}` (the reason
//!   is [`Rejection::code`], D-029), or `null`;
//! - `events`: a JSON array of domain events.

use std::future::Future;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use voidforge_core::command::{Command, Rejection};
use voidforge_core::state::{DomainEvent, PlayerId};

use crate::snapshot::EngineBuild;

/// What a record is (the `kind` column).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    /// A player's command, applied at the record's time.
    Command,
    /// Events that came due with no command.
    Advance,
    /// A rules table switch (not written yet).
    RulesChange,
    /// A system record (not written yet).
    System,
}

impl RecordKind {
    /// The `kind` column's value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Advance => "advance",
            Self::RulesChange => "rules_change",
            Self::System => "system",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "command" => Self::Command,
            "advance" => Self::Advance,
            "rules_change" => Self::RulesChange,
            "system" => Self::System,
            _ => return None,
        })
    }
}

/// The `payload` column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    /// JSON `null`: an advance.
    Empty,
    /// The command.
    Command(Command),
}

/// The `outcome` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// JSON `null`: an advance.
    Empty,
    /// The command was accepted.
    Accepted,
    /// The command was rejected with this reason.
    Rejected(Rejection),
}

/// The outcome's JSON object.
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
enum OutcomeJson {
    Accepted,
    Rejected { reason: Rejection },
}

/// One `log_records` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    /// Gap-free order, from 1.
    pub seq: i64,
    /// What it is.
    pub kind: RecordKind,
    /// The stamped time t, in game milliseconds.
    pub game_time: i64,
    /// When it was written, Unix milliseconds; audit only.
    pub wall_time_ms: i64,
    /// The issuing player.
    pub player: Option<PlayerId>,
    /// The client's key (P2.4).
    pub idempotency_key: Option<String>,
    /// The command, if any.
    pub payload: Payload,
    /// Its outcome, if any.
    pub outcome: Outcome,
    /// The domain events the step produced, in order.
    pub events: Vec<DomainEvent>,
    /// The rules in force at t.
    pub rules_version: u32,
    /// The build that wrote it.
    pub engine_build: EngineBuild,
}

/// Why the log could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// The database refused or failed.
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    /// A value does not fit its column (for example a rules version above `i32::MAX`).
    #[error("record {seq}: {what}")]
    Unrepresentable {
        /// The record.
        seq: i64,
        /// What does not fit.
        what: String,
    },
    /// A stored row is not a valid record.
    #[error("stored record {seq} is invalid: {what}")]
    Corrupt {
        /// The record.
        seq: i64,
        /// What is wrong.
        what: String,
    },
    /// The in-memory store refused, as a database would (a duplicate seq), or a test injected a
    /// failure.
    #[error("in-memory log: {0}")]
    Memory(String),
}

/// A durable, append-only log. `append` returns only once every record is durable, and appends
/// all of them or none.
pub trait LogStore: Send + 'static {
    /// The last record's seq and game time, or `None` for an empty log.
    fn last(&mut self) -> impl Future<Output = Result<Option<(i64, i64)>, LogError>> + Send;
    /// Appends `records` durably, all or none.
    fn append(
        &mut self,
        records: &[LogRecord],
    ) -> impl Future<Output = Result<(), LogError>> + Send;
    /// Every record, in seq order.
    fn records(&mut self) -> impl Future<Output = Result<Vec<LogRecord>, LogError>> + Send;
}

// --- JSON and UUID forms ---

fn payload_json(p: &Payload) -> Result<String, serde_json::Error> {
    match p {
        Payload::Empty => Ok("null".to_owned()),
        Payload::Command(c) => serde_json::to_string(c),
    }
}

fn outcome_json(o: Outcome) -> Result<String, serde_json::Error> {
    match o {
        Outcome::Empty => Ok("null".to_owned()),
        Outcome::Accepted => serde_json::to_string(&OutcomeJson::Accepted),
        Outcome::Rejected(reason) => serde_json::to_string(&OutcomeJson::Rejected { reason }),
    }
}

fn parse_payload(s: &str) -> Result<Payload, serde_json::Error> {
    Ok(match serde_json::from_str::<Option<Command>>(s)? {
        None => Payload::Empty,
        Some(c) => Payload::Command(c),
    })
}

fn parse_outcome(s: &str) -> Result<Outcome, serde_json::Error> {
    Ok(match serde_json::from_str::<Option<OutcomeJson>>(s)? {
        None => Outcome::Empty,
        Some(OutcomeJson::Accepted) => Outcome::Accepted,
        Some(OutcomeJson::Rejected { reason }) => Outcome::Rejected(reason),
    })
}

/// A player ID as a UUID string, hyphenated lowercase hex.
#[must_use]
pub fn player_uuid(p: PlayerId) -> String {
    let h = format!("{:032x}", p.0);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn parse_player_uuid(s: &str) -> Option<PlayerId> {
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    (hex.len() == 32)
        .then(|| u128::from_str_radix(&hex, 16).ok())
        .flatten()
        .map(PlayerId)
}

/// The JSON text of a record's columns, as the Postgres store writes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordJson {
    /// `payload`.
    pub payload: String,
    /// `outcome`.
    pub outcome: String,
    /// `events`.
    pub events: String,
}

impl LogRecord {
    /// The record's JSON columns.
    ///
    /// # Errors
    /// [`LogError::Unrepresentable`] if serde_json refuses a value (not expected).
    pub fn json(&self) -> Result<RecordJson, LogError> {
        let bad = |e: serde_json::Error| LogError::Unrepresentable {
            seq: self.seq,
            what: e.to_string(),
        };
        Ok(RecordJson {
            payload: payload_json(&self.payload).map_err(bad)?,
            outcome: outcome_json(self.outcome).map_err(bad)?,
            events: serde_json::to_string(&self.events).map_err(bad)?,
        })
    }
}

// --- The in-memory store ---

#[derive(Debug, Default)]
struct MemoryInner {
    records: Vec<LogRecord>,
    fail_appends: bool,
    appends: u64,
}

/// An in-memory log for tests. Clones share one log, so a test keeps a clone to read records and
/// inject failures while the host owns another.
#[derive(Debug, Clone, Default)]
pub struct MemoryLog(Arc<Mutex<MemoryInner>>);

impl MemoryLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes every later append fail (true) or succeed again (false).
    pub fn fail_appends(&self, fail: bool) {
        self.lock().fail_appends = fail;
    }

    /// The records so far.
    #[must_use]
    pub fn snapshot(&self) -> Vec<LogRecord> {
        self.lock().records.clone()
    }

    /// How many appends succeeded (one per durable write: group commit puts several records in
    /// one).
    #[must_use]
    pub fn appends(&self) -> u64 {
        self.lock().appends
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryInner> {
        // A poisoned lock means a test panicked while holding it; keep the data readable.
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl LogStore for MemoryLog {
    async fn last(&mut self) -> Result<Option<(i64, i64)>, LogError> {
        Ok(self.lock().records.last().map(|r| (r.seq, r.game_time)))
    }

    async fn append(&mut self, records: &[LogRecord]) -> Result<(), LogError> {
        let mut inner = self.lock();
        if inner.fail_appends {
            return Err(LogError::Memory("injected append failure".to_owned()));
        }
        // The same refusals the table's constraints make, all or none.
        for (i, r) in records.iter().enumerate() {
            let duplicate = inner
                .records
                .iter()
                .chain(&records[..i])
                .any(|o| o.seq == r.seq);
            if duplicate || r.seq < 1 {
                return Err(LogError::Memory(format!("seq {} refused", r.seq)));
            }
            i32::try_from(r.rules_version).map_err(|_| LogError::Unrepresentable {
                seq: r.seq,
                what: format!("rules version {} exceeds i32", r.rules_version),
            })?;
        }
        inner.records.extend_from_slice(records);
        inner.records.sort_by_key(|r| r.seq);
        inner.appends += 1;
        Ok(())
    }

    async fn records(&mut self) -> Result<Vec<LogRecord>, LogError> {
        Ok(self.lock().records.clone())
    }
}

// --- The Postgres store ---

/// The Postgres log, on the P2.1 schema. One `append` is one transaction, so a group of records
/// shares one durable commit.
#[derive(Debug, Clone)]
pub struct PgLog {
    pool: PgPool,
}

impl PgLog {
    /// A log on `pool`, whose database has the migrations applied. A connection belongs to the
    /// tokio runtime that opened it, so use the log on that runtime; for the engine host, create
    /// it with [`PgLog::lazy`] inside the host's store constructor.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A log whose single connection opens on first use, on the runtime that uses it. Call it
    /// inside a tokio runtime (the engine host calls its store constructor inside its own).
    #[must_use]
    pub fn lazy(options: sqlx::postgres::PgConnectOptions) -> Self {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy_with(options);
        Self { pool }
    }
}

impl LogStore for PgLog {
    async fn last(&mut self) -> Result<Option<(i64, i64)>, LogError> {
        let row = sqlx::query!("SELECT seq, game_time FROM log_records ORDER BY seq DESC LIMIT 1")
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| (r.seq, r.game_time)))
    }

    async fn append(&mut self, records: &[LogRecord]) -> Result<(), LogError> {
        let mut tx = self.pool.begin().await?;
        for r in records {
            let json = r.json()?;
            let rules_version =
                i32::try_from(r.rules_version).map_err(|_| LogError::Unrepresentable {
                    seq: r.seq,
                    what: format!("rules version {} exceeds i32", r.rules_version),
                })?;
            let player = r.player.map(player_uuid);
            sqlx::query!(
                r#"INSERT INTO log_records
                       (seq, kind, game_time, wall_time, player_id, idempotency_key, payload,
                        outcome, events, rules_version, engine_build)
                   VALUES ($1, $2, $3, 'epoch'::timestamptz + $4::bigint * interval '1 millisecond',
                           $5::text::uuid, $6, $7::text::jsonb, $8::text::jsonb, $9::text::jsonb,
                           $10, $11)"#,
                r.seq,
                r.kind.as_str(),
                r.game_time,
                r.wall_time_ms,
                player,
                r.idempotency_key,
                json.payload,
                json.outcome,
                json.events,
                rules_version,
                r.engine_build.as_str(),
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn records(&mut self) -> Result<Vec<LogRecord>, LogError> {
        let rows = sqlx::query!(
            r#"SELECT seq, kind, game_time,
                      (extract(epoch FROM wall_time) * 1000)::bigint AS "wall_time_ms!",
                      player_id::text AS player_id, idempotency_key,
                      payload::text AS "payload!", outcome::text AS "outcome!",
                      events::text AS "events!", rules_version, engine_build
               FROM log_records ORDER BY seq"#
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let seq = row.seq;
                let corrupt = |what: String| LogError::Corrupt { seq, what };
                Ok(LogRecord {
                    seq,
                    kind: RecordKind::parse(&row.kind)
                        .ok_or_else(|| corrupt(format!("kind {:?}", row.kind)))?,
                    game_time: row.game_time,
                    wall_time_ms: row.wall_time_ms,
                    player: match row.player_id {
                        None => None,
                        Some(p) => Some(
                            parse_player_uuid(&p)
                                .ok_or_else(|| corrupt(format!("player id {p:?}")))?,
                        ),
                    },
                    idempotency_key: row.idempotency_key,
                    payload: parse_payload(&row.payload)
                        .map_err(|e| corrupt(format!("payload: {e}")))?,
                    outcome: parse_outcome(&row.outcome)
                        .map_err(|e| corrupt(format!("outcome: {e}")))?,
                    events: serde_json::from_str(&row.events)
                        .map_err(|e| corrupt(format!("events: {e}")))?,
                    rules_version: u32::try_from(row.rules_version)
                        .map_err(|_| corrupt(format!("rules version {}", row.rules_version)))?,
                    engine_build: EngineBuild::new(row.engine_build)
                        .map_err(|e| corrupt(format!("engine build: {e}")))?,
                })
            })
            .collect()
    }
}
