// A float constant passed by name into a float API: in MIR the call reads
// `from_secs_f64(const HALF)`, so only the `_f64` API name shows the float in that function
// (the constant itself is reported through its own `const HALF: f64` item).
// expect-float-fn: HALF
// expect-float-fn: half_second
// expect-float-fn: pi_second

const HALF: f64 = 0.5;

pub fn half_second() -> std::time::Duration {
    std::time::Duration::from_secs_f64(HALF)
}

pub fn pi_second() -> std::time::Duration {
    std::time::Duration::from_secs_f64(std::f64::consts::PI)
}
