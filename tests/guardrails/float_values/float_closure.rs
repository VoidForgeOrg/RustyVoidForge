// A float inside a closure body.
// expect-float-fn: shifted::{closure#0}

pub fn shifted(values: &[u64]) -> Vec<u64> {
    values.iter().map(|v| v + u64::from(0.5f32.to_bits())).collect()
}
