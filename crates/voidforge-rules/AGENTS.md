# voidforge-rules

Rules data and a pure parser from rules files into the core's typed specs: `BuildingSpecs`, `ShipSpecs`, `ScoringSpecs`, the homeworld template and world-generation parameters. The rules are versioned TOML files in `rules/`; Rules v0 from the game design is the default, alongside a fast test variant where an hour of play takes a second.

Library. It is a stub today; P1.2 is the first package that fills it. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-core`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): toml (Tech stack: rules files parsed by this crate).
- Never: Any workspace crate other than core.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Parsing is pure: rules-file contents in, typed core specs or a clear error out. Invalid files fail with errors that say what is wrong (P1.2 done-when).
- Rules are data in one versioned table, and the starting values are the game design's [Rules v0](../../docs/Voidforge%20MVP%20Game%20Design.md#rules-v0). A rules change is a document change first.
- Log records and snapshots name the rules version in force, and replay refuses a mismatch ([Determinism rules](../../docs/Voidforge%20Architectural%20Design.md#determinism-rules)). A `rules_change` log record switches versions at a game time ([Persistence](../../docs/Voidforge%20Architectural%20Design.md#persistence)).
- The determinism rules apply here: no clocks, hash-ordered collections, threads, locks, system randomness, UUID generation or float arithmetic ([P0.4](../../docs/Voidforge%20Technical%20Plan.md#phase-0-foundations), [Determinism rules](../../docs/Voidforge%20Architectural%20Design.md#determinism-rules)). Review enforces them until P0.4 adds the clippy bans.

## Tests

```sh
cargo build -p voidforge-rules
cargo clippy -p voidforge-rules --all-targets -- -D warnings
cargo nextest run -p voidforge-rules --no-tests=warn   # drop --no-tests=warn once the workspace has a real test (D-004)
```

P1.2: Rules v0 loads, and invalid files fail with clear errors.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Rules v0](../../docs/Voidforge%20MVP%20Game%20Design.md#rules-v0), [State model](../../docs/Voidforge%20Architectural%20Design.md#state-model), [Persistence](../../docs/Voidforge%20Architectural%20Design.md#persistence), [Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine)
