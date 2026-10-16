// Determinism fixture: no exceptions. The bans are forbidden, so an item cannot allow or expect them.
// expect: E0453

#[allow(clippy::disallowed_methods)]
pub fn allowed_clock() -> u128 {
    std::time::Instant::now().elapsed().as_millis()
}

#[expect(clippy::disallowed_types)]
pub fn expected_map() -> usize {
    std::collections::HashMap::<u64, u64>::new().len()
}

#[allow(clippy::float_arithmetic)]
pub fn allowed_float(x: i64) -> i64 {
    (x as f64 * 1.5) as i64
}
