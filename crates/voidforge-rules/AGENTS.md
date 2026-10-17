# voidforge-rules

Rules data and a pure parser from rules files into the core's typed specs: `BuildingSpecs`, `ShipSpecs`, `ScoringSpecs`, the homeworld template and world-generation parameters. The rules are versioned TOML files in `rules/`; Rules v0 from the game design is the default, alongside a fast test variant where an hour of play takes a second.

Library. P1.2 delivered `parse(text)` ([D-020](../../docs/decision-log.md#d-020-rules-data-files-and-ids)). The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-core`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): toml (std, parse and serde only), serde (derive) for the file schema, and thiserror for its error type.
- Never: Any workspace crate other than core.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Parsing is pure: rules-file text in, validated `voidforge_core::rules::Rules` or an error out. No file, clock or network access; the caller reads the file.
- Strict schema: every field is required, unknown fields are rejected, and numbers must be integers. A schema error names the field and line. A value that breaks an invariant fails `Rules::validate` in core, which names the field (`buildings.drill.cost_g`) and the problem. Nothing falls back to a default.
- Files are written in base units (g, ms, g/h, distance units/h) with the source value in a comment. `v0-fast.toml` must stay exactly v0 with rates and speeds x3,600 and durations /3,600; a test enforces it.
- Rules are data in one versioned table, and the starting values are the game design's [Rules v0](../../docs/Voidforge%20MVP%20Game%20Design.md#rules-v0). A rules change is a document change first.
- Log records and snapshots name the rules version in force, and replay refuses a mismatch ([Determinism rules](../../docs/Voidforge%20Architectural%20Design.md#determinism-rules)). A `rules_change` log record switches versions at a game time ([Persistence](../../docs/Voidforge%20Architectural%20Design.md#persistence)).
- The determinism rules apply here: no clocks, hash-ordered collections, threads, locks, system randomness, UUID generation or float arithmetic ([P0.4](../../docs/Voidforge%20Technical%20Plan.md#phase-0-foundations), [Determinism rules](../../docs/Voidforge%20Architectural%20Design.md#determinism-rules)). This crate's `clippy.toml` lists the banned paths, and `lib.rs` forbids the lints they feed, so no item can opt out. `tests/guardrails/run_fixtures.py` proves every ban here ([D-010](../../docs/decision-log.md#d-010-determinism-bans-in-core-rules-and-worldgen)). To ban another item, add it to all three identical `clippy.toml` files (core, rules, worldgen) and to a fixture. `tests/guardrails/check_no_floats.py` also rejects any `f32` or `f64` in this crate's compiled library, including floats no clippy ban can see ([D-013](../../docs/decision-log.md#d-013-type-aware-float-check)).

## Tests

```sh
cargo build -p voidforge-rules
cargo clippy -p voidforge-rules --all-targets -- -D warnings
cargo nextest run -p voidforge-rules
```

The tests check:
- every Rules v0 and P1.12 value, against the source tables in tons, hours and world units;
- the game design's worked examples;
- that v0-fast is exactly the fast transform of v0;
- reading equivalence through P1.1 quantities: a fast second equals a v0 hour, and each build completes at its build time;
- invalid files (malformed, missing, unknown, float, string, negative, out of range, and each invariant), checking the error's field and text.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Rules v0](../../docs/Voidforge%20MVP%20Game%20Design.md#rules-v0), [State model](../../docs/Voidforge%20Architectural%20Design.md#state-model), [Persistence](../../docs/Voidforge%20Architectural%20Design.md#persistence), [Phase 1](../../docs/Voidforge%20Technical%20Plan.md#phase-1-core-engine)
