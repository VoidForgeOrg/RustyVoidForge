# voidforge-worldgen

Seeded world generation: it places the planets at 3D coordinates and produces the ordered list of homeworld candidates. Its output is the universe's first snapshot.

Library. P1.12 delivered `generate`: seed and expected player count in, a solved first `World` out, with planets placed uniformly and homeworld candidates chosen and shuffled from the seed ([D-031](../../docs/decision-log.md#d-031-world-generation); the placement option is provisional pending Tom). The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-core`, `voidforge-rules`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): rand_chacha, the only randomness source, seeded from the universe seed.
- Never: Any workspace crate other than core and rules, and any system or thread-local random generator.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- The same 64-bit seed always yields the same world and state hash. The seed is recorded in the first snapshot.
- Generation defaults are rules data, not constants (the [P1.12 defaults](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine)).
- Derived planet values (rates and next event) are stored with each planet, so a restored snapshot needs no re-solve.
- The determinism rules apply here: no clocks, hash-ordered collections, threads, locks, system randomness, UUID generation or float arithmetic ([P0.4](../../docs/Voidforge%20Technical%20Plan.md#phase-0-foundations), [Determinism rules](../../docs/Voidforge%20Architectural%20Design.md#determinism-rules)). This crate's `clippy.toml` lists the banned paths, and `lib.rs` forbids the lints they feed, so no item can opt out. `tests/guardrails/run_fixtures.py` proves every ban here ([D-010](../../docs/decision-log.md#d-010-determinism-bans-in-core-rules-and-worldgen)). To ban another item, add it to all three identical `clippy.toml` files (core, rules, worldgen) and to a fixture. `tests/guardrails/check_no_floats.py` also rejects any `f32` or `f64` in this crate's compiled library, including floats no clippy ban can see ([D-013](../../docs/decision-log.md#d-013-type-aware-float-check)).

## Tests

```sh
cargo build -p voidforge-worldgen
cargo clippy -p voidforge-worldgen --all-targets -- -D warnings
cargo nextest run -p voidforge-worldgen
```

P1.12: `src/tests.rs` pins the RNG stream and the unbiased sampler; `tests/worldgen.rs` checks every default by brute force on Rules v0 worlds of 1, 7 and 100 players (D-031 option A) and on altered rules, plus feasibility errors, the player limit and Join game; `tests/determinism.rs` is part of `just determinism` (in-process and two-process double runs over generation and a generated world played for 100 hours).

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [State model](../../docs/Voidforge%20Architectural%20Design.md#state-model), [Universe and planets](../../docs/Voidforge%20MVP%20Game%20Design.md#universe-and-planets), [Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine)
