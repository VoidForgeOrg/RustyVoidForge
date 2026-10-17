// Float literals in other spellings, each going straight into a float API.
// expect-float-fn: whole_seconds
// expect-float-fn: exponent_seconds

pub fn whole_seconds() -> std::time::Duration {
    std::time::Duration::from_secs_f64(2f64)
}

pub fn exponent_seconds() -> std::time::Duration {
    std::time::Duration::from_secs_f32(1e-3)
}
