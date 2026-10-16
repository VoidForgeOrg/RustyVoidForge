// Harness trap: a compile error unrelated to the ban must be reported ERROR, never a rejection.
// classify: ERROR
// expect: clippy::disallowed_methods std::time::Instant::now

pub fn broken() -> u128 {
    std::time::Instant::now().elapsed().as_millis() + does_not_exist()
}
