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
- Ended in P0.6: `voidforge-telemetry` added the first real tests, so CI, the justfile and every guide now run plain `cargo nextest run --workspace --locked`. A crate without tests still makes `-p <crate>` exit 4; its guide says to use the workspace command.
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
  - `test`: `cargo nextest run --workspace --locked`. It carried `--no-tests=warn` until P0.6 (D-004).
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
  - Floats: the `f32` and `f64` types, plus `clippy::float_arithmetic`. Clippy cannot see a float that is never named, so D-013 adds a type-aware check.
- Placement: clippy reads the nearest `clippy.toml` walking up from each crate, so a root file would also ban clocks and threads in host and server, which legitimately use them. The Technical Plan's layout was updated to match. A `clippy.toml` in any other crate, or at the root, is a deliberate choice that needs its own decision-log entry. The fixture run fails on any other `clippy.toml`, so adding one means updating that check in the same pull request as the entry.
- Paths in crates that are not dependencies yet (uuid, getrandom) use `allow-invalid`. Clippy cannot report a typo in those, so the fixture run requires every banned path to be expected by a fixture that adds the real crate and triggers it (D-012). Std paths have no `allow-invalid`, so a typo produces a clippy warning, and the clean baseline fails on it.
- Not banned, deliberately:
  - `Arc`: shared ownership is not concurrency by itself.
  - `Duration`: a value, not a clock.
  - External hash-map crates. A new crate must first pass the `deny.toml` allowlist and its decision entry, where this rule applies (D-008).
- Why: these are the Technical Plan P0.4 bans and the lint-enforced rows of the Architectural Design's [Determinism rules](Voidforge%20Architectural%20Design.md#determinism-rules).

### D-011: Web dependency isolation check

- Date and package: 2026-10-16, P0.4.
- Decision: `tests/guardrails/check_web_deps.py` runs in the `lint` job. It reads `cargo metadata`. It has two passes, over every dependency kind (normal, dev and build), and prints the offending path. Exit codes: 0 isolated, 1 violation, 2 could not check.
  - Resolved: any workspace crate other than `voidforge-api-types` and `voidforge-telemetry` anywhere in the graph cargo resolves from `voidforge-web` fails.
  - Declared: a manifest declaration of such a crate, by name, path or rename, fails. This covers `voidforge-web` and every workspace crate it reaches.
- Scope, stated honestly: for a declaration cargo drops from the graph, such as a path dependency on a binary-only crate, this enforces the declaration. Cargo links no library from a binary crate. Crates `voidforge-web` does not reach are not checked here.
- Fixed in P0.4 review: QA found that the first version read only web's own declarations, so web → telemetry → server by path passed. Reachable crates' declarations are now checked, with a regression case and allowed controls. Since P0.6 every binary depends on telemetry, so telemetry declaring a binary would be a cycle; the regression case uses api-types as the reachable crate instead.
- Why: [Project layering](Voidforge%20Technical%20Plan.md#project-layering) allows web only those two workspace crates, so it never links core, host, store or server, even transitively. The declared pass catches a path dependency on a binary crate, which cargo ignores with only a warning (D-003).

### D-012: Guardrail fixtures

- Date and package: 2026-10-16, P0.4.
- Decision: `tests/guardrails/run_fixtures.py` (Python 3 standard library only) runs in the `lint` job. It copies the workspace to a temporary directory and injects each fixture into the real crates, so their own `clippy.toml` and lint attributes are what gets tested. Then it runs the CI clippy command and compares diagnostics. The cases:
  - Each `determinism/<category>.rs` must be rejected in core, rules and worldgen with exactly the diagnostics its `expect:` lines name, all inside the fixture, and nothing else. It must pass in `voidforge-host`, which shows the bans are scoped.
  - Each `<category>.ok.rs` control must pass in all three.
  - `meta/` traps prove the harness reports a ban that does not fire as MISSING, and an unrelated compile or dependency error as ERROR, never as a rejection.
  - Web cases edit manifests and check the checker's exit code and exact violation lines.
  - It also checks that the three `clippy.toml` files are identical, that no other `clippy.toml` exists, and that every banned path has a fixture.
  - Each `float_values/<name>.rs` must be rejected by the float check (D-013) in all three crates, naming exactly its `expect-float-fn:` items; `.ok.rs` must pass and `.err.rs` must exit 2.
- Fixture-only crates: uuid 1.28.0, getrandom 0.4.3, rand_chacha 0.10.0 and toml 1.1.8 are added only inside the temporary copy. They never enter the workspace's `Cargo.lock` or the `deny.toml` allowlist. See the [version table](versions.md).
- Why: the P0.4 done-when needs a failing fixture for each banned call and forbidden dependency. Matching exact diagnostics keeps a missing import, a resolution failure or a different lint from counting as a rejection. Python is preinstalled on the CI runner and avoids adding a crate to the workspace.

### D-013: Type-aware float check

- Date and package: 2026-10-16, P0.4, from QA's review of the first candidate.
- Decision: `tests/guardrails/check_no_floats.py` runs in the `lint` job. It compiles the libraries of `voidforge-core`, `voidforge-rules` and `voidforge-worldgen` to MIR with stable `rustc --emit=mir`. It fails on any `f32` or `f64` there, naming the function, closure, const or static. String literals and the MIR allocation blocks that dump constant data as bytes are skipped. A float constant still shows up through its typed item or local. Exit codes: 0 none, 1 floats found, 2 could not check (build failure or no MIR).
- Why: QA showed `1.25f64.sqrt().to_bits()` passes clippy in all three crates. It names no float type and uses no float operator, so neither `disallowed_types` nor `float_arithmetic` sees it, and clippy has no type-aware float lint. MIR records the type of every local, constant and call, so a float computed by a method, a cast literal, a trait call, a closure, a const or a value returned by std or a dependency all show up. The clippy bans stay, because they point at the source line.
- Scope: library code of the three crates. Their test code is covered by the clippy bans, which run with `--all-targets`, but not by this check.
- Proof: the `float_values/` fixtures in D-012 must be rejected in all three crates, naming exactly the expected items; that includes a promoted constant array, whose bytes sit in a skipped allocation. Integer code and "f64" in long, short and byte-string constants must pass, and a crate that does not compile must make the check exit 2.
- Fixed in P0.4 review: QA found that a short string such as "f32 f64" was rejected, because MIR dumps it on one line without an address. Whole allocation blocks are now skipped, and skipping also stops at the next item header, so an unexpected format can never hide later code.

### D-014: Local stack

- Date and package: 2026-10-16, P0.5.
- Decision: `deploy/compose.yaml` (Compose project `voidforge`) runs Postgres 18.6, dex v2.46.0 and otel-lgtm 0.36.0. Images are pinned by tag and digest, and each service has a real healthcheck:
  - Postgres: `pg_isready`.
  - dex: its discovery document answers.
  - otel-lgtm: the ready file plus every component, collector included. This is stricter than the image's own check, which skips components that are not listening.

  Every port is published on 127.0.0.1 only, and Postgres data lives in the `voidforge_postgres-data` volume.
- dex configuration: `deploy/dex/config.yaml` (local) and `config.ci.yaml` (CI) are identical except that CI adds the password grant. The local config allows authorization code, refresh and device code. Client credentials are never allowed (Technical Plan, Decisions 8). Storage is in memory, there are two static test users, the confidential `voidforge-web` client redirects to `http://localhost:8081/callback`, and the public `voidforge-cli` client uses the device flow. All credentials are test-only.
- Config injection: the dex config is baked into a small local image (`deploy/dex/Dockerfile`, selected by `DEX_CONFIG`) instead of bind-mounted. On the owner's machine the colima VM cannot read `~/Documents`, so bind mounts and Compose file `configs` both fail with "operation not permitted". A build context is sent through the Docker API, so this works on any host without changing machine settings.
- `just up` runs `docker compose up --build --wait` and then `deploy/verify_stack.py`. The verifier checks every service by effect and exits 0 healthy, 1 a check failed, or 2 could not check (Docker or Compose missing). Its checks:
  - Postgres: a TCP connection and `select 1`.
  - dex from the host: discovery with exactly the expected grant types, PKCE S256 and JWKS keys. A device code is issued to the CLI client. The web client's PKCE request reaches the login page, and an unregistered redirect gets 400. Client credentials are refused. The password grant is refused locally, and with the CI config it issues an ID token with the right `iss` and `aud`.
  - dex from a host-network container: the same issuer and key IDs (D-015).
  - otel-lgtm: Grafana health, an OTLP/HTTP export accepted, and OTLP/gRPC listening.
- The `justfile` has `up`, `status`, `down`, `logs`, and `lint`, `supply-chain` and `test`, which run the same commands as the CI jobs of those names. The plan's other recipes, `e2e` (P3.6), `replay` (P2.7), `seed` (P2.9) and `bless` (P2.6), are added by the packages that implement them rather than stubbed, so no recipe pretends to work.

### D-015: One dex issuer for the browser, the host and containers

- Date and package: 2026-10-16, P0.5. Agreed with orch-advisor; the Technical Plan's Local development paragraph was updated in the same pull request.
- Decision: the issuer stays `http://localhost:5556/dex`. dex is published on `localhost:5556`, and containers that need the issuer use host networking. Today that is the discovery probe in `just up`; later the server and web images. A host-network container cannot use compose service names, so it reaches Postgres and the other services through their published `localhost` ports, as a binary run with cargo does. `just up` fails loudly with a hint if the host-network probe cannot reach dex. Verified with colima (Docker 29.5.2 in its Linux VM). Docker on a Linux host supports host networking by design, but this stack has not run there yet; the first such run is the CI end-to-end stage (P3.6). Docker Desktop needs its host-networking setting enabled.
- Evidence (dex v2.46.0, Docker 29.5.2 with colima):
  - The plan's "matching host alias" cannot redirect `localhost`. With `--add-host localhost:203.0.113.7`, `localhost` still resolves to loopback in the container.
  - An issuer such as `http://dex.localhost:5556/dex` with a compose network alias works for glibc and musl resolvers, but curl in a container resolves `*.localhost` to loopback regardless of DNS, so it depends on the client.
  - With host networking, curl and busybox in a container got the same discovery and issuer as the host, and a bridge-network container could not connect.
- Fallback for the Phase 5 images, if host networking becomes painful: keep the issuer string `http://localhost:5556/dex`, and have the containers fetch discovery and keys from an internal URL such as `http://dex:5556/dex/...`. That is a configuration split, not an issuer change. It is not built now.

### D-016: Telemetry bootstrap

- Date and package: 2026-10-16, P0.6. The layering change was agreed with orch-advisor and recorded in the Technical Plan in the same pull request.
- Decision: `voidforge_telemetry::init(service_name)` is the one telemetry setup. Every binary calls it first and calls `Telemetry::shutdown` before exit. It:
  - installs a global `tracing` subscriber that logs INFO and above to stderr;
  - when `OTEL_EXPORTER_OTLP_ENDPOINT` is set, adds an OpenTelemetry layer whose batch exporter sends OTLP/HTTP protobuf to `<endpoint>/v1/traces`, with `service.name` set to the binary's name and a 5-second export timeout;
  - when the variable is unset, logs locally and exports nothing, so a hand or offline run never waits on a collector.

  `shutdown` flushes and returns an error for spans it could not deliver, and the stub binaries exit 1 on that error. A second `init` in one process is an error. `trace_id(&span)` exposes a span's trace ID, so a run's trace can be found.
- Layering: Project layering now lets `voidforge-bots` and `voidforge-replay` depend on `voidforge-telemetry`. P0.6 says every binary uses the init function, and telemetry depends on no workspace crate, so this adds no path to the core. The web check (D-011) still allows web only api-types and telemetry, and the core stays uninstrumented.
- Stub binaries: each binary's `main` records one `stub_startup` span with attribute `binary` and prints `trace_id=<hex>` when exporting. `just trace-check` (`deploy/verify_trace.py`) runs all four against the local stack and finds each trace's span through Grafana's Tempo datasource. It also checks that an offline run exits 0 without exporting, and that an unreachable endpoint makes the binary exit 1 with the delivery error. The verifier waits for the span itself, because Tempo can answer 200 with an empty trace before ingestion completes.
- Out of scope: command-path spans and metrics belong to P5.1. Configuration through clap structs arrives with each binary's features. Until then the standard OpenTelemetry variable is the only setting.
- Tests: four unit tests in `voidforge-telemetry` against an in-process OTLP/HTTP listener: delivery, the delivery error, offline, and second init. They are the workspace's first real tests, which ended the D-004 exception.

### D-017: Telemetry dependencies, licenses and duplicates

- Date and package: 2026-10-16, P0.6.
- Direct dependencies of `voidforge-telemetry`, pinned exactly in `[workspace.dependencies]`:
  - tracing 0.1.44;
  - tracing-subscriber 0.3.23, with features std, fmt and registry, and no env-filter or ANSI;
  - opentelemetry 0.33.1 and opentelemetry_sdk 0.33.1, trace feature only;
  - opentelemetry-otlp 0.33.1: http-proto, reqwest-blocking-client and trace. No gRPC and no TLS, because the collector is local;
  - tracing-opentelemetry 0.34.0, no default features. It is the release built against opentelemetry 0.33;
  - thiserror 2.0.21.

  Binaries add `tracing` for their spans.
- Why these: the latest releases that work together, according to each crate's published dependency requirements on crates.io. The blocking HTTP client needs no async runtime in the binaries, and without TLS no OpenSSL or ring enters the tree.
- `deny.toml`:
  - Licenses: Apache-2.0, MIT and Unicode-3.0. Unicode-3.0 is required by the ICU crates under `url`; other licenses in the tree are only alternatives in dual-licensed crates.
  - Targets: cargo-deny checks only the targets we build (x86_64 and aarch64, Linux and macOS), so wasm and Windows-only crates are not allowlisted.
  - Allowlist: the 98 crates the telemetry stack brings in, listed by name.
  - Duplicates: one skip, `syn@2.0.119`. Proc-macro crates are mid-way from syn 2 to syn 3 (prost-derive and tracing-attributes use 2; async-trait and displaydoc use 3). Both are compile-time only. `multiple-versions = "deny"` otherwise stays.
