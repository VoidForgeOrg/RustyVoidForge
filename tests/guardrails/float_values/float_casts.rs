// An unsuffixed float literal cast to an integer.
// expect-float-fn: cast_literal

pub fn cast_literal() -> i64 {
    let x = 2.5;
    x as i64
}
