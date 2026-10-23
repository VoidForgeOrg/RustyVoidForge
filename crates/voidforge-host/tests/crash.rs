//! The crash-and-retry harness (Technical Plan P2.8; decision log D-042): the host runs in a
//! child process that aborts itself at a kill point chosen from the seed; clients retry every
//! unacknowledged command with the same key; every command ends with exactly one outcome.
//!
//! The parent (each `#[test]` here) re-invokes this test binary as a child (the ignored
//! `crash_child`), which recovers from the store (D-038), runs the real host, and talks JSON lines
//! over stdin/stdout, each prefixed `@crash `. The kill is `std::process::abort` at the Nth hit of
//! a named point, in the child's store wrapper or protocol loop: no destructors, no flush, and
//! Postgres rolls back an open transaction on the dropped connection. No production code has a
//! hook. Kill points and their crash-table rows are in D-042.
//!
//! Postgres's `append.during` dies inside PgLog's own open transaction: a test-only trigger on
//! `log_records`, armed through the `crash_hold` table, holds PgLog's first INSERT in `pg_sleep`
//! with `client_connection_check_interval` set; the child aborts once that backend is seen
//! sleeping, and the server rolls the transaction back when it finds the client gone.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command as Process, Stdio};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;

use common::filelog::FileLog;
use common::{EPOCH, build, settle};
use serde::{Deserialize, Serialize};
use voidforge_core::command::Command;
use voidforge_core::quantity::MS_PER_HOUR;
use voidforge_core::rules::Rules;
use voidforge_core::state::{DomainEvent, PlayerId};
use voidforge_host::{HostConfig, HostError, HostHandle, ManualClock, Receipt};
use voidforge_store::log::{
    IdempotencyKey, LogError, LogRecord, LogStore, MemoryLog, Outcome, PgLog, SnapshotMeta,
    SnapshotRow,
};
use voidforge_store::snapshot;
use voidforge_store::verify::{hex, verify_log};
use voidforge_testkit::determinism::{Economy, mix};
use voidforge_testkit::v0_rules;

const END_MS: i64 = 105 * MS_PER_HOUR;
const MAX_LIVES: u64 = 8;

/// The kill points (D-042), each with its crash-table row.
const FILE_POINTS: [&str; 8] = [
    "append.before",   // before the append
    "append.during",   // before the append (atomicity): a torn write before its commit line
    "append.after",    // after the append, before the reply
    "reply.after",     // after the reply
    "snapshot.before", // between reply and snapshot
    "snapshot.during", // a torn snapshot file, never renamed
    "snapshot.after",  // after the snapshot
    "recover.during",  // during recovery
];
/// Postgres: a snapshot is one autocommit INSERT, so `snapshot.during` is not a distinct point.
const PG_POINTS: [&str; 7] = [
    "append.before",
    "append.during", // inside PgLog's open transaction, after its first INSERT
    "append.after",
    "reply.after",
    "snapshot.before",
    "snapshot.after",
    "recover.during",
];

// --- The protocol ---

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ToChild {
    Clock(i64),
    Plan(Vec<u64>),
    Submit(Vec<Planned>),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Planned {
    command: Command,
    key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Got {
    seq: i64,
    t: i64,
    rejected: Option<String>,
    events: Vec<DomainEvent>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FromChild {
    Ok,
    Commands(Vec<Command>),
    Receipt(Got),
    Error(String),
    Bye,
}

const PREFIX: &str = "@crash ";

fn send_line<W: Write>(w: &mut W, msg: &impl Serialize) {
    writeln!(w, "{PREFIX}{}", serde_json::to_string(msg).unwrap()).unwrap();
    w.flush().unwrap();
}

// --- Where the store is ---

#[derive(Debug, Clone)]
enum Where {
    File(PathBuf),
    Pg(String),
}

impl Where {
    fn env(&self) -> String {
        match self {
            Self::File(p) => format!("file:{}", p.display()),
            Self::Pg(url) => format!("pg:{url}"),
        }
    }

    fn parse(s: &str) -> Self {
        match s.split_once(':').unwrap() {
            ("file", p) => Self::File(PathBuf::from(p)),
            ("pg", url) => Self::Pg(url.to_owned()),
            _ => panic!("store {s}"),
        }
    }
}

// --- The child: the host, a kill wrapper, and the protocol loop ---

/// The child's store: the real store, killed at the Nth hit of one point.
struct KillStore {
    file: Option<FileLog>,
    pg: Option<PgLog>,
    url: Option<String>,
    kill: Option<(String, u32)>,
    hits: BTreeMap<&'static str, u32>,
    recovering: bool,
}

/// Dies at `point`, naming the batch's size and first seq for the parent's checks.
fn die(point: &str, batch: usize, seq: i64) -> ! {
    eprintln!("@crash-kill {point} {batch} {seq}");
    std::process::abort()
}

/// The child's turn gate (OPR.42): it holds the host's next append while the child fills the
/// inbox, so which commands share a turn is fixed by the protocol, not by how the host's thread and
/// the child's happen to be scheduled. Without it, a slow runner let the host take each command
/// alone, and no seeded `append.*` kill met a multi-command turn (`mid_turn: false` on a0a6e6b's
/// CI). The host forms a turn's batch the moment it takes the first command, so a pair can't be
/// made certain without a hook in the host; the gate makes the shape `[first]`, then `[the rest]`.
/// Test code only: the host runs unchanged (D-042).
struct Gate {
    /// Set by the child before a submit; the next append takes it and holds.
    armed: AtomicBool,
    /// The host is held inside an append.
    entered: tokio::sync::Notify,
    /// The child has filled the inbox: the held append goes on.
    release: tokio::sync::Notify,
}

static GATE: LazyLock<Gate> = LazyLock::new(|| Gate {
    armed: AtomicBool::new(false),
    entered: tokio::sync::Notify::new(),
    release: tokio::sync::Notify::new(),
});

/// Submits `command` and polls it once in place, so on return it's in the host's inbox (or
/// refused at once), whatever the scheduler does next; the rest of the wait runs as a task.
async fn enqueue(
    handle: &HostHandle,
    command: Command,
    key: IdempotencyKey,
) -> tokio::task::JoinHandle<Result<Receipt, HostError>> {
    let h = handle.clone();
    let mut submit = Box::pin(async move { h.submit_with_key(command, key).await });
    match std::future::poll_fn(|cx| Poll::Ready(submit.as_mut().poll(cx))).await {
        Poll::Ready(r) => tokio::spawn(async move { r }),
        Poll::Pending => tokio::spawn(submit),
    }
}

impl KillStore {
    fn open(at: &Where, kill: Option<(String, u32)>, recovering: bool) -> Self {
        let (file, pg, url) = match at {
            Where::File(dir) => (Some(FileLog::new(dir)), None, None),
            Where::Pg(url) => (None, Some(PgLog::lazy_url(url).unwrap()), Some(url.clone())),
        };
        Self {
            file,
            pg,
            url,
            kill,
            hits: BTreeMap::new(),
            recovering,
        }
    }

    /// Counts a hit of `point`; true when this is the kill.
    fn hit(&mut self, point: &'static str) -> bool {
        let n = self.hits.entry(point).or_default();
        *n += 1;
        self.kill
            .as_ref()
            .is_some_and(|(p, at)| p == point && *at == *n)
    }

    /// Postgres `append.during`: arms the trigger, then aborts once PgLog's backend sleeps inside
    /// its open transaction, after its first INSERT.
    async fn hold_and_die(&self, batch: usize, seq: i64) {
        let url = self.url.clone().unwrap();
        let control = sqlx::PgPool::connect(&url).await.unwrap();
        sqlx::raw_sql("INSERT INTO crash_hold VALUES (true)")
            .execute(&control)
            .await
            .unwrap();
        tokio::spawn(async move {
            loop {
                let rows = sqlx::raw_sql(
                    "SELECT 1 FROM pg_stat_activity WHERE wait_event = 'PgSleep' \
                     AND datname = current_database() AND query LIKE '%INSERT INTO log_records%'",
                )
                .fetch_all(&control)
                .await
                .unwrap();
                if !rows.is_empty() {
                    die("append.during", batch, seq);
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });
    }
}

impl LogStore for KillStore {
    async fn last(&mut self) -> Result<Option<(i64, i64)>, LogError> {
        match &mut self.pg {
            Some(pg) => pg.last().await,
            None => self.file.as_mut().unwrap().last().await,
        }
    }

    async fn append(&mut self, records: &[LogRecord]) -> Result<(), LogError> {
        // Before the kill points count, so a kill at a held append still fires, after the release.
        if GATE.armed.swap(false, Ordering::SeqCst) {
            GATE.entered.notify_one();
            GATE.release.notified().await;
        }
        let batch = records.len();
        let seq = records.first().map_or(0, |r| r.seq);
        if self.hit("append.before") {
            die("append.before", batch, seq);
        }
        if self.hit("append.during") {
            match &self.file {
                Some(f) => {
                    // A torn append: every line but the commit, and half of the last record.
                    f.discard_uncommitted()?;
                    let bytes = FileLog::append_bytes(records)?;
                    let body = &bytes[..bytes.len()
                        - format!("{{\"commit\":{}}}\n", records.last().unwrap().seq).len()];
                    f.write_records(&body[..body.len() - body.len() / 4], false)?;
                    die("append.during", batch, seq);
                }
                None => self.hold_and_die(batch, seq).await,
            }
        }
        match &mut self.pg {
            Some(pg) => pg.append(records).await?,
            None => self.file.as_mut().unwrap().append(records).await?,
        }
        if self.hit("append.after") {
            die("append.after", batch, seq);
        }
        Ok(())
    }

    async fn records(&mut self) -> Result<Vec<LogRecord>, LogError> {
        match &mut self.pg {
            Some(pg) => pg.records().await,
            None => self.file.as_mut().unwrap().records().await,
        }
    }

    async fn find_keys(
        &mut self,
        keys: &[(PlayerId, IdempotencyKey)],
    ) -> Result<Vec<LogRecord>, LogError> {
        match &mut self.pg {
            Some(pg) => pg.find_keys(keys).await,
            None => self.file.as_mut().unwrap().find_keys(keys).await,
        }
    }

    async fn keyed_since(&mut self, since: i64) -> Result<Vec<LogRecord>, LogError> {
        match &mut self.pg {
            Some(pg) => pg.keyed_since(since).await,
            None => self.file.as_mut().unwrap().keyed_since(since).await,
        }
    }

    async fn records_after(&mut self, seq: i64) -> Result<Vec<LogRecord>, LogError> {
        if self.recovering && self.hit("recover.during") {
            die("recover.during", 0, seq);
        }
        match &mut self.pg {
            Some(pg) => pg.records_after(seq).await,
            None => self.file.as_mut().unwrap().records_after(seq).await,
        }
    }

    async fn write_snapshot(&mut self, row: &SnapshotRow) -> Result<(), LogError> {
        if self.hit("snapshot.before") {
            die("snapshot.before", 0, row.seq);
        }
        if self.hit("snapshot.during") {
            if let Some(f) = &self.file {
                let bytes = FileLog::snapshot_bytes(row);
                f.write_snapshot_bytes(row.seq, &bytes[..bytes.len() / 2], false)?;
            }
            die("snapshot.during", 0, row.seq);
        }
        match &mut self.pg {
            Some(pg) => pg.write_snapshot(row).await?,
            None => self.file.as_mut().unwrap().write_snapshot(row).await?,
        }
        if self.hit("snapshot.after") {
            die("snapshot.after", 0, row.seq);
        }
        Ok(())
    }

    async fn latest_snapshot(
        &mut self,
        build: Option<&str>,
    ) -> Result<Option<SnapshotRow>, LogError> {
        match &mut self.pg {
            Some(pg) => pg.latest_snapshot(build).await,
            None => self.file.as_mut().unwrap().latest_snapshot(build).await,
        }
    }

    async fn snapshot_index(&mut self) -> Result<Vec<SnapshotMeta>, LogError> {
        match &mut self.pg {
            Some(pg) => pg.snapshot_index().await,
            None => self.file.as_mut().unwrap().snapshot_index().await,
        }
    }

    async fn snapshot_at(&mut self, seq: i64) -> Result<Option<SnapshotRow>, LogError> {
        match &mut self.pg {
            Some(pg) => pg.snapshot_at(seq).await,
            None => self.file.as_mut().unwrap().snapshot_at(seq).await,
        }
    }
}

fn economy(seed: u64, commands: u64) -> Economy {
    Economy {
        seed,
        planets: 3,
        fleets: true,
        commands,
    }
}

fn got(receipt: &Receipt) -> Got {
    Got {
        seq: receipt.seq,
        t: receipt.t,
        rejected: receipt.outcome.as_ref().err().map(|r| r.code().to_owned()),
        events: receipt.outcome.clone().unwrap_or_default(),
    }
}

/// The child: run only by the parent, through `VOIDFORGE_CRASH_CHILD`.
#[test]
#[ignore = "the crash harness's child process"]
fn crash_child() {
    let Ok(at) = std::env::var("VOIDFORGE_CRASH_CHILD") else {
        return;
    };
    let at = Where::parse(&at);
    let env = |k: &str| std::env::var(k).unwrap();
    let seed: u64 = env("VOIDFORGE_CRASH_SEED").parse().unwrap();
    let commands: u64 = env("VOIDFORGE_CRASH_COMMANDS").parse().unwrap();
    let now: i64 = env("VOIDFORGE_CRASH_NOW").parse().unwrap();
    let kill = env("VOIDFORGE_CRASH_KILL")
        .split_once(':')
        .map(|(p, n)| (p.to_owned(), n.parse::<u32>().unwrap()));
    let e = economy(seed, commands);
    let rules = v0_rules();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut store = KillStore::open(&at, kill.clone(), true);
        let recovered = voidforge_host::recover(&mut store, &rules, &build())
            .await
            .expect("recovered");
        let clock = ManualClock::new(EPOCH + now);
        let config = HostConfig {
            snapshot_every_records: 4,
            ..HostConfig::new(EPOCH, build())
        };
        let host_at = at.clone();
        let host_kill = kill.clone();
        let (mut handle, task) = voidforge_host::spawn(
            recovered.world,
            rules,
            move || KillStore::open(&host_at, host_kill, false),
            clock.clone(),
            config,
        )
        .unwrap();
        let mut replies = 0_u32;
        let mut out = std::io::stdout();
        for line in std::io::stdin().lock().lines() {
            let line = line.unwrap();
            let Some(msg) = line.strip_prefix(PREFIX) else {
                continue;
            };
            match serde_json::from_str::<ToChild>(msg).unwrap() {
                ToChild::Clock(t) => {
                    clock.set(EPOCH + t);
                    settle(&mut handle, &clock).await;
                    send_line(&mut out, &FromChild::Ok);
                }
                ToChild::Plan(ks) => {
                    let world = handle.read().world.clone();
                    let commands = ks.iter().map(|&k| e.command(k, &world)).collect();
                    send_line(&mut out, &FromChild::Commands(commands));
                }
                ToChild::Submit(planned) => {
                    // A fixed shape for every submit (OPR.42): commands go one at a time until
                    // one's append is held at the gate (a retry of a durable command replies
                    // without appending, so the next one goes); then every other command goes
                    // into the inbox before the release. So the held turn has exactly its own
                    // commands and the next turn has all the rest: multi-command turns, the same
                    // on every run.
                    GATE.armed.store(true, Ordering::SeqCst);
                    let mut held = false;
                    let mut tasks = Vec::new();
                    for p in planned {
                        let key = IdempotencyKey::new(p.key).unwrap();
                        let mut t = enqueue(&handle, p.command, key).await;
                        if !held {
                            tokio::select! {
                                biased;
                                () = GATE.entered.notified() => held = true,
                                r = &mut t => {
                                    let r = r.unwrap();
                                    t = tokio::spawn(async move { r });
                                }
                            }
                        }
                        tasks.push(t);
                    }
                    if held {
                        GATE.release.notify_one();
                    } else {
                        // Every command was answered without an append: nothing is held.
                        GATE.armed.store(false, Ordering::SeqCst);
                    }
                    for t in tasks {
                        let reply = match t.await.unwrap() {
                            Ok(r) => FromChild::Receipt(got(&r)),
                            Err(e) => FromChild::Error(format!("{e:?}")),
                        };
                        send_line(&mut out, &reply);
                        replies += 1;
                        if kill
                            .as_ref()
                            .is_some_and(|(p, n)| p == "reply.after" && *n == replies)
                        {
                            die("reply.after", 0, 0);
                        }
                    }
                }
                ToChild::Shutdown => {
                    drop(handle);
                    task.join().expect("a clean stop");
                    send_line(&mut out, &FromChild::Bye);
                    return;
                }
            }
        }
    });
}

// --- The parent ---

struct Life {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Life {
    fn start(at: &Where, seed: u64, commands: u64, now: i64, kill: &Option<(String, u32)>) -> Self {
        let mut child = Process::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "crash_child",
                "--ignored",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env("VOIDFORGE_CRASH_CHILD", at.env())
            .env("VOIDFORGE_CRASH_SEED", seed.to_string())
            .env("VOIDFORGE_CRASH_COMMANDS", commands.to_string())
            .env("VOIDFORGE_CRASH_NOW", now.to_string())
            .env(
                "VOIDFORGE_CRASH_KILL",
                kill.as_ref()
                    .map_or("none".to_owned(), |(p, n)| format!("{p}:{n}")),
            )
            .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Always piped, so the kill marker is parsed; CRASH_DEBUG echoes it afterwards.
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    /// Sends a message and reads `n` replies, or the replies before the child died.
    fn ask(&mut self, msg: &ToChild, n: usize) -> (Vec<FromChild>, bool) {
        if std::env::var("CRASH_DEBUG").is_ok() {
            eprintln!(
                "ask {}",
                serde_json::to_string(msg)
                    .unwrap()
                    .chars()
                    .take(120)
                    .collect::<String>()
            );
        }
        if writeln!(
            self.stdin,
            "{PREFIX}{}",
            serde_json::to_string(msg).unwrap()
        )
        .and_then(|()| self.stdin.flush())
        .is_err()
        {
            return (Vec::new(), true);
        }
        let mut replies = Vec::new();
        let mut line = String::new();
        while replies.len() < n {
            line.clear();
            if self.stdout.read_line(&mut line).unwrap_or(0) == 0 {
                return (replies, true);
            }
            // libtest may print "test crash_child ... " before the first reply on the same line.
            if let Some(at) = line.find(PREFIX) {
                replies.push(serde_json::from_str(line[at + PREFIX.len()..].trim_end()).unwrap());
            }
        }
        (replies, false)
    }

    /// Waits for the child to end; the kill point, batch size and first seq if it died at one.
    fn end(self) -> Option<(String, usize, i64)> {
        drop(self.stdin);
        let out = self.child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        if std::env::var("CRASH_DEBUG").is_ok() {
            eprint!("{stderr}");
        }
        let kill = stderr.lines().find_map(|l| {
            let rest = l.strip_prefix("@crash-kill ")?;
            let mut f = rest.split(' ');
            let (p, b, s) = (f.next()?, f.next()?, f.next()?);
            Some((p.to_owned(), b.parse().unwrap(), s.parse().unwrap()))
        });
        assert!(
            kill.is_some() || out.status.success(),
            "the child failed without a kill: {stderr}"
        );
        kill
    }
}

/// A store the parent reads after a run.
async fn read_records(at: &Where) -> Vec<LogRecord> {
    match at {
        Where::File(dir) => FileLog::new(dir).committed().unwrap(),
        Where::Pg(url) => PgLog::lazy_url(url).unwrap().records().await.unwrap(),
    }
}

/// What one seed's run saw.
#[derive(Debug, Default, Serialize)]
struct Seen {
    /// Deaths per kill point.
    kills: BTreeMap<String, u32>,
    /// `append.*` kills of a batch of more than one record: inside a multi-command turn.
    mid_turn: u32,
    lives: u64,
    commands: usize,
}

impl Seen {
    fn add(&mut self, other: Seen) {
        for (p, n) in other.kills {
            *self.kills.entry(p).or_default() += n;
        }
        self.mid_turn += other.mid_turn;
        self.lives += other.lives;
        self.commands += other.commands;
    }

    fn points(&self) -> BTreeSet<String> {
        self.kills.keys().cloned().collect()
    }
}

/// One seed: lives killed at seeded points until the script and a clean shutdown complete, then
/// every check (D-042). Panics name the seed.
async fn run_seed(at: &Where, points: &[&str], seed: u64, commands: u64) -> Seen {
    let rules = v0_rules();
    let e = economy(seed, commands);
    let start = e.world(&rules);
    // The bootstrap snapshot at seq 0 (P2.9 writes it for a real universe).
    let row = SnapshotRow::new(0, &snapshot::encode(&start, &build()).unwrap());
    match at {
        Where::File(dir) => FileLog::new(dir).write_snapshot(&row).await.unwrap(),
        Where::Pg(url) => PgLog::lazy_url(url)
            .unwrap()
            .write_snapshot(&row)
            .await
            .unwrap(),
    }
    let mut script = e.script();
    script.sort();
    let mut groups: Vec<(i64, Vec<u64>)> = Vec::new();
    for (t, k) in script {
        match groups.last_mut() {
            Some((gt, ks)) if *gt == t => ks.push(k),
            _ => groups.push((t, vec![k])),
        }
    }
    let key = |k: u64| format!("s{seed}-k{k}");
    let mut planned: BTreeMap<String, Planned> = BTreeMap::new();
    let mut pending: Vec<Planned> = Vec::new();
    let mut acked: BTreeMap<String, Got> = BTreeMap::new();
    let mut next_group = 0;
    let mut now = 0;
    let mut finished = false;
    let mut seen = Seen::default();
    for life in 0..MAX_LIVES {
        seen.lives += 1;
        let kill = (life + 1 < MAX_LIVES && !mix(seed ^ life).is_multiple_of(4)).then(|| {
            let point = points[((seed * 7 + life) % points.len() as u64) as usize];
            let n = if point == "recover.during" {
                1
            } else {
                1 + (mix(seed.wrapping_add(life) ^ 0x5eed) % 3) as u32
            };
            (point.to_owned(), n)
        });
        let mut child = Life::start(at, seed, commands, now, &kill);
        let mut died = false;
        // Retry every unacknowledged command with its key.
        if !pending.is_empty() {
            let (replies, d) = child.ask(&ToChild::Submit(pending.clone()), pending.len());
            for (p, r) in pending.iter().zip(&replies) {
                match r {
                    FromChild::Receipt(g) => {
                        acked.insert(p.key.clone(), g.clone());
                    }
                    other => panic!("seed {seed}: retry of {}: {other:?}", p.key),
                }
            }
            pending.drain(..replies.len());
            died = d;
        }
        while !died && next_group < groups.len() {
            let (t, ks) = groups[next_group].clone();
            now = t;
            let (_, d) = child.ask(&ToChild::Clock(t), 1);
            if d {
                died = true;
                break;
            }
            // The host keeps working after the clock's reply (a snapshot after a turn), so the
            // child can die while we plan: like any death, the next life plans this group again
            // (dev-qa's F1 on 963646a).
            let (replies, d) = child.ask(&ToChild::Plan(ks.clone()), 1);
            if d {
                died = true;
                break;
            }
            let Some(FromChild::Commands(cmds)) = replies.into_iter().next() else {
                panic!("seed {seed}: no plan")
            };
            for (k, command) in ks.iter().zip(cmds) {
                let p = Planned {
                    command,
                    key: key(*k),
                };
                planned.insert(p.key.clone(), p.clone());
                pending.push(p);
            }
            next_group += 1;
            let (replies, d) = child.ask(&ToChild::Submit(pending.clone()), pending.len());
            for (p, r) in pending.iter().zip(&replies) {
                match r {
                    FromChild::Receipt(g) => {
                        acked.insert(p.key.clone(), g.clone());
                    }
                    other => panic!("seed {seed}: {}: {other:?}", p.key),
                }
            }
            pending.drain(..replies.len());
            died = d;
        }
        if !died {
            now = END_MS;
            let (_, d) = child.ask(&ToChild::Clock(END_MS), 1);
            died = d || child.ask(&ToChild::Shutdown, 1).1;
        }
        let ended = child.end();
        if let Some((point, batch, first)) = ended {
            assert!(died, "seed {seed}: a kill the parent did not see");
            // By effect: a batch killed before or during its append is absent (the transaction
            // or torn write was not kept); one killed after it is durable.
            let head = read_records(at).await.last().map_or(0, |r| r.seq);
            match point.as_str() {
                "append.before" | "append.during" => assert!(
                    head < first,
                    "seed {seed}: {point} at seq {first}, but the log reaches {head}"
                ),
                "append.after" => assert!(
                    head >= first + batch as i64 - 1,
                    "seed {seed}: append.after at seq {first}, but the log stops at {head}"
                ),
                _ => {}
            }
            if point.starts_with("append.") && batch > 1 {
                seen.mid_turn += 1;
            }
            after_death(at, &point).await;
            *seen.kills.entry(point).or_default() += 1;
        } else {
            assert!(
                !died,
                "seed {seed}: the child ended without a kill or a goodbye"
            );
            finished = true;
            break;
        }
    }
    assert!(finished, "seed {seed}: no clean life within {MAX_LIVES}");
    assert!(
        pending.is_empty(),
        "seed {seed}: unacknowledged after the end: {pending:?}"
    );
    check(at, &rules, seed, &start, &planned, &acked).await;
    seen.commands = planned.len();
    seen
}

/// After a death: for Postgres's `append.during`, disarm the trigger and wait until the dead
/// client's transaction is gone.
async fn after_death(at: &Where, point: &str) {
    if let (Where::Pg(url), "append.during") = (at, point) {
        let control = sqlx::PgPool::connect(url).await.unwrap();
        sqlx::raw_sql("DELETE FROM crash_hold")
            .execute(&control)
            .await
            .unwrap();
        for _ in 0..1_000 {
            let rows = sqlx::raw_sql(
                "SELECT 1 FROM pg_stat_activity WHERE wait_event = 'PgSleep' \
                 AND datname = current_database()",
            )
            .fetch_all(&control)
            .await
            .unwrap();
            if rows.is_empty() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the dead client's transaction is still open after 5 s");
    }
}

/// D-042's checks.
async fn check(
    at: &Where,
    rules: &Rules,
    seed: u64,
    start: &voidforge_core::state::World,
    planned: &BTreeMap<String, Planned>,
    acked: &BTreeMap<String, Got>,
) {
    let records = read_records(at).await;
    // Gap-free, from 1.
    for (i, r) in records.iter().enumerate() {
        assert_eq!(r.seq, i as i64 + 1, "seed {seed}: a gap at {i}");
    }
    // Exactly one record per planned command, and none without a plan.
    let mut by_key: BTreeMap<String, &LogRecord> = BTreeMap::new();
    for r in records.iter().filter(|r| r.idempotency_key.is_some()) {
        let k = r.idempotency_key.as_ref().unwrap().as_str().to_owned();
        assert!(
            by_key.insert(k.clone(), r).is_none(),
            "seed {seed}: {k} logged twice"
        );
        assert!(
            planned.contains_key(&k),
            "seed {seed}: {k} logged but never planned"
        );
    }
    for (k, p) in planned {
        let r = by_key
            .get(k)
            .unwrap_or_else(|| panic!("seed {seed}: {k} has no outcome in the log"));
        assert_eq!(r.player, Some(p.command.player), "seed {seed}: {k}");
        // The client's final receipt is the logged outcome.
        let g = acked
            .get(k)
            .unwrap_or_else(|| panic!("seed {seed}: {k} was never acknowledged"));
        let (rejected, own) = match r.outcome {
            Outcome::Rejected(reason) => (Some(reason.code().to_owned()), 0),
            Outcome::Accepted { own_events } => (None, own_events as usize),
            Outcome::Empty => panic!("seed {seed}: {k} has no outcome"),
        };
        let events = r.events[r.events.len() - own..].to_vec();
        assert_eq!(
            g,
            &Got {
                seq: r.seq,
                t: r.game_time,
                rejected,
                events
            },
            "seed {seed}: {k}'s receipt differs from its record"
        );
    }
    // The log verifies through the P2.7 path, from the seq-0 snapshot.
    let report = match at {
        Where::File(dir) => {
            verify_log(&mut FileLog::new(dir), rules, &build(), Some(0), None).await
        }
        Where::Pg(url) => {
            verify_log(
                &mut PgLog::lazy_url(url).unwrap(),
                rules,
                &build(),
                Some(0),
                None,
            )
            .await
        }
    }
    .unwrap_or_else(|e| panic!("seed {seed}: the log does not verify: {e}"));
    // The reference: a host that never died, given the logged commands at their logged times.
    let (reference, reference_hash) = reference_run(start, rules, &records).await;
    assert_eq!(
        reference, records,
        "seed {seed}: the log differs from a run without kills"
    );
    let mut world = report.world;
    world.advance_to(END_MS, rules).unwrap();
    assert_eq!(
        hex(&snapshot::state_hash(&world).unwrap()),
        reference_hash,
        "seed {seed}: the recovered World differs from a run without kills"
    );
}

/// The same commands, keys and times through a host that never dies.
async fn reference_run(
    start: &voidforge_core::state::World,
    rules: &Rules,
    records: &[LogRecord],
) -> (Vec<LogRecord>, String) {
    let log = MemoryLog::new();
    let clock = ManualClock::new(EPOCH);
    let (mut handle, task) = voidforge_host::spawn(
        start.clone(),
        rules.clone(),
        {
            let log = log.clone();
            move || log
        },
        clock.clone(),
        HostConfig::new(EPOCH, build()),
    )
    .unwrap();
    let mut i = 0;
    while i < records.len() {
        let t = records[i].game_time;
        clock.set(EPOCH + t);
        settle(&mut handle, &clock).await;
        // Every command logged at t, submitted together.
        let mut tasks = Vec::new();
        while i < records.len() && records[i].game_time == t {
            if let voidforge_store::log::Payload::Command(c) = &records[i].payload {
                let (h, c, k) = (
                    handle.clone(),
                    c.clone(),
                    records[i].idempotency_key.clone().unwrap(),
                );
                tasks.push(tokio::spawn(async move { h.submit_with_key(c, k).await }));
            }
            i += 1;
        }
        for t in tasks {
            t.await.unwrap().unwrap();
        }
    }
    clock.set(EPOCH + END_MS);
    settle(&mut handle, &clock).await;
    let mut world = (*handle.read().world).clone();
    drop(handle);
    task.join().unwrap();
    world.advance_to(END_MS, rules).unwrap();
    (log.snapshot(), hex(&snapshot::state_hash(&world).unwrap()))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("crash")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn knob(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .map_or(default, |v| v.parse().expect("a number"))
}

/// Whose scratch directories: the per-PR runs' or the nightly's (P5.3, D-066). Separate prefixes,
/// so a nightly run never reuses or removes a per-PR seed's directory, whatever its seed base.
#[derive(Clone, Copy)]
enum Run {
    PerPr,
    Nightly,
}

fn scratch_name(run: Run, seed: u64) -> String {
    match run {
        Run::PerPr => format!("file-{seed}"),
        Run::Nightly => format!("nightly-file-{seed}"),
    }
}

async fn file_seeds(run: Run, seeds: std::ops::Range<u64>, commands: u64) -> Seen {
    let mut all = Seen::default();
    for seed in seeds {
        let at = Where::File(scratch(&scratch_name(run, seed)));
        all.add(run_seed(&at, &FILE_POINTS, seed, commands).await);
    }
    all
}

/// Per PR: 12 fixed seeds on the file store; every kill point is reached, and a kill lands inside
/// a multi-command turn. With the turn gate, the counts are the same on every run (OPR.42). A
/// multi-command turn needs a group of three commands (the gate's second turn carries all but the
/// first), so the seeds run 60 commands: at 40 they have groups of one and two only.
#[tokio::test]
async fn every_retried_command_ends_with_exactly_one_outcome_on_the_file_store() {
    let seen = file_seeds(Run::PerPr, 0..12, 60).await;
    eprintln!("file store, 12 seeds: {seen:?}");
    let want: BTreeSet<String> = FILE_POINTS.iter().map(|p| (*p).to_owned()).collect();
    assert_eq!(seen.points(), want, "every kill point is hit");
    assert!(seen.mid_turn > 0, "a kill inside a multi-command turn");
}

#[test]
fn the_nightly_never_shares_a_scratch_directory_with_the_per_pr_runs() {
    for seed in [0, 11, 1_000] {
        assert_ne!(
            scratch_name(Run::Nightly, seed),
            scratch_name(Run::PerPr, seed)
        );
        assert!(!scratch_name(Run::Nightly, seed).starts_with("file-"));
    }
}

/// The night's coverage over all its seeds (P5.3, D-066): every kill point was reached, or the
/// night proves less than it says. Each seed's own checks (exactly one outcome per command) already
/// ran in `run_seed`. A kill inside a multi-command turn is reported, not required: its rate varies
/// too much between seed ranges (28 in seeds 1000..2000, 1 in 31000..32000) to bound, and a red
/// night must mean a defect. The per-PR test requires one on its fixed seeds.
fn assert_covered(seen: &Seen, points: &[&str]) {
    let want: BTreeSet<String> = points.iter().map(|p| (*p).to_owned()).collect();
    let reached = seen.points();
    let missed: Vec<&String> = want.difference(&reached).collect();
    assert!(missed.is_empty(), "kill points never reached: {missed:?}");
}

#[test]
fn a_night_that_misses_a_kill_point_fails() {
    let mut seen = Seen::default();
    for p in &FILE_POINTS[1..] {
        seen.kills.insert((*p).to_owned(), 1);
    }
    let missed = std::panic::catch_unwind(|| assert_covered(&seen, &FILE_POINTS));
    assert!(missed.is_err(), "append.before was never reached");
    seen.kills.insert(FILE_POINTS[0].to_owned(), 1);
    assert_covered(&seen, &FILE_POINTS);
}

/// The night's numbers for the job summary: one line, `@nightly-crash <json>`.
fn summary(store: &str, base: u64, seeds: u64, seen: &Seen) {
    let line =
        serde_json::json!({ "store": store, "seed_base": base, "seeds": seeds, "seen": seen });
    eprintln!("@nightly-crash {line}");
}

/// The nightly shape (P5.3, D-066): seeds from VOIDFORGE_CRASH_SEED_BASE, VOIDFORGE_CRASH_SEEDS of
/// them, VOIDFORGE_CRASH_COMMANDS each, in the nightly's own scratch directories. A failure names
/// its seed; then the night must have reached every kill point.
#[tokio::test]
#[ignore = "nightly: VOIDFORGE_CRASH_SEED_BASE, VOIDFORGE_CRASH_SEEDS, VOIDFORGE_CRASH_COMMANDS"]
async fn crash_nightly() {
    let base = knob("VOIDFORGE_CRASH_SEED_BASE", 1_000);
    let n = knob("VOIDFORGE_CRASH_SEEDS", 200);
    let seen = file_seeds(
        Run::Nightly,
        base..base + n,
        knob("VOIDFORGE_CRASH_COMMANDS", 60),
    )
    .await;
    summary("file", base, n, &seen);
    assert_covered(&seen, &FILE_POINTS);
}

/// The test database's URL: the stage's `DATABASE_URL` with this test's database.
fn pg_url(pool: &sqlx::PgPool) -> String {
    let base = std::env::var("DATABASE_URL").expect("the database stage sets DATABASE_URL");
    let db = pool.connect_options().get_database().unwrap().to_owned();
    format!("{}/{db}", base.rsplit_once('/').unwrap().0)
}

/// The test-only trigger that holds PgLog's INSERT inside its open transaction while armed, and
/// this test database's `client_connection_check_interval`, so a backend notices its client's
/// death while it sleeps and rolls back. (It must be set before a statement starts, so it is the
/// database's default for new connections, PgLog's included.)
const HOLD: &str = r"
DO $$ BEGIN
  EXECUTE format('ALTER DATABASE %I SET client_connection_check_interval = %L',
                 current_database(), '20ms');
END $$;
CREATE TABLE crash_hold (armed boolean NOT NULL);
CREATE FUNCTION crash_hold() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF EXISTS (SELECT 1 FROM crash_hold) THEN
    PERFORM pg_sleep(60);
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER crash_hold AFTER INSERT ON log_records FOR EACH ROW EXECUTE FUNCTION crash_hold();
";

/// `seeds` on one test database, each from an empty log.
async fn pg_seeds(pool: &sqlx::PgPool, seeds: std::ops::Range<u64>, commands: u64) -> Seen {
    sqlx::raw_sql(HOLD).execute(pool).await.unwrap();
    let url = pg_url(pool);
    let mut all = Seen::default();
    for seed in seeds {
        sqlx::raw_sql("TRUNCATE log_records, snapshots")
            .execute(pool)
            .await
            .unwrap();
        all.add(run_seed(&Where::Pg(url.clone()), &PG_POINTS, seed, commands).await);
    }
    all
}

/// Per PR, the Phase 2 gate's "against Postgres": 4 fixed seeds; every Postgres kill point is
/// reached, including a death inside PgLog's open transaction.
#[sqlx::test(migrator = "voidforge_store::MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn every_retried_command_ends_with_exactly_one_outcome_on_postgres(pool: sqlx::PgPool) {
    let all = pg_seeds(&pool, 0..4, 30).await;
    eprintln!("Postgres, 4 seeds: {all:?}");
    let want: BTreeSet<String> = PG_POINTS.iter().map(|p| (*p).to_owned()).collect();
    assert_eq!(all.points(), want, "every Postgres kill point is hit");
}

/// The nightly's Postgres seeds (P5.3, D-066): VOIDFORGE_CRASH_PG_SEEDS of them (default 100) from
/// VOIDFORGE_CRASH_SEED_BASE, VOIDFORGE_CRASH_PG_COMMANDS each (default 30), through the database
/// stage (`database.sh --nightly-crash`), so its database is a `_sqlx_test_*` one beside
/// voidforge_test, as every Postgres test's is. Every Postgres kill point must be reached.
#[sqlx::test(migrator = "voidforge_store::MIGRATOR")]
#[ignore = "nightly, needs Postgres: just nightly-crash"]
async fn crash_nightly_postgres(pool: sqlx::PgPool) {
    let base = knob("VOIDFORGE_CRASH_SEED_BASE", 1_000);
    let n = knob("VOIDFORGE_CRASH_PG_SEEDS", 100);
    let seen = pg_seeds(
        &pool,
        base..base + n,
        knob("VOIDFORGE_CRASH_PG_COMMANDS", 30),
    )
    .await;
    summary("postgres", base, n, &seen);
    assert_covered(&seen, &PG_POINTS);
}
