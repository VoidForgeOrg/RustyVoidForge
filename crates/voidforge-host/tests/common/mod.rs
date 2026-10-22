//! Shared by the host's test binaries: a gated store, waiting for the host to settle, and a
//! scripted economy driven through the host.

#![allow(dead_code)] // each binary uses part of this

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Semaphore, watch};
use voidforge_core::rules::Rules;
use voidforge_core::state::PlayerId;
use voidforge_core::state::World;
use voidforge_host::{HostConfig, HostHandle, ManualClock, Receipt};
use voidforge_store::log::{IdempotencyKey, LogError, LogRecord, LogStore, MemoryLog, SnapshotRow};
use voidforge_store::snapshot::EngineBuild;
use voidforge_testkit::determinism::Economy;

pub mod filelog;
pub mod golden;

/// The universe epoch the tests use: game time 0 is this Unix millisecond.
pub const EPOCH: i64 = 1_760_000_000_000;
/// Long enough for anything a test waits on; a hang fails instead of blocking forever.
pub const PATIENCE: Duration = Duration::from_secs(20);

pub fn build() -> EngineBuild {
    EngineBuild::new("0.1.0+host-test").unwrap()
}

pub fn config() -> HostConfig {
    HostConfig::new(EPOCH, build())
}

/// A clock at game time `t`.
pub fn clock_at(t: i64) -> ManualClock {
    ManualClock::new(EPOCH + t)
}

/// A store whose appends wait for a permit while held, so a test can keep the host inside a
/// durable write and see what it does meanwhile.
#[derive(Clone)]
pub struct GatedLog {
    pub inner: MemoryLog,
    gate: Arc<Semaphore>,
    entered: Arc<watch::Sender<u64>>,
    /// How long each snapshot write takes: a turn that snapshots stays open this long after its
    /// replies.
    snapshot_delay: Duration,
}

impl GatedLog {
    /// A gate that is closed: every append waits for [`GatedLog::release`].
    pub fn closed(inner: MemoryLog) -> Self {
        Self {
            inner,
            gate: Arc::new(Semaphore::new(0)),
            entered: Arc::new(watch::Sender::new(0)),
            snapshot_delay: Duration::ZERO,
        }
    }

    /// Each snapshot write takes `delay`.
    pub fn slow_snapshots(mut self, delay: Duration) -> Self {
        self.snapshot_delay = delay;
        self
    }

    /// Lets `n` appends through.
    pub fn release(&self, n: usize) {
        self.gate.add_permits(n);
    }

    /// Waits until `n` appends have started (and are waiting at the gate or done).
    pub async fn entered(&self, n: u64) {
        let mut rx = self.entered.subscribe();
        tokio::time::timeout(PATIENCE, rx.wait_for(|e| *e >= n))
            .await
            .expect("the host reached the append")
            .unwrap();
    }
}

impl LogStore for GatedLog {
    async fn last(&mut self) -> Result<Option<(i64, i64)>, LogError> {
        self.inner.last().await
    }

    async fn append(&mut self, records: &[LogRecord]) -> Result<(), LogError> {
        self.entered.send_modify(|e| *e += 1);
        self.gate
            .acquire()
            .await
            .expect("the gate stays open")
            .forget();
        self.inner.append(records).await
    }

    async fn records(&mut self) -> Result<Vec<LogRecord>, LogError> {
        self.inner.records().await
    }

    async fn find_keys(
        &mut self,
        keys: &[(PlayerId, IdempotencyKey)],
    ) -> Result<Vec<LogRecord>, LogError> {
        self.inner.find_keys(keys).await
    }

    async fn keyed_since(&mut self, since: i64) -> Result<Vec<LogRecord>, LogError> {
        self.inner.keyed_since(since).await
    }
    async fn records_after(&mut self, seq: i64) -> Result<Vec<LogRecord>, LogError> {
        self.inner.records_after(seq).await
    }

    async fn write_snapshot(&mut self, row: &SnapshotRow) -> Result<(), LogError> {
        tokio::time::sleep(self.snapshot_delay).await;
        self.inner.write_snapshot(row).await
    }

    async fn latest_snapshot(
        &mut self,
        build: Option<&str>,
    ) -> Result<Option<SnapshotRow>, LogError> {
        self.inner.latest_snapshot(build).await
    }

    async fn snapshot_index(
        &mut self,
    ) -> Result<Vec<voidforge_store::log::SnapshotMeta>, LogError> {
        self.inner.snapshot_index().await
    }

    async fn snapshot_at(&mut self, seq: i64) -> Result<Option<SnapshotRow>, LogError> {
        self.inner.snapshot_at(seq).await
    }
}

/// Waits until nothing in the published World is due at the clock's time: every due entry has
/// run and been published.
pub async fn settle(handle: &mut HostHandle, clock: &ManualClock) {
    use voidforge_host::Clock;
    tokio::time::timeout(PATIENCE, async {
        loop {
            let now = clock.now_unix_ms() - EPOCH;
            let rm = handle.read();
            if rm.world.queue().first().is_none_or(|e| e.at_ms > now) {
                return;
            }
            handle.changed().await.expect("the host is running");
        }
    })
    .await
    .expect("the host settled");
}

/// Drives an economy's command script through a host: at each scripted instant the clock moves
/// there, due events run first (and are logged as advances), then the command is generated from
/// the published World and submitted. Finally the clock moves to `end_ms`. Deterministic: the
/// same economy, rules and stores give the same records and receipts.
pub async fn drive(
    handle: &mut HostHandle,
    clock: &ManualClock,
    e: &Economy,
    end_ms: i64,
) -> Vec<Receipt> {
    let mut receipts = Vec::new();
    let mut script = e.script();
    script.sort();
    for (at, k) in script {
        clock.set(EPOCH + at);
        settle(handle, clock).await;
        let command = e.command(k, &handle.read().world);
        // Every command carries a key; every third is retried at once, and the retry must get
        // the first receipt back (D-037), changing nothing.
        let key = key_for(k);
        let receipt = handle
            .submit_with_key(command.clone(), key.clone())
            .await
            .expect("acknowledged");
        if k % 3 == 0 {
            let seq = handle.read().seq;
            let again = handle
                .submit_with_key(command, key)
                .await
                .expect("acknowledged");
            assert_eq!(again, receipt, "a retry gets the first receipt");
            assert_eq!(handle.read().seq, seq, "a retry appends nothing");
        }
        receipts.push(receipt);
    }
    clock.set(EPOCH + end_ms);
    settle(handle, clock).await;
    receipts
}

/// The key a script uses for its `k`th command.
pub fn key_for(k: u64) -> IdempotencyKey {
    IdempotencyKey::new(format!("script-{k}")).unwrap()
}

/// An economy with fleets and a generated command script, as the determinism suite uses.
pub fn economy(seed: u64) -> Economy {
    Economy {
        seed,
        planets: 3,
        fleets: true,
        commands: 60,
    }
}

/// The economy's starting World.
pub fn start(e: &Economy, rules: &Rules) -> World {
    e.world(rules)
}

/// A store that makes each append durable and then reports it failed: the crash table's "after
/// the append, before the reply" (the host stops; the record is in the log).
#[derive(Clone)]
pub struct AppendThenFail(pub MemoryLog);

impl LogStore for AppendThenFail {
    async fn last(&mut self) -> Result<Option<(i64, i64)>, LogError> {
        self.0.last().await
    }
    async fn append(&mut self, records: &[LogRecord]) -> Result<(), LogError> {
        self.0.append(records).await?;
        Err(LogError::Memory("crashed after the append".to_owned()))
    }
    async fn records(&mut self) -> Result<Vec<LogRecord>, LogError> {
        self.0.records().await
    }
    async fn find_keys(
        &mut self,
        keys: &[(PlayerId, IdempotencyKey)],
    ) -> Result<Vec<LogRecord>, LogError> {
        self.0.find_keys(keys).await
    }
    async fn keyed_since(&mut self, since: i64) -> Result<Vec<LogRecord>, LogError> {
        self.0.keyed_since(since).await
    }
    async fn records_after(&mut self, seq: i64) -> Result<Vec<LogRecord>, LogError> {
        self.0.records_after(seq).await
    }
    async fn write_snapshot(&mut self, row: &SnapshotRow) -> Result<(), LogError> {
        self.0.write_snapshot(row).await
    }
    async fn latest_snapshot(
        &mut self,
        build: Option<&str>,
    ) -> Result<Option<SnapshotRow>, LogError> {
        self.0.latest_snapshot(build).await
    }

    async fn snapshot_index(
        &mut self,
    ) -> Result<Vec<voidforge_store::log::SnapshotMeta>, LogError> {
        self.0.snapshot_index().await
    }

    async fn snapshot_at(&mut self, seq: i64) -> Result<Option<SnapshotRow>, LogError> {
        self.0.snapshot_at(seq).await
    }
}

/// A store that lets `n` appends through and then refuses every later one, without writing it:
/// a crash before the append, on any store (Postgres included), with no shutdown snapshot.
#[derive(Clone)]
pub struct FailAfter<T> {
    pub inner: T,
    pub left: Arc<std::sync::atomic::AtomicU64>,
}

impl<T: LogStore + Clone> FailAfter<T> {
    pub fn new(inner: T, n: u64) -> Self {
        Self {
            inner,
            left: Arc::new(std::sync::atomic::AtomicU64::new(n)),
        }
    }
}

impl<T: LogStore + Clone + Sync> LogStore for FailAfter<T> {
    async fn last(&mut self) -> Result<Option<(i64, i64)>, LogError> {
        self.inner.last().await
    }
    async fn append(&mut self, records: &[LogRecord]) -> Result<(), LogError> {
        use std::sync::atomic::Ordering;
        if self.left.load(Ordering::Relaxed) == 0 {
            return Err(LogError::Memory("crashed before the append".to_owned()));
        }
        self.left.fetch_sub(1, Ordering::Relaxed);
        self.inner.append(records).await
    }
    async fn records(&mut self) -> Result<Vec<LogRecord>, LogError> {
        self.inner.records().await
    }
    async fn find_keys(
        &mut self,
        keys: &[(PlayerId, IdempotencyKey)],
    ) -> Result<Vec<LogRecord>, LogError> {
        self.inner.find_keys(keys).await
    }
    async fn keyed_since(&mut self, since: i64) -> Result<Vec<LogRecord>, LogError> {
        self.inner.keyed_since(since).await
    }
    async fn records_after(&mut self, seq: i64) -> Result<Vec<LogRecord>, LogError> {
        self.inner.records_after(seq).await
    }
    async fn write_snapshot(&mut self, row: &SnapshotRow) -> Result<(), LogError> {
        self.inner.write_snapshot(row).await
    }
    async fn latest_snapshot(
        &mut self,
        build: Option<&str>,
    ) -> Result<Option<SnapshotRow>, LogError> {
        self.inner.latest_snapshot(build).await
    }

    async fn snapshot_index(
        &mut self,
    ) -> Result<Vec<voidforge_store::log::SnapshotMeta>, LogError> {
        self.inner.snapshot_index().await
    }

    async fn snapshot_at(&mut self, seq: i64) -> Result<Option<SnapshotRow>, LogError> {
        self.inner.snapshot_at(seq).await
    }
}

/// The upgraded build in the upgrade scenarios.
pub fn upgraded() -> EngineBuild {
    EngineBuild::new("0.2.0+upgrade").unwrap()
}

fn joins(n: usize, tag: &str) -> Vec<(voidforge_core::command::Command, IdempotencyKey)> {
    (0..n)
        .map(|i| {
            (
                voidforge_core::command::Command {
                    player: voidforge_testkit::ALICE,
                    action: voidforge_core::command::Action::JoinGame,
                },
                IdempotencyKey::new(format!("{tag}{i}")).unwrap(),
            )
        })
        .collect()
}

/// Runs a host of `build` on `store` (built on the host's thread by `make`) from a recovered
/// World, submits `commands` and stops, cleanly or by `crash_after` appends. Returns the
/// published World before the stop.
pub async fn run_build<S, M>(
    world: World,
    make: M,
    build: &EngineBuild,
    commands: Vec<(voidforge_core::command::Command, IdempotencyKey)>,
    crash_after: Option<u64>,
) -> World
where
    S: LogStore + Clone + Sync,
    M: FnOnce() -> S + Send + 'static,
{
    let mut cfg = config();
    cfg.engine_build = build.clone();
    cfg.snapshot_every_records = 1_000;
    let left = crash_after.unwrap_or(u64::MAX);
    let (handle, task) = voidforge_host::spawn(
        world,
        voidforge_testkit::v0_rules(),
        move || FailAfter::new(make(), left),
        clock_at(3_600_000),
        cfg,
    )
    .unwrap();
    for (c, k) in commands {
        if handle.submit_with_key(c, k).await.is_err() {
            break;
        }
    }
    let w = handle.read().world.as_ref().clone();
    drop(handle);
    let joined = task.join();
    assert_eq!(
        joined.is_err(),
        crash_after.is_some(),
        "a crash stops the host, a clean stop does not"
    );
    w
}

/// Carry (f), on one store (`inspect` reads it; `make` builds the host's): an upgrade start, a
/// crash after the upgrade, rollback before and after the new build wrote records, rollback
/// from a clean shutdown, and a newer format (D-038).
pub async fn upgrade_cases<S, M>(mut inspect: S, make: M)
where
    S: LogStore + Clone + Sync,
    M: Fn() -> S + Clone + Send + 'static,
{
    use voidforge_host::{RecoverError, recover};
    use voidforge_store::replay::ReplayError;
    let rules = voidforge_testkit::v0_rules();
    let mut b = voidforge_testkit::WorldBuilder::new();
    b.player(voidforge_testkit::ALICE);
    let world = b.build();
    let snap = voidforge_store::snapshot::encode(&world, &build()).unwrap();
    inspect
        .write_snapshot(&SnapshotRow::new(0, &snap))
        .await
        .unwrap();
    // Build A runs two records and stops cleanly: its snapshot is at the head (seq 2).
    let a = recover(&mut inspect, &rules, &build()).await.unwrap();
    let a_world = run_build(a.world, make.clone(), &build(), joins(2, "a"), None).await;
    // 1. Upgrade start: B begins from A's head snapshot with an empty tail.
    let up = recover(&mut inspect, &rules, &upgraded()).await.unwrap();
    assert_eq!((up.snapshot_seq, up.last_seq), (2, 2));
    assert_eq!(up.world, a_world);
    // Rollback before B writes anything: A recovers from its own snapshot.
    assert_eq!(
        recover(&mut inspect, &rules, &build())
            .await
            .unwrap()
            .last_seq,
        2
    );
    // 2. B appends two records and crashes before any snapshot of its own.
    let b_world = run_build(up.world, make.clone(), &upgraded(), joins(3, "b"), Some(2)).await;
    assert!(
        inspect
            .latest_snapshot(Some(upgraded().as_str()))
            .await
            .unwrap()
            .is_none()
    );
    let again = recover(&mut inspect, &rules, &upgraded()).await.unwrap();
    assert_eq!(
        (again.snapshot_seq, again.last_seq),
        (2, 4),
        "A's snapshot, B's tail"
    );
    assert_eq!(again.world, b_world);
    // The replay tool's verification agrees with recovery (phase 2 gate, R1 LOW): from A's
    // snapshot, B's records verify under B's pin, and A's pin refuses B's first record.
    let verified =
        voidforge_store::verify::verify_log(&mut inspect, &rules, &upgraded(), Some(2), None)
            .await
            .unwrap();
    assert_eq!((verified.start_seq, verified.last_seq), (2, 4));
    assert_eq!(verified.world, b_world);
    let refused =
        voidforge_store::verify::verify_log(&mut inspect, &rules, &build(), Some(2), None)
            .await
            .unwrap_err();
    assert!(
        matches!(&refused, voidforge_store::verify::VerifyError::Refused(f) if (f.seq, f.what) == (Some(3), "build")),
        "{refused:?}"
    );
    // Rollback after B wrote records without a snapshot: A would have to replay B's records,
    // refused.
    assert!(matches!(
        recover(&mut inspect, &rules, &build()).await,
        Err(RecoverError::Replay(ReplayError::CrossBuildRecord {
            seq: 3,
            ..
        }))
    ));
    // B, recovered, runs two more records and stops cleanly: its snapshot is the newest, at the
    // head (seq 6). 5. Rollback from that clean shutdown: A starts from B's snapshot, empty tail.
    let b_clean = run_build(again.world, make.clone(), &upgraded(), joins(2, "c"), None).await;
    let back = recover(&mut inspect, &rules, &build()).await.unwrap();
    assert_eq!((back.snapshot_seq, back.last_seq), (6, 6));
    assert_eq!(back.world, b_clean);
    // 6. A newer format at the head (a build this one cannot read): a typed error. B appends one
    // more record and crashes, and the newer build's snapshot is planted at that head (seq 7):
    // a snapshot past the head would be refused first (review M1).
    let world6 = recover(&mut inspect, &rules, &upgraded())
        .await
        .unwrap()
        .world;
    run_build(world6, make.clone(), &upgraded(), joins(2, "d"), Some(1)).await;
    let mut newer = inspect.latest_snapshot(None).await.unwrap().unwrap();
    let decoded = voidforge_store::snapshot::decode_any(&newer.payload).unwrap();
    let mut header = decoded.header;
    header.format = 2;
    let payload_start = newer.payload.len()
        - voidforge_store::canonical::to_bytes(&decoded.world)
            .unwrap()
            .len();
    let mut bytes = voidforge_store::snapshot::MAGIC.to_vec();
    bytes.extend(voidforge_store::canonical::to_bytes(&header).unwrap());
    bytes.extend_from_slice(&newer.payload[payload_start..]);
    newer.payload = bytes;
    newer.seq = inspect.last().await.unwrap().unwrap().0;
    assert_eq!(newer.seq, 7);
    inspect.write_snapshot(&newer).await.unwrap();
    assert!(matches!(
        recover(&mut inspect, &rules, &upgraded()).await,
        Err(RecoverError::Snapshot {
            error: voidforge_store::snapshot::SnapshotError::UnknownFormat(2),
            ..
        })
    ));
}

/// Carry (f), the refused case: A's records after A's newest snapshot (A crashed), then B
/// starts: B would have to replay A's records, refused; A recovers.
pub async fn upgrade_over_another_builds_tail_is_refused<S, M>(mut inspect: S, make: M)
where
    S: LogStore + Clone + Sync,
    M: Fn() -> S + Clone + Send + 'static,
{
    use voidforge_host::{RecoverError, recover};
    use voidforge_store::replay::ReplayError;
    let rules = voidforge_testkit::v0_rules();
    let mut b = voidforge_testkit::WorldBuilder::new();
    b.player(voidforge_testkit::ALICE);
    let world = b.build();
    let snap = voidforge_store::snapshot::encode(&world, &build()).unwrap();
    inspect
        .write_snapshot(&SnapshotRow::new(0, &snap))
        .await
        .unwrap();
    let a = recover(&mut inspect, &rules, &build()).await.unwrap();
    run_build(a.world, make.clone(), &build(), joins(3, "a"), Some(2)).await;
    assert!(matches!(
        recover(&mut inspect, &rules, &upgraded()).await,
        Err(RecoverError::Replay(ReplayError::CrossBuildRecord {
            seq: 1,
            ..
        }))
    ));
    assert_eq!(
        recover(&mut inspect, &rules, &build())
            .await
            .unwrap()
            .last_seq,
        2
    );
}

/// Review M1, on one store: a snapshot past the log head (it claims records the log does not
/// hold) is refused with both numbers, never recovered from (D-038).
pub async fn snapshot_past_head_is_refused<S, M>(mut inspect: S, make: M)
where
    S: LogStore + Clone + Sync,
    M: Fn() -> S + Clone + Send + 'static,
{
    use voidforge_host::{RecoverError, recover};
    let rules = voidforge_testkit::v0_rules();
    let mut b = voidforge_testkit::WorldBuilder::new();
    b.player(voidforge_testkit::ALICE);
    let world = b.build();
    let snap = voidforge_store::snapshot::encode(&world, &build()).unwrap();
    inspect
        .write_snapshot(&SnapshotRow::new(0, &snap))
        .await
        .unwrap();
    let a = recover(&mut inspect, &rules, &build()).await.unwrap();
    run_build(a.world, make, &build(), joins(3, "p"), Some(2)).await;
    // Records 1 and 2, no snapshot after the bootstrap; then a planted snapshot claiming seq 7.
    let mut planted = inspect.latest_snapshot(None).await.unwrap().unwrap();
    planted.seq = 7;
    inspect.write_snapshot(&planted).await.unwrap();
    assert!(matches!(
        recover(&mut inspect, &rules, &build()).await,
        Err(RecoverError::SnapshotPastHead {
            snapshot_seq: 7,
            head: 2
        })
    ));
}
