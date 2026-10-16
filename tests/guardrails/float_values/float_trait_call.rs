// Float addition through the trait method, which clippy::float_arithmetic does not see.
// expect-float-fn: add_bits

pub fn add_bits() -> u64 {
    std::ops::Add::add(1.0f64, 2.0).to_bits()
}
