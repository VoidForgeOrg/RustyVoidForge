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

Eleven crates in three layers; dependencies only point down. Each crate's `AGENTS.md` gives its purpose, allowed dependencies, invariants and test commands. The table in [Project layering](docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority.

| Crate | Kind | May depend on (workspace) |
| --- | --- | --- |
| [voidforge-core](crates/voidforge-core/AGENTS.md) | library | none; runtime crates serde and thiserror only, test-only dev-dependencies allowed |
| [voidforge-rules](crates/voidforge-rules/AGENTS.md) | library | core |
| [voidforge-testkit](crates/voidforge-testkit/AGENTS.md) | library, dev only | core, rules |
| [voidforge-worldgen](crates/voidforge-worldgen/AGENTS.md) | library | core, rules |
| [voidforge-store](crates/voidforge-store/AGENTS.md) | library | core |
| [voidforge-host](crates/voidforge-host/AGENTS.md) | library | core, store, telemetry |
| [voidforge-api-types](crates/voidforge-api-types/AGENTS.md) | library | none |
| [voidforge-telemetry](crates/voidforge-telemetry/AGENTS.md) | library | none |
| [voidforge-server](crates/voidforge-server/AGENTS.md) | binary | host, store, rules, worldgen, api-types, telemetry |
| [voidforge-bots](crates/voidforge-bots/AGENTS.md) | binary | api-types, telemetry |
| [voidforge-replay](crates/voidforge-replay/AGENTS.md) | binary | core, rules, store, telemetry |

Only `voidforge-host` changes the live `World`. `voidforge-replay` and tests also drive the core's state-changing entry points, offline and never against live state, and host recovery and replay share one function for turning a log record into a core call (P2.5). Only the binaries talk to the outside world, and each starts by calling `voidforge_telemetry::init` ([D-016](docs/decision-log.md#d-016-telemetry-bootstrap)). `voidforge-bots` reaches the server over HTTP alone.

The web frontend is not a crate. It is a static TypeScript React app in [`web/`](web/AGENTS.md), with its own guide, that reaches the server over HTTP alone ([D-063](docs/decision-log.md#d-063-the-web-frontend-is-a-static-react-app)).

`members = ["crates/*"]` makes every folder under `crates/` a workspace member, so anything placed there is built with the workspace.

## Commands

Run from the repository root. `rust-toolchain.toml` selects the pinned toolchain, and the [version table](docs/versions.md) has the tool versions CI uses. These are the commands CI runs ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)), so a red check reproduces locally.

| CI job | Command |
| --- | --- |
| lint | `.github/scripts/check-no-managed-blocks.sh` |
| lint | `.github/scripts/test-check-no-managed-blocks.sh` |
| lint | `.github/scripts/check-dex-parity.sh` and its cases, `.github/scripts/test-check-dex-parity.sh`: the CI dex config is the local one plus exactly the password grant ([D-052](docs/decision-log.md#d-052-the-end-to-end-scenario-the-bots-and-the-e2e-job)) |
| lint | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| lint | `python3 tests/guardrails/check_no_floats.py` |
| lint | `python3 tests/guardrails/check_bots_deps.py` |
| lint | `python3 tests/guardrails/run_fixtures.py`, which proves the determinism bans with fixtures ([tests/guardrails](tests/guardrails/README.md)) |
| supply-chain | `cargo deny check`, which needs [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) |
| supply-chain | `npm audit --prefix web` |
| web | in `web/`: `npm ci`, `npm run gen:api` with no diff in `src/api/schema.ts`, `npm run typecheck`, `npm run lint`, `npm test`, `npm run build`; needs the Node and npm versions pinned in `web/` ([D-063](docs/decision-log.md#d-063-the-web-frontend-is-a-static-react-app)) |
| test | `cargo nextest run --workspace --locked`, which needs [cargo-nextest](https://nexte.st) |
| determinism | `.github/scripts/determinism.sh`: the determinism suite, then the two-process double run ([D-024](docs/decision-log.md#d-024-determinism-suite-canonical-bytes-and-the-two-process-run)) |
| database | `.github/scripts/check-migrations-only-add.sh <base>` and its cases, `.github/scripts/test-check-migrations-only-add.sh` ([D-034](docs/decision-log.md#d-034-schema-and-migrations-sqlx-and-the-database-tests-stage)) |
| database | `.github/scripts/test-database-lock.sh` against a Postgres service: the database stage's lock refuses a stale holder and keeps out a second run ([D-044](docs/decision-log.md#d-044-phase-2-gate-repair-verification-failures-the-record-pin-the-database-stage-lock-and-the-epoch)) |
| database | `.github/scripts/database.sh` against a Postgres service: the migrations on an empty database twice, `cargo sqlx prepare --check`, then the Postgres tests of store and host; needs [sqlx-cli](docs/versions.md) |
| e2e | `.github/scripts/e2e.sh` against compose's Postgres and dex with the CI config: bots play the scenario through the API against a fresh universe and the real `serve` ([D-052](docs/decision-log.md#d-052-the-end-to-end-scenario-the-bots-and-the-e2e-job)), then `.github/scripts/device-login.py`: the terminal login through dex's device code flow, approved through dex's own pages ([D-053](docs/decision-log.md#d-053-terminal-login-the-device-code-flow-and-the-saved-login)), then the web walk: Playwright logs alice in through dex and plays the MVP loop through the built web app in Chromium |
| e2e | `cargo nextest run -p voidforge-server --test dex --locked --run-ignored only`: the token checks against the real dex ([D-046](docs/decision-log.md#d-046-token-checks)) |

`cargo build` builds every crate; the web app builds with npm in `web/`. The [`justfile`](justfile) has the same commands as `just lint`, `just supply-chain`, `just test`, `just web`, `just determinism` and `just db-test`. `just db-test` needs the local stack (`just up`). It uses only a dedicated `voidforge_test` database, the `_sqlx_test_*` databases it creates and drops, its lock database `voidforge_test_lock` and the lock self-test's per-run `voidforge_test_lock_selftest_<pid>`, and never touches `voidforge`. One run at a time uses a server (a second run waits; a lock left by a killed run is refused with the command that clears it), and its cleanup never ends another session. A direct `cargo nextest` against `voidforge_test` doesn't take the lock; `just db-test` refuses to start while one is using a test database ([D-034](docs/decision-log.md#d-034-schema-and-migrations-sqlx-and-the-database-tests-stage)). The store's Postgres tests are `#[ignore]`d, so `just test` compiles them but only `just db-test` runs them. `just migrate` applies the migrations to an explicit `DATABASE_URL`, and has no default.

`just e2e` runs the e2e job's scenario. It needs the local stack with the CI dex config (`DEX_CONFIG=config.ci.yaml just up`; `E2E_ISSUER` names another dex with that config instead), `web/`'s packages and Playwright's Chromium (`npm ci --prefix web`, then `npx --prefix web playwright install chromium`), and refuses with that hint when dex lacks the password grant; it never reconfigures dex. It uses only its own per-run database `voidforge_e2e_<time>_<pid>`, dropped on exit, and never touches `voidforge`. At start it drops leftovers whose creating pid is gone and lists the rest with the command that drops them ([D-052](docs/decision-log.md#d-052-the-end-to-end-scenario-the-bots-and-the-e2e-job)). `just smoke` is the same script with `--smoke`: only the web walk (Playwright's login and the MVP loop through the web app), with the same needs and owner safety ([D-062](docs/decision-log.md#d-062-the-smoke-test-is-the-e2e-jobs-web-walk-by-name)).

### Local stack

`just up` starts Postgres, dex and otel-lgtm ([`deploy/`](deploy/README.md)), waits until each is healthy, then verifies them by effect: Postgres answers a query; dex serves discovery and keys, accepts and refuses the right flows, and completes a device code login; a host-network container sees the same issuer; and otel-lgtm accepts OTLP. It fails loudly if anything is wrong. `just status` re-runs the verification and `just down` stops the stack, keeping Postgres data. Everything is published on `localhost` only:

| Service | Address |
| --- | --- |
| Postgres | `postgres://voidforge:voidforge-local-test-password@localhost:5432/voidforge_dev` (`just seed` and `just serve`'s database; the server's `voidforge` database is the owner's, and nothing defaults to it, D-055) |
| dex | issuer `http://localhost:5556/dex` |
| Grafana | `http://localhost:3000` |
| OTLP | gRPC `localhost:4317`, HTTP `localhost:4318` |

Binaries export traces when `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318` is set, and log locally without it. `just trace-check` runs every binary against the stack and finds each one's span through Grafana's trace backend.

The credentials and the static dex users and clients are test-only values ([deploy/README.md](deploy/README.md)). Recipes are added by the packages that implement them, not stubbed ([D-014](docs/decision-log.md#d-014-local-stack)).

Never add a repository-wide nextest override such as `NEXTEST_NO_TESTS`, and never add placeholder tests to satisfy the runner. The command-local `--no-tests=warn` used while the workspace had no tests ended in P0.6, with the first real tests ([D-004](docs/decision-log.md#d-004-zero-test-runs-before-the-first-real-test)). A crate with no tests yet makes `cargo nextest run -p <crate>` exit 4, so use the workspace command for it.

## Guardrails

- `unsafe` is forbidden in every crate through the workspace lints. A crate cannot re-allow it.
- Every crate's `Cargo.toml` keeps `[lints] workspace = true`.
- The determinism rules in the [Architectural Design](docs/Voidforge%20Architectural%20Design.md#determinism-rules) apply to `voidforge-core`, and the Technical Plan extends the same bans to `voidforge-rules` and `voidforge-worldgen`. Clippy enforces the bans through identical `clippy.toml` files in those three crates and a `#![forbid(...)]` in each `lib.rs`, so there are no exceptions. Fixtures in `tests/guardrails/` prove every ban ([D-010](docs/decision-log.md#d-010-determinism-bans-in-core-rules-and-worldgen)). A `clippy.toml` anywhere else is a deliberate choice that needs its own decision-log entry. Clippy cannot see a float that is never named, so `tests/guardrails/check_no_floats.py` also rejects any `f32` or `f64` in those crates' compiled MIR ([D-013](docs/decision-log.md#d-013-type-aware-float-check)).
- `voidforge-bots` may depend only on `voidforge-api-types` and `voidforge-telemetry` among workspace crates, directly or transitively, in any dependency kind, and `voidforge-server` never depends on `voidforge-core` directly. `tests/guardrails/check_bots_deps.py` enforces both in CI ([D-011](docs/decision-log.md#d-011-web-dependency-isolation-check), [D-055](docs/decision-log.md#d-055-phase-3-gate-repair)).
- CI runs the `lint`, `supply-chain`, `test`, `determinism` and `database` jobs on every pull request and on main. The first four are required checks on main ([D-007](docs/decision-log.md#d-007-ci-baseline-and-required-checks), [D-024](docs/decision-log.md#d-024-determinism-suite-canonical-bytes-and-the-two-process-run)). `database` becomes one when the owner applies the ruleset change ([D-034](docs/decision-log.md#d-034-schema-and-migrations-sqlx-and-the-database-tests-stage)). The `web` job runs on every pull request too, and becomes required the same way ([D-063](docs/decision-log.md#d-063-the-web-frontend-is-a-static-react-app)). From P1.5, every package keeps the determinism suite green.
- Migrations only ever add: a migration that exists on main is never edited, renamed or deleted, and a new one sorts after the last. The `database` job enforces this ([D-034](docs/decision-log.md#d-034-schema-and-migrations-sqlx-and-the-database-tests-stage)). sqlx queries are checked at compile time against the committed `.sqlx/` metadata; regenerate it with `cargo sqlx prepare --workspace -- --all-targets` against a migrated database whenever a query changes.
- `deny.toml` holds the dependency allowlist. A crate outside it fails `supply-chain`, so a new dependency means a new allowlist entry, its license and a decision-log entry ([D-008](docs/decision-log.md#d-008-cargo-deny-policy-and-dependency-allowlist)).
- Agent-harness managed blocks never enter tracked files. The `lint` job rejects their markers ([D-009](docs/decision-log.md#d-009-managed-block-markers-rejected-in-tracked-files)).
- `rules/` holds the versioned rules files: `v0.toml` (Rules v0) and `v0-fast.toml` (an hour per second). `voidforge-rules` parses them into `voidforge_core::rules::Rules` ([D-020](docs/decision-log.md#d-020-rules-data-files-and-ids)). Every mass is in grams, every duration in milliseconds, and every distance in distance units: a thousandth of a world unit ([D-021](docs/decision-log.md#d-021-fixed-point-distance-units)).
- `tests/golden/` holds the golden logs (P2.6): one directory per scenario, changed only through `just bless`, and a reviewer checks every change ([D-040](docs/decision-log.md#d-040-golden-logs-scenarios-recording-the-fixture-build-and-bless)).
- Not in the repository yet; it arrives with its package: `dockerfiles/` (P5.2).
