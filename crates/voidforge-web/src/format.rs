//! Every value a page shows, formatted the one documented way (phase4-ux §3; P4.3, D-060). The
//! pages format nothing themselves: a number reaches HTML only through this module.
//!
//! - Integers only, with no float anywhere: grams and distance units are divided exactly, and
//!   each row rounds the way §3 says (mass down, rates and distances toward zero, "in N s" up).
//! - Shown negatives use the minus sign `−` (U+2212); a value pre-filled into a form field uses
//!   the ASCII `-`, and [`parse_units`] accepts both (P4.3's D1).
//! - Times are relative to the view's `as_of`, never the web server's clock, so a page says
//!   exactly what held at the moment it shows. [`unix_ms`] reads the fixed shape api-types
//!   already checked (T3).

use voidforge_api_types::view::{FlowStatus, Stock};
use voidforge_api_types::wire::{BuildingType, Mission, PlayerId, Position, ShipType, Timestamp};

/// The minus sign shown in text (§3).
pub const MINUS: char = '\u{2212}';

/// Grams in a tenth of a tonne.
const G_PER_TENTH_T: i64 = 100_000;
/// Distance units in a tenth of a world unit (D-021: 1,000 per world unit).
const DU_PER_TENTH_U: i64 = 100;
/// Distance units in a world unit.
const DU_PER_U: i64 = 1_000;

/// Digits with thousands separated: `1,234,567`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A count of tenths as `1,234.5`.
fn tenths(n: u64) -> String {
    format!("{}.{}", grouped(n / 10), n % 10)
}

/// The sign shown before a magnitude: `−` for a negative, nothing otherwise.
fn minus(negative: bool) -> &'static str {
    if negative { "\u{2212}" } else { "" }
}

/// A whole number with thousands separated: `3,450`, `−1,200` (§3: scores).
#[must_use]
pub fn thousands(n: i64) -> String {
    format!("{}{}", minus(n < 0), grouped(n.unsigned_abs()))
}

/// Tonnes without the unit, rounded **down** to 0.1 t: `1,234.5`, `< 0.1` above 0, `0` (§3).
/// Rounding down never shows more in storage than there is.
#[must_use]
pub fn tonnes(g: i64) -> String {
    if g == 0 {
        return "0".to_owned();
    }
    if (1..G_PER_TENTH_T).contains(&g) {
        return "< 0.1".to_owned();
    }
    let t = g.div_euclid(G_PER_TENTH_T);
    format!("{}{}", minus(t < 0), tenths(t.unsigned_abs()))
}

/// A mass: `1,234.5 t`, `< 0.1 t`, `0 t` (§3).
#[must_use]
pub fn mass(g: i64) -> String {
    format!("{} t", tonnes(g))
}

/// A rate in tonnes per hour, signed, rounded **toward zero** to 0.1: `+36,000.0 t/h`,
/// `−8.0 t/h`, `0 t/h` (§3). A rate under 0.1 t/h either way keeps its sign (`+0.0 t/h`), so a
/// slow drain still shows its direction (P4.3's E3); only a rate of exactly 0 is `0 t/h`.
#[must_use]
pub fn rate(g_per_h: i64) -> String {
    if g_per_h == 0 {
        return "0 t/h".to_owned();
    }
    // Rust's `/` truncates toward zero.
    let t = g_per_h / G_PER_TENTH_T;
    let sign = if g_per_h > 0 { "+" } else { "\u{2212}" };
    format!("{sign}{} t/h", tenths(t.unsigned_abs()))
}

/// A stock without its rate: `1,234.5 t of 5,000.0 t`, with `(full)` at the cap and `(empty)`
/// at 0 (§3).
#[must_use]
pub fn stock(s: &Stock) -> String {
    let note = if s.amount_g == 0 {
        " (empty)"
    } else if s.amount_g == s.cap_g {
        " (full)"
    } else {
        ""
    };
    format!("{} of {}{note}", mass(s.amount_g), mass(s.cap_g))
}

/// Distance units as world units, rounded toward zero to 0.1, signed: `150.0`, `−20.5`.
fn units(du: i64) -> String {
    let t = du / DU_PER_TENTH_U;
    format!("{}{}", minus(t < 0), tenths(t.unsigned_abs()))
}

/// A distance: `123.4 u` (§3; D-021).
#[must_use]
pub fn distance(du: i64) -> String {
    format!("{} u", units(du))
}

/// A position: `(150.0, −20.5, 999.9)` (§3).
#[must_use]
pub fn position(p: &Position) -> String {
    format!("({}, {}, {})", units(p.x), units(p.y), units(p.z))
}

/// A speed: `10.0 u/h` (§3).
#[must_use]
pub fn speed(du_per_h: i64) -> String {
    format!("{} u/h", units(du_per_h))
}

/// Distance units as a form field's value: exact, ASCII `-`, no separators, at least one
/// decimal and at most three: `150.0`, `-20.5`, `999.999` (D1). [`parse_units`] reads it back.
#[must_use]
pub fn field_units(du: i64) -> String {
    let abs = du.unsigned_abs();
    let (whole, frac) = (abs / 1_000, abs % 1_000);
    let sign = if du < 0 { "-" } else { "" };
    let frac = format!("{frac:03}");
    let frac = frac.trim_end_matches('0');
    let frac = if frac.is_empty() { "0" } else { frac };
    format!("{sign}{whole}.{frac}")
}

/// A number of world units typed into a form, as distance units, exactly: an optional `-` or
/// `−`, digits, and up to three decimals (`150`, `-20.5`, `−0.125`, `.5`). None for anything
/// else, or past i64 (§4.4, §6.3). Surrounding spaces are ignored.
#[must_use]
pub fn parse_units(text: &str) -> Option<i64> {
    let text = text.trim();
    let (negative, digits) = match text
        .strip_prefix('-')
        .or_else(|| text.strip_prefix('\u{2212}'))
    {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if (whole.is_empty() && frac.is_empty())
        || (digits.contains('.') && frac.is_empty())
        || frac.len() > 3
        || !all_digits(whole)
        || !all_digits(frac)
    {
        return None;
    }
    let whole: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let frac: i64 = format!("{frac:0<3}").parse().ok()?;
    let du = whole.checked_mul(DU_PER_U)?.checked_add(frac)?;
    Some(if negative { -du } else { du })
}

/// A time as Unix milliseconds. api-types checked the shape (`YYYY-MM-DDTHH:MM:SS.mmmZ`) and
/// the calendar when it read the view; None only for text that never came from one.
#[must_use]
pub fn unix_ms(t: &Timestamp) -> Option<i64> {
    let b = t.as_str().as_bytes();
    if b.len() != 24 {
        return None;
    }
    let number = |from: usize, to: usize| -> Option<i64> {
        b[from..to].iter().try_fold(0_i64, |n, c| {
            c.is_ascii_digit().then(|| n * 10 + i64::from(c - b'0'))
        })
    };
    let days = days_from_civil(number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let secs = number(11, 13)? * 3_600 + number(14, 16)? * 60 + number(17, 19)?;
    Some((days * 86_400 + secs) * 1_000 + number(20, 23)?)
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A proleptic Gregorian date from days since 1970-01-01 (Howard Hinnant's `civil_from_days`),
/// the inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// The whole second a time is shown at (§3, P4.3's design review R1): a time after `as_of` is
/// rounded **up**, so it's never shown before it happens; `as_of` itself and any time before it
/// are cut. A countdown is then the difference of the two times as shown, so they agree.
fn shown_secs(t_ms: i64, as_of_ms: i64) -> i64 {
    let secs = t_ms.div_euclid(1_000);
    if t_ms > as_of_ms {
        secs + i64::from(t_ms.rem_euclid(1_000) != 0)
    } else {
        secs
    }
}

/// Unix seconds as (`2026-10-21`, `06:00:00`).
fn civil(secs: i64) -> (String, String) {
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    (
        format!("{year:04}-{month:02}-{day:02}"),
        format!("{:02}:{:02}:{:02}", s / 3_600, s % 3_600 / 60, s % 60),
    )
}

/// A time as `18:04:15 UTC`, with the date (`2026-10-21 06:00:00 UTC`) only when it isn't the
/// date `as_of` is shown on (§3). Rounded by [`shown_secs`]: up after `as_of`, cut otherwise.
#[must_use]
pub fn clock(t: &Timestamp, as_of: &Timestamp) -> String {
    let Some((t_ms, a_ms)) = unix_ms(t).zip(unix_ms(as_of)) else {
        return t.as_str().to_owned();
    };
    let (day, time) = civil(shown_secs(t_ms, a_ms));
    let (as_of_day, _) = civil(a_ms.div_euclid(1_000));
    if day == as_of_day {
        format!("{time} UTC")
    } else {
        format!("{day} {time} UTC")
    }
}

/// A time with how long until it, against the view's `as_of`: `18:04:15 UTC (in 3 s)`. The
/// countdown is the difference of the two times as shown, so it never says "in 0 s" before it
/// happens and always agrees with the clocks beside it. A time at or before `as_of` (a stale
/// view) is the clock alone: `18:04:15 UTC` (§3).
#[must_use]
pub fn at(t: &Timestamp, as_of: &Timestamp) -> String {
    let shown = clock(t, as_of);
    match unix_ms(t).zip(unix_ms(as_of)) {
        Some((t_ms, a_ms)) if t_ms > a_ms => {
            let secs = shown_secs(t_ms, a_ms) - a_ms.div_euclid(1_000);
            format!("{shown} (in {})", duration(secs.saturating_mul(1_000)))
        }
        _ => shown,
    }
}

/// The units a duration is shown in, largest first: a year is 365 days.
const UNITS: [(i64, &str); 5] = [
    (365 * 86_400, "y"),
    (86_400, "d"),
    (3_600, "h"),
    (60, "min"),
    (1, "s"),
];

/// A duration in its two largest units, rounded **up** to the whole second: `3 s`,
/// `4 min 10 s`, `2 h 14 min`, `3 d 4 h`, `2 y 40 d`; a smaller unit of 0 is left out (`2 h`).
/// The smaller unit is cut, not rounded, so `in 2 h 14 min` never claims more than it knows.
/// A negative duration is `0 s`.
#[must_use]
pub fn duration(ms: i64) -> String {
    let secs = if ms <= 0 {
        0
    } else {
        ms / 1_000 + i64::from(ms % 1_000 != 0)
    };
    let Some(i) = UNITS.iter().position(|(size, _)| secs >= *size) else {
        return "0 s".to_owned();
    };
    let (size, name) = UNITS[i];
    let mut out = format!("{} {name}", secs / size);
    if let Some((smaller, smaller_name)) = UNITS.get(i + 1) {
        let n = secs % size / smaller;
        if n > 0 {
            out.push_str(&format!(" {n} {smaller_name}"));
        }
    }
    out
}

/// Progress towards a cost: `42% (63.0 of 150.0 t), +50.0 t/h, done at 18:04:15 UTC (in 3 s)`.
/// The percent is rounded down, so it says 100% only when done. A null `eta` (nothing flows)
/// reads `waiting for ingots`; the web never computes an ETA of its own (§3, D-030).
#[must_use]
pub fn progress(
    delivered_g: i64,
    cost_g: i64,
    rate_g_per_h: i64,
    eta: Option<&Timestamp>,
    as_of: &Timestamp,
) -> String {
    let percent = if cost_g > 0 {
        (i128::from(delivered_g) * 100 / i128::from(cost_g)).clamp(0, 100)
    } else {
        100
    };
    let head = format!(
        "{percent}% ({} of {} t)",
        tonnes(delivered_g),
        tonnes(cost_g)
    );
    match eta {
        Some(eta) => format!("{head}, {}, done at {}", rate(rate_g_per_h), at(eta, as_of)),
        None => format!("{head}, waiting for ingots"),
    }
}

/// The public short form of a player's ID (phase4-ux Q1): `player` and the UUID's last 8 hex
/// digits, since a UUIDv7 starts with its creation time.
#[must_use]
pub fn short_name(player_id: &str) -> String {
    let tail = player_id
        .char_indices()
        .rev()
        .nth(7)
        .map_or(player_id, |(i, _)| &player_id[i..]);
    format!("player {tail}")
}

/// A player as the viewer reads them: `you`, or the short name (§3).
#[must_use]
pub fn player(id: &PlayerId, me: &PlayerId) -> String {
    if id == me {
        "you".to_owned()
    } else {
        short_name(id.as_str())
    }
}

/// A planet's owner: `you`, `player 8d9e0f12` or `unowned` (§3).
#[must_use]
pub fn owner(owner: Option<&PlayerId>, me: &PlayerId) -> String {
    owner.map_or_else(|| "unowned".to_owned(), |o| player(o, me))
}

/// A building type's name: `Drill`, `Refinery`, `Shipyard` (§3).
#[must_use]
pub fn building(t: BuildingType) -> &'static str {
    match t {
        BuildingType::Drill => "Drill",
        BuildingType::Refinery => "Refinery",
        BuildingType::Shipyard => "Shipyard",
    }
}

/// A ship type's name: `Colony Ship`, `Cargo Vessel` (§3).
#[must_use]
pub fn ship(t: ShipType) -> &'static str {
    match t {
        ShipType::ColonyShip => "Colony Ship",
        ShipType::CargoVessel => "Cargo Vessel",
    }
}

/// A count of ships of one type: `1 Colony Ship`, `2 Cargo Vessels`.
#[must_use]
pub fn ships(n: usize, t: ShipType) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("{n} {}{s}", ship(t))
}

/// A mission's name: `Colonize`, `Transport`, `Move` (§3).
#[must_use]
pub fn mission(m: Mission) -> &'static str {
    match m {
        Mission::Colonize => "Colonize",
        Mission::Transport => "Transport",
        Mission::Move => "Move",
    }
}

/// What a flow belongs to, which decides its words (§3's flow table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// An operational Drill.
    Drill,
    /// An operational Refinery.
    Refinery,
    /// An operational Shipyard.
    Shipyard,
    /// A building under construction, or a ship build: receiving ingots.
    Receiving,
}

/// §3's flow status copy. The view doesn't say which input limits a flow, so the words name the
/// likely causes without claiming one.
#[must_use]
pub fn flow(status: FlowStatus, of: Flow) -> &'static str {
    match (of, status) {
        (Flow::Receiving, FlowStatus::Running) => "receiving ingots",
        (_, FlowStatus::Running) => "running",
        (Flow::Drill, FlowStatus::Throttled) => "slowed: ore storage is full",
        (Flow::Drill, FlowStatus::Halted) => "halted: ore storage is full",
        (Flow::Refinery, FlowStatus::Throttled) => "slowed: short of ore, or ingot storage is full",
        (Flow::Refinery, FlowStatus::Halted) => "halted: no ore, or ingot storage is full",
        (Flow::Shipyard | Flow::Receiving, FlowStatus::Throttled) => "slowed: short of ingots",
        (Flow::Shipyard | Flow::Receiving, FlowStatus::Halted) => "halted: no ingots",
    }
}

/// An ordinal: `1st`, `2nd`, `3rd`, `4th`, `11th`, `22nd`.
#[must_use]
pub fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        Timestamp::try_from(s.to_owned()).unwrap()
    }

    #[test]
    fn whole_numbers() {
        for (n, s) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1,000"),
            (3_450, "3,450"),
            (-1_234_567, "−1,234,567"),
            (i64::MAX, "9,223,372,036,854,775,807"),
            (i64::MIN, "−9,223,372,036,854,775,808"),
        ] {
            assert_eq!(thousands(n), s, "{n}");
        }
    }

    /// Mass rounds down to 0.1 t, with `< 0.1 t` between 0 and 0.1 t.
    #[test]
    fn mass_rounds_down() {
        for (g, s) in [
            (0, "0 t"),
            (1, "< 0.1 t"),
            (99_999, "< 0.1 t"),
            (100_000, "0.1 t"),
            (199_999, "0.1 t"),
            (1_234_567_890, "1,234.5 t"),
            (5_000_000_000, "5,000.0 t"),
            (-1, "−0.1 t"),
            (-100_000, "−0.1 t"),
            (-100_001, "−0.2 t"),
            (i64::MAX, "9,223,372,036,854.7 t"),
            (i64::MIN, "−9,223,372,036,854.8 t"),
        ] {
            assert_eq!(mass(g), s, "{g}");
        }
    }

    /// Rates round toward zero, signed; exactly 0 is the only `0 t/h`.
    #[test]
    fn rates_round_toward_zero() {
        for (g, s) in [
            (0, "0 t/h"),
            (1, "+0.0 t/h"),
            (-1, "−0.0 t/h"),
            (99_999, "+0.0 t/h"),
            (100_000, "+0.1 t/h"),
            (-199_999, "−0.1 t/h"),
            (36_000_000_000, "+36,000.0 t/h"),
            (-8_000_000, "−8.0 t/h"),
            (-34_000_000, "−34.0 t/h"),
            (i64::MAX, "+9,223,372,036,854.7 t/h"),
            (i64::MIN, "−9,223,372,036,854.7 t/h"),
        ] {
            assert_eq!(rate(g), s, "{g}");
        }
    }

    #[test]
    fn stocks_say_full_and_empty() {
        let s = |amount_g, cap_g| {
            stock(&Stock {
                amount_g,
                rate_g_per_h: 0,
                cap_g,
            })
        };
        assert_eq!(s(1_234_500_000, 5_000_000_000), "1,234.5 t of 5,000.0 t");
        assert_eq!(
            s(5_000_000_000, 5_000_000_000),
            "5,000.0 t of 5,000.0 t (full)"
        );
        // A rounding never makes a stock read full: only the exact cap does.
        assert_eq!(s(4_999_999_999, 5_000_000_000), "4,999.9 t of 5,000.0 t");
        // Both round to 5,000.0 t, but only the exact cap is full.
        assert_eq!(s(5_000_000_000, 5_000_050_000), "5,000.0 t of 5,000.0 t");
        assert_eq!(s(0, 5_000_000_000), "0 t of 5,000.0 t (empty)");
        assert_eq!(s(1, 5_000_000_000), "< 0.1 t of 5,000.0 t");
    }

    /// Distances, positions and speeds: du ÷ 1,000, one decimal, toward zero, `−` shown.
    #[test]
    fn distances_positions_and_speeds() {
        for (du, s) in [
            (0, "0.0 u"),
            (99, "0.0 u"),
            (100, "0.1 u"),
            (123_456, "123.4 u"),
            (1_000_000, "1,000.0 u"),
            (i64::MAX, "9,223,372,036,854,775.8 u"),
        ] {
            assert_eq!(distance(du), s, "{du}");
        }
        let p = Position {
            x: 150_000,
            y: -20_500,
            z: 999_999,
        };
        assert_eq!(position(&p), "(150.0, −20.5, 999.9)");
        // Toward zero: −0.05 u is 0.0 (no minus on a zero), −20.55 u is −20.5.
        let p = Position {
            x: -50,
            y: -20_550,
            z: i64::MIN,
        };
        assert_eq!(position(&p), "(0.0, −20.5, −9,223,372,036,854,775.8)");
        assert_eq!(speed(10_000), "10.0 u/h");
        assert_eq!(speed(15_000), "15.0 u/h");
    }

    /// D1: `−20.5` shown, `-20.5` in a field, and both read back to the same distance units.
    #[test]
    fn form_values_use_the_hyphen_and_read_back_exactly() {
        for (du, field) in [
            (150_000, "150.0"),
            (-20_500, "-20.5"),
            (999_999, "999.999"),
            (-125, "-0.125"),
            (0, "0.0"),
            (1_010, "1.01"),
            (i64::MAX, "9223372036854775.807"),
            (i64::MIN + 1, "-9223372036854775.807"),
        ] {
            assert_eq!(field_units(du), field, "{du}");
            assert_eq!(parse_units(field), Some(du), "{field}");
        }
        assert_eq!(
            position(&Position {
                x: 0,
                y: -20_500,
                z: 0
            }),
            "(0.0, −20.5, 0.0)"
        );
        assert_eq!(parse_units("−20.5"), Some(-20_500));
        assert_eq!(parse_units(" 150 "), Some(150_000));
        assert_eq!(parse_units(".5"), Some(500));
        assert_eq!(parse_units("-0"), Some(0));
        for bad in [
            "",
            " ",
            "-",
            "−",
            ".",
            "5.",
            "+5",
            "1,000",
            "1.2345",
            "1e3",
            "--1",
            "−-1",
            "0x10",
            "١",
            "abc",
            "9223372036854775.808",
            "9223372036854776",
        ] {
            assert_eq!(parse_units(bad), None, "{bad:?}");
        }
    }

    /// T3: the parser at month ends, leap days and the century rules, against known Unix times.
    #[test]
    fn timestamps_read_as_unix_milliseconds() {
        for (t, ms) in [
            ("1970-01-01T00:00:00.000Z", 0),
            ("1970-01-01T00:00:00.001Z", 1),
            ("1969-12-31T23:59:59.999Z", -1),
            ("2000-02-29T00:00:00.000Z", 951_782_400_000),
            ("2000-03-01T00:00:00.000Z", 951_868_800_000),
            ("1900-03-01T00:00:00.000Z", -2_203_891_200_000),
            ("2024-02-29T23:59:59.999Z", 1_709_251_199_999),
            ("2024-03-01T00:00:00.000Z", 1_709_251_200_000),
            ("2025-12-31T23:59:59.999Z", 1_767_225_599_999),
            ("2026-01-01T00:00:00.000Z", 1_767_225_600_000),
            ("2026-04-30T12:00:00.000Z", 1_777_550_400_000),
            ("2026-05-01T12:00:00.000Z", 1_777_636_800_000),
            ("0000-01-01T00:00:00.000Z", -62_167_219_200_000),
            ("9999-12-31T23:59:59.999Z", 253_402_300_799_999),
        ] {
            assert_eq!(unix_ms(&ts(t)), Some(ms), "{t}");
        }
    }

    #[test]
    fn clocks_add_the_date_only_on_another_day() {
        let as_of = ts("2026-10-20T18:04:12.000Z");
        // A time after `as_of` rounds up; one before it is cut (R1).
        assert_eq!(
            clock(&ts("2026-10-20T18:04:15.999Z"), &as_of),
            "18:04:16 UTC"
        );
        assert_eq!(
            clock(&ts("2026-10-20T18:04:15.000Z"), &as_of),
            "18:04:15 UTC"
        );
        assert_eq!(
            clock(&ts("2026-10-20T18:04:11.999Z"), &as_of),
            "18:04:11 UTC"
        );
        assert_eq!(clock(&as_of, &as_of), "18:04:12 UTC");
        assert_eq!(
            clock(&ts("2026-10-21T06:00:00.000Z"), &as_of),
            "2026-10-21 06:00:00 UTC"
        );
        assert_eq!(
            clock(&ts("2026-10-19T23:59:59.000Z"), &as_of),
            "2026-10-19 23:59:59 UTC"
        );
    }

    /// "in N" is against `as_of` and rounds up to the second; at or before `as_of`, no "in".
    #[test]
    fn times_ahead_round_up_and_past_ones_have_no_in() {
        let as_of = ts("2026-10-20T18:04:12.000Z");
        let at_ = |t: &str| at(&ts(t), &as_of);
        assert_eq!(at_("2026-10-20T18:04:12.000Z"), "18:04:12 UTC");
        assert_eq!(at_("2026-10-20T18:04:12.001Z"), "18:04:13 UTC (in 1 s)");
        assert_eq!(at_("2026-10-20T18:04:12.999Z"), "18:04:13 UTC (in 1 s)");
        assert_eq!(at_("2026-10-20T18:04:13.000Z"), "18:04:13 UTC (in 1 s)");
        assert_eq!(at_("2026-10-20T18:04:13.001Z"), "18:04:14 UTC (in 2 s)");
        assert_eq!(at_("2026-10-20T18:04:15.000Z"), "18:04:15 UTC (in 3 s)");
        assert_eq!(at_("2026-10-20T18:04:11.999Z"), "18:04:11 UTC");
        assert_eq!(
            at_("2026-10-21T06:00:00.000Z"),
            "2026-10-21 06:00:00 UTC (in 11 h 55 min)"
        );
        // Across a month end and a leap day.
        let feb = ts("2028-02-28T23:59:59.500Z");
        assert_eq!(
            at(&ts("2028-03-01T00:00:00.000Z"), &feb),
            "2028-03-01 00:00:00 UTC (in 1 d)"
        );
        // dev-design's R1 example: `as_of` 08:21:03.400 is shown as 08:21:03, an arrival at
        // 08:21:23.100 as 08:21:24, so "in 21 s", as the clocks say; a valid_until 500 ms after
        // `as_of` isn't shown as the same second.
        let as_of = ts("2026-10-21T08:21:03.400Z");
        assert_eq!(clock(&as_of, &as_of), "08:21:03 UTC");
        assert_eq!(
            at(&ts("2026-10-21T08:21:23.100Z"), &as_of),
            "08:21:24 UTC (in 21 s)"
        );
        assert_eq!(
            clock(&ts("2026-10-21T08:21:03.900Z"), &as_of),
            "08:21:04 UTC"
        );
        assert_eq!(
            clock(&ts("2026-10-21T08:21:03.100Z"), &as_of),
            "08:21:03 UTC"
        );
        // Rounding up can cross midnight, and the date then shows.
        let late = ts("2026-10-20T23:59:59.500Z");
        assert_eq!(
            at(&ts("2026-10-20T23:59:59.600Z"), &late),
            "2026-10-21 00:00:00 UTC (in 1 s)"
        );
    }

    /// The date back from days is the inverse of the parser's, on every day of years around the
    /// leap-year rules and at the calendar's ends.
    #[test]
    fn civil_dates_round_trip() {
        let mut checked = 0;
        for year in [
            0, 1899, 1900, 1969, 1970, 1999, 2000, 2001, 2024, 2100, 9999,
        ] {
            for month in 1..=12 {
                for day in 1..=31 {
                    let t = format!("{year:04}-{month:02}-{day:02}T00:00:00.000Z");
                    let Ok(t) = Timestamp::try_from(t) else {
                        continue;
                    };
                    let ms = unix_ms(&t).unwrap();
                    assert_eq!(
                        civil_from_days(ms.div_euclid(86_400_000)),
                        (year, month, day)
                    );
                    checked += 1;
                }
            }
        }
        // Leap years among them: 0, 2000 and 2024 (1900 and 2100 aren't).
        assert_eq!(checked, 11 * 365 + 3);
    }

    #[test]
    fn durations_take_the_two_largest_units() {
        const S: i64 = 1_000;
        for (ms, s) in [
            (-5, "0 s"),
            (0, "0 s"),
            (1, "1 s"),
            (999, "1 s"),
            (1_000, "1 s"),
            (1_001, "2 s"),
            (3 * S, "3 s"),
            (59 * S, "59 s"),
            (60 * S, "1 min"),
            (250 * S, "4 min 10 s"),
            (3_600 * S - 1, "1 h"),
            (3_599 * S, "59 min 59 s"),
            ((2 * 3_600 + 14 * 60 + 59) * S, "2 h 14 min"),
            ((3 * 86_400 + 4 * 3_600 + 59 * 60) * S, "3 d 4 h"),
            (86_400 * S, "1 d"),
            ((2 * 365 + 40) * 86_400 * S, "2 y 40 d"),
            (365 * 86_400 * S, "1 y"),
            (i64::MAX, "292471208 y 247 d"),
        ] {
            assert_eq!(duration(ms), s, "{ms}");
        }
    }

    #[test]
    fn progress_floors_its_percent_and_never_invents_an_eta() {
        let as_of = ts("2026-10-20T18:04:12.000Z");
        let eta = ts("2026-10-20T18:04:15.000Z");
        assert_eq!(
            progress(63_000_000, 150_000_000, 50_000_000, Some(&eta), &as_of),
            "42% (63.0 of 150.0 t), +50.0 t/h, done at 18:04:15 UTC (in 3 s)"
        );
        assert_eq!(
            progress(149_999_999, 150_000_000, 50_000_000, Some(&eta), &as_of),
            "99% (149.9 of 150.0 t), +50.0 t/h, done at 18:04:15 UTC (in 3 s)"
        );
        assert_eq!(
            progress(150_000_000, 150_000_000, 0, Some(&as_of), &as_of),
            "100% (150.0 of 150.0 t), 0 t/h, done at 18:04:12 UTC"
        );
        assert_eq!(
            progress(0, 150_000_000, 0, None, &as_of),
            "0% (0 of 150.0 t), waiting for ingots"
        );
        assert_eq!(
            progress(i64::MAX, i64::MAX, 0, None, &as_of).get(..4),
            Some("100%")
        );
        assert_eq!(progress(5, 0, 0, None, &as_of).get(..4), Some("100%"));
    }

    #[test]
    fn players_names_and_words() {
        let me = PlayerId::try_from("01890a5d-ac96-774b-bcce-b302099a8057".to_owned()).unwrap();
        let other = PlayerId::try_from("01928c5e-7a3b-7c2d-9e4f-5a6b7c8d9e0f".to_owned()).unwrap();
        assert_eq!(player(&me, &me), "you");
        assert_eq!(player(&other, &me), "player 7c8d9e0f");
        assert_eq!(owner(None, &me), "unowned");
        assert_eq!(owner(Some(&other), &me), "player 7c8d9e0f");
        assert_eq!(short_name("abc"), "player abc");
        assert_eq!(ships(1, ShipType::ColonyShip), "1 Colony Ship");
        assert_eq!(ships(2, ShipType::CargoVessel), "2 Cargo Vessels");
        assert_eq!(building(BuildingType::Shipyard), "Shipyard");
        assert_eq!(mission(Mission::Transport), "Transport");
        for (n, s) in [
            (1, "1st"),
            (2, "2nd"),
            (3, "3rd"),
            (4, "4th"),
            (11, "11th"),
            (12, "12th"),
            (13, "13th"),
            (21, "21st"),
            (22, "22nd"),
            (101, "101st"),
            (111, "111th"),
        ] {
            assert_eq!(ordinal(n), s);
        }
    }

    #[test]
    fn flow_copy_is_section_3s_table() {
        use FlowStatus::{Halted, Running, Throttled};
        for (of, status, s) in [
            (Flow::Drill, Running, "running"),
            (Flow::Drill, Throttled, "slowed: ore storage is full"),
            (Flow::Drill, Halted, "halted: ore storage is full"),
            (Flow::Refinery, Running, "running"),
            (
                Flow::Refinery,
                Throttled,
                "slowed: short of ore, or ingot storage is full",
            ),
            (
                Flow::Refinery,
                Halted,
                "halted: no ore, or ingot storage is full",
            ),
            (Flow::Shipyard, Running, "running"),
            (Flow::Shipyard, Throttled, "slowed: short of ingots"),
            (Flow::Shipyard, Halted, "halted: no ingots"),
            (Flow::Receiving, Running, "receiving ingots"),
            (Flow::Receiving, Throttled, "slowed: short of ingots"),
            (Flow::Receiving, Halted, "halted: no ingots"),
        ] {
            assert_eq!(flow(status, of), s, "{of:?} {status:?}");
        }
    }
}
