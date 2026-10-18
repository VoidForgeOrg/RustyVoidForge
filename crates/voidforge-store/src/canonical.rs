//! The canonical encoding: postcard with its default configuration (decision log D-024, D-035).
//!
//! These are the bytes the state hash covers and the bytes a snapshot stores. testkit's
//! `determinism::canonical_bytes` makes the same call for the determinism suite. testkit may not
//! depend on store (Project layering), so store's `determinism` tests assert, on every harness
//! world, event sequence and command list, that the two give identical bytes. A non-default
//! postcard configuration (a flavor, a different varint form) must change both places together,
//! and the guard catches it if one is missed.

use serde::Serialize;

/// Encoding failed: postcard refused the value. For a `World`, only a poisoned world does that,
/// and [`crate::snapshot::encode`] reports it as `Poisoned` before calling this.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("canonical encoding failed: {0}")]
pub struct CanonicalError(pub postcard::Error);

/// The canonical bytes of a value: its postcard encoding, default configuration.
///
/// # Errors
/// [`CanonicalError`] if the value's `Serialize` implementation refuses, as a poisoned `World`
/// does (D-023).
pub fn to_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CanonicalError> {
    postcard::to_allocvec(value).map_err(CanonicalError)
}
