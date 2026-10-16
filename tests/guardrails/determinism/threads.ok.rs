// Control for threads.rs: sequential iteration is allowed, and so are thread items not on the list.
pub fn sum(values: &[u64]) -> u64 {
    values.iter().sum()
}

pub fn panicking() -> bool {
    std::thread::panicking()
}
