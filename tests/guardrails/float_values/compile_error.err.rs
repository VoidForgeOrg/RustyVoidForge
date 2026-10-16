// Trap: a crate that does not compile must make the float check exit 2, never pass.
pub fn broken() -> u64 {
    does_not_exist()
}
