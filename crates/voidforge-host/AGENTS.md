# voidforge-host

The engine host, the single writer for a universe. It runs the bounded inbox, stamping and sequence numbers, the durable log append, idempotency, snapshots and recovery, and publishes the read model.

Library. P2.3 added the loop: `spawn`, `HostHandle`, `HostTask`, `ReadModel`, `HostConfig` and the injected `Clock` (`SystemClock`, `ManualClock`) ([D-036](../../docs/decision-log.md#d-036-the-engine-host-loop-its-log-store-and-the-record-format)). P2.4 added idempotency: `submit_with_key`, the key cache and the log lookup, and `HostError::KeyConflict` ([D-037](../../docs/decision-log.md#d-037-idempotency-keys-at-any-age-the-cache-window-and-key-conflicts)). P2.5 added snapshots (a record-count schedule and clean shutdown) and `recover` ([D-038](../../docs/decision-log.md#d-038-snapshots-recovery-and-the-shared-recordcore-function)). The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-core`, `voidforge-store`, `voidforge-telemetry`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): The shell runtime from the Tech stack (tokio and tracing). Dev-dependencies (tests only): testkit, for scripted worlds and commands, and sqlx, for `#[sqlx::test]` (D-036).
- Never: Any workspace crate other than core, store and telemetry.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Only the host changes the live `World` ([Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering)). One thread owns it, and everything else reaches it through the bounded inbox ([Engine host and write path](../../docs/Voidforge%20Architectural%20Design.md#engine-host-and-write-path)).
- Host recovery and replay map a log record to `advance_to` or `apply` through one shared function, `voidforge_store::replay::step`, so they cannot drift apart. The live path builds every record from it and never calls `apply` or `advance_to` itself (a test enforces this). P2.7 reuses it.
- Recovery verifies by event replay and the state hash. `check_links` is structural only and does not validate gameplay fields (D-033, D-038). Recovery takes the newest snapshot of any build, and the whole tail must be the running build's records (replay refuses another build's record): one rule for restarts, upgrades, crashes after an upgrade, and rollbacks from a clean shutdown (D-038).
- Stamp t = max(wall clock − epoch, last t), so stamps never go backwards. The clock is injected (`Clock`), so tests drive it.
- One log record per step. A command record's events are the apply's advance followed by the command's own events; an advance that fires no events is not logged, but the read model is published after it (D-036).
- Sequence numbers are gap-free from 1 and continue after existing records. A turn's records share one durable append (group commit), and nothing is replied or published before it is durable.
- An engine error that poisons the World is fatal too, after the turn's earlier steps are appended and acknowledged.
- The store is built on the host's thread (`spawn` takes a constructor): a database connection belongs to the runtime that opened it (`PgLog::lazy`).
- A command is acknowledged only after its log record is durable. A failed append is fatal: the host stops and recovers.
- A known idempotency key returns the logged outcome without applying anything, at any age: keys within `key_window_ms` are cached (rebuilt from the log at start), and a miss is looked up in the log once per turn, so a used key never reaches the append (D-037). The same key with a different command is `KeyConflict`.
- The host publishes an immutable read model after every step, and reads never go through the command path.
- Recovery loads the newest snapshot with a matching build, replays later records, checks their recorded events, then advances to now.
- Instrumentation lives here and in the other shell crates, never in the core.
- The host re-exports the command vocabulary its own API already exposes in `Receipt` and `ReadModel`: `Command`, `Action`, `Rejection`, `DomainEvent`, `PlayerId` and `World`. The read vocabulary is re-exported too (D-049): `voidforge_core::view` (`Query`, `View`, `ViewError` and the view types) and the read-only state enums a mapping names. These are names only: nothing that mutates. The server names these types through the host and never depends on core. The read model's World is shared behind an `Arc`, so a reader cannot change the live World ([D-047](../../docs/decision-log.md#d-047-accounts-principal-to-player-and-join-game-exactly-once)).

## Tests

```sh
cargo build -p voidforge-host
cargo clippy -p voidforge-host --all-targets -- -D warnings
cargo nextest run -p voidforge-host      # the write path on the memory store, and the determinism double run
just db-test                             # needs `just up`: tests/postgres.rs, memory and Postgres identical
just determinism                         # includes the host's two-process double run
```

- `tests/host.rs` has one test per write-path step and crash-table row, on `MemoryLog` with a `ManualClock`. `tests/common` holds `GatedLog` (it holds the host inside an append), `settle` and the scripted economy `drive`.
- `tests/idempotency.rs` covers keys: repeats, conflicts, per-player keys, the same turn, a restart, the window edges, the expired-key attack, and a failed lookup.
- `tests/recovery.rs` covers snapshots and recovery: the crash table with restarts and retries, byte identity (a named test and a property test), and the snapshot choice and checks.
- `tests/postgres.rs` holds the done-when: the same script through `MemoryLog` and `PgLog` gives identical records, receipts and World. It's `#[ignore]`d outside the database stage. It also recovers every golden through Postgres.
- `tests/golden.rs` and `tests/common/golden.rs` are the golden logs (P2.6, [D-040](../../docs/decision-log.md#d-040-golden-logs-scenarios-recording-the-fixture-build-and-bless)): testkit's scenarios recorded through the host under the fixture build `0.1.0+golden`, replayed record by record with state hashes, a freshness check against a fresh recording, tamper tests and a coverage guard. `just bless` writes them.

- `tests/crash.rs` is the crash-and-retry harness (P2.8, [D-042](../../docs/decision-log.md#d-042-the-crash-and-retry-harness)). It runs the host in a child process that aborts at seeded kill points, on a file-backed test store (`tests/common/filelog.rs`) or Postgres, and checks that every retried command ends with exactly one outcome. Postgres's `append.during` dies inside PgLog's open transaction, held by a test-only trigger. The nightly shape is the ignored `crash_nightly`.

P2.3 to P2.8: commands round-trip through an in-memory store and Postgres, idempotency survives restart, snapshot round trip and recovery pass, the golden logs replay (also through Postgres), and every retried command ends with exactly one outcome under random kills, on both stores.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Engine host and write path](../../docs/Voidforge%20Architectural%20Design.md#engine-host-and-write-path), [Persistence](../../docs/Voidforge%20Architectural%20Design.md#persistence), [Read side and API](../../docs/Voidforge%20Architectural%20Design.md#read-side-and-api), [Phase 2](../../docs/Voidforge%20Technical%20Plan.md#phase-2-host-and-persistence)
