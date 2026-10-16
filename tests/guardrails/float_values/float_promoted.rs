// Floats in a promoted constant array: its bytes sit in an allocation block, which the check
// skips, but the promoted constant and the function both carry its type.
// expect-float-fn: first_bits
// expect-float-fn: first_bits::promoted[0]

pub fn first_bits() -> u64 {
    let values = &[1.5f64, 2.5];
    values[0].to_bits()
}
