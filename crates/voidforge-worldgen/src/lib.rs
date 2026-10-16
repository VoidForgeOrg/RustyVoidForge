//! Seeded world generation.

// Determinism guardrails (Technical Plan P0.4). Forbidden, so no item can opt out; the banned
// paths are in this crate's clippy.toml.
#![forbid(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::float_arithmetic
)]
