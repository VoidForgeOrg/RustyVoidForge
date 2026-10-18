//! P2.2: the snapshot codec's contract (decision log D-035). Round trips on harness worlds, the
//! layout and the hash, and every way decoding refuses: each refusal is a typed error, never a
//! panic, including truncation at every length and every single-byte flip.
//!
//! The determinism suite's side (the encoding guard, property round trips, the double runs) is in
//! `tests/determinism.rs`.

use serde::Serialize;
use voidforge_core::state::World;
use voidforge_store::canonical;
use voidforge_store::snapshot::{
    self, Decoded, ENGINE_BUILD, EngineBuild, Expected, FORMAT, Header, MAGIC, SnapshotError,
};
use voidforge_testkit::determinism::{Economy, canonical_bytes, run_economy};
use voidforge_testkit::{ALICE, BOB, WorldBuilder, busy_world, v0_rules};

const H: i64 = 3_600_000;

fn build() -> EngineBuild {
    EngineBuild::new("0.1.0+test").unwrap()
}

fn expect<'a>(world: &World, build: &'a EngineBuild) -> Expected<'a> {
    Expected {
        rules_version: world.rules_version,
        engine_build: build,
    }
}

/// Encode, decode and re-encode: the same bytes and hash, and the same World.
fn assert_round_trip(world: &World) -> snapshot::Snapshot {
    let b = build();
    let first = snapshot::encode(world, &b).unwrap();
    let Decoded {
        header,
        world: back,
    } = snapshot::decode(&first.bytes, &expect(world, &b)).unwrap();
    assert_eq!(&back, world, "the decoded World equals the original");
    assert_eq!(header, first.header);
    let second = snapshot::encode(&back, &b).unwrap();
    assert!(
        second.bytes == first.bytes,
        "re-encoding gives identical bytes"
    );
    assert_eq!(second.header.state_hash, first.header.state_hash);
    first
}

/// A snapshot assembled by hand: magic, an arbitrary header, an arbitrary payload.
fn assemble(header: &Header, payload: &[u8]) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend(canonical::to_bytes(header).unwrap());
    bytes.extend_from_slice(payload);
    bytes
}

/// A header that is right for `payload` (hash included) and claims `world`'s rules and time.
fn header_for(world: &World, payload: &[u8]) -> Header {
    Header {
        format: FORMAT,
        rules_version: world.rules_version,
        engine_build: build().as_str().to_owned(),
        game_time_ms: world.time_ms,
        state_hash: snapshot::hash_payload(payload),
    }
}

fn decode_err(bytes: &[u8], world: &World) -> SnapshotError {
    let b = build();
    snapshot::decode(bytes, &expect(world, &b)).expect_err("decoding must refuse")
}

// --- Round trips ---

#[test]
fn round_trip_the_busy_world() {
    assert_round_trip(&busy_world());
}

#[test]
fn round_trip_an_economy_mid_advance_with_fleets_and_commands() {
    let rules = v0_rules();
    let e = Economy {
        seed: 0x2_2001,
        planets: 4,
        fleets: true,
        commands: 165,
    };
    let mut seen = 0;
    let run = run_economy(&e, &rules, 100 * H, 25, |w| {
        assert_round_trip(w);
        seen += 1;
    })
    .unwrap();
    assert!(seen > 100, "observed {seen} intermediate worlds");
    assert!(!run.world.fleets.is_empty(), "fleets are in the world");
    assert!(
        run.outcomes.iter().any(|(_, o)| o.is_ok()),
        "commands were accepted"
    );
    assert_round_trip(&run.world);
}

#[test]
fn round_trip_a_generated_world_before_and_after_join_game() {
    // 100 players, as worldgen's own suite uses: under D-031's provisional option B the default
    // rules refuse small worlds (D-033), and 40 is not enough for every seed.
    let rules = v0_rules();
    let mut w = voidforge_worldgen::generate(0x2_2002, 100, &rules).unwrap();
    assert_round_trip(&w);
    let join = voidforge_core::command::Command {
        player: ALICE,
        action: voidforge_core::command::Action::JoinGame,
    };
    let applied = w.apply(&join, H, &rules).unwrap();
    assert!(applied.outcome.is_ok());
    assert_round_trip(&w);
    w.advance_to(100 * H, &rules).unwrap();
    assert_round_trip(&w);
}

#[test]
fn round_trip_an_empty_world_at_the_extremes_of_time() {
    for t in [i64::MIN, -1, 0, i64::MAX] {
        assert_round_trip(&World::new(t, u32::MAX, u64::MAX));
    }
}

// --- Layout, header and hash ---

#[test]
fn the_layout_is_magic_header_payload() {
    let w = busy_world();
    let s = snapshot::encode(&w, &build()).unwrap();
    let payload = canonical_bytes(&w);
    assert!(
        s.payload() == payload.as_slice(),
        "the payload is the canonical World bytes"
    );
    assert!(s.bytes == assemble(&s.header, &payload));
    assert_eq!(
        s.header,
        Header {
            format: 1,
            rules_version: w.rules_version,
            engine_build: "0.1.0+test".to_owned(),
            game_time_ms: w.time_ms,
            state_hash: blake3::hash(&payload).into(),
        }
    );
}

#[test]
fn the_hash_covers_the_world_payload_only() {
    let w = busy_world();
    let a = snapshot::encode(&w, &EngineBuild::new("0.1.0+aaaa").unwrap()).unwrap();
    let b = snapshot::encode(&w, &EngineBuild::new("0.1.0+bbbbbbbb").unwrap()).unwrap();
    assert!(a.bytes != b.bytes, "the headers differ");
    assert_eq!(
        a.header.state_hash, b.header.state_hash,
        "the same world, the same hash"
    );
    assert!(a.payload() == b.payload());
    assert_eq!(snapshot::state_hash(&w).unwrap(), a.header.state_hash);
    // A different world has a different hash.
    let mut later = w.clone();
    later.advance_to(w.time_ms + H, &v0_rules()).unwrap();
    assert_ne!(snapshot::state_hash(&later).unwrap(), a.header.state_hash);
}

#[test]
fn a_poisoned_world_is_never_snapshotted() {
    // A demolition ending after the end of time poisons (the commands suite's case).
    let rules = v0_rules();
    let mut b = WorldBuilder::new().at(i64::MAX - 1);
    let home = b.player(ALICE);
    let mut w = b.build();
    let drill = *w
        .buildings
        .iter()
        .find(|(_, b)| b.planet == home)
        .map(|(id, _)| id)
        .unwrap();
    let demolish = voidforge_core::command::Command {
        player: ALICE,
        action: voidforge_core::command::Action::DemolishBuilding { building: drill },
    };
    assert!(w.apply(&demolish, i64::MAX - 1, &rules).is_err());
    assert!(w.is_poisoned());
    assert_eq!(snapshot::encode(&w, &build()), Err(SnapshotError::Poisoned));
    assert_eq!(snapshot::state_hash(&w), Err(SnapshotError::Poisoned));
}

#[test]
fn engine_build_identities_are_validated() {
    for ok in ["0.1.0+0d1aa53a8031", "x", &"a".repeat(64)] {
        assert_eq!(EngineBuild::new(ok).unwrap().as_str(), ok);
    }
    for bad in [
        "",
        " ",
        "0.1.0 +x",
        "0.1.0+\u{e9}",
        "tab\there",
        &"a".repeat(65),
    ] {
        assert_eq!(
            EngineBuild::new(bad),
            Err(SnapshotError::InvalidBuild(bad.to_owned())),
            "{bad:?}"
        );
    }
    // The running build: "<workspace version>+<commit or unknown>".
    let current = EngineBuild::current();
    assert_eq!(current.as_str(), ENGINE_BUILD);
    let (version, commit) = ENGINE_BUILD.split_once('+').unwrap();
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
    assert!(commit == "unknown" || commit.bytes().all(|b| b.is_ascii_alphanumeric()));
}

// --- Every refusal, by variant ---

#[test]
fn decode_refuses_bad_magic_and_truncated_magic() {
    let w = busy_world();
    let mut bytes = snapshot::encode(&w, &build()).unwrap().bytes;
    for len in 0..MAGIC.len() {
        assert_eq!(
            decode_err(&bytes[..len], &w),
            SnapshotError::Truncated("magic"),
            "{len}"
        );
    }
    bytes[0] = b'X';
    assert_eq!(decode_err(&bytes, &w), SnapshotError::BadMagic);
    assert_eq!(
        decode_err(b"PK\x03\x04 not a snapshot", &w),
        SnapshotError::BadMagic
    );
}

#[test]
fn decode_refuses_a_truncated_or_malformed_header() {
    let w = busy_world();
    let bytes = snapshot::encode(&w, &build()).unwrap().bytes;
    assert_eq!(
        decode_err(&bytes[..MAGIC.len()], &w),
        SnapshotError::Truncated("header")
    );
    assert_eq!(
        decode_err(&bytes[..MAGIC.len() + 3], &w),
        SnapshotError::Truncated("header")
    );
    // An engine build that is not printable ASCII: the header parses, the identity is refused.
    let payload = canonical_bytes(&w);
    let mut header = header_for(&w, &payload);
    header.engine_build = "bad build".to_owned();
    assert!(matches!(
        decode_err(&assemble(&header, &payload), &w),
        SnapshotError::BadHeader(_)
    ));
    // Invalid UTF-8 inside the engine build string.
    let mut raw = MAGIC.to_vec();
    raw.extend(canonical::to_bytes(&(FORMAT, w.rules_version)).unwrap());
    raw.extend([2, 0xff, 0xfe]);
    assert!(matches!(decode_err(&raw, &w), SnapshotError::BadHeader(_)));
}

#[test]
fn decode_refuses_an_unknown_format() {
    let w = busy_world();
    let payload = canonical_bytes(&w);
    for format in [0, 2, u16::MAX] {
        let header = Header {
            format,
            ..header_for(&w, &payload)
        };
        assert_eq!(
            decode_err(&assemble(&header, &payload), &w),
            SnapshotError::UnknownFormat(format)
        );
    }
}

#[test]
fn decode_refuses_another_rules_version_or_build() {
    let w = busy_world();
    let b = build();
    let s = snapshot::encode(&w, &b).unwrap();
    let other_rules = Expected {
        rules_version: w.rules_version + 1,
        engine_build: &b,
    };
    assert_eq!(
        snapshot::decode(&s.bytes, &other_rules),
        Err(SnapshotError::RulesMismatch {
            expected: w.rules_version + 1,
            found: w.rules_version
        })
    );
    let other = EngineBuild::new("0.1.0+other").unwrap();
    assert_eq!(
        snapshot::decode(&s.bytes, &expect(&w, &other)),
        Err(SnapshotError::BuildMismatch {
            expected: "0.1.0+other".to_owned(),
            found: "0.1.0+test".to_owned()
        })
    );
    // decode_any reads it anyway, for tools that report what a snapshot is.
    assert_eq!(snapshot::decode_any(&s.bytes).unwrap().world, w);
}

#[test]
fn decode_refuses_a_payload_that_does_not_match_the_hash() {
    let w = busy_world();
    let mut bytes = snapshot::encode(&w, &build()).unwrap().bytes;
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    assert_eq!(decode_err(&bytes, &w), SnapshotError::HashMismatch);
}

#[test]
fn decode_refuses_a_payload_that_is_not_a_world() {
    let w = busy_world();
    // Hashed garbage: the hash matches, the bytes are no World.
    let payload = vec![0xff; 40];
    assert!(matches!(
        decode_err(&assemble(&header_for(&w, &payload), &payload), &w),
        SnapshotError::BadPayload(_)
    ));
    // A World cut short, rehashed.
    let full = canonical_bytes(&w);
    let cut = &full[..full.len() / 2];
    assert_eq!(
        decode_err(&assemble(&header_for(&w, cut), cut), &w),
        SnapshotError::BadPayload(postcard::Error::DeserializeUnexpectedEnd)
    );
}

#[test]
fn decode_goes_through_check_links() {
    // A World with a broken link, encoded and hashed honestly: only check_links can refuse it.
    let mut w = busy_world();
    let (&building, _) = w.buildings.iter().next().unwrap();
    let planet = w.buildings[&building].planet;
    w.planets.remove(&planet);
    let payload = canonical_bytes(&w);
    assert_eq!(
        decode_err(&assemble(&header_for(&w, &payload), &payload), &w),
        SnapshotError::Rejected
    );
}

#[test]
fn decode_refuses_trailing_bytes() {
    let w = busy_world();
    let mut payload = canonical_bytes(&w);
    payload.extend([0, 0, 0]);
    assert_eq!(
        decode_err(&assemble(&header_for(&w, &payload), &payload), &w),
        SnapshotError::TrailingBytes(3)
    );
}

#[test]
fn decode_refuses_a_header_that_disagrees_with_its_world() {
    let w = busy_world();
    let payload = canonical_bytes(&w);
    let time = Header {
        game_time_ms: w.time_ms + 1,
        ..header_for(&w, &payload)
    };
    assert_eq!(
        decode_err(&assemble(&time, &payload), &w),
        SnapshotError::HeaderWorldMismatch("game time")
    );
    // The header claims another rules version, and the reader expects that one.
    let rules = Header {
        rules_version: w.rules_version + 1,
        ..header_for(&w, &payload)
    };
    let b = build();
    let reader = Expected {
        rules_version: w.rules_version + 1,
        engine_build: &b,
    };
    assert_eq!(
        snapshot::decode(&assemble(&rules, &payload), &reader),
        Err(SnapshotError::HeaderWorldMismatch("rules version"))
    );
}

/// The World's public fields in serialization order: postcard writes a struct as its fields in
/// order and a map as its length and pairs, so a tuple with the players as a list of pairs writes
/// the same bytes, and lets a test choose their order.
#[derive(Serialize)]
struct PublicPrefix<'a, P: Serialize> {
    time_ms: i64,
    rules_version: u32,
    seed: u64,
    next_id: u64,
    players: P,
    planets: &'a std::collections::BTreeMap<
        voidforge_core::state::PlanetId,
        voidforge_core::state::Planet,
    >,
    buildings: &'a std::collections::BTreeMap<
        voidforge_core::state::BuildingId,
        voidforge_core::state::Building,
    >,
    ship_builds: &'a std::collections::BTreeMap<
        voidforge_core::state::ShipBuildId,
        voidforge_core::state::ShipBuild,
    >,
    ships:
        &'a std::collections::BTreeMap<voidforge_core::state::ShipId, voidforge_core::state::Ship>,
    fleets: &'a std::collections::BTreeMap<
        voidforge_core::state::FleetId,
        voidforge_core::state::Fleet,
    >,
    homeworld_candidates: &'a Vec<voidforge_core::state::PlanetId>,
}

#[test]
fn decode_refuses_a_non_canonical_payload() {
    // Players out of key order: postcard and BTreeMap accept it and sort, so the World decodes,
    // links and all, but its canonical bytes differ. Only the re-encoding check refuses it.
    let w = busy_world();
    assert!(w.players.contains_key(&ALICE) && w.players.contains_key(&BOB));
    let prefix = |players: Vec<_>| {
        canonical::to_bytes(&PublicPrefix {
            time_ms: w.time_ms,
            rules_version: w.rules_version,
            seed: w.seed,
            next_id: w.next_id,
            players,
            planets: &w.planets,
            buildings: &w.buildings,
            ship_builds: &w.ship_builds,
            ships: &w.ships,
            fleets: &w.fleets,
            homeworld_candidates: &w.homeworld_candidates,
        })
        .unwrap()
    };
    let canonical_world = canonical_bytes(&w);
    let sorted: Vec<_> = w.players.iter().collect();
    let in_order = prefix(sorted.clone());
    assert!(
        canonical_world.starts_with(&in_order),
        "the prefix reproduces the World's bytes"
    );
    let queue = &canonical_world[in_order.len()..];
    let mut payload = prefix(sorted.into_iter().rev().collect());
    payload.extend_from_slice(queue);
    assert!(payload != canonical_world && payload.len() == canonical_world.len());
    assert_eq!(
        decode_err(&assemble(&header_for(&w, &payload), &payload), &w),
        SnapshotError::NonCanonical
    );
}

// --- Corruption: every length, every byte ---

#[test]
fn every_truncation_is_a_typed_error() {
    let w = busy_world();
    let bytes = snapshot::encode(&w, &build()).unwrap().bytes;
    for len in 0..bytes.len() {
        let err = decode_err(&bytes[..len], &w);
        assert!(
            matches!(
                err,
                SnapshotError::Truncated(_) | SnapshotError::HashMismatch
            ),
            "length {len}: {err:?}"
        );
    }
}

#[test]
fn every_single_byte_flip_is_a_typed_error() {
    let w = busy_world();
    let bytes = snapshot::encode(&w, &build()).unwrap().bytes;
    for i in 0..bytes.len() {
        for mask in [0x01, 0x80, 0xff] {
            let mut flipped = bytes.clone();
            flipped[i] ^= mask;
            // A panic would fail the test; every flip must be refused.
            let _ = decode_err(&flipped, &w);
        }
    }
}

#[test]
fn decode_refuses_a_non_canonical_header() {
    // The header's first field, format 1, written as an overlong varint (0x81 0x00 instead of
    // 0x01). postcard accepts it and decodes the same header, so only the re-encoding check can
    // refuse it (P2.2 review LOW 1; D-038). The payload and hash are untouched.
    let w = busy_world();
    let s = snapshot::encode(&w, &build()).unwrap();
    let mut bytes = s.bytes.clone();
    assert_eq!(bytes[MAGIC.len()], 0x01, "format 1 is one varint byte");
    bytes.splice(MAGIC.len()..=MAGIC.len(), [0x81, 0x00]);
    assert_eq!(decode_err(&bytes, &w), SnapshotError::NonCanonicalHeader);
    // decode_any refuses it too: only the build and rules expectations are skipped there.
    assert_eq!(
        snapshot::decode_any(&bytes),
        Err(SnapshotError::NonCanonicalHeader)
    );
}
