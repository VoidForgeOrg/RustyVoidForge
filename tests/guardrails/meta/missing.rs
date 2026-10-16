// Harness trap: the expected ban never fires, so the case must be reported MISSING, not passed.
// classify: MISSING
// expect: clippy::disallowed_methods std::time::Instant::now

pub fn clean() -> u64 {
    1
}
