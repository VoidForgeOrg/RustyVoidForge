// Determinism fixture: threads. The core is single-threaded.
// expect: clippy::disallowed_methods std::thread::spawn
// expect: clippy::disallowed_methods std::thread::scope
// expect: clippy::disallowed_methods std::thread::sleep
// expect: clippy::disallowed_types std::thread::Builder
// expect: clippy::disallowed_types std::thread::JoinHandle
// expect: clippy::disallowed_types std::thread::Scope
// expect: clippy::disallowed_types std::thread::ScopedJoinHandle

pub fn spawn() -> std::thread::JoinHandle<()> {
    std::thread::spawn(|| {})
}

pub fn scope() -> bool {
    std::thread::scope(|s| {
        let handle: std::thread::ScopedJoinHandle<'_, ()> = s.spawn(|| {});
        handle.join().is_ok()
    })
}

pub fn scope_type(_scope: &std::thread::Scope<'_, '_>) {}

pub fn sleep() {
    std::thread::sleep(std::time::Duration::from_millis(1));
}

pub fn builder() -> std::thread::Builder {
    std::thread::Builder::new()
}
