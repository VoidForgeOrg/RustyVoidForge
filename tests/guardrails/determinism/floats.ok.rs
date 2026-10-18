// Control for floats.rs: integer arithmetic with 128-bit intermediates is allowed, checked
// (D-032 bans unchecked operators in these crates).
pub fn mul_div(a: i64, b: i64, c: i64) -> Option<i64> {
    let product = i128::from(a).checked_mul(i128::from(b))?;
    i64::try_from(product.checked_div(i128::from(c))?).ok()
}
