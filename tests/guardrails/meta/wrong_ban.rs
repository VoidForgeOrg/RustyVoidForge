// Harness trap: a different ban firing does not satisfy the expected one; that is ERROR.
// classify: ERROR
// expect: clippy::disallowed_types std::collections::HashSet

pub fn map() -> std::collections::HashMap<u64, u64> {
    std::collections::HashMap::new()
}
