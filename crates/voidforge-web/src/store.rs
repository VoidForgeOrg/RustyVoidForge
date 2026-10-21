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
//! - **Refusals are logged without a flood** (round 2, QA's D2; D-054's pattern): the first
//!   refusal after a quiet spell is a WARN, later ones are debug with a running count, a WARN
//!   repeats at most once a minute while refusals go on, and the first session admitted after
//!   them is an INFO with the total.
//!
//! One `std` mutex, held for one map operation and never across an await.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tower_sessions::cookie::time::OffsetDateTime;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, SessionStore};

/// The most live sessions kept: about 1 KB each, so about 50 MB.
pub const MAX_SESSIONS: usize = 50_000;
/// The smallest map a doubling sweep waits for.
pub const SWEEP_FLOOR: usize = 1_024;
/// The shortest time between two WARNs about refused sessions.
pub const WARN_EVERY: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
struct Inner {
    records: HashMap<Id, Record>,
    /// The map's size after the last sweep.
    swept: usize,
}

/// Refusals since the store last had room, for the log.
#[derive(Debug, Default)]
struct Pressure {
    /// Refused since the last admitted session.
    refused: u64,
    /// Refused since the last WARN.
    since_warn: u64,
    last_warn: Option<Instant>,
}

/// A bounded in-memory session store. Clones share one map.
#[derive(Debug, Clone)]
pub struct BoundedStore {
    inner: Arc<Mutex<Inner>>,
    pressure: Arc<Mutex<Pressure>>,
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
            pressure: Arc::new(Mutex::new(Pressure::default())),
            max,
        }
    }

    /// Logs a refused new session: a WARN after a quiet spell or a minute, debug otherwise.
    pub fn refused(&self) {
        let mut p = self.pressure.lock().unwrap_or_else(|e| e.into_inner());
        p.refused = p.refused.saturating_add(1);
        p.since_warn = p.since_warn.saturating_add(1);
        if p.last_warn.is_none_or(|at| at.elapsed() >= WARN_EVERY) {
            tracing::warn!(
                refused = p.since_warn,
                max = self.max,
                "the session store is full: refusing new logins"
            );
            p.since_warn = 0;
            p.last_warn = Some(Instant::now());
        } else {
            tracing::debug!(
                refused = p.refused,
                "a new login refused: the session store is full"
            );
        }
    }

    /// Notes an admitted new session: after refusals, one INFO with their total.
    pub fn admitted(&self) {
        let mut p = self.pressure.lock().unwrap_or_else(|e| e.into_inner());
        if p.refused > 0 {
            tracing::info!(refused = p.refused, "the session store has room again");
            *p = Pressure::default();
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

    /// Updates a stored session, and never recreates one (P4.4, review-r2's HIGH): tower-sessions
    /// creates every new id through [`SessionStore::create`] (a new session, after `flush`, after
    /// `cycle_id`), so a save for an id that's gone means the record was deleted (a logout, a
    /// login's new id) or expired after this request loaded it. Writing it back would log the old
    /// cookie in again; a logout is final. The request's own response is unaffected.
    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let mut inner = self.lock();
        match inner.records.get_mut(&record.id) {
            Some(stored) => *stored = record.clone(),
            None => tracing::debug!("a save for a session that's gone was dropped"),
        }
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

    /// Stores a record directly, as a session the store already holds.
    fn put(s: &BoundedStore, r: &Record) {
        s.lock().records.insert(r.id, r.clone());
    }

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
        put(&s, &r);
        assert_eq!(s.len(), 1);
        assert!(s.load(&r.id).await.expect("load").is_none());
        assert_eq!(s.len(), 0);
        r = record(Duration::minutes(10));
        put(&s, &r);
        assert!(s.load(&r.id).await.expect("load").is_some());
    }

    #[tokio::test]
    async fn expired_records_are_swept_once_the_map_doubles() {
        let s = BoundedStore::default();
        // Expired sessions nobody loads again: the anonymous /logins R1-1 found.
        for _ in 0..(2 * SWEEP_FLOOR - 1) {
            put(&s, &record(Duration::seconds(-1)));
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
        // A save never adds a session, so it can't pass the cap either.
        s.save(&record(Duration::minutes(10))).await.expect("save");
        assert_eq!(s.len(), 3);
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
            put(&s, &record(Duration::seconds(-1)));
        }
        assert!(s.has_room());
        let mut r = record(Duration::minutes(10));
        s.create(&mut r).await.expect("room after the sweep");
        assert_eq!(s.len(), 1);
    }

    /// Review-r2's HIGH on 05781b6: a request that loaded a session before its logout must not
    /// write it back after. A save only updates; a deleted id stays deleted.
    #[tokio::test]
    async fn a_save_never_recreates_a_deleted_session() {
        let s = BoundedStore::default();
        let mut r = record(Duration::minutes(10));
        s.create(&mut r).await.expect("create");
        let mut changed = r.clone();
        changed.data.insert("k".to_owned(), serde_json::json!(1));
        s.save(&changed).await.expect("save");
        assert_eq!(s.load(&r.id).await.expect("load"), Some(changed.clone()));
        s.delete(&r.id).await.expect("delete");
        s.save(&changed).await.expect("a late save");
        assert!(s.load(&r.id).await.expect("load").is_none());
        assert_eq!(s.len(), 0);
    }
}
