// Determinism fixture: clocks. Game time is an input stamped by the host; the core never reads a clock.
// expect: clippy::disallowed_methods std::time::Instant::now
// expect: clippy::disallowed_methods std::time::Instant::elapsed
// expect: clippy::disallowed_methods std::time::SystemTime::now
// expect: clippy::disallowed_methods std::time::SystemTime::elapsed
// expect: clippy::disallowed_methods uuid::Timestamp::now
// expect: clippy::disallowed_types std::time::Instant
// expect: clippy::disallowed_types std::time::SystemTime
// dependency: uuid = { version = "=1.28.0", features = ["v7"] }

pub fn instant_now() -> u128 {
    let start = std::time::Instant::now();
    start.elapsed().as_millis()
}

pub fn system_time_now() -> bool {
    std::time::SystemTime::now().elapsed().is_ok()
}

pub fn instant_type(t: std::time::Instant) -> std::time::Instant {
    t
}

pub fn system_time_type(t: std::time::SystemTime) -> std::time::SystemTime {
    t
}

pub fn uuid_timestamp_now() -> uuid::Timestamp {
    uuid::Timestamp::now(uuid::NoContext)
}
