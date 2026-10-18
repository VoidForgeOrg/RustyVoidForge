//! Golden logs (Technical Plan P2.6; decision log D-040): testkit's scenarios recorded through
//! the real host, and the replay that checks a recording.
//!
//! A golden is a directory under `tests/golden/` with three files:
//! - `start.vfsnap`: the start World as a snapshot envelope (D-035), under the fixture build;
//! - `records.jsonl`: the host's log records, one per line ([`LogRecord::to_line`]);
//! - `manifest.json`: the scenario, the rules version, the fixture build, the start hash, the
//!   state hash after every record, the end time and hash, and every player's score at the end.
//!
//! Goldens are recorded and replayed under [`FIXTURE`], a fixed test-only build, so they replay
//! on every commit. Production recovery keeps the real build's rule (D-038).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use voidforge_core::command::Command;
use voidforge_core::rules::Rules;
use voidforge_core::state::{EngineError, World};
use voidforge_host::{HostConfig, ManualClock};
use voidforge_store::log::{LogError, LogRecord, MemoryLog, player_uuid};
use voidforge_store::replay::{self, ReplayError};
use voidforge_store::snapshot::{self, EngineBuild, Expected, SnapshotError};
use voidforge_testkit::golden::{Expect, Scenario};

use super::{EPOCH, settle};

/// The fixture build every golden is recorded and replayed under (D-040).
pub const FIXTURE: &str = "0.1.0+golden";

pub fn fixture() -> EngineBuild {
    EngineBuild::new(FIXTURE).unwrap()
}

/// `tests/golden/` at the workspace root.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

pub const FILES: [&str; 3] = ["start.vfsnap", "records.jsonl", "manifest.json"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub scenario: String,
    pub about: String,
    pub rules_version: u32,
    pub engine_build: String,
    pub start_hash: String,
    pub records: Vec<Checkpoint>,
    pub end_ms: i64,
    pub end_hash: String,
    pub scores: Vec<Score>,
}

/// The state hash after a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub seq: i64,
    pub game_time: i64,
    pub state_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Score {
    pub player: String,
    pub score: i64,
}

/// A golden's three files, as bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    pub start: Vec<u8>,
    pub records: Vec<u8>,
    pub manifest: Vec<u8>,
}

impl Files {
    pub fn get(&self, name: &str) -> &[u8] {
        match name {
            "start.vfsnap" => &self.start,
            "records.jsonl" => &self.records,
            "manifest.json" => &self.manifest,
            _ => panic!("no golden file {name}"),
        }
    }

    pub fn read(dir: &Path) -> std::io::Result<Self> {
        Ok(Self {
            start: std::fs::read(dir.join("start.vfsnap"))?,
            records: std::fs::read(dir.join("records.jsonl"))?,
            manifest: std::fs::read(dir.join("manifest.json"))?,
        })
    }

    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        for name in FILES {
            std::fs::write(dir.join(name), self.get(name))?;
        }
        Ok(())
    }

    pub fn manifest(&self) -> Manifest {
        serde_json::from_slice(&self.manifest).expect("a golden manifest")
    }

    pub fn records(&self) -> Vec<LogRecord> {
        parse_records(&self.records).expect("golden records")
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash(world: &World) -> String {
    hex(&snapshot::state_hash(world).expect("a hashable World"))
}

fn parse_records(bytes: &[u8]) -> Result<Vec<LogRecord>, Failure> {
    let text = std::str::from_utf8(bytes).map_err(|e| Failure::Unreadable(e.to_string()))?;
    text.lines()
        .map(|line| LogRecord::from_line(line).map_err(Failure::Line))
        .collect()
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
    let mut lines = String::new();
    for r in &records {
        lines.push_str(&r.to_line().unwrap());
        lines.push('\n');
    }
    let mut manifest = serde_json::to_string_pretty(&manifest).unwrap();
    manifest.push('\n');
    Files {
        start,
        records: lines.into_bytes(),
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

/// Why a golden does not replay: the first problem, naming the record where there is one.
#[derive(Debug)]
pub enum Failure {
    Unreadable(String),
    Line(LogError),
    Build(String),
    Start(SnapshotError),
    StartHash,
    /// The record count or seqs differ from the manifest's checkpoints.
    Checkpoints,
    /// Replaying record `seq` failed (a divergence, a gap, another build or rules version).
    Record(ReplayError),
    /// The state after record `seq` has another hash than recorded.
    Hash {
        seq: i64,
    },
    EndEvents,
    End(EngineError),
    EndHash,
    Scores,
}

impl Failure {
    /// The record the failure is at, if any.
    pub fn seq(&self) -> Option<i64> {
        match self {
            Self::Record(
                ReplayError::Diverged { seq, .. }
                | ReplayError::Engine { seq, .. }
                | ReplayError::CrossBuildRecord { seq, .. }
                | ReplayError::RulesMismatch { seq, .. }
                | ReplayError::Malformed { seq, .. }
                | ReplayError::Unsupported { seq, .. },
            ) => Some(*seq),
            Self::Record(ReplayError::Gap { found, .. }) => Some(*found),
            Self::Hash { seq } => Some(*seq),
            _ => None,
        }
    }
}

/// Replays a golden: the start decoded strictly under the fixture build and the rules, every
/// record through the shared function (`store::replay`, which checks its events and outcome),
/// the state hash after every record, the end, and the scores. Returns the World at the end.
pub fn verify(files: &Files, rules: &Rules) -> Result<World, Failure> {
    let manifest: Manifest =
        serde_json::from_slice(&files.manifest).map_err(|e| Failure::Unreadable(e.to_string()))?;
    if manifest.engine_build != FIXTURE {
        return Err(Failure::Build(manifest.engine_build));
    }
    let build = fixture();
    let decoded = snapshot::decode(
        &files.start,
        &Expected {
            rules_version: rules.version,
            engine_build: &build,
        },
    )
    .map_err(Failure::Start)?;
    let mut world = decoded.world;
    if hash(&world) != manifest.start_hash {
        return Err(Failure::StartHash);
    }
    let records = parse_records(&files.records)?;
    let mut last = 0;
    for (i, r) in records.iter().enumerate() {
        replay::replay(&mut world, last, std::slice::from_ref(r), rules, &build)
            .map_err(Failure::Record)?;
        last = r.seq;
        let checkpoint = manifest.records.get(i).ok_or(Failure::Checkpoints)?;
        if checkpoint.seq != r.seq || checkpoint.game_time != r.game_time {
            return Err(Failure::Checkpoints);
        }
        if hash(&world) != checkpoint.state_hash {
            return Err(Failure::Hash { seq: r.seq });
        }
    }
    if manifest.records.len() != records.len() {
        return Err(Failure::Checkpoints);
    }
    let events = world
        .advance_to(manifest.end_ms, rules)
        .map_err(Failure::End)?;
    if !events.is_empty() {
        return Err(Failure::EndEvents);
    }
    if hash(&world) != manifest.end_hash {
        return Err(Failure::EndHash);
    }
    let scores: Vec<Score> = world
        .players
        .keys()
        .map(|&p| Score {
            player: player_uuid(p),
            score: world.score(p, manifest.end_ms, rules).unwrap_or(i64::MIN),
        })
        .collect();
    if scores != manifest.scores {
        return Err(Failure::Scores);
    }
    Ok(world)
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
            let files = Files::read(&d).unwrap_or_else(|e| panic!("{name}: {e}"));
            (name, files)
        })
        .collect()
}
