# voidforge-host

The engine host, the single writer for a universe. It runs the bounded inbox, stamping and sequence numbers, the durable log append, idempotency, snapshots and recovery, and publishes the read model.

Library. It is a stub today; P2.3 is the first package that fills it. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-core`, `voidforge-store`, `voidforge-telemetry`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): The shell runtime from the Tech stack (tokio and tracing).
- Never: Any workspace crate other than core, store and telemetry.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- It is the only code that calls the core's state-changing entry points. One thread owns the `World`, and everything else reaches it through the bounded inbox ([Engine host and write path](../../docs/Voidforge%20Architectural%20Design.md#engine-host-and-write-path)).
- Stamp t = max(wall clock, last t), so stamps never go backwards.
- A command is acknowledged only after its log record is durable. A failed append is fatal: the host stops and recovers.
- A known idempotency key returns the logged outcome without applying anything. The key map is rebuilt from the log on recovery.
- The host publishes an immutable read model after every step, and reads never go through the command path.
- Recovery loads the newest snapshot with a matching build, replays later records, checks their recorded events, then advances to now.
- Instrumentation lives here and in the other shell crates, never in the core.

## Tests

```sh
cargo build -p voidforge-host
cargo clippy -p voidforge-host --all-targets -- -D warnings
cargo nextest run -p voidforge-host --no-tests=warn   # drop --no-tests=warn once the workspace has a real test (D-004)
```

P2.3 to P2.5: commands round-trip through an in-memory store and Postgres, idempotency survives restart, and snapshot round trip and recovery pass. P2.8: crash-and-retry.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Engine host and write path](../../docs/Voidforge%20Architectural%20Design.md#engine-host-and-write-path), [Persistence](../../docs/Voidforge%20Architectural%20Design.md#persistence), [Read side and API](../../docs/Voidforge%20Architectural%20Design.md#read-side-and-api), [Phase 2](../../docs/Voidforge%20Technical%20Plan.md#phase-2-host-and-persistence)
