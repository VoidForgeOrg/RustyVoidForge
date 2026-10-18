//! The host's clock: the wall clock is an input (Architectural Design, Determinism rules), so
//! it is injected, and tests drive a manual one.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;

/// Wall time in Unix milliseconds, and a way to wait for it.
pub trait Clock: Send + Sync + 'static {
    /// Now, in Unix milliseconds.
    fn now_unix_ms(&self) -> i64;
    /// Resolves once [`Clock::now_unix_ms`] is at or after `at`. It never resolves early and never
    /// spins.
    fn sleep_until_unix_ms(&self, at: i64) -> impl Future<Output = ()> + Send;
}

/// The system clock. A sleep re-checks the clock when it wakes, because wall time can jump.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

/// The longest single sleep, so a far deadline still re-checks a clock that jumped forward.
const MAX_SLEEP: Duration = Duration::from_secs(60);

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> i64 {
        // Before 1970 or beyond i64 milliseconds is not a clock this host can run on.
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the system clock is after 1970");
        i64::try_from(since.as_millis()).expect("the system clock fits i64 milliseconds")
    }

    async fn sleep_until_unix_ms(&self, at: i64) {
        loop {
            let now = self.now_unix_ms();
            if now >= at {
                return;
            }
            let wait = u64::try_from(at.saturating_sub(now)).unwrap_or(u64::MAX);
            tokio::time::sleep(Duration::from_millis(wait).min(MAX_SLEEP)).await;
        }
    }
}

/// A clock that moves only when a test sets it. Clones share one time.
#[derive(Debug, Clone)]
pub struct ManualClock(Arc<watch::Sender<i64>>);

impl ManualClock {
    /// A clock at `unix_ms`.
    #[must_use]
    pub fn new(unix_ms: i64) -> Self {
        Self(Arc::new(watch::Sender::new(unix_ms)))
    }

    /// Sets the time, forwards or backwards, and wakes sleepers that are now due.
    pub fn set(&self, unix_ms: i64) {
        self.0.send_replace(unix_ms);
    }
}

impl Clock for ManualClock {
    fn now_unix_ms(&self) -> i64 {
        *self.0.borrow()
    }

    async fn sleep_until_unix_ms(&self, at: i64) {
        let mut rx = self.0.subscribe();
        // The sender lives as long as this clock, so the wait ends only when the time is due.
        let _ = rx.wait_for(|now| *now >= at).await;
    }
}
