// Determinism fixture: system randomness. Only seeded generators (rand_chacha from the universe seed).
// expect: clippy::disallowed_types std::hash::RandomState
// expect: clippy::disallowed_types getrandom::SysRng
// expect: clippy::disallowed_methods getrandom::fill
// expect: clippy::disallowed_methods getrandom::fill_uninit
// expect: clippy::disallowed_methods getrandom::u32
// expect: clippy::disallowed_methods getrandom::u64
// dependency: getrandom = { version = "=0.4.3", features = ["sys_rng"] }

pub fn random_state() -> std::hash::RandomState {
    std::hash::RandomState::new()
}

pub fn random_state_reexport() -> std::collections::hash_map::RandomState {
    std::collections::hash_map::RandomState::new()
}

pub fn sys_rng() -> getrandom::SysRng {
    getrandom::SysRng
}

pub fn fill(buf: &mut [u8]) -> bool {
    getrandom::fill(buf).is_ok()
}

pub fn fill_uninit(buf: &mut [std::mem::MaybeUninit<u8>]) -> bool {
    getrandom::fill_uninit(buf).is_ok()
}

pub fn random_u32() -> bool {
    getrandom::u32().is_ok()
}

pub fn random_u64() -> bool {
    getrandom::u64().is_ok()
}
