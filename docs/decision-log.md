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
- Decision: `voidforge-server`, `voidforge-web`, `voidforge-bots` and `voidforge-replay` are binary crates; the other eight are libraries. All twelve were registered in `[workspace.dependencies]` by path; since P0.4 only the eight libraries are. No crate declares a workspace or external dependency yet. `members = ["crates/*"]`.
- Why: the binaries are the processes the plan runs or ships, and the libraries are what they link ([Project layering](Voidforge%20Technical%20Plan.md#project-layering)). Edges are added by the packages that first need them.
- Resolved in P0.4, from the P0.1 review:
  - The four binary crates are no longer in `[workspace.dependencies]`. Before, such an entry was inert: cargo ignores a dependency on a crate with no library target and only warns. Now `<binary>.workspace = true` fails cargo outright. A path dependency on a binary crate is still only a warning to cargo, so the web check reads declared dependencies as well as the resolved graph (D-011).
  - `members = ["crates/*"]` makes every folder under `crates/` a workspace member, so the guardrail fixtures live in `tests/guardrails/` and run against a temporary copy of the workspace (D-012).

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
  - `lint`: the managed-block check and its regression cases, `cargo fmt --all --check`, and clippy with warnings as errors.
  - `supply-chain`: `cargo deny check`.
  - `test`: `cargo nextest run --workspace --locked --no-tests=warn`, with the D-004 removal TODO.
- Pins:
  - The runner is `ubuntu-24.04`, not `ubuntu-latest`.
  - Third-party actions are pinned to full commit SHAs, with the release tag in a comment.
  - cargo-deny and cargo-nextest are installed at exact versions.
  - The toolchain comes from `rust-toolchain.toml`.
  - Versions are listed in the [version table](versions.md).
- Caching: Swatinem/rust-cache on the jobs that compile (`lint` and `test`). `supply-chain` compiles nothing.
- Required checks: the repository ruleset "main: required CI checks" (id 25258005) targets the default branch. It requires `lint`, `supply-chain` and `test` from the GitHub Actions app (integration 15368), requires branches to be up to date before merging, and has no bypass actors.
  - Consequence: a commit can reach main only through a pull request whose three checks passed on its up-to-date head. Direct pushes to main are refused, documentation changes included.
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
- Decision: the `lint` job runs `.github/scripts/check-no-managed-blocks.sh`. It fails if any tracked file contains the agent-harness managed-block marker. The script builds the marker from two halves so its own source never matches. It exits 0 when clean, 1 when markers are found, and 2 when it cannot check, such as a missing directory, a non-repository or a Git error. An error therefore fails the job instead of passing it (fixed after the P0.3 review found the first version failed open). `.github/scripts/test-check-no-managed-blocks.sh` runs regression cases in throwaway repositories on every CI run: clean, committed marker, staged marker, markers only in untracked or skip-worktree files, non-repository, Git failure and missing directory.
- Why: the root `AGENTS.md` and `CLAUDE.md` may carry local managed blocks in an owner checkout (D-005), protected there by `skip-worktree` and a local exclude. This check is the mechanical backstop if either protection is lost.

### D-010: Determinism bans in core, rules and worldgen

- Date and package: 2026-10-16, P0.4. Agreed with orch-advisor.
- Decision: `crates/voidforge-{core,rules,worldgen}/clippy.toml` are identical and list the banned paths. Each of those crates' `lib.rs` has `#![forbid(clippy::disallowed_methods, clippy::disallowed_types, clippy::float_arithmetic)]`, so an `#[allow]` or `#[expect]` is a hard error (E0453): there are no exceptions. The categories are:
  - Clocks: `Instant` and `SystemTime` (the types, `now` and `elapsed`), and uuid's `Timestamp::now`.
  - Hash-ordered collections: `HashMap` and `HashSet`.
  - Threads: `spawn`, `scope`, `sleep`, `Builder`, `JoinHandle`, `Scope` and `ScopedJoinHandle`.
  - Locks and concurrency primitives: `Mutex`, `RwLock`, `Condvar`, `Barrier`, `Once`, `OnceLock`, `LazyLock`, mpsc channels and their ends, and every `std::sync::atomic` type.
  - System randomness: `std::hash::RandomState`, and getrandom's `fill`, `fill_uninit`, `u32`, `u64` and `SysRng`.
  - UUID generation: uuid's `new_v1`, `now_v1`, `new_v3`, `new_v4`, `new_v5`, `new_v6`, `now_v6`, `new_v7`, `now_v7` and `new_v8`.
  - Floats: the `f32` and `f64` types, plus `clippy::float_arithmetic`.
- Placement: clippy reads the nearest `clippy.toml` walking up from each crate, so a root file would also ban clocks and threads in host and server, which legitimately use them. The Technical Plan's layout was updated to match. A `clippy.toml` in any other crate, or at the root, is a deliberate choice that needs its own decision-log entry. The fixture run fails on any other `clippy.toml`, so adding one means updating that check in the same pull request as the entry.
- Paths in crates that are not dependencies yet (uuid, getrandom) use `allow-invalid`. Clippy cannot report a typo in those, so the fixture run requires every banned path to be expected by a fixture that adds the real crate and triggers it (D-012). Std paths have no `allow-invalid`, so a typo produces a clippy warning, and the clean baseline fails on it.
- Not banned, deliberately:
  - `Arc`: shared ownership is not concurrency by itself.
  - `Duration`: a value, not a clock.
  - Name-free float literals that never reach arithmetic or a typed binding. They cannot reach state without naming a float type.
  - External hash-map crates. A new crate must first pass the `deny.toml` allowlist and its decision entry, where this rule applies (D-008).
- Why: these are the Technical Plan P0.4 bans and the lint-enforced rows of the Architectural Design's [Determinism rules](Voidforge%20Architectural%20Design.md#determinism-rules).

### D-011: Web dependency isolation check

- Date and package: 2026-10-16, P0.4.
- Decision: `tests/guardrails/check_web_deps.py` runs in the `lint` job. It reads `cargo metadata`. It fails if `voidforge-web` declares a dependency of any kind (normal, dev or build, renamed or by path) on a workspace crate other than `voidforge-api-types` and `voidforge-telemetry`, or if any such crate appears anywhere in its resolved dependency graph. It prints the offending path. Exit codes: 0 isolated, 1 violation, 2 could not check.
- Why: [Project layering](Voidforge%20Technical%20Plan.md#project-layering) allows web only those two workspace crates, so it never links core, host, store or server, even transitively. The declared-dependency pass catches a path dependency on a binary crate, which cargo ignores with only a warning (D-003).

### D-012: Guardrail fixtures

- Date and package: 2026-10-16, P0.4.
- Decision: `tests/guardrails/run_fixtures.py` (Python 3 standard library only) runs in the `lint` job. It copies the workspace to a temporary directory and injects each fixture into the real crates, so their own `clippy.toml` and lint attributes are what gets tested. Then it runs the CI clippy command and compares diagnostics. The cases:
  - Each `determinism/<category>.rs` must be rejected in core, rules and worldgen with exactly the diagnostics its `expect:` lines name, all inside the fixture, and nothing else. It must pass in `voidforge-host`, which shows the bans are scoped.
  - Each `<category>.ok.rs` control must pass in all three.
  - `meta/` traps prove the harness reports a ban that does not fire as MISSING, and an unrelated compile or dependency error as ERROR, never as a rejection.
  - Web cases edit manifests and check the checker's exit code and exact violation lines.
  - It also checks that the three `clippy.toml` files are identical, that no other `clippy.toml` exists, and that every banned path has a fixture.
- Fixture-only crates: uuid 1.28.0, getrandom 0.4.3 and rand_chacha 0.10.0 are added only inside the temporary copy. They never enter the workspace's `Cargo.lock` or the `deny.toml` allowlist. See the [version table](versions.md).
- Why: the P0.4 done-when needs a failing fixture for each banned call and forbidden dependency. Matching exact diagnostics keeps a missing import, a resolution failure or a different lint from counting as a rejection. Python is preinstalled on the CI runner and avoids adding a crate to the workspace.
