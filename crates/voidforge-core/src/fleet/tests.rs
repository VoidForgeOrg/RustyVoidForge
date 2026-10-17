//! Fleet arithmetic: distance and travel time (Fleets; D-021, D-027). The named fleet tests, on
//! real Rules v0 worlds, are in testkit's `tests/fleets.rs`.

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

use super::*;

const H: i64 = MS_PER_HOUR;

fn at(x: i64, y: i64, z: i64) -> Position {
    Position { x, y, z }
}

#[test]
fn distance_is_exact_on_a_pythagorean_quadruple() {
    // 2² + 3² + 6² = 7², in world units of 1,000 distance units.
    assert_eq!(distance(at(0, 0, 0), at(2_000, -3_000, 6_000)), Some(7_000));
    assert_eq!(distance(at(2_000, -3_000, 6_000), at(0, 0, 0)), Some(7_000));
    assert_eq!(distance(at(5, 5, 5), at(5, 5, 5)), Some(0));
}

#[test]
fn distance_rounds_down() {
    // √2 = 1.41…, √3 = 1.73…: both read 1.
    assert_eq!(distance(at(0, 0, 0), at(1, 1, 0)), Some(1));
    assert_eq!(distance(at(0, 0, 0), at(1, 1, 1)), Some(1));
    // 1,000² + 1² is just past 1,000².
    assert_eq!(distance(at(0, 0, 0), at(1_000, 1, 0)), Some(1_000));
}

#[test]
fn distance_beyond_64_bits_is_none_not_a_panic() {
    let far = at(i64::MIN, i64::MIN, i64::MIN);
    let near = at(i64::MAX, i64::MAX, i64::MAX);
    // Each squared difference is just under 2^128: the sum leaves i128.
    assert_eq!(distance(far, near), None);
    // One axis: (2^64 − 1)² fits in i128, but its root, 2^64 − 1, is not an i64.
    assert_eq!(distance(at(i64::MIN, 0, 0), at(i64::MAX, 0, 0)), None);
    // The largest root that is: i64::MAX itself.
    assert_eq!(distance(at(0, 0, 0), at(i64::MAX, 0, 0)), Some(i64::MAX));
}

#[test]
fn travel_time_is_exact_on_whole_hours_and_rounds_up_otherwise() {
    // 10 world units at a Colony Ship's 10 units per hour: one hour exactly.
    assert_eq!(travel_time_ms(10_000, 10_000), Some(H));
    // 1 distance unit at 15,000 per hour: 240 ms exactly.
    assert_eq!(travel_time_ms(1, 15_000), Some(240));
    // 1 unit at 7,000 per hour: 514.28… ms, so 515.
    assert_eq!(travel_time_ms(1, 7_000), Some(515));
    // Faster than one unit per ms still takes a whole millisecond.
    assert_eq!(travel_time_ms(1, 54_000_000), Some(1));
    assert_eq!(travel_time_ms(0, 10_000), Some(0));
}

#[test]
fn travel_time_beyond_64_bits_is_none_not_a_panic() {
    assert_eq!(travel_time_ms(i64::MAX, 1), None);
    assert_eq!(travel_time_ms(i64::MAX, i64::MAX), Some(H));
    // Defensive: speeds are positive by the rules, and distances are never negative.
    assert_eq!(travel_time_ms(10, 0), None);
    assert_eq!(travel_time_ms(-1, 10), None);
}

fn config(seed: u64) -> Config {
    Config {
        cases: 256,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(seed),
        ..Config::default()
    }
}

fn coordinate() -> impl Strategy<Value = i64> {
    -1_000_000_000i64..=1_000_000_000
}

proptest! {
    #![proptest_config(config(0x0f1e_e7a7_0000_0001))]

    /// The distance d is the integer square root: d² ≤ the sum of squares < (d + 1)².
    #[test]
    fn distance_is_the_integer_square_root(
        a in (coordinate(), coordinate(), coordinate()),
        b in (coordinate(), coordinate(), coordinate()),
    ) {
        let d = distance(at(a.0, a.1, a.2), at(b.0, b.1, b.2)).unwrap();
        let sum: i128 = [(a.0, b.0), (a.1, b.1), (a.2, b.2)]
            .iter()
            .map(|&(p, q)| (i128::from(q) - i128::from(p)).pow(2))
            .sum();
        let d = i128::from(d);
        prop_assert!(d * d <= sum && sum < (d + 1) * (d + 1));
    }

    /// The travel time t is the smallest whole millisecond in which the fleet covers the
    /// distance: t × speed ≥ distance × 3,600,000 > (t − 1) × speed.
    #[test]
    fn travel_time_is_the_first_whole_millisecond_that_covers_the_distance(
        distance in 0i64..=10_000_000_000,
        speed in 1i64..=100_000_000,
    ) {
        let t = i128::from(travel_time_ms(distance, speed).unwrap());
        let (d, s, h) = (i128::from(distance), i128::from(speed), i128::from(H));
        prop_assert!(t * s >= d * h);
        prop_assert!(t == 0 || (t - 1) * s < d * h);
    }
}
