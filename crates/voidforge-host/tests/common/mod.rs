//! Shared by the host's test binaries: a gated store, waiting for the host to settle, and a
//! scripted economy driven through the host.

#![allow(dead_code)] // each binary uses part of this

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Semaphore, watch};
use voidforge_core::rules::Rules;
use voidforge_core::state::World;
use voidforge_host::{HostConfig, HostHandle, ManualClock, Receipt};
use voidforge_store::log::{LogError, LogRecord, LogStore, MemoryLog};
use voidforge_store::snapshot::EngineBuild;
use voidforge_testkit::determinism::Economy;

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
}

impl GatedLog {
    /// A gate that is closed: every append waits for [`GatedLog::release`].
    pub fn closed(inner: MemoryLog) -> Self {
        Self {
            inner,
            gate: Arc::new(Semaphore::new(0)),
            entered: Arc::new(watch::Sender::new(0)),
        }
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
        receipts.push(handle.submit(command).await.expect("acknowledged"));
    }
    clock.set(EPOCH + end_ms);
    settle(handle, clock).await;
    receipts
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
