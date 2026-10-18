//! The snapshot codec (Architectural Design, Persistence; decision log D-024, D-035).
//!
//! A snapshot is self-describing:
//!
//! ```text
//! bytes   = MAGIC ‖ header ‖ payload
//! MAGIC   = b"VFSNAP"
//! header  = canonical bytes of Header (format, rules version, engine build, game time, hash)
//! payload = canonical bytes of the World, to the end of the input
//! ```
//!
//! `state_hash` is BLAKE3-256 over the payload alone, so a world's hash never depends on the
//! build or format that wrote it. The `snapshots` row stores the whole envelope in `payload`, and
//! its other columns repeat the header (P2.5 checks they agree).
//!
//! Decoding goes through the World's own serde boundary, so `check_links` always runs (D-022),
//! and accepts only canonical bytes: re-encoding the decoded World must give the payload back.

use serde::{Deserialize, Serialize};
use voidforge_core::state::World;

use crate::canonical::{self, CanonicalError};

/// The first bytes of every snapshot.
pub const MAGIC: &[u8; 6] = b"VFSNAP";

/// The codec format this build writes and reads.
pub const FORMAT: u16 = 1;

/// The running build's identity, `"<workspace version>+<git commit>"`, set at build time
/// (`build.rs`). It is "+unknown" when neither `VOIDFORGE_GIT_SHA` nor git was available.
pub const ENGINE_BUILD: &str = env!("VOIDFORGE_ENGINE_BUILD");

/// The longest engine build identity.
pub const ENGINE_BUILD_MAX: usize = 64;

/// The identity of the engine build that wrote a snapshot: 1 to 64 bytes of printable ASCII,
/// without spaces. Recovery uses only snapshots of its own build; replays never cross builds.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EngineBuild(String);

impl EngineBuild {
    /// A build identity.
    ///
    /// # Errors
    /// [`SnapshotError::InvalidBuild`] if it is empty, longer than [`ENGINE_BUILD_MAX`] bytes, or
    /// holds anything but printable ASCII without spaces.
    pub fn new(id: impl Into<String>) -> Result<Self, SnapshotError> {
        let id = id.into();
        let valid = !id.is_empty()
            && id.len() <= ENGINE_BUILD_MAX
            && id.bytes().all(|b| b.is_ascii_graphic());
        if valid {
            Ok(Self(id))
        } else {
            Err(SnapshotError::InvalidBuild(id))
        }
    }

    /// This binary's build, [`ENGINE_BUILD`].
    ///
    /// # Panics
    /// Never: `build.rs` only produces valid identities.
    #[must_use]
    pub fn current() -> Self {
        Self::new(ENGINE_BUILD).expect("build.rs writes a valid engine build")
    }

    /// The identity as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A snapshot's header. Its fields map to the `snapshots` columns of the same names; the row's
/// `seq` comes from the writer, because a World does not know its log position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// The codec format, [`FORMAT`].
    pub format: u16,
    /// The rules version in force; equals the World's.
    pub rules_version: u32,
    /// The build that wrote it.
    pub engine_build: String,
    /// World time in milliseconds; equals the World's.
    pub game_time_ms: i64,
    /// BLAKE3-256 of the payload: the World's canonical bytes.
    pub state_hash: [u8; 32],
}

/// An encoded snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The header, as written into `bytes`.
    pub header: Header,
    /// The whole snapshot: magic, header and payload.
    pub bytes: Vec<u8>,
    payload_at: usize,
}

impl Snapshot {
    /// The payload: the World's canonical bytes, which `header.state_hash` covers.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.bytes[self.payload_at..]
    }
}

/// A decoded snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// The header, checked against the World and the payload.
    pub header: Header,
    /// The World, through its serde boundary (`check_links` included).
    pub world: World,
}

/// What a reader requires of a snapshot: its rules version and build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expected<'a> {
    /// The rules version the reader runs.
    pub rules_version: u32,
    /// The build the reader is.
    pub engine_build: &'a EngineBuild,
}

/// Why a snapshot could not be encoded or decoded. Corrupt input is always one of these, never a
/// panic.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    /// A poisoned world may be mid-instant and is never written (D-023).
    #[error("a poisoned world cannot be snapshotted")]
    Poisoned,
    /// The canonical encoder refused the value.
    #[error(transparent)]
    Encode(#[from] CanonicalError),
    /// An engine build identity that is empty, too long or not printable ASCII.
    #[error("invalid engine build identity {0:?}")]
    InvalidBuild(String),
    /// The input ended inside the magic or the header.
    #[error("the snapshot is truncated in its {0}")]
    Truncated(&'static str),
    /// The input does not start with [`MAGIC`].
    #[error("not a snapshot: bad magic")]
    BadMagic,
    /// The header does not decode, or names an invalid engine build.
    #[error("the snapshot header is malformed: {0}")]
    BadHeader(String),
    /// A format this build does not read.
    #[error("unknown snapshot format {0} (this build reads {FORMAT})")]
    UnknownFormat(u16),
    /// The snapshot's rules version is not the reader's.
    #[error("the snapshot is for rules version {found}, expected {expected}")]
    RulesMismatch {
        /// The reader's.
        expected: u32,
        /// The snapshot's.
        found: u32,
    },
    /// The snapshot was written by another build.
    #[error("the snapshot was written by build {found}, expected {expected}")]
    BuildMismatch {
        /// The reader's.
        expected: String,
        /// The snapshot's.
        found: String,
    },
    /// The payload's BLAKE3 hash is not the header's.
    #[error("the payload does not match the state hash")]
    HashMismatch,
    /// The payload is not a well-formed encoding of a World.
    #[error("the payload is not a World: {0}")]
    BadPayload(postcard::Error),
    /// The payload decodes, but the World refuses it: `check_links` or a field's own validation.
    #[error("the payload decodes but the World refuses it (check_links or a field check)")]
    Rejected,
    /// Bytes follow the World.
    #[error("{0} bytes follow the World")]
    TrailingBytes(usize),
    /// The header disagrees with the World it describes.
    #[error("the header's {0} differs from the World's")]
    HeaderWorldMismatch(&'static str),
    /// The payload is not the canonical encoding of the World it decodes to.
    #[error("the payload is not the World's canonical encoding")]
    NonCanonical,
    /// The header is not the canonical encoding of the header it decodes to (for example an
    /// overlong integer), so the same header could be written two ways (P2.2 review LOW 1).
    #[error("the header is not its canonical encoding")]
    NonCanonicalHeader,
}

/// BLAKE3-256 of a World's canonical bytes: the state hash (D-024).
#[must_use]
pub fn hash_payload(payload: &[u8]) -> [u8; 32] {
    blake3::hash(payload).into()
}

/// The state hash of a World: BLAKE3-256 of its canonical bytes.
///
/// # Errors
/// [`SnapshotError::Poisoned`] for a poisoned world.
pub fn state_hash(world: &World) -> Result<[u8; 32], SnapshotError> {
    Ok(hash_payload(&world_bytes(world)?))
}

fn world_bytes(world: &World) -> Result<Vec<u8>, SnapshotError> {
    if world.is_poisoned() {
        return Err(SnapshotError::Poisoned);
    }
    Ok(canonical::to_bytes(world)?)
}

/// Encodes a World as a snapshot written by `build`.
///
/// # Errors
/// [`SnapshotError::Poisoned`] for a poisoned world; [`SnapshotError::Encode`] if the encoder
/// refuses anything else (not expected).
pub fn encode(world: &World, build: &EngineBuild) -> Result<Snapshot, SnapshotError> {
    let payload = world_bytes(world)?;
    let header = Header {
        format: FORMAT,
        rules_version: world.rules_version,
        engine_build: build.as_str().to_owned(),
        game_time_ms: world.time_ms,
        state_hash: hash_payload(&payload),
    };
    let head = canonical::to_bytes(&header)?;
    let mut bytes = Vec::with_capacity(MAGIC.len() + head.len() + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&head);
    let payload_at = bytes.len();
    bytes.extend_from_slice(&payload);
    Ok(Snapshot {
        header,
        bytes,
        payload_at,
    })
}

/// Decodes a snapshot, requiring the reader's rules version and build.
///
/// # Errors
/// Every [`SnapshotError`] decode variant, checked in this order: the magic, the header, the
/// format, the rules version and build, the hash, the World (with `check_links`), trailing
/// bytes, the header against the World, and the canonical re-encoding.
pub fn decode(bytes: &[u8], expect: &Expected<'_>) -> Result<Decoded, SnapshotError> {
    decode_with(bytes, Some(expect))
}

/// Decodes a snapshot of any rules version and build, **for inspection only** (tools that report
/// what a snapshot is). Its header's rules version and engine build are **not verified** against
/// any expectation, so never restore from it: recovery uses [`decode`] and cross-checks the
/// `snapshots` row's columns against the header (P2.2 review LOW 2; D-038). Every other check of
/// [`decode`] still applies.
///
/// # Errors
/// As [`decode`], without the rules and build mismatches.
pub fn decode_any(bytes: &[u8]) -> Result<Decoded, SnapshotError> {
    decode_with(bytes, None)
}

fn decode_with(bytes: &[u8], expect: Option<&Expected<'_>>) -> Result<Decoded, SnapshotError> {
    let rest = match bytes.strip_prefix(MAGIC.as_slice()) {
        Some(rest) => rest,
        None if MAGIC.starts_with(bytes) => return Err(SnapshotError::Truncated("magic")),
        None => return Err(SnapshotError::BadMagic),
    };
    let (header, payload) = match postcard::take_from_bytes::<Header>(rest) {
        Ok(parts) => parts,
        Err(postcard::Error::DeserializeUnexpectedEnd) => {
            return Err(SnapshotError::Truncated("header"));
        }
        Err(e) => return Err(SnapshotError::BadHeader(e.to_string())),
    };
    if header.format != FORMAT {
        return Err(SnapshotError::UnknownFormat(header.format));
    }
    // The header must be its own canonical encoding, as the payload must be.
    let header_len = rest.len() - payload.len();
    if canonical::to_bytes(&header)? != rest[..header_len] {
        return Err(SnapshotError::NonCanonicalHeader);
    }
    let build = EngineBuild::new(header.engine_build.clone())
        .map_err(|_| SnapshotError::BadHeader("invalid engine build".to_owned()))?;
    if let Some(expect) = expect {
        if header.rules_version != expect.rules_version {
            return Err(SnapshotError::RulesMismatch {
                expected: expect.rules_version,
                found: header.rules_version,
            });
        }
        if &build != expect.engine_build {
            return Err(SnapshotError::BuildMismatch {
                expected: expect.engine_build.as_str().to_owned(),
                found: header.engine_build,
            });
        }
    }
    // Corrupt bytes stop here, before they reach serde.
    if hash_payload(payload) != header.state_hash {
        return Err(SnapshotError::HashMismatch);
    }
    let (world, trailing) = match postcard::take_from_bytes::<World>(payload) {
        Ok(parts) => parts,
        Err(postcard::Error::SerdeDeCustom) => return Err(SnapshotError::Rejected),
        Err(e) => return Err(SnapshotError::BadPayload(e)),
    };
    if !trailing.is_empty() {
        return Err(SnapshotError::TrailingBytes(trailing.len()));
    }
    if world.rules_version != header.rules_version {
        return Err(SnapshotError::HeaderWorldMismatch("rules version"));
    }
    if world.time_ms != header.game_time_ms {
        return Err(SnapshotError::HeaderWorldMismatch("game time"));
    }
    if world_bytes(&world)? != payload {
        return Err(SnapshotError::NonCanonical);
    }
    Ok(Decoded { header, world })
}
