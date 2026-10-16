// Float values the clippy bans cannot see: no float type is named and no float operator is used.
// expect-float-fn: sin_bits
// expect-float-fn: sqrt_bits

pub fn sin_bits() -> u64 {
    1.25f64.sin().to_bits()
}

pub fn sqrt_bits() -> u64 {
    1.25f64.sqrt().to_bits()
}
