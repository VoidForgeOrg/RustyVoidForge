# voidforge-replay

The replay and divergence finder: it replays a log, verifies the recorded events and state hashes, and pinpoints the first divergent record.

Binary. P2.7 filled it ([D-041](../../docs/decision-log.md#d-041-the-replay-cli-one-verification-path-the-build-pin-and-exit-codes)). It's a thin command line over `voidforge_store::{golden, verify}`, the one verification path it shares with the host's tests. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Usage

```
voidforge-replay golden  <PATH>...  [--rules <FILE>] [--json]
voidforge-replay postgres --rules <FILE> [--database-url <URL>] [--from <SEQ>] [--to <SEQ>] [--build <ID>] [--json]
voidforge-replay inspect golden <PATH>... [--json]
voidforge-replay inspect postgres [--database-url <URL>] [--json]
```

- **`golden`** verifies golden directories, or directories of goldens: the start, every record, the hash after every record, the end and the scores. It runs under the fixture build `0.1.0+golden`, with `rules/v<N>.toml` from the nearest ancestor unless `--rules` is given.
- **`postgres`** verifies from a snapshot (`--from`, default the earliest) to the head (or `--to`). `--database-url` defaults to `DATABASE_URL` and is never printed.
  - Every later snapshot row is a checkpoint: it's decoded strictly and cross-checked, then compared with the replayed World brought to the snapshot's own game time (which reproduces unlogged eventless advances).
  - A snapshot past the log head is refused.
  - The build pin is on the records, as in recovery: every snapshot row, the start's included, decodes under the build its row names. So after an engine upgrade, the new build's records verify on the earlier build's snapshot ([D-044](../../docs/decision-log.md#d-044-phase-2-gate-repair-verification-failures-the-record-pin-the-database-stage-lock-and-the-epoch)).
- **`inspect`** reports builds, rules versions, snapshot headers and record ranges. It reads headers through `decode_any`, so **nothing it prints is verified** (D-035): text output says UNVERIFIED, and JSON says `"verified": false`.
- **`--build <ID>`** asserts the build that wrote the log (default: this binary's own `ENGINE_BUILD`). **A pass under `--build` means this binary's code reproduces that log, not that the named build's code was verified.** A production incident log is verified by its own build's binary. The output always names the build and whether `--build` gave it.

## Exit codes and output (a stable contract)

| Code | Meaning |
| --- | --- |
| 0 | verified |
| 1 | diverged: the first divergent record is reported |
| 2 | usage error (clap) |
| 3 | refused: another build or rules version |
| 4 | input error: unreadable files, an unreachable database, no snapshot |
| 5 | telemetry failed to start, or failed to shut down after a pass |

- With several sources, each is reported, and the exit code is the first failing source's.
- `--json` prints one document: `{"result", "sources": [{"source", "result", "build", "build_from_flag", "rules_version", "records", "first_seq", "last_seq", "checkpoints", "end_hash", "divergence": {"seq", "what", "expected", "found"}, "error"}]}`.
- `what` is one of `events`, `outcome`, `state_hash`, `gap`, `engine`, `malformed`, `snapshot`, `checkpoints`, `end_events`, `end_hash`, `scores`. For a refusal it's `build`, `rules_version` or `kind`.
- The seq is the record's own, except for a gap (a missing or reordered record), where it's the seq expected there.
- Stdout holds no times, so the same input gives byte-identical output. The trace ID goes to stderr.

## Allowed dependencies

- Workspace: `voidforge-core`, `voidforge-rules`, `voidforge-store`, `voidforge-telemetry`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): clap (anyhow isn't needed: errors are typed); tracing for spans; tokio to drive the store; serde and serde_json for `--json`.
- Never: any workspace crate other than core, rules, store and telemetry.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Telemetry: `main` calls `voidforge_telemetry::init` once, first, and `Telemetry::shutdown` before exit ([D-016](../../docs/decision-log.md#d-016-telemetry-bootstrap)). Telemetry that couldn't be delivered is a loud ERROR on stderr and never changes the exit code, which reports the command itself; only the hidden `telemetry-check`, whose job is delivery, fails on it ([D-064](../../docs/decision-log.md#d-064-telemetry-a-commands-trace-the-hosts-metrics-and-trace-context-across-http), T6). It never configures its own exporter or subscriber.
- Replay is an offline caller of the core: it rebuilds a world from a snapshot and the log and never touches live state. Every record goes through `voidforge_store::replay::replay_record`, the same function host recovery uses. There's no second path.
- Replays are pinned to a build and a rules version, and a mismatch is refused (exit 3). The build pin applies to records; snapshot rows decode under their own build.
- A failure is never turned into a value: a core error (a score out of range, say) is a divergence, and an unreadable golden, or one with its snapshot or records but no manifest, is an input error naming it, never skipped.
- `advance` records are recomputed and checked against their recorded events, never applied.
- Replaying the same log gives byte-identical state, the same domain events and the same output every time.

## Tests

```sh
cargo build -p voidforge-replay
cargo clippy -p voidforge-replay --all-targets -- -D warnings
cargo nextest run -p voidforge-replay
```

- `tests/cli.rs` drives the binary as a process:
  - every golden verifies;
  - the tamper matrix (an event swapped or edited, an outcome flipped, a hash, a record removed or the last one dropped, two records swapped, the end hash, a score) is pinpointed to its seq with exit 1;
  - a scoring error in the core diverges with exit 1, even when the manifest names `i64::MIN`;
  - an unreadable golden, or one missing its manifest, in a directory of goldens is an input error (exit 4) beside the verified rest;
  - a build or rules mismatch is refused with exit 3;
  - inspection is labeled unverified;
  - the output is byte-identical;
  - usage and input errors give exits 2 and 4;
  - `--help` says what a pass under `--build` means.
- `tests/postgres.rs` (the database stage) covers a golden loaded into Postgres with snapshot checkpoints, a tampered row and checkpoint, another build refused, records on an earlier build's snapshot verifying, and inspection.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Persistence](../../docs/Voidforge%20Architectural%20Design.md#persistence), [Determinism rules](../../docs/Voidforge%20Architectural%20Design.md#determinism-rules), [Phase 2](../../docs/Voidforge%20Technical%20Plan.md#phase-2-host-and-persistence)
