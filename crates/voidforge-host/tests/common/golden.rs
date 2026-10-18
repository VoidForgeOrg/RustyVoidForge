//! Golden logs (Technical Plan P2.6; decision log D-040): testkit's scenarios recorded through
//! the real host. The format and its verification are `voidforge_store::golden`'s, shared with the
//! replay tool (D-041); this file records and does the file IO.

use std::path::{Path, PathBuf};

use voidforge_core::command::Command;
use voidforge_core::state::World;
use voidforge_host::{HostConfig, ManualClock};
use voidforge_store::golden::{Checkpoint, FILES, FIXTURE, Files, Manifest, Score, fixture};
use voidforge_store::log::{MemoryLog, player_uuid};
use voidforge_store::replay;
use voidforge_store::snapshot;
use voidforge_store::verify::hex;
use voidforge_testkit::golden::{Expect, Scenario};

use super::{EPOCH, settle};

/// `tests/golden/` at the workspace root.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

pub fn read(dir: &Path) -> std::io::Result<Files> {
    Ok(Files {
        start: std::fs::read(dir.join("start.vfsnap"))?,
        records: std::fs::read(dir.join("records.jsonl"))?,
        manifest: std::fs::read(dir.join("manifest.json"))?,
    })
}

pub fn write(files: &Files, dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for name in FILES {
        std::fs::write(dir.join(name), files.get(name))?;
    }
    Ok(())
}

fn hash(world: &World) -> String {
    hex(&snapshot::state_hash(world).expect("a hashable World"))
}

/// Runs a scenario through the real host (a `MemoryLog`, a `ManualClock`) and returns its
/// golden files. Panics if a cue gets the wrong outcome, a retry is not idempotent, or the
/// recording does not replay to the host's own World.
pub fn record(s: &Scenario) -> Files {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (records, host_end) = runtime.block_on(async {
        let log = MemoryLog::new();
        let clock = ManualClock::new(EPOCH + s.world.time_ms);
        let (mut handle, task) = voidforge_host::spawn(
            s.world.clone(),
            s.rules.clone(),
            {
                let log = log.clone();
                move || log
            },
            clock.clone(),
            HostConfig::new(EPOCH, fixture()),
        )
        .unwrap();
        let mut last = s.world.time_ms;
        for (i, cue) in s.cues.iter().enumerate() {
            assert!(cue.at_ms >= last, "{}: cue {i} goes back in time", s.name);
            last = cue.at_ms;
            run_to(&mut handle, &clock, cue.at_ms).await;
            let command = Command {
                player: cue.player,
                action: (cue.action)(&handle.read().world),
            };
            let key = cue
                .key
                .map(|k| voidforge_store::log::IdempotencyKey::new(k).unwrap());
            let submit = |command: Command| {
                let (handle, key) = (handle.clone(), key.clone());
                async move {
                    match key {
                        Some(key) => handle.submit_with_key(command, key).await,
                        None => handle.submit(command).await,
                    }
                    .expect("acknowledged")
                }
            };
            let receipt = submit(command.clone()).await;
            match (cue.expect, &receipt.outcome) {
                (Expect::Accepted, Ok(_)) => {}
                (Expect::Rejected(code), Err(r)) if r.code() == code => {}
                (want, got) => panic!(
                    "{}: cue {i} ({command:?}) wanted {want:?}, got {:?}",
                    s.name,
                    got.as_ref().map_err(|r| r.code())
                ),
            }
            if cue.retry {
                let seq = handle.read().seq;
                assert_eq!(submit(command).await, receipt, "{}: cue {i} retry", s.name);
                assert_eq!(handle.read().seq, seq, "{}: cue {i} retry appended", s.name);
            }
        }
        assert!(s.end_ms >= last, "{}: the end is before a cue", s.name);
        run_to(&mut handle, &clock, s.end_ms).await;
        let end = (*handle.read().world).clone();
        drop(handle);
        task.join().unwrap();
        (log.snapshot(), end)
    });

    let rules = &s.rules;
    let build = fixture();
    let start = snapshot::encode(&s.world, &build).unwrap().bytes;
    let mut world = s.world.clone();
    let mut checkpoints = Vec::new();
    for r in &records {
        replay::replay_record(&mut world, r, rules, &build)
            .unwrap_or_else(|e| panic!("{}: the recording does not replay: {e}", s.name));
        checkpoints.push(Checkpoint {
            seq: r.seq,
            game_time: r.game_time,
            state_hash: hash(&world),
        });
    }
    // The end: the host advanced to the end without a record only if nothing fired (D-036), so
    // both Worlds advance there with no events and must be byte-identical.
    let mut host_end = host_end;
    for w in [&mut world, &mut host_end] {
        let events = w.advance_to(s.end_ms, rules).unwrap();
        assert!(events.is_empty(), "{}: unlogged events at the end", s.name);
    }
    assert_eq!(
        hash(&world),
        hash(&host_end),
        "{}: the replay differs from the host's World",
        s.name
    );
    let scores = world
        .players
        .keys()
        .map(|&p| Score {
            player: player_uuid(p),
            score: world.score(p, s.end_ms, rules).unwrap(),
        })
        .collect();
    let manifest = Manifest {
        scenario: s.name.to_owned(),
        about: s.about.to_owned(),
        rules_version: rules.version,
        engine_build: FIXTURE.to_owned(),
        start_hash: hash(&s.world),
        records: checkpoints,
        end_ms: s.end_ms,
        end_hash: hash(&world),
        scores,
    };
    let mut manifest = serde_json::to_string_pretty(&manifest).unwrap();
    manifest.push('\n');
    Files {
        start,
        records: Files::records_text(&records).unwrap(),
        manifest: manifest.into_bytes(),
    }
}

/// Moves the clock to `t` the way a live host's clock gets there: through each due event's own
/// time in turn, so each advance record is stamped when its events came due, as with the system
/// clock, rather than all at `t`.
async fn run_to(handle: &mut voidforge_host::HostHandle, clock: &ManualClock, t: i64) {
    loop {
        let next = handle.read().world.queue().first().map(|e| e.at_ms);
        match next {
            Some(at) if at < t => {
                clock.set(EPOCH + at);
                settle(handle, clock).await;
            }
            _ => break,
        }
    }
    clock.set(EPOCH + t);
    settle(handle, clock).await;
}

/// The goldens on disk, by directory name, sorted.
pub fn on_disk() -> Vec<(String, Files)> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root())
        .expect("tests/golden exists")
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|d| {
            let name = d.file_name().unwrap().to_string_lossy().into_owned();
            let files = read(&d).unwrap_or_else(|e| panic!("{name}: {e}"));
            (name, files)
        })
        .collect()
}
