# voidforge-worldgen

Seeded world generation: it places the planets at 3D coordinates and produces the ordered list of homeworld candidates. Its output is the universe's first snapshot.

Library. It is a stub today; P1.12 is the first package that fills it. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-core`, `voidforge-rules`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): rand_chacha, the only randomness source, seeded from the universe seed.
- Never: Any workspace crate other than core and rules, and any system or thread-local random generator.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- The same 64-bit seed always yields the same world and state hash. The seed is recorded in the first snapshot.
- Generation defaults are rules data, not constants (the [P1.12 defaults](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine)).
- Derived planet values (rates and next event) are stored with each planet, so a restored snapshot needs no re-solve.
- The determinism rules apply here: no clocks, hash-ordered collections, threads, locks, system randomness, UUID generation or float arithmetic ([P0.4](../../docs/Voidforge%20Technical%20Plan.md#phase-0-foundations), [Determinism rules](../../docs/Voidforge%20Architectural%20Design.md#determinism-rules)). Review enforces them until P0.4 adds the clippy bans.

## Tests

```sh
cargo build -p voidforge-worldgen
cargo clippy -p voidforge-worldgen --all-targets -- -D warnings
cargo nextest run -p voidforge-worldgen --no-tests=warn   # drop --no-tests=warn once the workspace has a real test (D-004)
```

P1.12: the same seed always yields the same state hash, and every world-generation default holds.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [State model](../../docs/Voidforge%20Architectural%20Design.md#state-model), [Universe and planets](../../docs/Voidforge%20MVP%20Game%20Design.md#universe-and-planets), [Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine)
