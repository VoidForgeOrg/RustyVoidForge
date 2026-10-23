//! Engine host: inbox, stamping, log, snapshots, read model.
//!
//! The single writer for a universe (Architectural Design, Engine host and write path; decision
//! log D-036). One OS thread, running a current-thread tokio runtime, owns the `World`; everything
//! else reaches it through a bounded inbox ([`HostHandle::submit`]) or reads the immutable
//! [`ReadModel`] it publishes.
//!
//! Each turn of the loop:
//! 1. waits for a command or for the next queue entry to come due, never polling;
//! 2. looks up each command's idempotency key: a known one gets its first receipt back, and
//!    nothing is applied or appended (D-037);
//! 3. stamps t = max(wall clock − epoch, last t) and takes the next sequence number;
//! 4. runs `apply(command, t)`, or `advance_to(t)` when only time has passed;
//! 5. appends the turn's records in one durable write (group commit: commands already waiting
//!    join the turn, up to `max_batch`, with no added delay);
//! 6. publishes the read model, then replies: when a command's reply arrives, [`HostHandle::read`]
//!    already shows its effect (D-036 as corrected by D-055). The API relies on it: a player reads
//!    its own writes, and a new player's first view finds it joined.
//!
//! One record per step. A command record's events are the apply's advance followed by the
//! command's own events, and replay compares the flattened list. An advance that fires no events
//! is not logged, because D-024's observation invariance means the next logged step reaches the
//! same state, but the read model is still published after it. A failed append, or an engine error
//! that poisons the world, is fatal: the host stops, and nothing it did not make durable is ever
//! acknowledged.
//!
//! Idempotency (D-037): a repeated (player, key) always gets its first receipt, at any age. Keys
//! from the last `key_window_ms` of game time are cached; a key that misses the cache is looked up
//! in the log (one query per turn), so a duplicate never reaches the append, where the table's
//! unique constraint would make it fatal. The same key with a different command is
//! [`HostError::KeyConflict`].

pub mod clock;
mod instruments;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{mpsc, oneshot, watch};
use tracing::Instrument;

use crate::instruments::{Gauges, Instruments, NONE};
use voidforge_core::state::EngineError;
use voidforge_store::log::SnapshotRow;
use voidforge_store::log::{
    IdempotencyKey, LogError, LogRecord, LogStore, Outcome, Payload, RecordKind,
};
use voidforge_store::replay::{self, ReplayError, Step};
use voidforge_store::snapshot::{self, EngineBuild, Expected, SnapshotError};

pub use clock::{Clock, ManualClock, SystemClock};
/// The command vocabulary this API already exposes (in [`Receipt`] and [`ReadModel`]), so the
/// server can name it without depending on the core (decision log D-047).
pub use voidforge_core::command::{Action, Command, Rejection};
/// The rules type `recover` and `spawn` take, so `serve` can pass a parsed rules file between its
/// runtimes without depending on the core (decision log D-050). A name only.
pub use voidforge_core::rules::Rules;
/// The rules' spec types the server's `GET /rules` mapping names, so it can map the loaded rules
/// without depending on the core (decision log D-058). Names only: not the module, its constants,
/// its error or the worldgen spec.
pub use voidforge_core::rules::{
    BuildCost, BuildingSpecs, DrillSpec, HomeworldSpec, MassPoints, PlanetSpec, RefinerySpec,
    ScoringSpecs, ShipSpec, ShipSpecs, ShipyardSpec,
};
/// The read vocabulary the server maps to the wire: the views, and the read-only state enums
/// and IDs they name. Names only; nothing here changes the live World (D-049).
pub use voidforge_core::solver::FlowLabel;
pub use voidforge_core::state::{
    BuildingId, BuildingKind, Cargo, FleetId, FleetState, Mission, PlanetId, Position, ShipBuildId,
    ShipBuildStatus, ShipId, ShipKind,
};
pub use voidforge_core::state::{DomainEvent, PlayerId, World};
pub use voidforge_core::view;

/// How a host runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfig {
    /// The universe epoch in Unix milliseconds: game time is wall time minus this. An input:
    /// `seed` records it once with the universe, and `serve` reads it back on every start, never
    /// inventing one (decision log D-048, D-050).
    pub epoch_unix_ms: i64,
    /// How many commands may wait in the inbox before [`HostError::Busy`].
    pub inbox_capacity: usize,
    /// The most commands one turn takes, and so the most records one durable write holds.
    pub max_batch: usize,
    /// The build written into every record.
    pub engine_build: EngineBuild,
    /// How much game time of idempotency keys stays cached in memory. Older keys are still known,
    /// through the log (D-037); the window only bounds the cache.
    pub key_window_ms: i64,
    /// Snapshot after a turn once this many records were appended since the last one (and at
    /// clean shutdown).
    pub snapshot_every_records: u64,
}

/// A day of game time: the default key cache window.
pub const DEFAULT_KEY_WINDOW_MS: i64 = 24 * 3_600_000;

impl HostConfig {
    /// The defaults: an inbox of 1,024, turns of up to 64 commands, a day of cached keys, and a
    /// snapshot every 1,000 records.
    #[must_use]
    pub fn new(epoch_unix_ms: i64, engine_build: EngineBuild) -> Self {
        Self {
            epoch_unix_ms,
            inbox_capacity: 1024,
            max_batch: 64,
            engine_build,
            key_window_ms: DEFAULT_KEY_WINDOW_MS,
            snapshot_every_records: 1_000,
        }
    }
}

/// What a caller gets for an acknowledged command: its record is durable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// The command's record.
    pub seq: i64,
    /// The stamped time it was applied at.
    pub t: i64,
    /// Accepted with the command's own events, or rejected with its reason.
    pub outcome: Result<Vec<DomainEvent>, Rejection>,
}

/// Why a command was not acknowledged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    /// The inbox is full; try again later (the API answers 503 with Retry-After).
    #[error("the engine host is busy")]
    Busy,
    /// The host has stopped. A command in flight when it stopped was not acknowledged; its record
    /// was not written (the crash table's "before the append").
    #[error("the engine host has stopped")]
    Stopped,
    /// The player already used this idempotency key for a different command, logged at `seq`.
    /// Nothing was applied (the API answers 422 `idempotency_key_conflict`; D-037).
    #[error("the idempotency key was used for a different command (record {seq})")]
    KeyConflict {
        /// The record of the key's first use.
        seq: i64,
    },
}

/// Why a host stopped on its own, or could not start.
#[derive(Debug, thiserror::Error)]
pub enum Fatal {
    /// The store could not be read at start.
    #[error("the log could not be read at start: {0}")]
    Start(LogError),
    /// The World is behind the log's last record, so it cannot be the log's World.
    #[error("the world at {world_ms} ms is behind the log's last record at {log_ms} ms")]
    WorldBehindLog {
        /// The World's time.
        world_ms: i64,
        /// The last record's game time.
        log_ms: i64,
    },
    /// Looking keys up in the log failed: the host cannot tell a new key from a used one.
    #[error("an idempotency key lookup failed: {0}")]
    Lookup(LogError),
    /// An append failed: the host stops rather than run with memory the log does not contain.
    #[error("an append failed: {0}")]
    Append(LogError),
    /// The engine failed and poisoned the World (D-023); it must be discarded and recovered.
    #[error("the engine failed: {0}")]
    Engine(EngineError),
    /// The host thread or its runtime could not start, or the thread panicked.
    #[error("the host thread failed: {0}")]
    Thread(String),
}

/// Counters, read without the command path.
#[derive(Debug, Default)]
pub struct HostStats {
    wakes: AtomicU64,
    appends: AtomicU64,
    steps: AtomicU64,
    key_lookups: AtomicU64,
    replays: AtomicU64,
    snapshots: AtomicU64,
    snapshot_failures: AtomicU64,
}

impl HostStats {
    /// Turns of the loop: one per command batch or due-event wake. An idle host does not wake.
    pub fn wakes(&self) -> u64 {
        self.wakes.load(Ordering::Relaxed)
    }

    /// Durable writes.
    pub fn appends(&self) -> u64 {
        self.appends.load(Ordering::Relaxed)
    }

    /// Steps run: commands applied and time-only advances.
    pub fn steps(&self) -> u64 {
        self.steps.load(Ordering::Relaxed)
    }

    /// Log lookups for keys that missed the cache: at most one per turn.
    pub fn key_lookups(&self) -> u64 {
        self.key_lookups.load(Ordering::Relaxed)
    }

    /// Repeated keys answered with their first receipt.
    pub fn replays(&self) -> u64 {
        self.replays.load(Ordering::Relaxed)
    }

    /// Snapshots written.
    pub fn snapshots(&self) -> u64 {
        self.snapshots.load(Ordering::Relaxed)
    }

    /// Snapshot writes that failed (each also an error event).
    pub fn snapshot_failures(&self) -> u64 {
        self.snapshot_failures.load(Ordering::Relaxed)
    }
}

/// The immutable state readers see, published after every durable write and after every step
/// that changes the World without a record.
#[derive(Debug, Clone)]
pub struct ReadModel {
    /// The last durable record, 0 before any.
    pub seq: i64,
    /// The World at its time; run `world.view(query, t, &rules)` on it.
    pub world: Arc<World>,
    /// The rules in force.
    pub rules: Arc<Rules>,
}

struct Submission {
    command: Command,
    key: Option<IdempotencyKey>,
    reply: oneshot::Sender<Result<Receipt, HostError>>,
    /// The submitter's span: the command's spans in the host are its children (P5.1).
    caller: tracing::Span,
    /// `host.inbox`, open from the submit until the host takes it: the inbox wait.
    inbox: tracing::Span,
}

/// The way in: submit commands and read the read model. Clones share one host. The host stops
/// on its own once every handle is gone.
#[derive(Debug, Clone)]
pub struct HostHandle {
    inbox: mpsc::Sender<Submission>,
    read: watch::Receiver<Arc<ReadModel>>,
    stats: Arc<HostStats>,
    gauges: Arc<Gauges>,
}

impl HostHandle {
    /// Submits a command and waits until its record is durable.
    ///
    /// # Errors
    /// [`HostError::Busy`] at once if the inbox is full; [`HostError::Stopped`] if the host has
    /// stopped or stops before the command's record is durable.
    pub async fn submit(&self, command: Command) -> Result<Receipt, HostError> {
        self.send(command, None).await
    }

    /// Submits a command with the player's idempotency key. A key the player used before gets
    /// that first receipt back (original seq, t and outcome), applying and appending nothing.
    ///
    /// # Errors
    /// As [`HostHandle::submit`], and [`HostError::KeyConflict`] if the key was used for a
    /// different command.
    pub async fn submit_with_key(
        &self,
        command: Command,
        key: IdempotencyKey,
    ) -> Result<Receipt, HostError> {
        self.send(command, Some(key)).await
    }

    async fn send(
        &self,
        command: Command,
        key: Option<IdempotencyKey>,
    ) -> Result<Receipt, HostError> {
        let (reply, receipt) = oneshot::channel();
        let caller = tracing::Span::current();
        let inbox = tracing::info_span!(parent: &caller, "host.inbox");
        // Counted before the send, so the host's take never sees the depth below zero.
        self.gauges.depth.fetch_add(1, Ordering::Relaxed);
        let sent = self.inbox.try_send(Submission {
            command,
            key,
            reply,
            caller,
            inbox,
        });
        if let Err(e) = sent {
            self.gauges.depth.fetch_sub(1, Ordering::Relaxed);
            return Err(match e {
                mpsc::error::TrySendError::Full(_) => HostError::Busy,
                mpsc::error::TrySendError::Closed(_) => HostError::Stopped,
            });
        }
        receipt.await.map_err(|_| HostError::Stopped)?
    }

    /// The latest read model.
    #[must_use]
    pub fn read(&self) -> Arc<ReadModel> {
        self.read.borrow().clone()
    }

    /// Waits until a read model this handle has not seen is published, then returns it.
    ///
    /// # Errors
    /// [`HostError::Stopped`] once the host has stopped.
    pub async fn changed(&mut self) -> Result<Arc<ReadModel>, HostError> {
        self.read.changed().await.map_err(|_| HostError::Stopped)?;
        Ok(self.read.borrow_and_update().clone())
    }

    /// The host's counters.
    #[must_use]
    pub fn stats(&self) -> &HostStats {
        &self.stats
    }
}

/// The host's thread. [`HostTask::join`] waits for it to stop.
#[derive(Debug)]
pub struct HostTask {
    thread: std::thread::JoinHandle<Result<(), Fatal>>,
}

impl HostTask {
    /// Waits for the host to stop: `Ok` once every handle was dropped, or why it stopped.
    ///
    /// # Errors
    /// The [`Fatal`] reason.
    pub fn join(self) -> Result<(), Fatal> {
        self.thread
            .join()
            .map_err(|_| Fatal::Thread("the host thread panicked".to_owned()))?
    }

    /// Whether the host has stopped.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }
}

type Started = std::sync::mpsc::SyncSender<Result<(), Fatal>>;

/// Starts a host owning `world`, appending to the store that `make_store` creates. The log's
/// existing records come first: sequence numbers continue after them, and the World must not be
/// behind the last one (P2.5's recovery provides the World that matches the log).
///
/// `make_store` runs on the host's thread, inside its runtime, so a store's connections are
/// opened and driven there (a database connection belongs to the runtime that opened it; see
/// `PgLog::lazy`).
///
/// # Errors
/// [`Fatal::Start`] or [`Fatal::WorldBehindLog`] if the host cannot start; [`Fatal::Thread`] if
/// its thread or runtime cannot be created.
pub fn spawn<S: LogStore, C: Clock>(
    world: World,
    rules: Rules,
    make_store: impl FnOnce() -> S + Send + 'static,
    clock: C,
    config: HostConfig,
) -> Result<(HostHandle, HostTask), Fatal> {
    let (inbox, rx) = mpsc::channel(config.inbox_capacity.max(1));
    let rules = Arc::new(rules);
    let (publish, read) = watch::channel(Arc::new(ReadModel {
        seq: 0,
        world: Arc::new(world.clone()),
        rules: rules.clone(),
    }));
    let stats = Arc::new(HostStats::default());
    let gauges = Arc::new(Gauges::default());
    let clock = Arc::new(clock);
    let instruments = Instruments::new(&gauges, &clock, config.epoch_unix_ms);
    let (started, started_rx) = std::sync::mpsc::sync_channel(1);
    let stats_host = stats.clone();
    let gauges_host = gauges.clone();
    let thread = std::thread::Builder::new()
        .name("voidforge-host".to_owned())
        .spawn(move || {
            // Every driver tokio has (time, and the IO a database store needs).
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => runtime.block_on(async move {
                    let host = Host {
                        last_t: world.time_ms,
                        world,
                        rules,
                        store: make_store(),
                        clock,
                        config,
                        rx,
                        publish,
                        stats: stats_host,
                        gauges: gauges_host,
                        instruments,
                        next_seq: 1,
                        keys: HashMap::new(),
                        since_snapshot: 0,
                        last_snapshot_seq: None,
                    };
                    host.run(started).await
                }),
                Err(e) => {
                    let _ = started.send(Err(Fatal::Thread(e.to_string())));
                    Ok(())
                }
            }
        })
        .map_err(|e| Fatal::Thread(e.to_string()))?;
    let task = HostTask { thread };
    match started_rx.recv() {
        Ok(Ok(())) => Ok((
            HostHandle {
                inbox,
                read,
                stats,
                gauges,
            },
            task,
        )),
        Ok(Err(fatal)) => {
            let _ = task.thread.join();
            Err(fatal)
        }
        Err(_) => Err(task
            .join()
            .err()
            .unwrap_or_else(|| Fatal::Thread("the host stopped while starting".to_owned()))),
    }
}

struct Host<S, C> {
    world: World,
    rules: Arc<Rules>,
    store: S,
    clock: Arc<C>,
    config: HostConfig,
    rx: mpsc::Receiver<Submission>,
    publish: watch::Sender<Arc<ReadModel>>,
    stats: Arc<HostStats>,
    gauges: Arc<Gauges>,
    instruments: Instruments,
    next_seq: i64,
    last_t: i64,
    /// Cached idempotency keys: the first command and receipt for each (player, key).
    keys: HashMap<(PlayerId, IdempotencyKey), Known>,
    /// Records appended since the last snapshot.
    since_snapshot: u64,
    /// The seq of the newest snapshot in the store, if known.
    last_snapshot_seq: Option<i64>,
}

/// A key's first use.
#[derive(Debug, Clone)]
struct Known {
    command: Command,
    receipt: Receipt,
}

/// A reply, sent once the turn's records are durable.
struct Pending {
    reply: oneshot::Sender<Result<Receipt, HostError>>,
    answer: Result<Receipt, HostError>,
}

/// The first use of a key, as its record holds it: the command and the receipt it got.
fn known(record: &LogRecord) -> Result<((PlayerId, IdempotencyKey), Known), LogError> {
    let corrupt = |what: &str| LogError::Corrupt {
        seq: record.seq,
        what: what.to_owned(),
    };
    let (Some(player), Some(key), Payload::Command(command)) =
        (record.player, &record.idempotency_key, &record.payload)
    else {
        return Err(corrupt("a keyed record without a player or command"));
    };
    let outcome = match record.outcome {
        Outcome::Accepted { own_events } => {
            let own = usize::try_from(own_events).unwrap_or(usize::MAX);
            let from = record
                .events
                .len()
                .checked_sub(own)
                .ok_or_else(|| corrupt("more own events than events"))?;
            Ok(record.events[from..].to_vec())
        }
        Outcome::Rejected(reason) => Err(reason),
        Outcome::Empty => return Err(corrupt("a keyed record without an outcome")),
    };
    Ok((
        (player, key.clone()),
        Known {
            command: command.clone(),
            receipt: Receipt {
                seq: record.seq,
                t: record.game_time,
                outcome,
            },
        },
    ))
}

impl<S: LogStore, C: Clock> Host<S, C> {
    /// Starts, reports the start through `started`, then serves until stopped.
    async fn run(mut self, started: Started) -> Result<(), Fatal> {
        match self.store.last().await {
            Err(e) => {
                let _ = started.send(Err(Fatal::Start(e)));
                return Ok(());
            }
            Ok(Some((_, log_ms))) if self.world.time_ms < log_ms => {
                let _ = started.send(Err(Fatal::WorldBehindLog {
                    world_ms: self.world.time_ms,
                    log_ms,
                }));
                return Ok(());
            }
            Ok(Some((seq, _))) => self.next_seq = seq + 1,
            Ok(None) => {}
        }
        // The key cache, rebuilt from the log's keyed records within the window.
        let since = self.world.time_ms.saturating_sub(self.config.key_window_ms);
        let rebuilt = self
            .store
            .keyed_since(since)
            .await
            .and_then(|records| records.iter().map(known).collect::<Result<Vec<_>, _>>());
        match rebuilt {
            Ok(entries) => self.keys.extend(entries),
            Err(e) => {
                let _ = started.send(Err(Fatal::Start(e)));
                return Ok(());
            }
        }
        match self.store.latest_snapshot(None).await {
            Ok(row) => {
                if let Some(r) = &row {
                    self.gauges
                        .snapshot_at
                        .store(r.game_time, Ordering::Relaxed);
                }
                self.last_snapshot_seq = row.map(|r| r.seq);
            }
            Err(e) => {
                let _ = started.send(Err(Fatal::Start(e)));
                return Ok(());
            }
        }
        // The read model names the log head from the start, so a host spawned on a non-empty
        // log (recovery always is) does not report seq 0 until its next append (carry e).
        self.publish.send_replace(Arc::new(ReadModel {
            seq: self.next_seq - 1,
            world: Arc::new(self.world.clone()),
            rules: self.rules.clone(),
        }));
        self.due_changed();
        let _ = started.send(Ok(()));
        let result = self.serve().await;
        // Stopping: refuse what is still queued, so no caller waits forever.
        self.rx.close();
        while self.rx.try_recv().is_ok() {
            self.gauges.depth.fetch_sub(1, Ordering::Relaxed);
        }
        // A clean shutdown leaves a snapshot at the log head (an engine upgrade starts from it).
        if result.is_ok() {
            self.snapshot().await;
        }
        result
    }

    async fn serve(&mut self) -> Result<(), Fatal> {
        loop {
            // 1. Wait for a command or for the next queue entry to come due.
            let due = self.world.queue().first().map(|e| e.at_ms);
            let wake_at = due.map(|at| at.saturating_add(self.config.epoch_unix_ms));
            let first = tokio::select! {
                biased;
                message = self.rx.recv() => match message {
                    Some(submission) => Some(submission),
                    None => return Ok(()),
                },
                () = sleep_until(&*self.clock, wake_at) => None,
            };
            self.stats.wakes.fetch_add(1, Ordering::Relaxed);
            let mut batch = Vec::new();
            if let Some(first) = first {
                batch.push(first);
                while batch.len() < self.config.max_batch.max(1) {
                    match self.rx.try_recv() {
                        Ok(submission) => batch.push(submission),
                        Err(_) => break,
                    }
                }
            }
            // Taken: each command's inbox wait ends here.
            for submission in &mut batch {
                self.gauges.depth.fetch_sub(1, Ordering::Relaxed);
                submission.inbox = tracing::Span::none();
            }
            let span = tracing::info_span!(parent: None, "host.turn", commands = batch.len());
            for submission in &batch {
                voidforge_telemetry::link(&span, &submission.caller);
            }
            self.turn(batch).instrument(span).await?;
            self.due_changed();
        }
    }

    /// One turn: the key lookup, the steps, one durable write, then the replies and the read
    /// model.
    async fn turn(&mut self, batch: Vec<Submission>) -> Result<(), Fatal> {
        // 2. Keys that miss the cache are looked up in the log, all in one query.
        let mut misses: Vec<(PlayerId, IdempotencyKey)> = batch
            .iter()
            .filter_map(|s| s.key.clone().map(|k| (s.command.player, k)))
            .filter(|pk| !self.keys.contains_key(pk))
            .collect();
        misses.sort();
        misses.dedup();
        if !misses.is_empty() {
            self.stats.key_lookups.fetch_add(1, Ordering::Relaxed);
            // The lookup's own span, inside the turn's (P5.1 QA's N3, D-066): its time was
            // invisible in the turn's.
            let lookup = tracing::info_span!(
                "host.keys",
                misses = misses.len(),
                found = tracing::field::Empty
            );
            let found = self
                .store
                .find_keys(&misses)
                .instrument(lookup.clone())
                .await
                .and_then(|records| records.iter().map(known).collect::<Result<Vec<_>, _>>());
            if let Ok(entries) = &found {
                lookup.record("found", entries.len());
            }
            drop(lookup);
            match found {
                Ok(entries) => self.keys.extend(entries),
                Err(e) => {
                    tracing::error!(error = %e, "key lookup failed; the host stops");
                    return Err(Fatal::Lookup(e));
                }
            }
        }
        let mut records = Vec::new();
        let mut pending = Vec::new();
        let mut failure = None;
        let mut changed = false;
        if batch.is_empty() {
            // 4. Only time has passed.
            let t = self.stamp();
            self.stats.steps.fetch_add(1, Ordering::Relaxed);
            changed = true;
            let advance = tracing::info_span!("host.advance", t);
            let started = std::time::Instant::now();
            let stepped =
                advance.in_scope(|| replay::step(&mut self.world, &Step::Advance, t, &self.rules));
            self.instruments.step(started.elapsed(), false);
            match stepped {
                // No events: not logged; the read model is still published below.
                Ok(stepped) if stepped.events.is_empty() => {}
                Ok(stepped) => {
                    records.push(self.record(t, None, None, stepped.events, stepped.outcome));
                }
                Err(e) => failure = Some(e),
            }
        }
        let batch_size = batch.len();
        let mut appends = Vec::new();
        for Submission {
            command,
            key,
            reply,
            caller,
            inbox: _,
        } in batch
        {
            // The command's span in its caller's trace: its type, player and, once known, its seq
            // and outcome. Never its idempotency key (P5.1 M1).
            let span = tracing::info_span!(
                parent: &caller,
                "host.command",
                command.r#type = instruments::action_type(&command.action),
                player_id = instruments::player_uuid(command.player.0),
                seq = tracing::field::Empty,
                outcome = tracing::field::Empty,
            );
            if failure.is_some() {
                // Not applied after a failure: dropping the reply answers Stopped.
                continue;
            }
            // A known key: its first receipt, or a conflict. Nothing applied or appended.
            if let Some(known) = key
                .as_ref()
                .and_then(|k| self.keys.get(&(command.player, k.clone())))
            {
                let answer = if known.command == command {
                    self.stats.replays.fetch_add(1, Ordering::Relaxed);
                    span.record("outcome", "replayed");
                    span.record("seq", known.receipt.seq);
                    Ok(known.receipt.clone())
                } else {
                    span.record("outcome", "key_conflict");
                    Err(HostError::KeyConflict {
                        seq: known.receipt.seq,
                    })
                };
                pending.push(Pending { reply, answer });
                continue;
            }
            let t = self.stamp();
            // 4. Through the shared record→core function, as replay runs it (D-038).
            let started = std::time::Instant::now();
            let stepped = tracing::info_span!(parent: &span, "host.step", t).in_scope(|| {
                replay::step(&mut self.world, &Step::Command(&command), t, &self.rules)
            });
            self.instruments.step(started.elapsed(), true);
            let stepped = match stepped {
                Ok(stepped) => stepped,
                Err(e) => {
                    // The poisoning command is not logged; dropping its reply answers Stopped.
                    failure = Some(e);
                    continue;
                }
            };
            self.stats.steps.fetch_add(1, Ordering::Relaxed);
            changed = true;
            let record = self.record(
                t,
                Some(&command),
                key.clone(),
                stepped.events,
                stepped.outcome,
            );
            let receipt = Receipt {
                seq: record.seq,
                t,
                outcome: stepped.result,
            };
            span.record("seq", record.seq);
            span.record(
                "outcome",
                match &receipt.outcome {
                    Ok(_) => "accepted",
                    Err(rejection) => rejection.code(),
                },
            );
            appends.push(span);
            if let Some(key) = key {
                // Cached now, so a repeat later in this turn gets this receipt, after the append.
                self.keys.insert(
                    (command.player, key),
                    Known {
                        command,
                        receipt: receipt.clone(),
                    },
                );
            }
            pending.push(Pending {
                reply,
                answer: Ok(receipt),
            });
            records.push(record);
        }
        // 5. One durable write for the turn, before anything is acknowledged.
        if !records.is_empty() {
            // One append for the turn, shown in each command's trace (T1): one `host.append` per
            // command, all open around the single write, each saying the batch it was part of.
            let shared: Vec<tracing::Span> = appends
                .iter()
                .map(|command| {
                    tracing::info_span!(parent: command, "host.append", records = records.len(), batch.size = batch_size)
                })
                .collect();
            let span = tracing::info_span!(
                "host.append",
                records = records.len(),
                batch.size = batch_size
            );
            let started = std::time::Instant::now();
            let appended = self.store.append(&records).instrument(span).await;
            self.instruments.append(started.elapsed());
            drop(shared);
            if let Err(e) = appended {
                tracing::error!(error = %e, "append failed; the host stops");
                return Err(Fatal::Append(e));
            }
            self.stats.appends.fetch_add(1, Ordering::Relaxed);
        }
        // The command spans end before their replies, so a caller's trace is whole when it hears.
        drop(appends);
        // 6. Publish, then reply: every step that happened is durable or needs no record, and a
        // caller that reads after its reply sees its own effect (D-055).
        if failure.is_none() && changed {
            let seq = records.last().map_or(self.publish.borrow().seq, |r| r.seq);
            self.publish.send_replace(Arc::new(ReadModel {
                seq,
                world: Arc::new(self.world.clone()),
                rules: self.rules.clone(),
            }));
        }
        for p in pending {
            let _ = p.reply.send(p.answer);
        }
        // Keys older than the window leave the cache; the log still knows them.
        let oldest = self.last_t.saturating_sub(self.config.key_window_ms);
        self.keys.retain(|_, k| k.receipt.t >= oldest);
        // 3. Snapshots between turns, never mid-turn, once enough records were appended.
        self.since_snapshot = self.since_snapshot.saturating_add(records.len() as u64);
        if failure.is_none() && self.since_snapshot >= self.config.snapshot_every_records.max(1) {
            self.snapshot().await;
        }
        match failure {
            None => Ok(()),
            Some(e) => {
                tracing::error!(error = %e, "the engine failed; the host stops");
                Err(Fatal::Engine(e))
            }
        }
    }

    /// Writes a snapshot of the World as it is between turns, after record `next_seq - 1`. A
    /// failure is not fatal (the log is the truth), but it is loud: an error event and
    /// `HostStats::snapshot_failures`; the next schedule point retries (D-038).
    async fn snapshot(&mut self) {
        let seq = self.next_seq - 1;
        if self.last_snapshot_seq == Some(seq) {
            // The newest snapshot already covers every record; nothing new to keep.
            self.since_snapshot = 0;
            return;
        }
        let span = tracing::info_span!("host.snapshot", seq, state_hash = tracing::field::Empty);
        let written = match snapshot::encode(&self.world, &self.config.engine_build) {
            Ok(snap) => {
                span.record("state_hash", hex(&snap.header.state_hash));
                self.store
                    .write_snapshot(&SnapshotRow::new(seq, &snap))
                    .instrument(span.clone())
                    .await
                    .map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
        };
        match written {
            Ok(()) => {
                self.gauges
                    .snapshot_at
                    .store(self.world.time_ms, Ordering::Relaxed);
                self.stats.snapshots.fetch_add(1, Ordering::Relaxed);
                self.last_snapshot_seq = Some(seq);
                self.since_snapshot = 0;
            }
            Err(error) => {
                self.stats.snapshot_failures.fetch_add(1, Ordering::Relaxed);
                tracing::error!(seq, %error, "snapshot failed; the host carries on");
            }
        }
    }

    /// Publishes the oldest queue entry's game time for the event-lag gauge.
    fn due_changed(&self) {
        let due = self.world.queue().first().map_or(NONE, |e| e.at_ms);
        self.gauges.oldest_due.store(due, Ordering::Relaxed);
    }

    /// 3. t = max(wall clock − epoch, last t): stamps never go backwards.
    fn stamp(&mut self) -> i64 {
        let wall = self
            .clock
            .now_unix_ms()
            .saturating_sub(self.config.epoch_unix_ms);
        self.last_t = self.last_t.max(wall);
        self.last_t
    }

    /// A record with the next sequence number. Numbers are taken only for records that will be
    /// appended, and a failed append is fatal, so the log stays gap-free.
    fn record(
        &mut self,
        t: i64,
        command: Option<&Command>,
        key: Option<IdempotencyKey>,
        events: Vec<DomainEvent>,
        outcome: Outcome,
    ) -> LogRecord {
        let seq = self.next_seq;
        self.next_seq += 1;
        tracing::debug!(seq, t, command = command.is_some(), "host.step");
        LogRecord {
            seq,
            kind: if command.is_some() {
                RecordKind::Command
            } else {
                RecordKind::Advance
            },
            game_time: t,
            wall_time_ms: self.clock.now_unix_ms(),
            player: command.map(|c| c.player),
            idempotency_key: key,
            payload: command.map_or(Payload::Empty, |c| Payload::Command(c.clone())),
            outcome,
            events,
            rules_version: self.world.rules_version,
            engine_build: self.config.engine_build.clone(),
        }
    }
}

/// Sleeps until `at`, or forever when nothing is due.
async fn sleep_until<C: Clock>(clock: &C, at: Option<i64>) {
    match at {
        Some(at) => clock.sleep_until_unix_ms(at).await,
        None => std::future::pending().await,
    }
}

/// Why a universe could not be recovered from its store.
#[derive(Debug, thiserror::Error)]
pub enum RecoverError {
    /// The store holds no snapshot at all: the universe was never bootstrapped (P2.9 writes the
    /// first snapshot, at seq 0, before any record).
    #[error("no snapshot: the universe has not been bootstrapped")]
    NoSnapshot,
    /// The newest snapshot claims more records than the log holds.
    #[error("the snapshot at seq {snapshot_seq} is past the log head (seq {head})")]
    SnapshotPastHead {
        /// The snapshot's seq.
        snapshot_seq: i64,
        /// The log's last seq (0 when empty).
        head: i64,
    },
    /// The store could not be read.
    #[error("the store could not be read: {0}")]
    Store(#[from] LogError),
    /// The snapshot did not decode strictly (D-035).
    #[error("the snapshot at seq {seq} is invalid: {error}")]
    Snapshot {
        /// The snapshot's seq.
        seq: i64,
        /// Why.
        error: SnapshotError,
    },
    /// A `snapshots` column disagrees with the decoded header (P2.2 review LOW 2).
    #[error("the snapshot at seq {seq}: column {column} disagrees with its header")]
    ColumnMismatch {
        /// The snapshot's seq.
        seq: i64,
        /// Which column.
        column: &'static str,
    },
    /// Replaying the tail failed: a divergence, a gap, another build's record, or an engine
    /// error. The World is discarded.
    #[error("replay failed: {0}")]
    Replay(#[from] ReplayError),
}

/// A recovered World and where it came from.
#[derive(Debug)]
pub struct Recovered {
    /// The World after the snapshot and the replayed tail; `spawn` it.
    pub world: World,
    /// The snapshot's seq.
    pub snapshot_seq: i64,
    /// The last record replayed (the snapshot's seq when the tail is empty).
    pub last_seq: i64,
}

/// Recovers a universe from its store (Architectural Design, Persistence; D-038):
/// 1. the newest snapshot, of any build. The tail after it must hold only `build`'s records:
///    replay refuses another build's record, so no replay crosses builds. A snapshot in a format
///    this build cannot read is a typed error;
/// 2. decoded strictly ([`snapshot::decode`]), with the row's columns cross-checked against the
///    header;
/// 3. every later record replayed through the shared function ([`replay::replay`]), which checks
///    each record's events and outcome and refuses another build's record.
///
/// Verification is event replay plus the state hash; `check_links` (inside decoding) is a
/// structural check only (D-033). The host then catches up to now on its first wake, and
/// `spawn` rebuilds the idempotency keys from the log (D-037).
///
/// # Errors
/// [`RecoverError`].
pub async fn recover<S: LogStore>(
    store: &mut S,
    rules: &Rules,
    build: &EngineBuild,
) -> Result<Recovered, RecoverError> {
    // The newest snapshot, of any build. The tail after it must hold only this build's records,
    // which replay enforces (CrossBuildRecord), so replays never cross builds. That one rule
    // covers a normal restart, an engine upgrade, a crash after one, and a rollback from a clean
    // shutdown (D-038, carry f).
    let row = store
        .latest_snapshot(None)
        .await?
        .ok_or(RecoverError::NoSnapshot)?;
    let seq = row.seq;
    // A snapshot past the log head claims records the log does not hold; recovering from it
    // would make the next acknowledged commands vanish at the following recovery (review M1).
    let head = store.last().await?.map_or(0, |(seq, _)| seq);
    if seq > head {
        return Err(RecoverError::SnapshotPastHead {
            snapshot_seq: seq,
            head,
        });
    }
    let row_build = EngineBuild::new(row.engine_build.clone())
        .map_err(|error| RecoverError::Snapshot { seq, error })?;
    let expect = Expected {
        rules_version: rules.version,
        engine_build: &row_build,
    };
    let decoded = snapshot::decode(&row.payload, &expect)
        .map_err(|error| RecoverError::Snapshot { seq, error })?;
    let header = &decoded.header;
    let mismatch = |column| RecoverError::ColumnMismatch { seq, column };
    if header.game_time_ms != row.game_time {
        return Err(mismatch("game_time"));
    }
    if header.rules_version != row.rules_version {
        return Err(mismatch("rules_version"));
    }
    if header.engine_build != row.engine_build {
        return Err(mismatch("engine_build"));
    }
    if header.state_hash.as_slice() != row.state_hash.as_slice() {
        return Err(mismatch("state_hash"));
    }
    let mut world = decoded.world;
    let tail = store.records_after(seq).await?;
    let last_seq = replay::replay(&mut world, seq, &tail, rules, build)?;
    Ok(Recovered {
        world,
        snapshot_seq: seq,
        last_seq,
    })
}

/// Lower-case hex, for the snapshot span's state hash.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use voidforge_core::command::Action;
    use voidforge_core::state::BuildingKind;
    use voidforge_store::log::MemoryLog;
    use voidforge_testkit::{ALICE, WorldBuilder, v0_rules};

    use super::*;

    /// Publish, then reply (D-036 as corrected by D-055; review-r1's R1-G5): when a command's
    /// reply arrives, `read()` already shows its effect. Phase 3 relies on it: read-your-writes,
    /// and a new player's first view after its join.
    ///
    /// Made certain, not raced: while the test holds a borrow of the read model, the host can't
    /// publish (`watch` holds a read lock for every borrow), so with publish-then-reply the reply
    /// can't arrive. Reply-then-publish would deliver it at once.
    #[tokio::test]
    async fn a_reply_never_arrives_before_its_read_model() {
        let mut b = WorldBuilder::new();
        let home = b.player(ALICE);
        let world = b.build();
        let epoch = 1_760_000_000_000;
        let (handle, task) = spawn(
            world.clone(),
            v0_rules(),
            MemoryLog::new,
            ManualClock::new(epoch),
            HostConfig::new(epoch, EngineBuild::new("0.1.0+host-test").unwrap()),
        )
        .unwrap();
        let command = Command {
            player: ALICE,
            action: Action::BuildBuilding {
                planet: home,
                kind: BuildingKind::Drill,
            },
        };
        let held = handle.read.borrow();
        let reply = tokio::spawn({
            let handle = handle.clone();
            async move { handle.submit(command).await }
        });
        let mut reply = std::pin::pin!(reply);
        assert!(
            tokio::time::timeout(Duration::from_millis(500), &mut reply)
                .await
                .is_err(),
            "the reply arrived while the read model couldn't be published"
        );
        drop(held);
        let receipt = tokio::time::timeout(Duration::from_secs(20), reply)
            .await
            .expect("the reply, once the publish can happen")
            .unwrap()
            .unwrap();
        assert!(receipt.outcome.is_ok(), "{receipt:?}");
        let read = handle.read();
        assert!(read.seq >= receipt.seq, "{} < {}", read.seq, receipt.seq);
        assert_ne!(*read.world, world, "the read model shows the Drill");
        drop((handle, read));
        task.join().unwrap();
    }
}
