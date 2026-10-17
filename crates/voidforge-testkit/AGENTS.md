# voidforge-testkit

Test builders for every entity, the determinism suite's shared helpers, and golden logs. It exists for tests: other crates take it as a dev-dependency only.

Library, dev only. P1.3 delivered `WorldBuilder`, `busy_world`, `v0_rules` and the `ALICE` and `BOB` test players ([D-022](../../docs/decision-log.md#d-022-state-model-ids-and-test-builders)). The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-core`, `voidforge-rules`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): proptest and insta, the test tooling, when needed; serde_json as a dev-dependency for round-trip tests.
- Never: Any workspace crate other than core and rules. No crate may take testkit as a normal (non-dev) dependency.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Builders create valid entities from the rules' defaults (Rules v0 unless given others), allocate IDs from the world's own counter (predictable and dense), keep both sides of every link in step, and check links in `build()`. Misuse panics with a message; `try_build()` returns the first broken link.
- `busy_world()` holds every entity, state, mission and queue kind, including a stale entry. Its test has an exhaustive variant census, so a new variant must be added there.
- It holds the determinism suite from P1.5, which every later package must keep green ([Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine)).
- Golden files in `tests/golden/` change only through `just bless`, and a reviewer checks every change ([Agent factory rules](../../docs/Voidforge%20Technical%20Plan.md#agent-factory-rules)). Both arrive with P2.6.

## Tests

```sh
cargo build -p voidforge-testkit
cargo clippy -p voidforge-testkit --all-targets -- -D warnings
cargo nextest run -p voidforge-testkit
```

P1.3 covers every builder, rules defaults including the fast rules, fleet and recall bookkeeping, dense and overridable IDs, link failures, and the busy world's variant census, round trip and byte-for-byte determinism. P1.5 adds the determinism suite and P2.6 the golden logs.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Testing strategy](../../docs/Voidforge%20Architectural%20Design.md#testing-strategy), [Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine), [Phase 2](../../docs/Voidforge%20Technical%20Plan.md#phase-2-host-and-persistence)
