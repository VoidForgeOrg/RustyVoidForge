// Control for clocks.rs: integer game time and durations are allowed.
pub fn advance(game_time_ms: i64, delta_ms: i64) -> i64 {
    game_time_ms + delta_ms
}

pub fn total(a_ms: u64, b_ms: u64) -> std::time::Duration {
    std::time::Duration::from_millis(a_ms) + std::time::Duration::from_millis(b_ms)
}
