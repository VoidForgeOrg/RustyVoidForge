// Control for hash_collections.rs: ordered collections are allowed.
pub fn map() -> std::collections::BTreeMap<u64, u64> {
    std::collections::BTreeMap::new()
}

pub fn set() -> std::collections::BTreeSet<u64> {
    std::collections::BTreeSet::new()
}
