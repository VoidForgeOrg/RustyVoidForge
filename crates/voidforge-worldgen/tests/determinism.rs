//! World generation in the determinism suite (D-024, D-031): run by `just determinism` and the
//! CI job of that name, which also runs [`two_process_double_run`] in two separate processes.
//! Generated worlds are compared on canonical postcard bytes, the bytes P2.2's state hash covers.

use voidforge_core::command::{Action, Command};
use voidforge_core::quantity::MS_PER_HOUR;
use voidforge_core::rules::Rules;
use voidforge_core::state::World;
use voidforge_testkit::determinism::{CAROL, canonical_bytes};
use voidforge_testkit::{ALICE, BOB, v0_rules};
use voidforge_worldgen::generate;

/// A generated world, three players joined at time 0 (each at the next free candidate), then
/// played for 100 hours through the production engine: generation as a starting point.
fn played(seed: u64, players: u32, rules: &Rules) -> World {
    let mut w = generate(seed, players, rules).unwrap();
    for player in [ALICE, BOB, CAROL] {
        let join = Command {
            player,
            action: Action::JoinGame,
        };
        w.apply(&join, 0, rules).unwrap().outcome.unwrap();
    }
    w.advance_to(100 * MS_PER_HOUR, rules).unwrap();
    w.check_links().unwrap();
    w
}

#[test]
fn generation_double_run_in_process() {
    for seed in [0, 9, u64::MAX] {
        let a = canonical_bytes(&generate(seed, 100, &v0_rules()).unwrap());
        let b = canonical_bytes(&generate(seed, 100, &v0_rules()).unwrap());
        assert!(a == b, "seed {seed}");
    }
}

#[test]
fn a_generated_world_played_twice_gives_identical_bytes() {
    let a = canonical_bytes(&played(4, 7, &v0_rules()));
    let b = canonical_bytes(&played(4, 7, &v0_rules()));
    assert!(a == b);
}

/// The bytes the two-process run compares: fixed seeds and sizes, generated and played.
fn recorded() -> Vec<u8> {
    let runs: Vec<Vec<u8>> = [0u64, 1, u64::MAX]
        .iter()
        .map(|&seed| canonical_bytes(&generate(seed, 100, &v0_rules()).unwrap()))
        .chain(
            [5u64, 6]
                .iter()
                .map(|&seed| canonical_bytes(&played(seed, 7, &v0_rules()))),
        )
        .collect();
    canonical_bytes(&runs)
}

/// Run twice, as two processes, by `.github/scripts/determinism.sh`: `VOIDFORGE_DOUBLE_RUN=write`
/// saves the bytes to `VOIDFORGE_DOUBLE_RUN_FILE`, then `compare` requires identical bytes.
#[test]
#[ignore = "run in two processes by .github/scripts/determinism.sh"]
fn two_process_double_run() {
    let mode =
        std::env::var("VOIDFORGE_DOUBLE_RUN").expect("VOIDFORGE_DOUBLE_RUN is write or compare");
    let file =
        std::env::var("VOIDFORGE_DOUBLE_RUN_FILE").expect("VOIDFORGE_DOUBLE_RUN_FILE is set");
    let bytes = recorded();
    match mode.as_str() {
        "write" => std::fs::write(&file, &bytes).expect("the bytes are written"),
        "compare" => {
            let first = std::fs::read(&file)
                .unwrap_or_else(|e| panic!("the first process's bytes are missing at {file}: {e}"));
            assert!(
                first == bytes,
                "the second process produced different bytes"
            );
        }
        other => panic!("VOIDFORGE_DOUBLE_RUN must be write or compare, not {other}"),
    }
}
