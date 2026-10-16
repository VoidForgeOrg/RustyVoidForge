# voidforge-replay

The replay and divergence finder: it replays a log, verifies the recorded events and state hashes, and pinpoints the first divergent record.

Binary. It is a stub today; P2.7 is the first package that fills it. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-core`, `voidforge-rules`, `voidforge-store`, `voidforge-telemetry`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): clap and anyhow; tracing for spans.
- Never: Any workspace crate other than core, rules, store and telemetry.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Telemetry: `main` calls `voidforge_telemetry::init` once, first, and `Telemetry::shutdown` before exit, reporting a failed shutdown with a non-zero exit ([D-016](../../docs/decision-log.md#d-016-telemetry-bootstrap)). It never configures its own exporter or subscriber.
- Replay is an offline caller of the core: it rebuilds a world from a snapshot and the log and never touches live state. It maps each log record to `advance_to` or `apply` through the same shared function as host recovery, which P2.5 adds and P2.7 reuses ([Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering)).
- Replays are pinned to the rules version and engine build named in the log and snapshots, and a mismatch is refused.
- `advance` records are recomputed and checked against their recorded events, never applied.
- Replaying the same log gives byte-identical state and the same domain events every time.

## Tests

```sh
cargo build -p voidforge-replay
cargo clippy -p voidforge-replay --all-targets -- -D warnings
cargo nextest run --workspace --locked   # this crate has no tests yet, so `-p voidforge-replay` alone exits 4
```

P2.7: golden logs replay cleanly, and a tampered log is pinpointed.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Persistence](../../docs/Voidforge%20Architectural%20Design.md#persistence), [Determinism rules](../../docs/Voidforge%20Architectural%20Design.md#determinism-rules), [Phase 2](../../docs/Voidforge%20Technical%20Plan.md#phase-2-host-and-persistence)
