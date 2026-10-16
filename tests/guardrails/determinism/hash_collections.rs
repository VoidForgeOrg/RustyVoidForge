// Determinism fixture: hash-ordered collections. Every collection iterates in ID order.
// expect: clippy::disallowed_types std::collections::HashMap
// expect: clippy::disallowed_types std::collections::HashSet

pub fn map() -> std::collections::HashMap<u64, u64> {
    std::collections::HashMap::new()
}

pub fn set() -> std::collections::HashSet<u64> {
    std::collections::HashSet::new()
}
