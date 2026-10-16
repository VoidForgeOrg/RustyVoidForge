// Control: integer-only code passes, and so does the text "f64" inside a string constant.
pub fn mul_div(a: i64, b: i64, c: i64) -> i64 {
    ((i128::from(a) * i128::from(b)) / i128::from(c)) as i64
}

pub fn millis(d: std::time::Duration) -> u128 {
    d.as_millis()
}

pub fn label() -> &'static str {
    "f64 and f32 are not allowed here"
}
