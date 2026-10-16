# Voidforge: agent guide

Voidforge is a persistent space strategy MMO built as a headless, deterministic discrete-event engine behind an open API. This repository is the Rust implementation of the MVP. The repository root is the cargo workspace.

## Sources of truth

The documents decide; code follows them. If code and documents disagree, stop and raise the question rather than guess, and land the document change before the code.

- [Voidforge MVP Game Design](docs/Voidforge%20MVP%20Game%20Design.md): the rules of the game, including Rules v0.
- [Voidforge Architectural Design](docs/Voidforge%20Architectural%20Design.md): engine behavior, determinism rules, persistence and the API.
- [Voidforge Technical Plan](docs/Voidforge%20Technical%20Plan.md): stack, crate layering, phases, work packages and the [Agent factory rules](docs/Voidforge%20Technical%20Plan.md#agent-factory-rules).
- [Decision log](docs/decision-log.md): every team decision, including each new dependency.
- [Version table](docs/versions.md): pinned toolchain, tools and dependency versions.

## How work lands

- One work package from the Technical Plan is one pull request, and its title starts with the package ID (for example `P1.4: ...`). Packages run one at a time, in ID order.
- Each package is one increment on a working main: existing checks stay green and the package's "Done when" row is proven.
- Contracts land first: a package that adds public types or traits merges before the packages that use them.
- A new dependency, a change to a row of the Technical Plan's Decisions table, or a clarification of a document needs an entry in the [decision log](docs/decision-log.md). A new or changed version also goes in the [version table](docs/versions.md).
- Use the [pull request template](.github/pull_request_template.md).

## Workspace layering

Twelve crates in three layers; dependencies only point down. Each crate's `AGENTS.md` gives its purpose, allowed dependencies, invariants and test commands. The table in [Project layering](docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority.

| Crate | Kind | May depend on (workspace) |
| --- | --- | --- |
| [voidforge-core](crates/voidforge-core/AGENTS.md) | library | none; external crates serde and thiserror only |
| [voidforge-rules](crates/voidforge-rules/AGENTS.md) | library | core |
| [voidforge-testkit](crates/voidforge-testkit/AGENTS.md) | library, dev only | core, rules |
| [voidforge-worldgen](crates/voidforge-worldgen/AGENTS.md) | library | core, rules |
| [voidforge-store](crates/voidforge-store/AGENTS.md) | library | core |
| [voidforge-host](crates/voidforge-host/AGENTS.md) | library | core, store, telemetry |
| [voidforge-api-types](crates/voidforge-api-types/AGENTS.md) | library | none |
| [voidforge-telemetry](crates/voidforge-telemetry/AGENTS.md) | library | none |
| [voidforge-server](crates/voidforge-server/AGENTS.md) | binary | host, store, rules, worldgen, api-types, telemetry |
| [voidforge-web](crates/voidforge-web/AGENTS.md) | binary | api-types, telemetry only |
| [voidforge-bots](crates/voidforge-bots/AGENTS.md) | binary | api-types |
| [voidforge-replay](crates/voidforge-replay/AGENTS.md) | binary | core, rules, store |

Only `voidforge-host` changes the live `World`. `voidforge-replay` and tests also drive the core's state-changing entry points, offline and never against live state, and host recovery and replay share one function for turning a log record into a core call (P2.5). Only the binaries talk to the outside world. `voidforge-web` and `voidforge-bots` reach the server over HTTP alone.

`members = ["crates/*"]` makes every folder under `crates/` a workspace member, so anything placed there is built with the workspace.

## Commands

Run from the repository root. `rust-toolchain.toml` selects the pinned toolchain (see the [version table](docs/versions.md)).

| Purpose | Command |
| --- | --- |
| Build | `cargo build` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Format | `cargo fmt --all --check` |
| Tests | `cargo nextest run`, which needs [cargo-nextest](https://nexte.st) |

The workspace has no tests yet, so plain `cargo nextest run` exits with nextest's no-tests code (4). Until the first real test lands (P1.1 at the latest), use `cargo nextest run --no-tests=warn` on the command line. Never add a repository-wide override such as `NEXTEST_NO_TESTS`, and never add placeholder tests to satisfy the runner ([decision log D-004](docs/decision-log.md#d-004-zero-test-runs-before-the-first-real-test)).

## Guardrails

- `unsafe` is forbidden in every crate through the workspace lints. A crate cannot re-allow it.
- Every crate's `Cargo.toml` keeps `[lints] workspace = true`.
- The determinism rules in the [Architectural Design](docs/Voidforge%20Architectural%20Design.md#determinism-rules) apply to `voidforge-core`, and the Technical Plan extends the same bans to `voidforge-rules` and `voidforge-worldgen`. They are enforced by review for now. Clippy bans in `clippy.toml` arrive with P0.4.
- These are not in the repository yet; each arrives with its package: CI and `deny.toml` (P0.3), determinism clippy bans and the web dependency check (P0.4), `justfile` and `deploy/` (P0.5), `rules/` (P1.2), `tests/golden/` (P2.6) and `dockerfiles/` (P5.2).
