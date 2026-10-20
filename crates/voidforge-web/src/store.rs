//! The session store (P4.1 repair round 1, R1-1; D-057): in memory, like tower-sessions'
//! `MemoryStore`, but bounded. `MemoryStore` never removes an expired record, so every
//! anonymous `GET /login` would stay for the process's life.
//!
//! - **Expired records go:** a load that finds one removes it, and whenever the map has doubled
//!   since the last sweep (and holds at least [`SWEEP_FLOOR`]), a sweep removes them all. So the
//!   map holds at most about twice the live sessions, and an anonymous session lives 10 minutes
//!   after its last change.
//! - **A hard cap** of [`MAX_SESSIONS`] live sessions: a new session past it is refused, and
//!   `/login` answers 503 rather than grow. Logged-in sessions already stored are never evicted
//!   for room.
//!
//! One `std` mutex, held for one map operation and never across an await.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use tower_sessions::cookie::time::OffsetDateTime;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, SessionStore};

/// The most live sessions kept: about 1 KB each, so about 50 MB.
pub const MAX_SESSIONS: usize = 50_000;
/// The smallest map a doubling sweep waits for.
pub const SWEEP_FLOOR: usize = 1_024;

#[derive(Debug, Default)]
struct Inner {
    records: HashMap<Id, Record>,
    /// The map's size after the last sweep.
    swept: usize,
}

/// A bounded in-memory session store. Clones share one map.
#[derive(Debug, Clone)]
pub struct BoundedStore {
    inner: Arc<Mutex<Inner>>,
    max: usize,
}

impl Default for BoundedStore {
    fn default() -> Self {
        Self::new(MAX_SESSIONS)
    }
}

fn active(r: &Record, now: OffsetDateTime) -> bool {
    r.expiry_date > now
}

impl BoundedStore {
    /// A store holding at most `max` live sessions.
    #[must_use]
    pub fn new(max: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            max,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// How many records the map holds, expired ones included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().records.len()
    }

    /// Whether the map is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether a new session would fit, after removing the expired ones if it's full.
    #[must_use]
    pub fn has_room(&self) -> bool {
        let mut inner = self.lock();
        room(&mut inner, self.max, OffsetDateTime::now_utc())
    }
}

fn sweep(inner: &mut Inner, now: OffsetDateTime) {
    inner.records.retain(|_, r| active(r, now));
    inner.swept = inner.records.len();
}

fn sweep_if_doubled(inner: &mut Inner, now: OffsetDateTime) {
    if inner.records.len() >= 2 * inner.swept.max(SWEEP_FLOOR) {
        sweep(inner, now);
    }
}

/// Room for one more record: sweeps first when full.
fn room(inner: &mut Inner, max: usize, now: OffsetDateTime) -> bool {
    if inner.records.len() >= max {
        sweep(inner, now);
    }
    inner.records.len() < max
}

fn full() -> session_store::Error {
    session_store::Error::Backend("the session store is full".to_owned())
}

#[async_trait]
impl SessionStore for BoundedStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        let now = OffsetDateTime::now_utc();
        let mut inner = self.lock();
        sweep_if_doubled(&mut inner, now);
        if !room(&mut inner, self.max, now) {
            return Err(full());
        }
        while inner.records.contains_key(&record.id) {
            record.id = Id::default();
        }
        inner.records.insert(record.id, record.clone());
        Ok(())
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let now = OffsetDateTime::now_utc();
        let mut inner = self.lock();
        if !inner.records.contains_key(&record.id) {
            sweep_if_doubled(&mut inner, now);
            if !room(&mut inner, self.max, now) {
                return Err(full());
            }
        }
        inner.records.insert(record.id, record.clone());
        Ok(())
    }

    async fn load(&self, id: &Id) -> session_store::Result<Option<Record>> {
        let now = OffsetDateTime::now_utc();
        let mut inner = self.lock();
        match inner.records.get(id) {
            Some(r) if active(r, now) => Ok(Some(r.clone())),
            Some(_) => {
                inner.records.remove(id);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    async fn delete(&self, id: &Id) -> session_store::Result<()> {
        self.lock().records.remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tower_sessions::cookie::time::Duration;

    use super::*;

    fn record(expires_in: Duration) -> Record {
        Record {
            id: Id::default(),
            data: HashMap::new(),
            expiry_date: OffsetDateTime::now_utc() + expires_in,
        }
    }

    #[tokio::test]
    async fn an_expired_record_is_removed_when_loaded() {
        let s = BoundedStore::default();
        let mut r = record(Duration::seconds(-1));
        s.save(&r).await.expect("save");
        assert_eq!(s.len(), 1);
        assert!(s.load(&r.id).await.expect("load").is_none());
        assert_eq!(s.len(), 0);
        r = record(Duration::minutes(10));
        s.save(&r).await.expect("save");
        assert!(s.load(&r.id).await.expect("load").is_some());
    }

    #[tokio::test]
    async fn expired_records_are_swept_once_the_map_doubles() {
        let s = BoundedStore::default();
        // Expired sessions nobody loads again: the anonymous /logins R1-1 found.
        for _ in 0..(2 * SWEEP_FLOOR - 1) {
            s.save(&record(Duration::seconds(-1))).await.expect("save");
        }
        assert_eq!(s.len(), 2 * SWEEP_FLOOR - 1);
        let mut live = record(Duration::minutes(10));
        s.create(&mut live).await.expect("create");
        let mut another = record(Duration::minutes(10));
        s.create(&mut another).await.expect("create");
        assert_eq!(s.len(), 2, "the sweep left only the live sessions");
        assert!(s.load(&live.id).await.expect("load").is_some());
    }

    #[tokio::test]
    async fn the_cap_refuses_a_new_session_and_keeps_the_stored_ones() {
        let s = BoundedStore::new(3);
        let mut kept = Vec::new();
        for _ in 0..3 {
            let mut r = record(Duration::minutes(10));
            s.create(&mut r).await.expect("create");
            kept.push(r);
        }
        assert!(!s.has_room());
        let mut extra = record(Duration::minutes(10));
        assert!(s.create(&mut extra).await.is_err());
        assert!(s.save(&record(Duration::minutes(10))).await.is_err());
        // A stored session still saves, and nothing was evicted for room.
        s.save(&kept[0]).await.expect("an existing session saves");
        for r in &kept {
            assert!(s.load(&r.id).await.expect("load").is_some());
        }
        assert_eq!(s.len(), 3);
    }

    #[tokio::test]
    async fn expired_sessions_make_room_at_the_cap() {
        let s = BoundedStore::new(3);
        for _ in 0..3 {
            s.save(&record(Duration::seconds(-1))).await.expect("save");
        }
        assert!(s.has_room());
        let mut r = record(Duration::minutes(10));
        s.create(&mut r).await.expect("room after the sweep");
        assert_eq!(s.len(), 1);
    }
}
