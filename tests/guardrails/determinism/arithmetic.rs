// Determinism fixture: unchecked arithmetic (D-032). Every integer operation in the core, rules and
// worldgen is checked or strict, so nothing wraps silently in release.
// expect: clippy::arithmetic_side_effects

pub fn total(a: i64, b: i64) -> i64 {
    a + b
}

pub fn scaled(grams: i128, points: i128) -> i128 {
    grams * points
}

pub fn share(left: u64, n: u64) -> u64 {
    left / n
}
