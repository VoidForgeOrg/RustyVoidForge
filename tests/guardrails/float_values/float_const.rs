// A float evaluated in a constant and a static.
// expect-float-fn: BITS
// expect-float-fn: STATIC_BITS

pub const BITS: u64 = 0.5f64.to_bits();
pub static STATIC_BITS: u32 = 0.25f32.to_bits();
