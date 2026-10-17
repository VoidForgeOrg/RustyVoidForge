# voidforge-testkit

Test builders for every entity, the determinism suite's shared helpers, and golden logs. It exists for tests: other crates take it as a dev-dependency only.

Library, dev only. P1.3 delivered `WorldBuilder`, `busy_world`, `v0_rules` and the `ALICE` and `BOB` test players ([D-022](../../docs/decision-log.md#d-022-state-model-ids-and-test-builders)). P1.5 delivered the determinism suite, `tests/determinism.rs`, and the `determinism` module its harnesses share ([D-024](../../docs/decision-log.md#d-024-determinism-suite-canonical-bytes-and-the-two-process-run)). The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-core`, `voidforge-rules`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): postcard and serde for canonical bytes; proptest and serde_json as dev-dependencies. proptest stays a dev-dependency, because as a normal one it duplicates rand in cargo-deny's graph (D-024). insta waits for its first use.
- Never: Any workspace crate other than core and rules. No crate may take testkit as a normal (non-dev) dependency.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Builders create valid entities from the rules' defaults (Rules v0 unless given others), allocate IDs from the world's own counter (predictable and dense), keep both sides of every link in step, and check links in `build()`. Misuse panics with a message; `try_build()` returns the first broken link.
- Builders schedule only through the core's `World::schedule_planet` and `World::schedule_fleet` (D-023), so every send, recall and planet event bumps a version as the engine does.
- `busy_world()` holds every entity, state, mission and queue kind, including a stale entry. Its test has an exhaustive variant census, so a new variant must be added there.
- It holds the determinism suite from P1.5, which every later package must keep green ([Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine)). `just determinism`, and the required CI job of that name, run it and then the two-process double run.
- Runs are compared on `determinism::canonical_bytes`, postcard. P2.2's state hash is BLAKE3 over exactly these World bytes, never a second encoding (D-024).
- Golden files in `tests/golden/` change only through `just bless`, and a reviewer checks every change ([Agent factory rules](../../docs/Voidforge%20Technical%20Plan.md#agent-factory-rules)). Both arrive with P2.6.

## Tests

```sh
cargo build -p voidforge-testkit
cargo clippy -p voidforge-testkit --all-targets -- -D warnings
cargo nextest run -p voidforge-testkit
```

P1.3 covers every builder, rules defaults including the fast rules, fleet and recall bookkeeping, dense and overridable IDs, link failures, and the busy world's variant census, round trip and byte-for-byte determinism. P1.5 adds the determinism suite (`just determinism`) and P2.6 the golden logs.

## Extending the determinism suite

Every package from P1.6 on joins these harnesses, in `tests/determinism.rs` and `src/determinism.rs`, rather than writing parallel ones. QA and review check that it did.

| A package that adds | Joins | How |
| --- | --- | --- |
| A real step (solver P1.6/P1.7, fleets P1.8) | Observation invariance, double run, `check_links` after every advance | Run its scenarios through production `advance_to` once it no longer returns `StepNotBuilt`, alongside the `Churn` runs. Keep `Churn` for the loop itself. Scenarios must produce events; the event-count assertions stay. P1.6 added `Economy` and the `solved_*` tests; P1.7 extends `Economy` with builds that complete and demolitions that end. |
| A command (`apply`, P1.9/P1.10) | Double run; observation invariance | Generated command sequences run twice give equal bytes. Commands stamped between observation points must not depend on how the span was split. |
| A view (P1.11) | Observation invariance ("Pure reads") | Call `view` at every observation point. The bytes must not change. |
| A new entity, state or queue kind | `Scenario`, `Churn` and the property generators | Generate it, so the harnesses reach it. |
| Phase 2 snapshots, logs and golden replays | This stage (CI/CD table) | Snapshot round trip and golden replays join `just determinism`. The log-based two-process replay replaces nothing here; it adds to it. |

Invariants that wait on the solver and later packages. Add them as properties when their package lands; never fake them before:

- Done in P1.6 (`solved_*`, D-025): no negative or over-cap stock at every observation point, and the next event strictly after every solve. Mass is accounted for: an exact scaled ledger between solves, and each snap within one ms of flow plus one gram. These run through the production solver on `Economy` worlds under `slow_build_rules`. P1.7 must let builds complete there and keep these green, with completions moving mass from delivered ingots into buildings and ships.
- TODO(P1.7, P1.9): slots are respected: buildings, including those under construction or being demolished, never exceed slots.
- TODO(P1.8): each ship is in exactly one place (one roster or one fleet). `check_links` covers the links, and the property adds conservation across missions.

Mutations must keep failing the suite: queue insertion-order dependence, a read that writes, a non-carried remainder, a remainder dropped across a rate change (checked against an independent millisecond stepper, because both invariance paths would drop it alike), a stale entry processed, process-level nondeterminism (two-process run only), a deferred entry at `t`, and a late crossing. The P1.5 proof has the table.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Testing strategy](../../docs/Voidforge%20Architectural%20Design.md#testing-strategy), [Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine), [Phase 2](../../docs/Voidforge%20Technical%20Plan.md#phase-2-host-and-persistence)
