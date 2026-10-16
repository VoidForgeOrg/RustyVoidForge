// Harness trap: the code needs a crate the fixture does not add; that is ERROR, not a rejection.
// classify: ERROR
// expect: clippy::disallowed_methods uuid::Uuid::new_v4

pub fn v4() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}
