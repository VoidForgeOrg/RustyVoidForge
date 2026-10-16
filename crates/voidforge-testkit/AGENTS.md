# voidforge-testkit

Test builders for every entity, the determinism suite's shared helpers, and golden logs. It exists for tests: other crates take it as a dev-dependency only.

Library, dev only. It is a stub today; P1.3 is the first package that fills it. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-core`, `voidforge-rules`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): proptest and insta, the test tooling.
- Never: Any workspace crate other than core and rules. No crate may take testkit as a normal (non-dev) dependency.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- It holds the determinism suite from P1.5, which every later package must keep green ([Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine)).
- Golden files in `tests/golden/` change only through `just bless`, and a reviewer checks every change ([Agent factory rules](../../docs/Voidforge%20Technical%20Plan.md#agent-factory-rules)). Both arrive with P2.6.

## Tests

```sh
cargo build -p voidforge-testkit
cargo clippy -p voidforge-testkit --all-targets -- -D warnings
cargo nextest run --workspace --locked   # this crate has no tests yet, so `-p voidforge-testkit` alone exits 4
```

P1.3: builders can create any entity. P1.5: the determinism suite. P2.6: golden logs recorded with their state hashes.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Testing strategy](../../docs/Voidforge%20Architectural%20Design.md#testing-strategy), [Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine), [Phase 2](../../docs/Voidforge%20Technical%20Plan.md#phase-2-host-and-persistence)
