//! The determinism suite's snapshot round trip (Technical Plan, CI/CD: "Double run, observation
//! invariance, golden replays and snapshot round trip"; decision log D-024, D-035).
//!
//! testkit may not depend on store (Project layering), so this binary drives testkit's harnesses
//! (`Scenario`, `Economy`, the busy world) and worldgen's generated worlds from the store side,
//! as worldgen's own `determinism` binary does. `just determinism` and the CI job run it, and run
//! [`two_process_double_run`] in two processes.
//!
//! | Check | Tests |
//! | --- | --- |
//! | One encoding: store's bytes are the suite's bytes | `canonical_encoding_is_the_determinism_suites` |
//! | The state hash is BLAKE3 over those bytes (D-024) | `state_hash_is_blake3_of_the_determinism_canonical_bytes` |
//! | Encode, decode, re-encode identical | `snapshot_round_trip_*` |
//! | Total order, through snapshots | `double_run_in_process`, `two_process_double_run` |

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use serde::Serialize;
use voidforge_core::quantity::MS_PER_HOUR;
use voidforge_core::rules::Rules;
use voidforge_core::state::World;
use voidforge_store::canonical;
use voidforge_store::snapshot::{self, EngineBuild, Expected};
use voidforge_testkit::determinism::{
    Economy, Scenario, canonical_bytes, run_bytes, run_economy, run_straight, slow_build_rules,
};
use voidforge_testkit::{busy_world, v0_rules};

/// The architecture's scale: 100 hours.
const SPAN_MS: i64 = 100 * MS_PER_HOUR;
/// Commands per generated script, as the testkit suite uses.
const COMMANDS: u64 = 165;
/// The shortest gap between a scenario's events: 10 minutes.
const MIN_GAP_MS: i64 = 10 * 60_000;

fn config(cases: u32, seed: u64) -> Config {
    Config {
        cases,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        ..Config::default()
    }
}

/// A fixed build, so the snapshot bytes never depend on git state.
fn build() -> EngineBuild {
    EngineBuild::new("0.1.0+determinism").unwrap()
}

/// Encode, decode and re-encode: identical bytes, the same World, and the D-024 hash.
fn round_trip(world: &World) -> Vec<u8> {
    let b = build();
    let s = snapshot::encode(world, &b).unwrap();
    let expect = Expected {
        rules_version: world.rules_version,
        engine_build: &b,
    };
    let back = snapshot::decode(&s.bytes, &expect).unwrap().world;
    assert!(&back == world, "the decoded World equals the original");
    let again = snapshot::encode(&back, &b).unwrap();
    assert!(again.bytes == s.bytes, "re-encoding gives identical bytes");
    assert_eq!(
        s.header.state_hash,
        <[u8; 32]>::from(blake3::hash(&canonical_bytes(world)))
    );
    s.bytes
}

/// The guard: store's canonical bytes equal the suite's.
fn same_bytes<T: Serialize + ?Sized>(what: &str, value: &T) {
    assert!(
        canonical::to_bytes(value).unwrap() == canonical_bytes(value),
        "store and testkit encode {what} differently"
    );
}

fn scenarios() -> impl Iterator<Item = Scenario> {
    (0..8u64).map(|i| Scenario {
        seed: 0x2_2100 + i,
        planets: 1 + i % 6,
        fleets: i % 5,
        max_gap_ms: MIN_GAP_MS + (i as i64 * 997_331) % (2 * MS_PER_HOUR),
        horizon_ms: SPAN_MS - MS_PER_HOUR,
    })
}

/// The suite's four economy kinds: slow builds, solved planets, fleets, and commands.
fn economies() -> Vec<(Economy, Rules)> {
    (0..2u64)
        .flat_map(|i| {
            [
                (
                    Economy {
                        seed: 0x2_2200 + i,
                        planets: 1 + i,
                        fleets: false,
                        commands: 0,
                    },
                    slow_build_rules(),
                ),
                (
                    Economy {
                        seed: 0x2_2300 + i,
                        planets: 2 + i,
                        fleets: false,
                        commands: 0,
                    },
                    v0_rules(),
                ),
                (
                    Economy {
                        seed: 0x2_2400 + i,
                        planets: 2 + i,
                        fleets: true,
                        commands: 0,
                    },
                    v0_rules(),
                ),
                (
                    Economy {
                        seed: 0x2_2500 + i,
                        planets: 2 + i,
                        fleets: i == 0,
                        commands: COMMANDS,
                    },
                    v0_rules(),
                ),
            ]
        })
        .collect()
}

#[test]
fn canonical_encoding_is_the_determinism_suites() {
    same_bytes("the busy world", &busy_world());
    for s in scenarios() {
        same_bytes("a scenario's start", &s.world());
        let (w, events) = run_straight(&s, SPAN_MS).unwrap();
        same_bytes("a scenario's end", &w);
        same_bytes("a scenario's steps", &events);
        same_bytes("a scenario run", &(&w, &events));
        assert!(canonical::to_bytes(&(&w, &events)).unwrap() == run_bytes(&w, &events));
    }
    for (e, rules) in economies() {
        same_bytes("an economy's start", &e.world(&rules));
        let run = run_economy(&e, &rules, SPAN_MS, 10, |w| {
            same_bytes("a world mid-run", w)
        })
        .unwrap();
        same_bytes("an economy's end", &run.world);
        same_bytes("an economy's events", &run.events);
        same_bytes("an economy's commands and outcomes", &run.outcomes);
        same_bytes("an economy run", &run);
    }
    let generated = voidforge_worldgen::generate(0x2_2600, 100, &v0_rules()).unwrap();
    same_bytes("a generated world", &generated);
}

#[test]
fn state_hash_is_blake3_of_the_determinism_canonical_bytes() {
    // D-024's promise, against blake3 called directly on the suite's own bytes.
    let worlds = [busy_world(), scenarios().next().unwrap().world()];
    for w in &worlds {
        let direct: [u8; 32] = blake3::hash(&canonical_bytes(w)).into();
        assert_eq!(snapshot::state_hash(w).unwrap(), direct);
        assert_eq!(
            snapshot::encode(w, &build()).unwrap().header.state_hash,
            direct
        );
    }
}

/// Busy scenarios, as the testkit suite draws them.
fn scenario() -> impl Strategy<Value = Scenario> {
    (
        any::<u64>(),
        1u64..=6,
        0u64..=6,
        MIN_GAP_MS..=2 * MS_PER_HOUR,
    )
        .prop_map(|(seed, planets, fleets, max_gap_ms)| Scenario {
            seed,
            planets,
            fleets,
            max_gap_ms,
            horizon_ms: SPAN_MS - MS_PER_HOUR,
        })
}

proptest! {
    #![proptest_config(config(48, 0x2_2701))]

    #[test]
    fn snapshot_round_trip_on_scenarios_at_any_instant(s in scenario(), cut in 0i64..SPAN_MS) {
        round_trip(&s.world());
        let (mid, _) = run_straight(&s, cut).unwrap();
        round_trip(&mid);
        let (end, _) = run_straight(&s, SPAN_MS).unwrap();
        round_trip(&end);
    }
}

proptest! {
    #![proptest_config(config(12, 0x2_2702))]

    #[test]
    fn snapshot_round_trip_on_economies_at_every_stop(
        seed in any::<u64>(),
        planets in 2u64..=5,
        fleets in any::<bool>(),
        commands in prop_oneof![Just(0u64), Just(COMMANDS)],
    ) {
        let rules = v0_rules();
        let e = Economy { seed, planets, fleets, commands };
        let run = run_economy(&e, &rules, SPAN_MS, 20, |w| {
            round_trip(w);
        })
        .unwrap();
        round_trip(&run.world);
    }
}

#[test]
fn snapshot_round_trip_on_generated_worlds() {
    for seed in [0, 0x2_2800, u64::MAX] {
        round_trip(&voidforge_worldgen::generate(seed, 100, &v0_rules()).unwrap());
    }
}

/// The snapshots of fixed runs: each scenario's and economy's end, and every economy's world at
/// each of its stops.
fn recorded_snapshots() -> Vec<u8> {
    let mut all: Vec<Vec<u8>> = Vec::new();
    for s in scenarios() {
        all.push(round_trip(&run_straight(&s, SPAN_MS).unwrap().0));
    }
    for (e, rules) in economies() {
        let run = run_economy(&e, &rules, SPAN_MS, 10, |w| all.push(round_trip(w))).unwrap();
        all.push(round_trip(&run.world));
    }
    canonical_bytes(&all)
}

#[test]
fn double_run_in_process() {
    let a = recorded_snapshots();
    let b = recorded_snapshots();
    assert!(a == b, "two runs gave different snapshots");
}

/// The two-process double run. `.github/scripts/determinism.sh` runs this twice, as two
/// processes: `VOIDFORGE_DOUBLE_RUN=write` saves the snapshots to `VOIDFORGE_DOUBLE_RUN_FILE`,
/// then `compare` runs again and requires identical bytes. A missing file fails.
#[test]
#[ignore = "run by .github/scripts/determinism.sh in two processes"]
fn two_process_double_run() {
    let mode =
        std::env::var("VOIDFORGE_DOUBLE_RUN").expect("VOIDFORGE_DOUBLE_RUN is write or compare");
    let file =
        std::env::var("VOIDFORGE_DOUBLE_RUN_FILE").expect("VOIDFORGE_DOUBLE_RUN_FILE is set");
    let bytes = recorded_snapshots();
    match mode.as_str() {
        "write" => std::fs::write(&file, &bytes).expect("the bytes are written"),
        "compare" => {
            let first = std::fs::read(&file)
                .unwrap_or_else(|e| panic!("the first process's bytes are missing at {file}: {e}"));
            assert_eq!(first.len(), bytes.len(), "the runs differ in length");
            assert!(
                first == bytes,
                "the two processes produced different snapshots"
            );
        }
        other => panic!("VOIDFORGE_DOUBLE_RUN must be write or compare, not {other}"),
    }
}
