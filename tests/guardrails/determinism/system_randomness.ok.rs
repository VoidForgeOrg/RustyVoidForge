// Control for system_randomness.rs: a generator seeded from the universe seed is allowed. It uses
// worldgen's own pin (D-031), which the harness keeps in the crate that already declares it.
// dependency: rand_chacha = { version = "=0.9.0", default-features = false }
use rand_chacha::rand_core::{RngCore, SeedableRng};

pub fn first_roll(universe_seed: u64) -> u64 {
    rand_chacha::ChaCha8Rng::seed_from_u64(universe_seed).next_u64()
}
