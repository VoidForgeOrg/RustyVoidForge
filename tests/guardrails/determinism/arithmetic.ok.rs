// Control for arithmetic.rs: checked operations (with a typed outcome) and strict ones (where a
// bound is proven; they panic in every profile instead of wrapping) are allowed.
pub fn total(a: i64, b: i64) -> Option<i64> {
    a.checked_add(b)
}

pub fn counted(n: u32) -> u64 {
    u64::from(n).strict_add(1)
}
