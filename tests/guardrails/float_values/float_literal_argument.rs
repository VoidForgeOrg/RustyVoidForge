// A float literal passed straight into a float API: MIR has no f32/f64 local for it, only the
// constant `0.5f64` inside the call (Phase 0 review-r1 finding).
// expect-float-fn: ticks_for_half_second
// expect-float-fn: tolerance_ok

pub fn ticks_for_half_second() -> u64 {
    std::time::Duration::from_secs_f64(0.5).as_millis() as u64
}

pub fn tolerance_ok(d: std::time::Duration) -> bool {
    d < std::time::Duration::from_secs_f32(0.25)
}
