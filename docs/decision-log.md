# Decision log

The team's technical decisions, newest last. The fourteen baseline choices live in the [Technical Plan's Decisions table](Voidforge%20Technical%20Plan.md#decisions). Changing one sets its row to Changed there and adds an entry here.

Add an entry for every new dependency, every Decisions-table change and every clarification of a document ([Agent factory rules](Voidforge%20Technical%20Plan.md#agent-factory-rules)). Versions themselves live in the [version table](versions.md).

Each entry records its date, the package that made it, the decision, the reasons, and where it is recorded or enforced.

### D-001: Rust toolchain 1.99.1, edition 2024

- Date and package: 2026-10-16, P0.1 (PR #1).
- Decision: `rust-toolchain.toml` pins channel `1.99.1` with the minimal profile plus `clippy` and `rustfmt`. Every crate uses edition 2024 and `rust-version = "1.99.1"` from `[workspace.package]`, and the workspace uses `resolver = "3"`.
- Why: 1.99.1 was the newest stable release when pinned. It was released 2026-10-13, and the upstream stable channel manifest dated 2026-10-15 lists it. Pinning the patch release gives local runs, CI and images the same compiler ([Tech stack](Voidforge%20Technical%20Plan.md#tech-stack)). Resolver 3 is the edition 2024 default, and a virtual manifest must state it.
- Changing it: an upgrade is its own pull request that updates `rust-toolchain.toml`, `rust-version`, the [version table](versions.md) and this log.

### D-002: Shared workspace lints, unsafe forbidden

- Date and package: 2026-10-16, P0.1 (PR #1).
- Decision: `[workspace.lints]` sets `unsafe_code = "forbid"`. It warns on `unused_lifetimes`, `unused_qualifications`, `clippy::all`, `clippy::dbg_macro`, `clippy::todo` and `clippy::unimplemented`. Every crate inherits these with `[lints] workspace = true`.
- Why: P0.1 requires workspace lints with `unsafe` forbidden. `forbid` cannot be re-allowed inside a crate (rustc E0453), which was checked in all twelve crates. Warnings become errors in CI through clippy `-D warnings` from P0.3 ([CI/CD](Voidforge%20Technical%20Plan.md#cicd)).

### D-003: Crate shapes and workspace registration

- Date and package: 2026-10-16, P0.1 (PR #1).
- Decision: `voidforge-server`, `voidforge-web`, `voidforge-bots` and `voidforge-replay` are binary crates; the other eight are libraries. All twelve are registered in `[workspace.dependencies]` by path. No crate declares a workspace or external dependency yet. `members = ["crates/*"]`.
- Why: the binaries are the processes the plan runs or ships, and the libraries are what they link ([Project layering](Voidforge%20Technical%20Plan.md#project-layering)). Edges are added by the packages that first need them.
- Open for P0.4 (from the P0.1 review):
  - The four binary entries in `[workspace.dependencies]` are inert. Cargo ignores a dependency on a crate with no library target and only warns, so a forbidden edge such as web to server does not fail the build today. P0.4's dependency check is the mechanical guard, and it should decide whether to drop these entries.
  - Every folder under `crates/` becomes a workspace member. P0.4's banned-call and forbidden-dependency fixtures must live outside `crates/` or be excluded, or they will break the workspace build.

### D-004: Zero-test runs before the first real test

- Date and package: 2026-10-16, P0.1. The rule is in the Technical Plan on main (aeee630, wording reconciled in ff0681c, PR #1).
- Decision: nextest fails a run that finds no tests (exit 4). While the workspace has no real test, `cargo nextest run --no-tests=warn` is used on the command line. P0.1 proved the runner that way. P0.3's `test` job does the same, with a `TODO(P1.1)` comment naming the package that removes it. The exception ends with the first package that adds a real test, P1.1 at the latest. After that the plain `cargo nextest run` must pass.
- Ruled out: repository-wide overrides such as `NEXTEST_NO_TESTS` in config or the environment, and placeholder tests written only to satisfy the runner. Either would let a run that finds no tests pass silently, for example after a filter typo.
- Recorded in: the P0.1 and P0.3 rows of [Phase 0](Voidforge%20Technical%20Plan.md#phase-0-foundations), and the zero-test rule in [Agent factory rules](Voidforge%20Technical%20Plan.md#agent-factory-rules).

### D-005: Where conventions live

- Date and package: 2026-10-16, P0.2.
- Decision: the decision log is `docs/decision-log.md`, the version table is `docs/versions.md`, and the pull request template is `.github/pull_request_template.md`. The root `AGENTS.md` holds project guidance, and each crate has its own `AGENTS.md`. The README links all of them.
- Why: P0.2 asks for these artifacts and the plan does not name their paths. Keeping the log and versions beside the specs in `docs/` means one folder holds everything agents must follow.
- Local note: an agent harness may add its own blocks to a local copy of the root `AGENTS.md` or `CLAUDE.md`. Those blocks are local to that checkout and are never committed. The tracked root `AGENTS.md` holds project guidance only.

### D-006: Offline callers of the core

- Date and package: 2026-10-16, a clarification of the Technical Plan on main (98341c2), recorded in P0.2.
- Decision: only `voidforge-host` changes the live `World`. `voidforge-replay`, which rebuilds a world from a snapshot and the log, and tests also drive the core's state-changing entry points, offline and never against live state. Host recovery and replay map a log record to `advance_to` or `apply` through one shared function: P2.5 adds it to a crate both may depend on, and P2.7 reuses it.
- Why: the earlier sentence ("only `voidforge-host` calls the core's state-changing entry points") contradicted replay's role and allowed dependencies. The rule exists to protect live state, not to grant exclusive call rights. One shared function keeps recovery and replay from drifting apart.
- Recorded in: [Project layering](Voidforge%20Technical%20Plan.md#project-layering).

### D-007: CI baseline and required checks

- Date and package: 2026-10-16, P0.3.
- Decision: one GitHub Actions workflow, `.github/workflows/ci.yml`, runs on pull requests to main and on pushes to main. It has three jobs, each a required status check on main from the GitHub Actions app:
  - `lint`: the managed-block check and a self-test that it rejects a marker, `cargo fmt --all --check`, and clippy with warnings as errors.
  - `supply-chain`: `cargo deny check`.
  - `test`: `cargo nextest run --workspace --locked --no-tests=warn`, with the D-004 removal TODO.
- Pins:
  - The runner is `ubuntu-24.04`, not `ubuntu-latest`.
  - Third-party actions are pinned to full commit SHAs, with the release tag in a comment.
  - cargo-deny and cargo-nextest are installed at exact versions.
  - The toolchain comes from `rust-toolchain.toml`.
  - Versions are listed in the [version table](versions.md).
- Caching: Swatinem/rust-cache on the jobs that compile (`lint` and `test`). `supply-chain` compiles nothing.
- Why: these are the lint, supply-chain, and unit and property test stages of [CI/CD](Voidforge%20Technical%20Plan.md#cicd) that P0.3 delivers. The determinism, database and end-to-end stages, and image builds, arrive with their packages. They are not stubbed here. Pinning by SHA keeps the workflow's inputs immutable. Requiring the checks from the GitHub Actions app means a status posted by anything else cannot satisfy them.

### D-008: cargo-deny policy and dependency allowlist

- Date and package: 2026-10-16, P0.3.
- Decision: `deny.toml` contains:
  - Advisories: deny vulnerabilities and yanked crates.
  - Licenses: none allowed yet, because no external crate exists. Unpublished workspace crates are ignored.
  - Bans: duplicate versions and wildcard requirements are denied, apart from workspace path dependencies.
  - Allowlist: `[bans].allow` names the twelve workspace crates. Once that list is non-empty, cargo-deny rejects any crate not on it, workspace crates included.
  - Sources: only crates.io.
- Adding a dependency: add it to `[bans].allow` and its license to `[licenses].allow` in the same pull request, with a decision-log entry and a [version table](versions.md) row ([Agent factory rules](Voidforge%20Technical%20Plan.md#agent-factory-rules)).
- Why: the [Tech stack](Voidforge%20Technical%20Plan.md#tech-stack) makes cargo-deny responsible for licenses, advisories and a dependency allowlist, and the allowlist makes "a new dependency needs a decision" mechanical.

### D-009: Managed-block markers rejected in tracked files

- Date and package: 2026-10-16, P0.3.
- Decision: the `lint` job runs `.github/scripts/check-no-managed-blocks.sh`. It fails if any tracked file contains the agent-harness managed-block marker. The script builds the marker from two halves so its own source never matches. A second step builds a throwaway repository with a marker and checks that the script rejects it.
- Why: the root `AGENTS.md` and `CLAUDE.md` may carry local managed blocks in an owner checkout (D-005), protected there by `skip-worktree` and a local exclude. This check is the mechanical backstop if either protection is lost.
