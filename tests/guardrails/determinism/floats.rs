// Determinism fixture: floats. Integers only: 64-bit storage, 128-bit intermediates.
// expect: clippy::disallowed_types f32
// expect: clippy::disallowed_types f64
// expect: clippy::float_arithmetic

pub fn double(x: f64) -> f64 {
    x * 2.0
}

pub fn add(x: f32, y: f32) -> f32 {
    x + y
}

pub fn scaled(x: i64) -> i64 {
    (x as f64 * 1.5) as i64
}
