//! Bounded waiting (D-052): every wait has a deadline, and the next poll comes at the view's
//! `valid_until` or within a second, whichever is sooner. There are no blind sleeps and no
//! unbounded waits.
//!
//! A deadline is measured from the server's own times: how long a view says is left (an `eta` or
//! an `arrives_at` minus its `as_of`), plus slack, from when the bot read it. So the bot's clock
//! and the server's never have to agree.

use std::time::Duration;

use tokio::time::Instant;
use voidforge_api_types::wire::Timestamp;

use crate::Failure;

/// Slack on top of what a view says is left.
pub const SLACK: Duration = Duration::from_secs(10);

/// The longest gap between two polls.
const MAX_POLL: Duration = Duration::from_secs(1);

/// The shortest gap between two polls.
const MIN_POLL: Duration = Duration::from_millis(100);

/// Milliseconds since the Unix epoch of an RFC 3339 UTC time with milliseconds, the only form
/// [`Timestamp`] holds.
#[must_use]
pub fn unix_ms(time: &Timestamp) -> i64 {
    let b = time.as_str().as_bytes();
    let number = |from: usize, to: usize| -> i64 {
        b[from..to]
            .iter()
            .fold(0, |n, c| n * 10 + i64::from(c - b'0'))
    };
    let (year, month, day) = (number(0, 4), number(5, 7), number(8, 10));
    // Days from 1970-01-01 to the date: Howard Hinnant's days_from_civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let ms_of_day =
        ((number(11, 13) * 60 + number(14, 16)) * 60 + number(17, 19)) * 1_000 + number(20, 23);
    days * 86_400_000 + ms_of_day
}

/// A Unix time in seconds as RFC 3339 UTC (`2026-10-20T07:23:26Z`), for messages: Howard
/// Hinnant's civil_from_days, the inverse of [`unix_ms`]'s date arithmetic.
#[must_use]
pub fn utc(unix_s: i64) -> String {
    let (days, secs) = (unix_s.div_euclid(86_400), unix_s.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3_600,
        secs % 3_600 / 60,
        secs % 60
    )
}

/// How long from `as_of` to `until`, never negative.
#[must_use]
pub fn between(as_of: &Timestamp, until: &Timestamp) -> Duration {
    Duration::from_millis(u64::try_from(unix_ms(until) - unix_ms(as_of)).unwrap_or(0))
}

/// The deadline for something a view says is `left` away: now plus that plus [`SLACK`].
#[must_use]
pub fn deadline_after(left: Duration) -> Instant {
    Instant::now() + left + SLACK
}

/// What one poll found.
pub enum Poll<T> {
    /// Done, with the result.
    Ready(T),
    /// Not yet. `next` is the view's `valid_until` and `as_of`, when it has them, and `last`
    /// describes what it showed, for the deadline's message.
    Pending {
        /// The view's `as_of` and `valid_until`.
        next: Option<(Timestamp, Timestamp)>,
        /// What the last view showed.
        last: String,
    },
}

/// Polls `poll` until it's ready or `deadline` passes; a passed deadline is
/// [`Failure::Deadline`] for `step`, with the last view.
///
/// # Errors
/// [`Failure::Deadline`], or whatever `poll` returns.
pub async fn until<T, F, Fut>(step: &str, deadline: Instant, mut poll: F) -> Result<T, Failure>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Poll<T>, Failure>>,
{
    loop {
        let (next, last) = match poll().await? {
            Poll::Ready(value) => return Ok(value),
            Poll::Pending { next, last } => (next, last),
        };
        let now = Instant::now();
        if now >= deadline {
            return Err(Failure::Deadline {
                step: step.to_owned(),
                detail: last,
            });
        }
        let gap = next
            .map_or(MAX_POLL, |(as_of, valid_until)| {
                between(&as_of, &valid_until)
            })
            .clamp(MIN_POLL, MAX_POLL);
        tokio::time::sleep_until((now + gap).min(deadline)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> Timestamp {
        Timestamp::try_from(text.to_owned()).unwrap()
    }

    #[test]
    fn unix_ms_matches_known_instants() {
        assert_eq!(unix_ms(&t("1970-01-01T00:00:00.000Z")), 0);
        assert_eq!(unix_ms(&t("1970-01-01T00:00:01.234Z")), 1_234);
        // The prototype's epoch (seed's own output, 2026-10-20).
        assert_eq!(unix_ms(&t("2026-10-20T01:15:54.557Z")), 1_792_458_954_557);
        // Leap days and the end of a leap year.
        assert_eq!(unix_ms(&t("2000-02-29T00:00:00.000Z")), 951_782_400_000);
        assert_eq!(unix_ms(&t("2024-12-31T23:59:59.999Z")), 1_735_689_599_999);
        assert_eq!(unix_ms(&t("2099-12-31T23:59:59.999Z")), 4_102_444_799_999);
    }

    #[test]
    fn utc_inverts_unix_ms() {
        for text in [
            "1970-01-01T00:00:00.000Z",
            "2000-02-29T23:59:59.000Z",
            "2026-10-20T07:23:26.000Z",
            "2024-12-31T23:59:59.000Z",
            "2099-12-31T23:59:59.000Z",
        ] {
            let ms = unix_ms(&t(text));
            assert_eq!(utc(ms / 1_000), text.replace(".000", ""));
        }
    }

    #[test]
    fn between_is_the_difference_and_never_negative() {
        let a = t("2026-10-20T01:17:21.602Z");
        let b = t("2026-10-20T01:17:57.380Z");
        assert_eq!(between(&a, &b), Duration::from_millis(35_778));
        assert_eq!(between(&b, &a), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn a_passed_deadline_names_the_step_and_the_last_view() {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut polls = 0;
        let result: Result<(), _> = until("the step", deadline, || {
            polls += 1;
            let last = format!("poll {polls}");
            async move { Ok(Poll::Pending { next: None, last }) }
        })
        .await;
        let Err(Failure::Deadline { step, detail }) = result else {
            panic!("a deadline failure");
        };
        assert_eq!(step, "the step");
        // Polls at 0, 1, 2 and 3 s: one a second, and one at the deadline itself.
        assert_eq!(detail, "poll 4");
        assert_eq!(polls, 4);
    }

    #[tokio::test(start_paused = true)]
    async fn polls_follow_valid_until_within_the_bounds() {
        let start = Instant::now();
        let mut seen = Vec::new();
        let value = until("the step", start + Duration::from_secs(60), || {
            seen.push(start.elapsed());
            let n = seen.len();
            // valid_until 250 ms after as_of, then 10 ms (clamped up to 100 ms), then ready.
            let next = match n {
                1 => Some((t("2026-10-20T00:00:00.000Z"), t("2026-10-20T00:00:00.250Z"))),
                _ => Some((t("2026-10-20T00:00:00.000Z"), t("2026-10-20T00:00:00.010Z"))),
            };
            async move {
                Ok(if n == 3 {
                    Poll::Ready(n)
                } else {
                    Poll::Pending {
                        next,
                        last: String::new(),
                    }
                })
            }
        })
        .await
        .unwrap();
        assert_eq!(value, 3);
        assert_eq!(
            seen,
            [
                Duration::ZERO,
                Duration::from_millis(250),
                Duration::from_millis(350)
            ]
        );
    }
}
