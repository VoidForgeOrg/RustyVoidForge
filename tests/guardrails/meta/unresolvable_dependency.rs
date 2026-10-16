// Harness trap: cargo cannot resolve the dependency; that is ERROR, not a rejection.
// classify: ERROR
// expect: clippy::disallowed_methods uuid::Uuid::new_v4
// dependency: uuid = { version = "=0.0.999", features = ["v4"] }

pub fn v4() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}
