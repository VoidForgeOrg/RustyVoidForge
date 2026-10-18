//! Engine host: inbox, stamping, log, snapshots, read model.
//!
//! The single writer for a universe (Architectural Design, Engine host and write path; decision
//! log D-036). One OS thread, running a current-thread tokio runtime, owns the `World`; everything
//! else reaches it through a bounded inbox ([`HostHandle::submit`]) or reads the immutable
//! [`ReadModel`] it publishes.
//!
//! Each turn of the loop:
//! 1. waits for a command or for the next queue entry to come due, never polling;
//! 2. (idempotency is P2.4);
//! 3. stamps t = max(wall clock − epoch, last t) and takes the next sequence number;
//! 4. runs `apply(command, t)`, or `advance_to(t)` when only time has passed;
//! 5. appends the turn's records in one durable write (group commit: commands already waiting
//!    join the turn, up to `max_batch`, with no added delay);
//! 6. replies and publishes the read model.
//!
//! One record per step. A command record's events are the apply's advance followed by the
//! command's own events, and replay compares the flattened list. An advance that fires no events
//! is not logged, because D-024's observation invariance means the next logged step reaches the
//! same state, but the read model is still published after it. A failed append, or an engine error
//! that poisons the world, is fatal: the host stops, and nothing it did not make durable is ever
//! acknowledged.

pub mod clock;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{mpsc, oneshot, watch};
use tracing::Instrument;
use voidforge_core::command::{Command, Rejection};
use voidforge_core::rules::Rules;
use voidforge_core::state::{DomainEvent, EngineError, World};
use voidforge_store::log::{LogError, LogRecord, LogStore, Outcome, Payload, RecordKind};
use voidforge_store::snapshot::EngineBuild;

pub use clock::{Clock, ManualClock, SystemClock};

/// How a host runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfig {
    /// The universe epoch in Unix milliseconds: game time is wall time minus this. An input until
    /// the universe bootstrap (P2.9) records it.
    pub epoch_unix_ms: i64,
    /// How many commands may wait in the inbox before [`HostError::Busy`].
    pub inbox_capacity: usize,
    /// The most commands one turn takes, and so the most records one durable write holds.
    pub max_batch: usize,
    /// The build written into every record.
    pub engine_build: EngineBuild,
}

impl HostConfig {
    /// The defaults: an inbox of 1,024 and turns of up to 64 commands.
    #[must_use]
    pub fn new(epoch_unix_ms: i64, engine_build: EngineBuild) -> Self {
        Self {
            epoch_unix_ms,
            inbox_capacity: 1024,
            max_batch: 64,
            engine_build,
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
    reply: oneshot::Sender<Receipt>,
}

/// The way in: submit commands and read the read model. Clones share one host. The host stops
/// on its own once every handle is gone.
#[derive(Debug, Clone)]
pub struct HostHandle {
    inbox: mpsc::Sender<Submission>,
    read: watch::Receiver<Arc<ReadModel>>,
    stats: Arc<HostStats>,
}

impl HostHandle {
    /// Submits a command and waits until its record is durable.
    ///
    /// # Errors
    /// [`HostError::Busy`] at once if the inbox is full; [`HostError::Stopped`] if the host has
    /// stopped or stops before the command's record is durable.
    pub async fn submit(&self, command: Command) -> Result<Receipt, HostError> {
        let (reply, receipt) = oneshot::channel();
        self.inbox
            .try_send(Submission { command, reply })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => HostError::Busy,
                mpsc::error::TrySendError::Closed(_) => HostError::Stopped,
            })?;
        receipt.await.map_err(|_| HostError::Stopped)
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
    let (started, started_rx) = std::sync::mpsc::sync_channel(1);
    let stats_host = stats.clone();
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
                        next_seq: 1,
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
        Ok(Ok(())) => Ok((HostHandle { inbox, read, stats }, task)),
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
    clock: C,
    config: HostConfig,
    rx: mpsc::Receiver<Submission>,
    publish: watch::Sender<Arc<ReadModel>>,
    stats: Arc<HostStats>,
    next_seq: i64,
    last_t: i64,
}

/// A step's reply, sent once its record is durable.
struct Pending {
    reply: oneshot::Sender<Receipt>,
    receipt: Receipt,
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
        let _ = started.send(Ok(()));
        let result = self.serve().await;
        // Stopping: refuse what is still queued, so no caller waits forever.
        self.rx.close();
        while self.rx.try_recv().is_ok() {}
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
                () = sleep_until(&self.clock, wake_at) => None,
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
            let span = tracing::info_span!("host.turn", commands = batch.len());
            self.turn(batch).instrument(span).await?;
        }
    }

    /// One turn: the steps, one durable write, then the replies and the read model.
    async fn turn(&mut self, batch: Vec<Submission>) -> Result<(), Fatal> {
        let mut records = Vec::new();
        let mut pending = Vec::new();
        let mut failure = None;
        if batch.is_empty() {
            // 4. Only time has passed.
            let t = self.stamp();
            self.stats.steps.fetch_add(1, Ordering::Relaxed);
            match self.world.advance_to(t, &self.rules) {
                // No events: not logged; the read model is still published below.
                Ok(events) if events.is_empty() => {}
                Ok(events) => records.push(self.record(t, None, events, Outcome::Empty)),
                Err(e) => failure = Some(e),
            }
        }
        for Submission { command, reply } in batch {
            if failure.is_some() {
                // Not applied after a failure: dropping the reply answers Stopped.
                continue;
            }
            let t = self.stamp();
            let applied = match self.world.apply(&command, t, &self.rules) {
                Ok(applied) => applied,
                Err(e) => {
                    // The poisoning command is not logged; dropping its reply answers Stopped.
                    failure = Some(e);
                    continue;
                }
            };
            self.stats.steps.fetch_add(1, Ordering::Relaxed);
            let mut events = applied.advanced;
            let (outcome, result) = match applied.outcome {
                Ok(own) => {
                    events.extend_from_slice(&own);
                    (Outcome::Accepted, Ok(own))
                }
                Err(reason) => (Outcome::Rejected(reason), Err(reason)),
            };
            let record = self.record(t, Some(&command), events, outcome);
            pending.push(Pending {
                reply,
                receipt: Receipt {
                    seq: record.seq,
                    t,
                    outcome: result,
                },
            });
            records.push(record);
        }
        // 5. One durable write for the turn, before anything is acknowledged.
        if !records.is_empty() {
            let span = tracing::info_span!("host.append", records = records.len());
            if let Err(e) = self.store.append(&records).instrument(span).await {
                tracing::error!(error = %e, "append failed; the host stops");
                return Err(Fatal::Append(e));
            }
            self.stats.appends.fetch_add(1, Ordering::Relaxed);
        }
        // 6. Reply and publish: every step that happened is durable or needs no record.
        if failure.is_none() {
            let seq = records.last().map_or(self.publish.borrow().seq, |r| r.seq);
            self.publish.send_replace(Arc::new(ReadModel {
                seq,
                world: Arc::new(self.world.clone()),
                rules: self.rules.clone(),
            }));
        }
        for p in pending {
            let _ = p.reply.send(p.receipt);
        }
        match failure {
            None => Ok(()),
            Some(e) => {
                tracing::error!(error = %e, "the engine failed; the host stops");
                Err(Fatal::Engine(e))
            }
        }
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
            idempotency_key: None,
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
