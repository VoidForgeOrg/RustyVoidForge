//! The append-only log: typed records, the store interface, an in-memory store for tests and the
//! Postgres store (Architectural Design, Persistence; decision log D-034, D-036).
//!
//! Records are typed, never `serde_json::Value`: a `PlayerId` is a `u128`, which a `Value`
//! cannot hold exactly. The Postgres store writes the JSON columns as text and reads them back
//! as text into the types, and jsonb keeps big integers exactly.
//!
//! The JSON forms (D-034: JSON values, never SQL NULL):
//! - `payload`: the command's serde JSON, or `null` for a record without one;
//! - `outcome`: `{"result":"accepted","own_events":n}` (the record's last n events are the
//!   command's own; D-037), `{"result":"rejected","reason":"<code>"}` (the reason
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
    /// The command was accepted. The record's last `own_events` events are the command's own;
    /// the ones before them came due in the apply's advance (D-036, D-037).
    Accepted {
        /// How many of the record's events, at its end, the command produced.
        own_events: u32,
    },
    /// The command was rejected with this reason.
    Rejected(Rejection),
}

/// The outcome's JSON object.
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
enum OutcomeJson {
    Accepted { own_events: u32 },
    Rejected { reason: Rejection },
}

/// A client's idempotency key: 1 to 128 bytes of printable ASCII without spaces (D-037). Keys
/// are per player: two players may use the same key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdempotencyKey(String);

/// The longest idempotency key, in bytes.
pub const IDEMPOTENCY_KEY_MAX: usize = 128;

/// A key that is empty, too long, or not printable ASCII without spaces.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("an idempotency key is 1 to {IDEMPOTENCY_KEY_MAX} bytes of printable ASCII without spaces")]
pub struct InvalidKey;

impl IdempotencyKey {
    /// A key.
    ///
    /// # Errors
    /// [`InvalidKey`] if it is empty, longer than [`IDEMPOTENCY_KEY_MAX`] bytes, or holds
    /// anything but printable ASCII without spaces.
    pub fn new(key: impl Into<String>) -> Result<Self, InvalidKey> {
        let key = key.into();
        let valid = !key.is_empty()
            && key.len() <= IDEMPOTENCY_KEY_MAX
            && key.bytes().all(|b| b.is_ascii_graphic());
        if valid {
            Ok(Self(key))
        } else {
            Err(InvalidKey)
        }
    }

    /// The key as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
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
    /// The client's key, unique per player.
    pub idempotency_key: Option<IdempotencyKey>,
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

/// One `snapshots` row (D-035's column mapping): `payload` is the whole snapshot envelope, and
/// the other columns repeat its header, which recovery cross-checks (D-038).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRow {
    /// The last log record included; 0 before any.
    pub seq: i64,
    /// The header's game time.
    pub game_time: i64,
    /// The header's rules version.
    pub rules_version: u32,
    /// The header's engine build.
    pub engine_build: String,
    /// The header's state hash.
    pub state_hash: Vec<u8>,
    /// The whole snapshot: magic, header and World.
    pub payload: Vec<u8>,
}

impl SnapshotRow {
    /// The row for `snapshot`, taken after record `seq`.
    #[must_use]
    pub fn new(seq: i64, snapshot: &crate::snapshot::Snapshot) -> Self {
        Self {
            seq,
            game_time: snapshot.header.game_time_ms,
            rules_version: snapshot.header.rules_version,
            engine_build: snapshot.header.engine_build.clone(),
            state_hash: snapshot.header.state_hash.to_vec(),
            payload: snapshot.bytes.clone(),
        }
    }
}

/// A snapshot row without its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotMeta {
    /// The last log record included; 0 before any.
    pub seq: i64,
    /// The header's game time.
    pub game_time: i64,
    /// The header's rules version.
    pub rules_version: u32,
    /// The header's engine build.
    pub engine_build: String,
    /// The header's state hash.
    pub state_hash: Vec<u8>,
}

impl SnapshotRow {
    /// The row's columns without the payload.
    #[must_use]
    pub fn meta(&self) -> SnapshotMeta {
        SnapshotMeta {
            seq: self.seq,
            game_time: self.game_time,
            rules_version: self.rules_version,
            engine_build: self.engine_build.clone(),
            state_hash: self.state_hash.clone(),
        }
    }
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
    /// The records holding any of these (player, key) pairs, in seq order.
    fn find_keys(
        &mut self,
        keys: &[(PlayerId, IdempotencyKey)],
    ) -> impl Future<Output = Result<Vec<LogRecord>, LogError>> + Send;
    /// The records with a key at or after game time `since`, in seq order.
    fn keyed_since(
        &mut self,
        since: i64,
    ) -> impl Future<Output = Result<Vec<LogRecord>, LogError>> + Send;
    /// The records after `seq`, in seq order.
    fn records_after(
        &mut self,
        seq: i64,
    ) -> impl Future<Output = Result<Vec<LogRecord>, LogError>> + Send;
    /// Writes a snapshot row durably.
    fn write_snapshot(
        &mut self,
        row: &SnapshotRow,
    ) -> impl Future<Output = Result<(), LogError>> + Send;
    /// The newest snapshot (highest seq), of `build` if given, of any build otherwise.
    fn latest_snapshot(
        &mut self,
        build: Option<&str>,
    ) -> impl Future<Output = Result<Option<SnapshotRow>, LogError>> + Send;
    /// Every snapshot's columns except the payload, in seq order: the replay tool's checkpoints
    /// and inventory (D-041).
    fn snapshot_index(
        &mut self,
    ) -> impl Future<Output = Result<Vec<SnapshotMeta>, LogError>> + Send;
    /// The snapshot taken after record `seq`, if there is one.
    fn snapshot_at(
        &mut self,
        seq: i64,
    ) -> impl Future<Output = Result<Option<SnapshotRow>, LogError>> + Send;
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
        Outcome::Accepted { own_events } => {
            serde_json::to_string(&OutcomeJson::Accepted { own_events })
        }
        Outcome::Rejected(reason) => serde_json::to_string(&OutcomeJson::Rejected { reason }),
    }
}

/// Events in their log JSON form, for reports (never fails: an unrepresentable value is shown
/// as an error string).
pub(crate) fn events_json(events: &[DomainEvent]) -> String {
    serde_json::to_string(events).unwrap_or_else(|e| format!("<{e}>"))
}

/// An outcome in its log JSON form, for reports.
pub(crate) fn outcome_text(o: Outcome) -> String {
    outcome_json(o).unwrap_or_else(|e| format!("<{e}>"))
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
        Some(OutcomeJson::Accepted { own_events }) => Outcome::Accepted { own_events },
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

/// A record as one line of JSON, the golden files' form (P2.6; decision log D-040): the columns
/// by name, with `payload`, `outcome` and `events` as JSON values in their documented forms, not
/// as strings.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LineJson {
    seq: i64,
    kind: String,
    game_time: i64,
    wall_time_ms: i64,
    player_id: Option<String>,
    idempotency_key: Option<String>,
    payload: Option<Command>,
    outcome: Option<OutcomeJson>,
    events: Vec<DomainEvent>,
    rules_version: u32,
    engine_build: String,
}

impl LogRecord {
    /// The record as one line of JSON (no newline): every column by name, the JSON columns as
    /// values in their documented forms. [`LogRecord::from_line`] reads it back exactly.
    ///
    /// # Errors
    /// [`LogError::Unrepresentable`] if serde_json refuses a value (not expected).
    pub fn to_line(&self) -> Result<String, LogError> {
        let line = LineJson {
            seq: self.seq,
            kind: self.kind.as_str().to_owned(),
            game_time: self.game_time,
            wall_time_ms: self.wall_time_ms,
            player_id: self.player.map(player_uuid),
            idempotency_key: self.idempotency_key.as_ref().map(|k| k.as_str().to_owned()),
            payload: match &self.payload {
                Payload::Empty => None,
                Payload::Command(c) => Some(c.clone()),
            },
            outcome: match self.outcome {
                Outcome::Empty => None,
                Outcome::Accepted { own_events } => Some(OutcomeJson::Accepted { own_events }),
                Outcome::Rejected(reason) => Some(OutcomeJson::Rejected { reason }),
            },
            events: self.events.clone(),
            rules_version: self.rules_version,
            engine_build: self.engine_build.as_str().to_owned(),
        };
        serde_json::to_string(&line).map_err(|e| LogError::Unrepresentable {
            seq: self.seq,
            what: e.to_string(),
        })
    }

    /// Reads a record written by [`LogRecord::to_line`], with the same checks as a Postgres row.
    ///
    /// # Errors
    /// [`LogError::Corrupt`] if the line is not a valid record (seq 0 when unreadable).
    pub fn from_line(line: &str) -> Result<Self, LogError> {
        let l: LineJson = serde_json::from_str(line).map_err(|e| LogError::Corrupt {
            seq: 0,
            what: format!("record line: {e}"),
        })?;
        let seq = l.seq;
        let corrupt = |what: String| LogError::Corrupt { seq, what };
        Ok(Self {
            seq,
            kind: RecordKind::parse(&l.kind)
                .ok_or_else(|| corrupt(format!("kind {:?}", l.kind)))?,
            game_time: l.game_time,
            wall_time_ms: l.wall_time_ms,
            player: match l.player_id {
                None => None,
                Some(p) => {
                    Some(parse_player_uuid(&p).ok_or_else(|| corrupt(format!("player id {p:?}")))?)
                }
            },
            idempotency_key: match l.idempotency_key {
                None => None,
                Some(k) => Some(
                    IdempotencyKey::new(k).map_err(|e| corrupt(format!("idempotency key: {e}")))?,
                ),
            },
            payload: l.payload.map_or(Payload::Empty, Payload::Command),
            outcome: match l.outcome {
                None => Outcome::Empty,
                Some(OutcomeJson::Accepted { own_events }) => Outcome::Accepted { own_events },
                Some(OutcomeJson::Rejected { reason }) => Outcome::Rejected(reason),
            },
            events: l.events,
            rules_version: l.rules_version,
            engine_build: EngineBuild::new(l.engine_build)
                .map_err(|e| corrupt(format!("engine build: {e}")))?,
        })
    }
}

// --- The in-memory store ---

#[derive(Debug, Default)]
struct MemoryInner {
    records: Vec<LogRecord>,
    fail_appends: bool,
    appends: u64,
    lookups: u64,
    snapshots: Vec<SnapshotRow>,
    fail_snapshots: bool,
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

    /// Makes every later snapshot write fail (true) or succeed again (false).
    pub fn fail_snapshots(&self, fail: bool) {
        self.lock().fail_snapshots = fail;
    }

    /// The snapshot rows written so far, in write order.
    #[must_use]
    pub fn snapshot_rows(&self) -> Vec<SnapshotRow> {
        self.lock().snapshots.clone()
    }

    /// Replaces a stored record (tests: a tampered log).
    pub fn tamper(&self, record: LogRecord) {
        let mut inner = self.lock();
        if let Some(r) = inner.records.iter_mut().find(|r| r.seq == record.seq) {
            *r = record;
        }
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

    /// How many key lookups (`find_keys`) were made.
    #[must_use]
    pub fn lookups(&self) -> u64 {
        self.lock().lookups
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
            // UNIQUE (player_id, idempotency_key), as the table refuses it (D-034).
            if let (Some(player), Some(key)) = (r.player, &r.idempotency_key) {
                let reused =
                    inner.records.iter().chain(&records[..i]).any(|o| {
                        o.player == Some(player) && o.idempotency_key.as_ref() == Some(key)
                    });
                if reused {
                    return Err(LogError::Memory(format!(
                        "seq {}: the player's key {:?} is already logged",
                        r.seq,
                        key.as_str()
                    )));
                }
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

    async fn find_keys(
        &mut self,
        keys: &[(PlayerId, IdempotencyKey)],
    ) -> Result<Vec<LogRecord>, LogError> {
        self.lock().lookups += 1;
        Ok(self
            .lock()
            .records
            .iter()
            .filter(|r| match (r.player, &r.idempotency_key) {
                (Some(p), Some(k)) => keys.iter().any(|(kp, kk)| *kp == p && kk == k),
                _ => false,
            })
            .cloned()
            .collect())
    }

    async fn keyed_since(&mut self, since: i64) -> Result<Vec<LogRecord>, LogError> {
        Ok(self
            .lock()
            .records
            .iter()
            .filter(|r| r.idempotency_key.is_some() && r.game_time >= since)
            .cloned()
            .collect())
    }

    async fn records_after(&mut self, seq: i64) -> Result<Vec<LogRecord>, LogError> {
        Ok(self
            .lock()
            .records
            .iter()
            .filter(|r| r.seq > seq)
            .cloned()
            .collect())
    }

    async fn write_snapshot(&mut self, row: &SnapshotRow) -> Result<(), LogError> {
        let mut inner = self.lock();
        if inner.fail_snapshots {
            return Err(LogError::Memory("injected snapshot failure".to_owned()));
        }
        // The table's constraints: seq is the key and not negative; a 32-byte hash.
        if row.seq < 0
            || row.state_hash.len() != 32
            || inner.snapshots.iter().any(|s| s.seq == row.seq)
        {
            return Err(LogError::Memory(format!(
                "snapshot at seq {} refused",
                row.seq
            )));
        }
        inner.snapshots.push(row.clone());
        Ok(())
    }

    async fn latest_snapshot(
        &mut self,
        build: Option<&str>,
    ) -> Result<Option<SnapshotRow>, LogError> {
        Ok(self
            .lock()
            .snapshots
            .iter()
            .filter(|s| build.is_none_or(|b| s.engine_build == b))
            .max_by_key(|s| s.seq)
            .cloned())
    }

    async fn snapshot_index(&mut self) -> Result<Vec<SnapshotMeta>, LogError> {
        let mut index: Vec<SnapshotMeta> = self
            .lock()
            .snapshots
            .iter()
            .map(SnapshotRow::meta)
            .collect();
        index.sort_by_key(|s| s.seq);
        Ok(index)
    }

    async fn snapshot_at(&mut self, seq: i64) -> Result<Option<SnapshotRow>, LogError> {
        Ok(self.lock().snapshots.iter().find(|s| s.seq == seq).cloned())
    }
}

// --- The Postgres store ---

/// A `log_records` row as the Postgres store reads it: the JSON columns as text.
struct Row {
    seq: i64,
    kind: String,
    game_time: i64,
    wall_time_ms: i64,
    player_id: Option<String>,
    idempotency_key: Option<String>,
    payload: String,
    outcome: String,
    events: String,
    rules_version: i32,
    engine_build: String,
}

impl Row {
    fn into_record(self) -> Result<LogRecord, LogError> {
        let seq = self.seq;
        let corrupt = |what: String| LogError::Corrupt { seq, what };
        Ok(LogRecord {
            seq,
            kind: RecordKind::parse(&self.kind)
                .ok_or_else(|| corrupt(format!("kind {:?}", self.kind)))?,
            game_time: self.game_time,
            wall_time_ms: self.wall_time_ms,
            player: match self.player_id {
                None => None,
                Some(p) => {
                    Some(parse_player_uuid(&p).ok_or_else(|| corrupt(format!("player id {p:?}")))?)
                }
            },
            idempotency_key: match self.idempotency_key {
                None => None,
                Some(k) => Some(
                    IdempotencyKey::new(k).map_err(|e| corrupt(format!("idempotency key: {e}")))?,
                ),
            },
            payload: parse_payload(&self.payload).map_err(|e| corrupt(format!("payload: {e}")))?,
            outcome: parse_outcome(&self.outcome).map_err(|e| corrupt(format!("outcome: {e}")))?,
            events: serde_json::from_str(&self.events)
                .map_err(|e| corrupt(format!("events: {e}")))?,
            rules_version: u32::try_from(self.rules_version)
                .map_err(|_| corrupt(format!("rules version {}", self.rules_version)))?,
            engine_build: EngineBuild::new(self.engine_build)
                .map_err(|e| corrupt(format!("engine build: {e}")))?,
        })
    }
}

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
    /// [`PgLog::lazy`] from a database URL (the replay tool's `--database-url`).
    ///
    /// # Errors
    /// [`LogError::Database`] if the URL is not a Postgres URL.
    pub fn lazy_url(url: &str) -> Result<Self, LogError> {
        let options: sqlx::postgres::PgConnectOptions = url.parse()?;
        Ok(Self::lazy(options))
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
                r.idempotency_key.as_ref().map(IdempotencyKey::as_str),
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
        sqlx::query_as!(
            Row,
            r#"SELECT seq, kind, game_time,
                      (extract(epoch FROM wall_time) * 1000)::bigint AS "wall_time_ms!",
                      player_id::text AS player_id, idempotency_key,
                      payload::text AS "payload!", outcome::text AS "outcome!",
                      events::text AS "events!", rules_version, engine_build
               FROM log_records ORDER BY seq"#
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(Row::into_record)
        .collect()
    }

    async fn find_keys(
        &mut self,
        keys: &[(PlayerId, IdempotencyKey)],
    ) -> Result<Vec<LogRecord>, LogError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let players: Vec<String> = keys.iter().map(|(p, _)| player_uuid(*p)).collect();
        let names: Vec<String> = keys.iter().map(|(_, k)| k.as_str().to_owned()).collect();
        // One indexed lookup on UNIQUE (player_id, idempotency_key) for the whole turn.
        sqlx::query_as!(
            Row,
            r#"SELECT seq, kind, game_time,
                      (extract(epoch FROM wall_time) * 1000)::bigint AS "wall_time_ms!",
                      player_id::text AS player_id, idempotency_key,
                      payload::text AS "payload!", outcome::text AS "outcome!",
                      events::text AS "events!", rules_version, engine_build
               FROM log_records
               WHERE (player_id, idempotency_key) IN
                     (SELECT p::uuid, k FROM unnest($1::text[], $2::text[]) AS u(p, k))
               ORDER BY seq"#,
            &players,
            &names,
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(Row::into_record)
        .collect()
    }

    async fn keyed_since(&mut self, since: i64) -> Result<Vec<LogRecord>, LogError> {
        sqlx::query_as!(
            Row,
            r#"SELECT seq, kind, game_time,
                      (extract(epoch FROM wall_time) * 1000)::bigint AS "wall_time_ms!",
                      player_id::text AS player_id, idempotency_key,
                      payload::text AS "payload!", outcome::text AS "outcome!",
                      events::text AS "events!", rules_version, engine_build
               FROM log_records
               WHERE idempotency_key IS NOT NULL AND game_time >= $1
               ORDER BY seq"#,
            since,
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(Row::into_record)
        .collect()
    }

    async fn records_after(&mut self, seq: i64) -> Result<Vec<LogRecord>, LogError> {
        sqlx::query_as!(
            Row,
            r#"SELECT seq, kind, game_time,
                      (extract(epoch FROM wall_time) * 1000)::bigint AS "wall_time_ms!",
                      player_id::text AS player_id, idempotency_key,
                      payload::text AS "payload!", outcome::text AS "outcome!",
                      events::text AS "events!", rules_version, engine_build
               FROM log_records WHERE seq > $1 ORDER BY seq"#,
            seq,
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(Row::into_record)
        .collect()
    }

    async fn write_snapshot(&mut self, row: &SnapshotRow) -> Result<(), LogError> {
        let rules_version =
            i32::try_from(row.rules_version).map_err(|_| LogError::Unrepresentable {
                seq: row.seq,
                what: format!("rules version {} exceeds i32", row.rules_version),
            })?;
        sqlx::query!(
            r#"INSERT INTO snapshots (seq, game_time, rules_version, engine_build, state_hash, payload)
               VALUES ($1, $2, $3, $4, $5, $6)"#,
            row.seq,
            row.game_time,
            rules_version,
            row.engine_build,
            row.state_hash,
            row.payload,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn latest_snapshot(
        &mut self,
        build: Option<&str>,
    ) -> Result<Option<SnapshotRow>, LogError> {
        let row = sqlx::query!(
            r#"SELECT seq, game_time, rules_version, engine_build, state_hash, payload
               FROM snapshots WHERE ($1::text IS NULL OR engine_build = $1)
               ORDER BY seq DESC LIMIT 1"#,
            build,
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok(SnapshotRow {
                seq: r.seq,
                game_time: r.game_time,
                rules_version: u32::try_from(r.rules_version).map_err(|_| LogError::Corrupt {
                    seq: r.seq,
                    what: format!("snapshot rules version {}", r.rules_version),
                })?,
                engine_build: r.engine_build,
                state_hash: r.state_hash,
                payload: r.payload,
            })
        })
        .transpose()
    }

    async fn snapshot_index(&mut self) -> Result<Vec<SnapshotMeta>, LogError> {
        let rows = sqlx::query!(
            r#"SELECT seq, game_time, rules_version, engine_build, state_hash
               FROM snapshots ORDER BY seq"#
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(SnapshotMeta {
                    seq: r.seq,
                    game_time: r.game_time,
                    rules_version: u32::try_from(r.rules_version).map_err(|_| {
                        LogError::Corrupt {
                            seq: r.seq,
                            what: format!("snapshot rules version {}", r.rules_version),
                        }
                    })?,
                    engine_build: r.engine_build,
                    state_hash: r.state_hash,
                })
            })
            .collect()
    }

    async fn snapshot_at(&mut self, seq: i64) -> Result<Option<SnapshotRow>, LogError> {
        let row = sqlx::query!(
            r#"SELECT seq, game_time, rules_version, engine_build, state_hash, payload
               FROM snapshots WHERE seq = $1"#,
            seq,
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok(SnapshotRow {
                seq: r.seq,
                game_time: r.game_time,
                rules_version: u32::try_from(r.rules_version).map_err(|_| LogError::Corrupt {
                    seq: r.seq,
                    what: format!("snapshot rules version {}", r.rules_version),
                })?,
                engine_build: r.engine_build,
                state_hash: r.state_hash,
                payload: r.payload,
            })
        })
        .transpose()
    }
}
