// Control for floats.rs: integer arithmetic with 128-bit intermediates is allowed.
pub fn mul_div(a: i64, b: i64, c: i64) -> i64 {
    ((i128::from(a) * i128::from(b)) / i128::from(c)) as i64
}
