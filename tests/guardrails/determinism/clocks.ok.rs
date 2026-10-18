// Control for clocks.rs: integer game time and durations are allowed (checked, D-032).
pub fn advance(game_time_ms: i64, delta_ms: i64) -> Option<i64> {
    game_time_ms.checked_add(delta_ms)
}

pub fn total(a_ms: u64, b_ms: u64) -> Option<std::time::Duration> {
    std::time::Duration::from_millis(a_ms).checked_add(std::time::Duration::from_millis(b_ms))
}
