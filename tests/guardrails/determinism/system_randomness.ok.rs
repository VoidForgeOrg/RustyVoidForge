// Control for system_randomness.rs: a generator seeded from the universe seed is allowed.
// dependency: rand_chacha = "=0.10.0"
use rand_chacha::rand_core::{Rng, SeedableRng};

pub fn first_roll(universe_seed: u64) -> u64 {
    rand_chacha::ChaCha8Rng::seed_from_u64(universe_seed).next_u64()
}
