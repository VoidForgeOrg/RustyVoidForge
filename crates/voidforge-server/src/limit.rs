//! Per-principal rate limits (Technical Plan P3.8; decision log D-054).
//!
//! Each principal has two budgets: reads (every method but `POST`) and writes (`POST /commands`),
//! so a reload loop can't spend the budget a player needs to play. Each budget is GCRA, the
//! virtual-scheduling form of a token bucket: per key one theoretical arrival time (TAT). With
//! the emission interval T = 1 s / rate and the burst B, the allowance is τ = (B − 1)·T. A request
//! at `now` conforms when `max(TAT, now) − now ≤ τ`, and then the TAT moves to
//! `max(TAT, now) + T`. **A refusal changes nothing**, so a client that waits its Retry-After
//! succeeds, and a loop that ignores 429 is held to the steady rate rather than shut out.
//!
//! The check runs inside the [`Player`](crate::accounts::Player) extractor, after the token is
//! verified and before the account lookup, so a 429 never joins, never reaches the host and never
//! consumes an idempotency key. 401s and the 503 while dex's keys load have no principal and are
//! never counted; `/openapi.json` takes no `Player` and is never limited.
//!
//! **The key holds no subject.** It's a SipHash of (issuer, subject) under a key drawn once per
//! process, so the raw subject (personal data) is never stored here, logged or printed. Two
//! principals could share a budget only through a 64-bit collision.
//!
//! **Memory is bounded.** An entry whose TAT is at or before now answers exactly as no entry
//! (`max(TAT, now) = now`), so removing it never changes an answer. A sweep removes those entries
//! whenever the map has doubled since the last sweep, so the map holds at most twice the larger
//! of [`SWEEP_FLOOR`] and the principals that sent a request within one budget's B·T before the
//! last sweep.
//!
//! Per-principal limits don't bound the aggregate: the host's inbox, answering 503 `busy` when
//! full, remains its guard against many players at once (the advisor's ruling).

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::http::Method;

use crate::auth::Principal;

/// The fewest entries the map holds before it sweeps at all.
pub const SWEEP_FLOOR: usize = 1_024;

/// How often a refusal logs at WARN at most; the ones between log at debug, and the next WARN
/// counts them (a limited client mustn't flood the log).
pub const WARN_EVERY: Duration = Duration::from_secs(60);

/// A monotonic clock: [`Instant::now`] in `serve`, a stepped one in tests.
pub type Now = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Which budget a request spends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    /// Every method but `POST`.
    Read,
    /// `POST`: commands.
    Write,
}

impl Class {
    /// The class of a request, from its method alone: the path never decides.
    #[must_use]
    pub fn of(method: &Method) -> Self {
        if method == Method::POST {
            Self::Write
        } else {
            Self::Read
        }
    }
}

/// One budget: a steady rate and a burst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Requests a second, steady (at least 1).
    pub per_sec: u32,
    /// Requests back to back (at least 1).
    pub burst: u32,
}

/// Both budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Reads.
    pub read: Budget,
    /// Writes.
    pub write: Budget,
}

impl Limits {
    /// The defaults (the advisor's A1, measured against the P3.6 bots): reads 10 a second with
    /// bursts of 40, writes 2 a second with bursts of 10.
    pub const DEFAULT: Self = Self {
        read: Budget {
            per_sec: 10,
            burst: 40,
        },
        write: Budget {
            per_sec: 2,
            burst: 10,
        },
    };

    fn budget(&self, class: Class) -> Budget {
        match class {
            Class::Read => self.read,
            Class::Write => self.write,
        }
    }
}

/// A refusal: whole seconds after which one request of this class conforms, if the principal
/// sends nothing else of it meanwhile. At least 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryAfter(pub u64);

/// A key: a hash of the principal, never the subject itself, and the class.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key(u64, Class);

/// Prints neither the hash nor anything it came from.
impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Key(<principal>, {:?})", self.1)
    }
}

struct State {
    tats: HashMap<Key, Instant>,
    /// The map's size after the last sweep.
    swept: usize,
    /// When a refusal last logged at WARN, and the refusals since.
    warned: Option<Instant>,
    quiet: u64,
}

/// The limiter: both budgets for every principal, under one lock held for one map operation and
/// never across an await.
pub struct Limiter {
    limits: Limits,
    now: Now,
    hasher: RandomState,
    state: Mutex<State>,
}

impl std::fmt::Debug for Limiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Limiter")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Limiter {
    /// A limiter with `limits` on the clock `now`.
    ///
    /// # Panics
    /// When a rate or a burst is 0: `serve`'s flags refuse those first.
    #[must_use]
    pub fn new(limits: Limits, now: Now) -> Self {
        for b in [limits.read, limits.write] {
            assert!(b.per_sec >= 1 && b.burst >= 1, "a budget of {b:?}");
        }
        Self {
            limits,
            now,
            hasher: RandomState::new(),
            state: Mutex::new(State {
                tats: HashMap::new(),
                swept: 0,
                warned: None,
                quiet: 0,
            }),
        }
    }

    /// The limits in force.
    #[must_use]
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Spends one request of `class` from `principal`'s budget, or refuses it.
    ///
    /// # Errors
    /// [`RetryAfter`] when the budget is spent; nothing changes.
    pub fn check(&self, principal: &Principal, class: Class) -> Result<(), RetryAfter> {
        let key = Key(
            self.hasher
                .hash_one((principal.issuer.as_str(), principal.subject.as_str())),
            class,
        );
        let budget = self.limits.budget(class);
        let interval = Duration::from_secs(1) / budget.per_sec;
        let allowance = interval * (budget.burst - 1);
        let now = (self.now)();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let tat = state.tats.get(&key).map_or(now, |&t| t.max(now));
        let ahead = tat.duration_since(now);
        let next = tat.checked_add(interval);
        match next {
            Some(next) if ahead <= allowance => {
                state.tats.insert(key, next);
                if state.tats.len() > 2 * state.swept.max(SWEEP_FLOOR) {
                    state.tats.retain(|_, t| *t > now);
                    state.swept = state.tats.len();
                }
                Ok(())
            }
            // `next` is None only an `Instant` overflow away: refuse rather than panic.
            _ => {
                let wait = ahead.saturating_sub(allowance);
                let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
                state.quiet += 1;
                if state
                    .warned
                    .is_none_or(|w| now.duration_since(w) >= WARN_EVERY)
                {
                    tracing::warn!(
                        class = ?class,
                        refused = state.quiet,
                        "rate limited: requests over a principal's budget were refused"
                    );
                    state.warned = Some(now);
                    state.quiet = 0;
                } else {
                    tracing::debug!(class = ?class, "rate limited");
                }
                Err(RetryAfter(secs.max(1)))
            }
        }
    }

    /// How many entries the map holds: for the memory-bound tests.
    #[must_use]
    pub fn entries(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .tats
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clock the test steps.
    #[derive(Clone)]
    struct Stepped(Arc<Mutex<Instant>>);

    impl Stepped {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(Instant::now())))
        }

        fn now(&self) -> Now {
            let c = self.0.clone();
            Arc::new(move || *c.lock().unwrap())
        }

        fn step(&self, by: Duration) {
            *self.0.lock().unwrap() += by;
        }
    }

    fn principal(subject: &str) -> Principal {
        Principal {
            issuer: "http://localhost:5556/dex".to_owned(),
            subject: subject.to_owned(),
        }
    }

    fn limits(read: (u32, u32), write: (u32, u32)) -> Limits {
        Limits {
            read: Budget {
                per_sec: read.0,
                burst: read.1,
            },
            write: Budget {
                per_sec: write.0,
                burst: write.1,
            },
        }
    }

    const MS: Duration = Duration::from_millis(1);
    const NS: Duration = Duration::from_nanos(1);

    #[test]
    fn the_class_is_the_methods() {
        assert_eq!(Class::of(&Method::POST), Class::Write);
        for m in [
            Method::GET,
            Method::HEAD,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
        ] {
            assert_eq!(Class::of(&m), Class::Read, "{m}");
        }
    }

    #[test]
    fn the_defaults_are_the_advisors() {
        // The ruled numbers themselves, not the constant read back (LEARNED, P3.4).
        assert_eq!(Limits::DEFAULT, limits((10, 40), (2, 10)));
    }

    /// A burst of exactly B, then refusals that say 1 s; one request every T after that; and the
    /// first request one T after a refusal succeeds (Retry-After is true).
    #[test]
    fn a_burst_then_the_steady_rate() {
        for (rate, burst) in [(10, 40), (2, 10), (1, 1), (3, 5), (1_000, 10_000)] {
            let clock = Stepped::new();
            let l = Limiter::new(limits((rate, burst), (1, 1)), clock.now());
            let p = principal("alice");
            for i in 0..burst {
                assert_eq!(l.check(&p, Class::Read), Ok(()), "{rate}/s B{burst}: #{i}");
            }
            assert_eq!(
                l.check(&p, Class::Read),
                Err(RetryAfter(1)),
                "{rate}/s B{burst}"
            );
            let t = Duration::from_secs(1) / rate;
            for i in 0..5 {
                clock.step(t - NS);
                assert_eq!(
                    l.check(&p, Class::Read),
                    Err(RetryAfter(1)),
                    "{rate}/s B{burst}: a nanosecond early, #{i}"
                );
                clock.step(NS);
                assert_eq!(l.check(&p, Class::Read), Ok(()), "{rate}/s B{burst}: #{i}");
                assert_eq!(l.check(&p, Class::Read), Err(RetryAfter(1)));
            }
            // Back after an idle spell, with the old entry still held (nothing swept it): the
            // burst is exactly B again, never more for the time spent away.
            clock.step(Duration::from_secs(100_000));
            for i in 0..burst {
                assert_eq!(
                    l.check(&p, Class::Read),
                    Ok(()),
                    "{rate}/s B{burst}: idle #{i}"
                );
            }
            assert_eq!(
                l.check(&p, Class::Read),
                Err(RetryAfter(1)),
                "{rate}/s B{burst}: idle"
            );
        }
    }

    /// Waiting the Retry-After it was given, and then sending one request, succeeds, whatever
    /// fraction of a second the wait really needed.
    #[test]
    fn waiting_retry_after_succeeds() {
        for (rate, burst, offset_ms) in [(1, 1, 0), (1, 1, 999), (2, 10, 250), (10, 40, 37)] {
            let clock = Stepped::new();
            let l = Limiter::new(limits((rate, burst), (rate, burst)), clock.now());
            let p = principal("alice");
            for _ in 0..burst {
                l.check(&p, Class::Write).unwrap();
            }
            clock.step(MS * offset_ms);
            let mut refused = 0;
            while let Err(RetryAfter(secs)) = l.check(&p, Class::Write) {
                refused += 1;
                assert!(secs >= 1, "Retry-After {secs}");
                clock.step(Duration::from_secs(secs));
            }
            assert!(
                refused <= 1,
                "{rate}/s B{burst} +{offset_ms} ms: {refused} refusals"
            );
        }
    }

    /// A refusal changes nothing: a hundred refused requests don't push the next success back.
    #[test]
    fn a_refusal_spends_nothing() {
        let clock = Stepped::new();
        let l = Limiter::new(limits((2, 3), (1, 1)), clock.now());
        let p = principal("alice");
        for _ in 0..3 {
            l.check(&p, Class::Read).unwrap();
        }
        for _ in 0..100 {
            assert_eq!(l.check(&p, Class::Read), Err(RetryAfter(1)));
            clock.step(MS);
        }
        // 100 ms of refusals; the next slot is still 500 ms after the burst: refused at 499 ms,
        // accepted at 500.
        clock.step(MS * 399);
        assert_eq!(l.check(&p, Class::Read), Err(RetryAfter(1)));
        clock.step(MS);
        assert_eq!(l.check(&p, Class::Read), Ok(()));
    }

    /// One principal's budget is its own, and so is each class's.
    #[test]
    fn principals_and_classes_are_independent() {
        let clock = Stepped::new();
        let l = Limiter::new(limits((1, 2), (1, 3)), clock.now());
        let (alice, bob) = (principal("alice"), principal("bob"));
        let other_issuer = Principal {
            issuer: "http://elsewhere/dex".to_owned(),
            subject: "alice".to_owned(),
        };
        for _ in 0..2 {
            l.check(&alice, Class::Read).unwrap();
        }
        assert!(l.check(&alice, Class::Read).is_err());
        for _ in 0..3 {
            assert_eq!(l.check(&alice, Class::Write), Ok(()));
        }
        assert!(l.check(&alice, Class::Write).is_err());
        for p in [&bob, &other_issuer] {
            for _ in 0..2 {
                assert_eq!(l.check(p, Class::Read), Ok(()));
            }
            for _ in 0..3 {
                assert_eq!(l.check(p, Class::Write), Ok(()));
            }
        }
    }

    /// Entries past their window go at the next sweep, and the map stays bounded under any number
    /// of principals; a live entry survives every sweep.
    #[test]
    fn memory_is_bounded_and_a_sweep_keeps_live_entries() {
        let clock = Stepped::new();
        let l = Limiter::new(limits((10, 40), (2, 10)), clock.now());
        let alice = principal("alice");
        for _ in 0..40 {
            l.check(&alice, Class::Read).unwrap();
        }
        // Enough others, at the same instant, to force sweeps while Alice is live.
        for i in 0..(4 * SWEEP_FLOOR) {
            l.check(&principal(&format!("p{i}")), Class::Read).unwrap();
        }
        assert!(
            l.entries() > 2 * SWEEP_FLOOR,
            "every one is live: nothing swept"
        );
        assert_eq!(
            l.check(&alice, Class::Read),
            Err(RetryAfter(1)),
            "a sweep kept Alice's spent budget"
        );
        // One request past everyone's window: the next growth sweeps them all out.
        clock.step(Duration::from_secs(5));
        for round in 0..20 {
            for i in 0..(3 * SWEEP_FLOOR) {
                l.check(&principal(&format!("r{round}-{i}")), Class::Read)
                    .unwrap();
            }
            clock.step(Duration::from_secs(5));
            assert!(
                l.entries() <= 2 * 3 * SWEEP_FLOOR + 1,
                "round {round}: {} entries",
                l.entries()
            );
        }
        // And Alice, long idle, has her whole burst back.
        for _ in 0..40 {
            assert_eq!(l.check(&alice, Class::Read), Ok(()));
        }
    }

    /// The subject never appears in a Debug or a log line (the lead's T1 note).
    #[test]
    fn the_subject_is_never_printed() {
        let subject = "CgVhbGljZRIFbG9jYWw-secret-subject";
        let clock = Stepped::new();
        let l = Limiter::new(limits((1, 1), (1, 1)), clock.now());
        let p = principal(subject);
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = {
            let buf = buf.clone();
            move || Capture(buf.clone())
        };
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(writer)
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            l.check(&p, Class::Read).unwrap();
            for _ in 0..3 {
                l.check(&p, Class::Read).unwrap_err();
            }
        });
        let logged = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(logged.contains("rate limited"), "{logged}");
        let printed = format!("{l:?} {logged}");
        for part in [subject, "secret"] {
            assert!(!printed.contains(part), "{printed}");
        }
        let key = Key(l.hasher.hash_one(("i", subject)), Class::Read);
        let shown = format!("{key:?}");
        assert_eq!(shown, "Key(<principal>, Read)");
    }

    /// A limited client logs one WARN a minute, counting the refusals between, and debug lines
    /// otherwise.
    #[test]
    fn refusals_are_logged_damped() {
        let clock = Stepped::new();
        let l = Limiter::new(limits((1, 1), (1, 1)), clock.now());
        let p = principal("alice");
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = {
            let buf = buf.clone();
            move || Capture(buf.clone())
        };
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(writer)
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            l.check(&p, Class::Read).unwrap();
            // 1 WARN, then 9 debug lines; 59 s later still debug; at 60 s a WARN counting 11.
            for _ in 0..10 {
                l.check(&p, Class::Read).unwrap_err();
            }
            clock.step(Duration::from_secs(59) + MS * 500);
            l.check(&p, Class::Read).unwrap();
            l.check(&p, Class::Read).unwrap_err();
            clock.step(MS * 500);
            l.check(&p, Class::Read).unwrap_err();
        });
        let logged = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        let warns: Vec<&str> = logged.lines().filter(|l| l.contains(" WARN ")).collect();
        assert_eq!(warns.len(), 2, "{logged}");
        assert!(warns[0].contains("refused=1"), "{}", warns[0]);
        assert!(warns[1].contains("refused=11"), "{}", warns[1]);
        assert_eq!(
            logged.lines().filter(|l| l.contains(" DEBUG ")).count(),
            10,
            "{logged}"
        );
    }

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
