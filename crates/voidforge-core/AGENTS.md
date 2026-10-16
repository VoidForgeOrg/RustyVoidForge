# voidforge-core

The engine: world state, the per-planet solver, commands, domain events and views. It is a pure, single-threaded state machine with three entry points, `advance_to(t)`, `apply(command, t)` and `view(query, t)`, and each call returns the domain events it produced.

Library. It is a stub today; P1.1 is the first package that fills it. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: None. No workspace crate may be a dependency of the core.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): serde and thiserror only.
- Never: Anything else, including tokio, tracing and uuid. The core must not depend on the shell.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- No I/O, no clock reads, no threads, no random numbers ([Engine core](../../docs/Voidforge%20Architectural%20Design.md#engine-core)). Game time is a signed 64-bit count of milliseconds since the universe epoch, passed in by the caller ([Time model](../../docs/Voidforge%20Architectural%20Design.md#time-model)).
- Integers only: 64-bit storage, 128-bit intermediates, masses in grams, rates in grams per hour. No float types ([State model](../../docs/Voidforge%20Architectural%20Design.md#state-model)).
- Every collection iterates in ID order. Entities refer to each other by integer ID, never by pointer.
- New entities take `World.next_id`. Player UUIDs and other external keys arrive in commands; the core never generates them.
- One rounding rule: values round down with a carried remainder, crossing times round up, and a crossing quantity snaps to its boundary with its remainder reset to 0 ([Rounding formulas](../../docs/Voidforge%20Architectural%20Design.md#rounding-formulas)).
- Total order: the queue is ordered by (time, kind rank, entity ID). At one instant, `PlanetChange` entries run by planet ID, then fleet entries by fleet ID, then commands by sequence number. A step never schedules work at its own instant ([Engine core](../../docs/Voidforge%20Architectural%20Design.md#engine-core)).
- `view` never writes; checkpoints happen only on events and commands.
- A command is validated against the state at its stamped time and applied atomically, or rejected with a reason code and no change ([Commands](../../docs/Voidforge%20Architectural%20Design.md#commands)).
- A next event that is not strictly after t, or too many transitions at one instant, stops the engine with an error. That is a bug, not a game state ([Next event](../../docs/Voidforge%20Architectural%20Design.md#next-event)).
- Only `voidforge-host` calls `advance_to` and `apply`. The core does no instrumentation; it returns step statistics as plain data.
- The determinism rules apply here: no clocks, hash-ordered collections, threads, locks, system randomness, UUID generation or float arithmetic ([P0.4](../../docs/Voidforge%20Technical%20Plan.md#phase-0-foundations), [Determinism rules](../../docs/Voidforge%20Architectural%20Design.md#determinism-rules)). Review enforces them until P0.4 adds the clippy bans.

## Tests

```sh
cargo build -p voidforge-core
cargo clippy -p voidforge-core --all-targets -- -D warnings
cargo nextest run -p voidforge-core --no-tests=warn   # drop --no-tests=warn once the workspace has a real test (D-004)
```

P1.1 to P1.11 add the solver, fleet, command and view tests, and P1.5 adds the determinism suite: property tests, observation invariance and an in-process double run. Every later package must keep that suite green ([Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine), [Testing strategy](../../docs/Voidforge%20Architectural%20Design.md#testing-strategy)).

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Engine core](../../docs/Voidforge%20Architectural%20Design.md#engine-core), [Per-planet solver](../../docs/Voidforge%20Architectural%20Design.md#per-planet-solver), [Fleets](../../docs/Voidforge%20Architectural%20Design.md#fleets), [Commands](../../docs/Voidforge%20Architectural%20Design.md#commands), [Determinism rules](../../docs/Voidforge%20Architectural%20Design.md#determinism-rules), [Game engine model](../../docs/Voidforge%20MVP%20Game%20Design.md#game-engine-model), [Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine)
