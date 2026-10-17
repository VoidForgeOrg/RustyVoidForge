//! Engine: state, solver, commands, events and views.

// Determinism guardrails (Technical Plan P0.4). Forbidden, so no item can opt out; the banned
// paths are in this crate's clippy.toml.
#![forbid(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::float_arithmetic
)]

pub mod quantity;
pub mod rules;
pub mod solver;
pub mod state;
