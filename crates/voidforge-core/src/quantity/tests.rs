//! Quantity tests. Generated cases compare the closed forms against [`Stepper`], a reference that
//! advances one millisecond at a time and moves whole grams in and out of an accumulator: no
//! division, no floor and no crossing formula, so it shares no algebra with the implementation.

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

use super::*;

/// Independent reference: value and accumulator (in 1/3,600,000 g), one millisecond per step.
#[derive(Debug, Clone, Copy)]
struct Stepper {
    value: i128,
    acc: i128,
    rate: i128,
}

impl Stepper {
    fn from(q: &Quantity) -> Self {
        Self {
            value: i128::from(q.value_g()),
            acc: i128::from(q.remainder()),
            rate: i128::from(q.rate_g_per_h()),
        }
    }

    fn step(&mut self) {
        self.acc += self.rate;
        while self.acc >= H {
            self.value += 1;
            self.acc -= H;
        }
        while self.acc < 0 {
            self.value -= 1;
            self.acc += H;
        }
    }

    fn after(mut self, ms: i64) -> Self {
        for _ in 0..ms {
            self.step();
        }
        self
    }

    /// Milliseconds until `reached(value)` holds, stepping at most `limit`; None if never.
    fn first(mut self, limit: i64, reached: impl Fn(i128) -> bool) -> Option<i64> {
        if reached(self.value) {
            return Some(0);
        }
        for ms in 1..=limit {
            self.step();
            if reached(self.value) {
                return Some(ms);
            }
        }
        None
    }
}

const HORIZON_MS: i64 = 20_000;

/// Fixed seed and no failure files, so every run explores the same cases.
fn config() -> Config {
    Config {
        cases: 512,
        rng_seed: RngSeed::Fixed(0x7016_0f02_7e57_5eed),
        failure_persistence: None,
        ..Config::default()
    }
}

/// Rates spanning slow (less than a gram per hour-fraction), per-millisecond and fast flows.
fn rate() -> impl Strategy<Value = i64> {
    prop_oneof![
        Just(0_i64),
        -3_000_i64..=3_000,
        -2 * MS_PER_HOUR..=2 * MS_PER_HOUR,
        prop_oneof![
            Just(MS_PER_HOUR),
            Just(-MS_PER_HOUR),
            Just(MS_PER_HOUR - 1),
            Just(1 - MS_PER_HOUR)
        ],
        -60 * MS_PER_HOUR..=60 * MS_PER_HOUR,
    ]
}

/// Remainders with emphasis on the edges of the range.
fn remainder() -> impl Strategy<Value = i64> {
    prop_oneof![Just(0_i64), Just(1), Just(MS_PER_HOUR - 1), 0..MS_PER_HOUR]
}

fn quantity() -> impl Strategy<Value = Quantity> {
    (
        -1_000_000_i64..=1_000_000,
        rate(),
        remainder(),
        -5_000_000_i64..=5_000_000,
    )
        .prop_map(|(value, rate, remainder, checkpoint)| {
            Quantity::from_parts(value, rate, remainder, checkpoint)
                .expect("generated remainder is in range")
        })
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn reads_and_checkpoints_match_millisecond_stepping(q in quantity(), dt in 0_i64..=HORIZON_MS) {
        let reference = Stepper::from(&q).after(dt);
        let t = q.checkpoint_ms() + dt;
        prop_assert_eq!(i128::from(q.value_at(t).unwrap()), reference.value);
        let advanced = q.advanced_to(t).unwrap();
        prop_assert_eq!(i128::from(advanced.value_g()), reference.value);
        prop_assert_eq!(i128::from(advanced.remainder()), reference.acc);
        prop_assert_eq!(advanced.rate_g_per_h(), q.rate_g_per_h());
        prop_assert_eq!(advanced.checkpoint_ms(), t);
    }

    #[test]
    fn checkpointing_in_steps_equals_one_jump(
        q in quantity(),
        cuts in proptest::collection::vec(0_i64..=HORIZON_MS, 0..6),
        dt in 0_i64..=HORIZON_MS,
    ) {
        let end = q.checkpoint_ms() + dt;
        let mut times: Vec<i64> = cuts.iter().map(|c| q.checkpoint_ms() + c % (dt + 1)).collect();
        times.sort_unstable();
        let mut stepped = q;
        for t in times {
            // Reads in between change nothing.
            let _ = stepped.value_at(end).unwrap();
            stepped = stepped.advanced_to(t).unwrap();
        }
        prop_assert_eq!(stepped.advanced_to(end).unwrap(), q.advanced_to(end).unwrap());
    }

    #[test]
    fn rising_crossing_is_the_first_reaching_millisecond(q in quantity(), above in 0_i64..=40) {
        let boundary = q.value_g() + above;
        let expected = Stepper::from(&q).first(HORIZON_MS, |v| v >= i128::from(boundary));
        let got = q.rising_crossing(boundary).unwrap();
        match expected {
            Some(ms) => prop_assert_eq!(got, Some(q.checkpoint_ms() + ms)),
            None if q.rate_g_per_h() <= 0 => prop_assert_eq!(got, None),
            None => prop_assert!(got.is_some_and(|t| t > q.checkpoint_ms() + HORIZON_MS)),
        }
    }

    #[test]
    fn falling_crossing_is_the_first_reaching_millisecond(q in quantity(), below in 0_i64..=40) {
        let boundary = q.value_g() - below;
        let expected = Stepper::from(&q).first(HORIZON_MS, |v| v <= i128::from(boundary));
        let got = q.falling_crossing(boundary).unwrap();
        match expected {
            Some(ms) => prop_assert_eq!(got, Some(q.checkpoint_ms() + ms)),
            None if q.rate_g_per_h() >= 0 => prop_assert_eq!(got, None),
            None => prop_assert!(got.is_some_and(|t| t > q.checkpoint_ms() + HORIZON_MS)),
        }
    }

    #[test]
    fn a_boundary_behind_the_rate_is_never_crossed(q in quantity(), gap in 1_i64..=1_000) {
        if q.rate_g_per_h() <= 0 {
            prop_assert_eq!(q.rising_crossing(q.value_g() + gap).unwrap(), None);
        }
        if q.rate_g_per_h() >= 0 {
            prop_assert_eq!(q.falling_crossing(q.value_g() - gap).unwrap(), None);
        }
    }

    #[test]
    fn snapping_lands_exactly_on_the_boundary(q in quantity(), boundary in -1_000_000_i64..=1_000_000, dt in 0_i64..=HORIZON_MS) {
        let t = q.checkpoint_ms() + dt;
        let snapped = q.snapped(boundary, t).unwrap();
        prop_assert_eq!(snapped.value_g(), boundary);
        prop_assert_eq!(snapped.remainder(), 0);
        prop_assert_eq!(snapped.rate_g_per_h(), q.rate_g_per_h());
        prop_assert_eq!(snapped.checkpoint_ms(), t);
        prop_assert_eq!(snapped.value_at(t).unwrap(), boundary);
    }
}

// Named cases.

#[test]
fn ore_storage_with_1000_t_free_filling_at_2_t_per_h_is_full_at_hour_500() {
    // Game design and Architectural Design, Testing strategy.
    let free = Quantity::new(0, 2_000_000, 0);
    assert_eq!(
        free.rising_crossing(1_000_000_000).unwrap(),
        Some(500 * MS_PER_HOUR)
    );
}

#[test]
fn a_falling_gram_is_debited_at_the_first_millisecond() {
    // 1 g at -1 g/h: the exact value at 1 ms is just under 1 g, which reads 0.
    let q = Quantity::new(1, -1, 0);
    assert_eq!(q.value_at(1).unwrap(), 0);
    assert_eq!(q.advanced_to(1).unwrap().remainder(), MS_PER_HOUR - 1);
    assert_eq!(q.falling_crossing(0).unwrap(), Some(1));
}

#[test]
fn falling_is_not_the_mirror_of_rising() {
    // A mirrored rising formula would put this crossing at one hour, not one millisecond.
    let q = Quantity::new(1, -1, 0);
    // ceil(((value - boundary) * H + remainder) / speed) with value 1, boundary 0, speed 1:
    let mirrored = ceil_div(H, 1);
    assert_eq!(mirrored, 3_600_000);
    assert_eq!(q.falling_crossing(0).unwrap(), Some(1));
}

#[test]
fn a_rising_remainder_is_carried_until_it_makes_a_gram() {
    let q = Quantity::new(10, 1, 0);
    assert_eq!(q.value_at(MS_PER_HOUR - 1).unwrap(), 10);
    assert_eq!(q.value_at(MS_PER_HOUR).unwrap(), 11);
    assert_eq!(
        q.advanced_to(MS_PER_HOUR - 1).unwrap().remainder(),
        MS_PER_HOUR - 1
    );
}

#[test]
fn a_remainder_carried_in_reaches_the_boundary_one_millisecond_early() {
    // remainder H-1 plus 1 g/h reaches the next gram after exactly 1 ms.
    let q = Quantity::from_parts(5, 1, MS_PER_HOUR - 1, 100).unwrap();
    assert_eq!(q.rising_crossing(6).unwrap(), Some(101));
    assert_eq!(q.value_at(100).unwrap(), 5);
    assert_eq!(q.value_at(101).unwrap(), 6);
}

#[test]
fn exact_and_one_short_crossings() {
    // 3.6 t/h is exactly 1 g/ms: 7 g are gained at 7 ms, not 6.
    let q = Quantity::new(0, MS_PER_HOUR, 0);
    assert_eq!(q.rising_crossing(7).unwrap(), Some(7));
    assert_eq!(q.value_at(6).unwrap(), 6);
    let q = Quantity::new(7, -MS_PER_HOUR, 0);
    assert_eq!(q.falling_crossing(0).unwrap(), Some(7));
    assert_eq!(q.value_at(6).unwrap(), 1);
}

#[test]
fn a_fast_rate_overshoots_and_the_snap_lands_on_the_boundary() {
    // 10 g/ms jumps past 15 g between 1 ms and 2 ms; the crossing is 2 ms and snapping clamps.
    let q = Quantity::new(0, 10 * MS_PER_HOUR, 0);
    assert_eq!(q.rising_crossing(15).unwrap(), Some(2));
    assert_eq!(q.value_at(2).unwrap(), 20);
    assert_eq!(q.snapped(15, 2).unwrap().value_g(), 15);
}

#[test]
fn zero_rate_never_moves_or_crosses() {
    let q = Quantity::from_parts(42, 0, 123, 7).unwrap();
    assert_eq!(q.value_at(7 + 10 * MS_PER_HOUR).unwrap(), 42);
    assert_eq!(q.rising_crossing(43).unwrap(), None);
    assert_eq!(q.falling_crossing(41).unwrap(), None);
    assert_eq!(
        q.rising_crossing(42).unwrap(),
        Some(7),
        "already at the boundary"
    );
}

#[test]
fn already_past_the_boundary_crosses_at_the_checkpoint() {
    let q = Quantity::new(50, 1_000, 9);
    assert_eq!(q.rising_crossing(40).unwrap(), Some(9));
    assert_eq!(q.falling_crossing(60).unwrap(), Some(9));
}

#[test]
fn a_rate_change_checkpoints_first() {
    let q = Quantity::new(0, MS_PER_HOUR, 0)
        .with_rate(10, -2 * MS_PER_HOUR)
        .unwrap();
    assert_eq!(
        (
            q.value_g(),
            q.rate_g_per_h(),
            q.remainder(),
            q.checkpoint_ms()
        ),
        (10, -2 * MS_PER_HOUR, 0, 10)
    );
    assert_eq!(q.value_at(13).unwrap(), 4);
}

#[test]
fn reading_before_the_checkpoint_is_an_error() {
    let q = Quantity::new(1, 1, 100);
    let error = QuantityError::BeforeCheckpoint {
        time_ms: 99,
        checkpoint_ms: 100,
    };
    assert_eq!(q.value_at(99), Err(error));
    assert_eq!(q.advanced_to(99), Err(error));
    assert_eq!(q.with_rate(99, 5), Err(error));
    assert_eq!(q.snapped(0, 99), Err(error));
}

#[test]
fn remainders_outside_the_range_are_rejected() {
    assert_eq!(
        Quantity::from_parts(0, 0, -1, 0),
        Err(QuantityError::RemainderOutOfRange(-1))
    );
    assert_eq!(
        Quantity::from_parts(0, 0, MS_PER_HOUR, 0),
        Err(QuantityError::RemainderOutOfRange(MS_PER_HOUR))
    );
    assert!(Quantity::from_parts(0, 0, MS_PER_HOUR - 1, 0).is_ok());
}

#[test]
fn values_leaving_the_64_bit_range_are_errors_not_wrapped() {
    let up = Quantity::new(i64::MAX, MS_PER_HOUR, 0);
    assert_eq!(up.value_at(0).unwrap(), i64::MAX);
    assert_eq!(up.value_at(1), Err(QuantityError::ValueOutOfRange));
    assert_eq!(up.advanced_to(1), Err(QuantityError::ValueOutOfRange));
    let down = Quantity::new(i64::MIN, -1, 0);
    assert_eq!(down.value_at(1), Err(QuantityError::ValueOutOfRange));
}

#[test]
fn extreme_rates_and_spans_stay_exact() {
    // The largest span (i64::MIN to i64::MAX) at the most negative rate does not overflow i128.
    let q = Quantity::new(0, i64::MIN, i64::MIN);
    assert_eq!(q.value_at(i64::MIN + MS_PER_HOUR).unwrap(), i64::MIN);
    assert_eq!(q.value_at(i64::MAX), Err(QuantityError::ValueOutOfRange));
    let q = Quantity::new(0, i64::MAX, i64::MIN);
    assert_eq!(q.value_at(i64::MAX), Err(QuantityError::ValueOutOfRange));
    // Boundaries at the ends of the range are reachable without overflow in the formula.
    assert!(
        Quantity::new(i64::MIN, i64::MAX, 0)
            .rising_crossing(i64::MAX)
            .unwrap()
            .is_some()
    );
}

#[test]
fn crossings_beyond_the_64_bit_clock_are_errors() {
    let q = Quantity::new(0, 1, i64::MAX - 10);
    assert_eq!(q.rising_crossing(1), Err(QuantityError::TimeOutOfRange));
    let q = Quantity::new(0, -1, i64::MAX - 10);
    assert_eq!(q.falling_crossing(-2), Err(QuantityError::TimeOutOfRange));
    // One that just fits.
    let q = Quantity::new(0, MS_PER_HOUR, i64::MAX - 10);
    assert_eq!(q.rising_crossing(10).unwrap(), Some(i64::MAX));
}

#[test]
fn generated_cases_cover_the_interesting_classes() {
    // Guards the property tests above against strategies that drift into vacuous cases: draw
    // the same number of quantities as one property run and count the classes they exercise.
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::new(config());
    let (mut rising, mut falling, mut zero, mut edge_remainder) = (0, 0, 0, 0);
    let (mut rise_found, mut fall_found, mut fast) = (0, 0, 0);
    for _ in 0..config().cases {
        let q = quantity()
            .new_tree(&mut runner)
            .expect("draw a quantity")
            .current();
        let rate = q.rate_g_per_h();
        match rate.signum() {
            1 => rising += 1,
            -1 => falling += 1,
            _ => zero += 1,
        }
        if q.remainder() == 0 || q.remainder() == MS_PER_HOUR - 1 {
            edge_remainder += 1;
        }
        if rate.abs() > MS_PER_HOUR {
            fast += 1;
        }
        let stepper = Stepper::from(&q);
        if rate > 0
            && stepper
                .first(HORIZON_MS, |v| v >= i128::from(q.value_g() + 1))
                .is_some_and(|ms| ms > 0)
        {
            rise_found += 1;
        }
        if rate < 0
            && stepper
                .first(HORIZON_MS, |v| v <= i128::from(q.value_g() - 1))
                .is_some_and(|ms| ms > 0)
        {
            fall_found += 1;
        }
    }
    let counts = [
        rising,
        falling,
        zero,
        edge_remainder,
        rise_found,
        fall_found,
        fast,
    ];
    println!(
        "rising {rising}, falling {falling}, zero {zero}, edge remainder {edge_remainder}, rising crossing found {rise_found}, falling crossing found {fall_found}, faster than 1 g/ms {fast}"
    );
    assert!(
        counts.iter().all(|&count| count >= 25),
        "a class is under-sampled: {counts:?}"
    );
}
