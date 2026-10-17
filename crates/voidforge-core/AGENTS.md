# voidforge-core

The engine: world state, the per-planet solver, commands, domain events and views. It is a pure, single-threaded state machine with three entry points, `advance_to(t)`, `apply(command, t)` and `view(query, t)`, and each call returns the domain events it produced.

Library. P1.1 delivered `quantity` ([D-018](../../docs/decision-log.md#d-018-quantities-and-crossing-times)); the rest arrives with P1.2 to P1.11. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: None. No workspace crate may be a dependency of the core.
- External at runtime, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): serde and thiserror only (thiserror today; serde arrives with P1.3).
- Dev-dependencies, for tests only and never linked into the library: proptest ([D-019](../../docs/decision-log.md#d-019-test-only-dev-dependencies-in-the-core)).
- Never: Anything else, including tokio, tracing and uuid. The core must not depend on the shell.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- No I/O, no clock reads, no threads, no random numbers ([Engine core](../../docs/Voidforge%20Architectural%20Design.md#engine-core)). Game time is a signed 64-bit count of milliseconds since the universe epoch, passed in by the caller ([Time model](../../docs/Voidforge%20Architectural%20Design.md#time-model)).
- Integers only: 64-bit storage, 128-bit intermediates, masses in grams, rates in grams per hour. No float types ([State model](../../docs/Voidforge%20Architectural%20Design.md#state-model)).
- Every collection iterates in ID order. Entities refer to each other by integer ID, never by pointer.
- New entities take `World.next_id`. Player UUIDs and other external keys arrive in commands; the core never generates them.
- One rounding rule: values round down with a carried remainder (floor division, non-negative remainder, never Rust's truncating `/`). A crossing is the first millisecond at which that read reaches the boundary; falling crossings are not the mirror of rising ones. A crossing quantity snaps to its boundary with its remainder reset to 0 ([Rounding formulas](../../docs/Voidforge%20Architectural%20Design.md#rounding-formulas)). `quantity::Quantity` implements this; every result outside 64 bits is a `QuantityError`, never wrapped or saturated.
- Total order: the queue is ordered by (time, kind rank, entity ID). At one instant, `PlanetChange` entries run by planet ID, then fleet entries by fleet ID, then commands by sequence number. A step never schedules work at its own instant ([Engine core](../../docs/Voidforge%20Architectural%20Design.md#engine-core)).
- `view` never writes; checkpoints happen only on events and commands.
- A command is validated against the state at its stamped time and applied atomically, or rejected with a reason code and no change ([Commands](../../docs/Voidforge%20Architectural%20Design.md#commands)).
- A next event that is not strictly after t, or too many transitions at one instant, stops the engine with an error. That is a bug, not a game state ([Next event](../../docs/Voidforge%20Architectural%20Design.md#next-event)).
- Only `voidforge-host` changes the live `World`. Two offline callers also drive `advance_to` and `apply`, never against live state: `voidforge-replay`, which rebuilds a world from a snapshot and the log, and tests ([Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering)).
- The core does no instrumentation; it returns step statistics as plain data.
- The determinism rules apply here: no clocks, hash-ordered collections, threads, locks, system randomness, UUID generation or float arithmetic ([P0.4](../../docs/Voidforge%20Technical%20Plan.md#phase-0-foundations), [Determinism rules](../../docs/Voidforge%20Architectural%20Design.md#determinism-rules)). This crate's `clippy.toml` lists the banned paths, and `lib.rs` forbids the lints they feed, so no item can opt out. `tests/guardrails/run_fixtures.py` proves every ban here ([D-010](../../docs/decision-log.md#d-010-determinism-bans-in-core-rules-and-worldgen)). To ban another item, add it to all three identical `clippy.toml` files (core, rules, worldgen) and to a fixture. `tests/guardrails/check_no_floats.py` also rejects any `f32` or `f64` in this crate's compiled library, including floats no clippy ban can see ([D-013](../../docs/decision-log.md#d-013-type-aware-float-check)).

## Tests

```sh
cargo build -p voidforge-core
cargo clippy -p voidforge-core --all-targets -- -D warnings
cargo nextest run -p voidforge-core
```

P1.1's quantity tests compare reads, checkpoints and crossings on generated cases against an independent millisecond-stepping reference, plus named boundary and error cases (`src/quantity/tests.rs`; proptest runs with a fixed seed). P1.2 to P1.11 add the solver, fleet, command and view tests, and P1.5 adds the determinism suite: property tests, observation invariance and an in-process double run. Every later package must keep that suite green ([Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine), [Testing strategy](../../docs/Voidforge%20Architectural%20Design.md#testing-strategy)).

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Engine core](../../docs/Voidforge%20Architectural%20Design.md#engine-core), [Per-planet solver](../../docs/Voidforge%20Architectural%20Design.md#per-planet-solver), [Fleets](../../docs/Voidforge%20Architectural%20Design.md#fleets), [Commands](../../docs/Voidforge%20Architectural%20Design.md#commands), [Determinism rules](../../docs/Voidforge%20Architectural%20Design.md#determinism-rules), [Game engine model](../../docs/Voidforge%20MVP%20Game%20Design.md#game-engine-model), [Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine)
