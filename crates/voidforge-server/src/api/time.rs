//! Game time on the wire: the universe epoch plus game milliseconds, as RFC 3339 UTC with
//! milliseconds (D-045's `Timestamp`), by Howard Hinnant's days-from-civil algorithm, inverted
//! (decision log D-049, T6). A time whose year leaves 0000 to 9999, or whose epoch plus game time
//! overflows, is an error, never a wrapped date.

use voidforge_api_types::wire::Timestamp;

const MS_PER_DAY: i64 = 86_400_000;

/// Why a game time can't be put on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("game time {t} ms past the epoch {epoch_unix_ms} is not a date in years 0000 to 9999")]
pub struct OutOfRange {
    /// The epoch, Unix milliseconds.
    pub epoch_unix_ms: i64,
    /// The game time.
    pub t: i64,
}

/// The wire form of game time `t` under `epoch_unix_ms`.
///
/// # Errors
/// [`OutOfRange`] when the sum overflows or the year leaves 0000 to 9999.
pub fn timestamp(epoch_unix_ms: i64, t: i64) -> Result<Timestamp, OutOfRange> {
    let err = OutOfRange { epoch_unix_ms, t };
    let unix_ms = epoch_unix_ms.checked_add(t).ok_or(err)?;
    let days = unix_ms.div_euclid(MS_PER_DAY);
    let ms_of_day = unix_ms.rem_euclid(MS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    if !(0..=9999).contains(&year) {
        return Err(err);
    }
    let text = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        ms_of_day / 3_600_000,
        ms_of_day / 60_000 % 60,
        ms_of_day / 1_000 % 60,
        ms_of_day % 1_000,
    );
    // Our own output, checked by the contract's own rule: a mismatch is a bug here, and the
    // tests compare the two over a wide range.
    Timestamp::try_from(text).map_err(|_| err)
}

/// (year, month, day) for a count of days since 1970-01-01, in the proleptic Gregorian
/// calendar. `days` is within ±2^63 / 86,400,000, so every step below fits i64.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(epoch: i64, t: i64) -> String {
        serde_json::to_value(timestamp(epoch, t).unwrap())
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// The ruling's fixed vectors, plus the epoch itself and a millisecond before midnight.
    #[test]
    fn fixed_dates_format_exactly() {
        let day = MS_PER_DAY;
        assert_eq!(text(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(text(0, day - 1), "1970-01-01T23:59:59.999Z");
        assert_eq!(text(951_782_400_000, 0), "2000-02-29T00:00:00.000Z");
        assert_eq!(text(4_107_542_400_000, 0), "2100-03-01T00:00:00.000Z");
        // 2100 is a common year: the day before 1 March is 28 February.
        assert_eq!(text(4_107_542_400_000, -1), "2100-02-28T23:59:59.999Z");
        assert_eq!(text(1_760_000_000_000, 123), "2025-10-09T08:53:20.123Z");
        assert_eq!(text(0, -1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn the_year_range_ends_are_inclusive_and_beyond_them_is_an_error() {
        // 0000-01-01 and 9999-12-31T23:59:59.999.
        let first = -62_167_219_200_000;
        let last = 253_402_300_799_999;
        assert_eq!(text(first, 0), "0000-01-01T00:00:00.000Z");
        assert_eq!(text(last, 0), "9999-12-31T23:59:59.999Z");
        assert!(timestamp(first, -1).is_err());
        assert!(timestamp(last, 1).is_err());
        assert!(timestamp(i64::MAX, 1).is_err(), "overflow, never a wrap");
        assert!(timestamp(i64::MIN, -1).is_err());
        assert!(timestamp(i64::MAX, 0).is_err());
        assert!(timestamp(i64::MIN, 0).is_err());
    }

    /// Every day across a wide range: each formats, the contract accepts it, and consecutive
    /// days are consecutive dates.
    #[test]
    fn every_day_from_1600_to_2400_is_a_valid_increasing_date() {
        let start = -11_676_096_000_000; // 1600-01-01
        let mut previous = String::new();
        for d in 0..292_194 {
            let s = text(start, d * MS_PER_DAY + d % 86_400_000);
            assert!(s > previous, "{s} after {previous}");
            previous = s;
        }
        assert!(previous.starts_with("2399-12-31"), "{previous}");
    }
}
