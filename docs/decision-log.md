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

- Date and package: 2026-10-16, P0.3. Licenses, targets, the allowlist and the duplicate skip were extended by [D-017](#d-017-telemetry-dependencies-licenses-and-duplicates) in P0.6; the policy and the procedure for adding a dependency below still hold. The licenses and allowlist described here are the state at P0.3.
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
- Decision: `tests/guardrails/check_no_floats.py` runs in the `lint` job. It compiles the libraries of `voidforge-core`, `voidforge-rules` and `voidforge-worldgen` to MIR with stable `rustc --emit=mir`. It fails on `f32` or `f64` there, naming the function, closure, const or static: as a type or path word (`f64`, `core::f64::...`), right after a digit (a float literal such as `const 0.5f64`) or right after an underscore (a float API such as `from_secs_f64`). String literals and the MIR allocation blocks that dump constant data as bytes are skipped. A float constant still shows up through its typed item or local. Exit codes: 0 none, 1 floats found, 2 could not check (build failure or no MIR).
- Why: QA showed `1.25f64.sqrt().to_bits()` passes clippy in all three crates. It names no float type and uses no float operator, so neither `disallowed_types` nor `float_arithmetic` sees it, and clippy has no type-aware float lint. MIR records the type of every local, constant and call, so a float computed by a method, a cast literal, a trait call, a closure, a const or a value returned by std or a dependency all show up. The clippy bans stay, because they point at the source line.
- Scope: library code of the three crates. Their test code is covered by the clippy bans, which run with `--all-targets`, but not by this check.
- Proof: the `float_values/` fixtures in D-012 must be rejected in all three crates, naming exactly the expected items; that includes a promoted constant array, whose bytes sit in a skipped allocation. Integer code and "f64" in long, short and byte-string constants must pass, and a crate that does not compile must make the check exit 2.
- Fixed after the Phase 0 gate review: review-r1 found that `Duration::from_secs_f64(0.5)` passed both float guards. A literal passed straight into a call creates no float-typed local, and the pattern needed a word boundary before `f64`, which `0.5f64` and `from_secs_f64` lack. The pattern now also matches after a digit or underscore. Regression fixtures:
  - float literal arguments, and other literal spellings;
  - a constant passed by name, caught only through the underscore case;
  - a literal given to `toml::Value::Float`, caught only through the digit case. Controls: integer suffixes, hex digits spelling `f64`, and identifiers with `f64` inside. They fail on the old pattern and pass on the new one.
- Remaining limit: the check reads MIR text, so a float that appears nowhere in a function's MIR goes unseen. An example is an external crate's float constant passed straight to that crate's function, if the function name doesn't end in `_f32` or `_f64`. No dependency these crates may use today has such an API. Reviewers of the first dependency added to them should check it. The MIR text format is not a stable interface, so a toolchain upgrade must re-run these fixtures ([version table](versions.md#pinned-in-the-repository)).
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

  `shutdown` flushes and then always shuts the exporter down, even when the flush failed, and returns an error for spans it could not deliver. The stub binaries exit 1 on that error. In the first candidate a failed flush skipped the shutdown, so the exporter kept sending spans recorded later; review-r2 found it, and a regression test now covers it. A second `init` in one process is an error. `trace_id(&span)` exposes a span's trace ID, so a run's trace can be found.
- Layering: Project layering now lets `voidforge-bots` and `voidforge-replay` depend on `voidforge-telemetry`. P0.6 says every binary uses the init function, and telemetry depends on no workspace crate, so this adds no path to the core. The web check (D-011) still allows web only api-types and telemetry, and the core stays uninstrumented.
- Stub binaries: each binary's `main` records one `stub_startup` span with attribute `binary` and prints `trace_id=<hex>` when exporting. `just trace-check` (`deploy/verify_trace.py`) runs all four against the local stack and finds each trace's span through Grafana's Tempo datasource. It also checks that an offline run exits 0 without exporting, and that an unreachable endpoint makes the binary exit 1 with the delivery error. The verifier waits for the span itself, because Tempo can answer 200 with an empty trace before ingestion completes.
- Out of scope: command-path spans and metrics belong to P5.1. Configuration through clap structs arrives with each binary's features. Until then the standard OpenTelemetry variable is the only setting.
- Tests: five unit tests in `voidforge-telemetry` against an in-process OTLP/HTTP listener: delivery, the delivery error, a rejected flush still stopping the exporter, offline, and second init. They are the workspace's first real tests, which ended the D-004 exception.

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

### D-018: Quantities and crossing times

- Date and package: 2026-10-17, P1.1. Agreed with orch-advisor; the Architectural Design's Rounding formulas and Determinism rules were corrected in the same pull request.
- Decision: `voidforge_core::quantity::Quantity` holds:
  - a value in grams;
  - a rate in grams per hour;
  - a remainder in `0..3_600_000` (units of 1/3,600,000 g);
  - a checkpoint time in milliseconds.

  The API:
  - `value_at(t)` reads purely: `value + floor((rate·Δt + remainder) / 3,600,000)`, with Euclidean division and a non-negative remainder, in 128-bit intermediates.
  - `advanced_to(t)` checkpoints, carrying the remainder; `with_rate(t, rate)` checkpoints and then changes the rate.
  - `rising_crossing(B)` and `falling_crossing(B)` return the first millisecond at or after the checkpoint at which that read reaches B, or `None` when the rate never gets there.
  - `snapped(B, t)` sets the value to B and the remainder to 0.

  `MS_PER_HOUR` is fixed at 3,600,000 and never scaled by rules.
- Crossing formulas: rising Δ = ⌈((B − v)·H − ρ) / r⌉; falling Δ = ⌊((v − B − 1)·H + ρ) / s⌋ + 1 with s = −r; Δ = 0 if already there. The architecture's "mirrored form" would be ⌈((v − B)·H + ρ) / s⌉. It disagreed with millisecond stepping in about half of generated falling cases: 1 g at −1 g/h reads 0 at 1 ms, while the mirror says one hour. So the architecture now states the falling form and the crossing definition, and a crossing discards less than one millisecond of flow plus one gram, which bounds the conservation-test tolerance.
- Errors (`QuantityError`):
  - a remainder outside its range;
  - a time before the checkpoint;
  - a value outside `i64`;
  - a crossing time outside `i64`.

  Nothing wraps or saturates.
- Proof: proptest with a fixed seed and 512 cases per property compares reads, checkpoints, rising and falling crossings, partitioned checkpointing and snapping against a reference that steps one millisecond at a time through a gram accumulator, with no division or crossing formula. A coverage test asserts that every class of generated case is sampled. There are named cases for the game design's hour-500 example, falling debit, remainders, exact and one-short boundaries, overshoot and snap, zero rate, already-past, rate changes, and each error. Six deliberately planted arithmetic defects each fail at least one generated oracle property.

### D-019: Test-only dev-dependencies in the core

- Date and package: 2026-10-17, P1.1. Agreed with orch-advisor.
- Decision: Project layering's "serde and thiserror only" for `voidforge-core` means runtime dependencies. Test tooling from the Tech stack may be a `[dev-dependencies]` entry, which never links into the library. The first is proptest 1.12.0 (std only: no fork or timeout features), with a fixed seed and no failure-persistence files, so every run explores the same cases. The determinism clippy bans still apply to test code (`--all-targets`). The MIR float check covers library code only (D-013). `deny.toml` allowlists proptest and its eight dependencies.

### D-020: Rules data, files and IDs

- Date and package: 2026-10-17, P1.2. The IDs and the validation split were agreed with orch-advisor.
- Typed specs in `voidforge_core::rules`: `Rules` holds `version`, `name`, `planet`, `homeworld`, `buildings` (drill, refinery, shipyard), `ships` (colony_ship, cargo_vessel), `scoring` and `worldgen` (the P1.12 defaults). Fields are fixed per kind, with no maps. Masses are grams, durations milliseconds, rates grams per hour, distances distance units (D-021). Counts are `u32`; everything else is `i64`.
- Validation in core, `Rules::validate`, on the first broken invariant, naming the field:
  - positive slots, caps, costs, times, rates, speeds, parallel builds and worldgen sizes;
  - homeworld buildings fit the slots, and stocks fit within the caps;
  - every build's cost over its time is an exact whole number of grams per hour within 64 bits (`BuildCost::rate_g_per_h` is `Some`), so a build fed at that rate takes exactly its build time;
  - refinery output = intake × `ingots_per_ore`;
  - Cargo Vessel capacity is positive, while other ships may carry nothing;
  - score points are not negative and `per_g` is positive;
  - planet spacing is less than the cube.

  It lives in core so every source of rules gets the same checks.
- Parsing in `voidforge-rules`: `parse(text)` reads TOML with serde into raw structs that reject unknown fields, require every field, and accept integers only. It converts them into the core types and validates. It does no I/O. Schema errors carry the field and line, invariant errors the field path.
- Files: `rules/v0.toml` is the game design's Rules v0 plus the P1.12 world generation defaults, in base units with the source values as comments. `rules/v0-fast.toml` is v0 with rates and speeds ×3,600 and durations ÷3,600. Masses, capacities, slots, starting stocks, scoring and distances are unchanged, and so is the clock. A test proves the fast file equals that transform of v0, and every v0 duration divides exactly.
- IDs: each file has an integer `version` (the `rules_version` in log records and snapshots) and a `name`: v0 is 0 "v0", v0-fast is 1 "v0-fast". An ID identifies a rule set and doesn't order balancing generations. An ID is assigned once and never reused or edited; a changed rule set gets a new ID.

### D-021: Fixed-point distance units

- Date and package: 2026-10-17, P1.2. Chosen by orch-advisor; the Architectural Design's State model row now states the scale.
- Decision: 1 world unit = 1,000 distance units (`DISTANCE_UNITS_PER_WORLD_UNIT`). Coordinates, distances and speeds are integer distance units (speeds per hour). Rules v0 speeds are 10,000 and 15,000 distance units per hour; fast speeds are 36,000,000 and 54,000,000. World generation still places planets on whole world units (P1.12) and stores them ×1,000. Rules worldgen distances are in distance units.
- Why: the architecture calls these fixed-point. With whole world units, the integer square root in the distance formula would lose up to one world unit, about six minutes of Colony Ship travel, on every trip. Post-MVP interception needs positions mid-flight. The values stay small: coordinates up to 1e6, summed squares about 3e12, distance × 3,600,000 about 6e12, all within `i64`.

### D-022: State model, IDs and test builders

- Date and package: 2026-10-17, P1.3. Representation agreed with orch-advisor, including two changes the advisor asked for: builds keep their start terms, and the planet's next event is stored on the planet.
- Decision: `voidforge_core::state::World` holds `time_ms`, `rules_version`, `seed`, `next_id`, BTreeMaps of players, planets, buildings, ship builds, ships and fleets, the ordered `homeworld_candidates`, and the event queue's data. Every collection iterates in ID order.
  - **IDs**: one global `u64` counter shared by planets, buildings, ship builds, ships and fleets, wrapped in typed newtypes. `World::allocate_id` fails with `IdsExhausted` at `u64::MAX` and never wraps. `PlayerId` is the external UUID's 128 bits; the core never generates it.
  - **Links**: the child holds the truth (`Building.planet`, `ShipBuild.shipyard`, `Ship.location` = roster or fleet). Parents keep ordered ID sets for lookup (`Planet.buildings`, `Planet.roster`, `Fleet.ships`). A Shipyard's queue is its ship builds in ID order, because IDs only grow.
  - **Builds keep their start terms**: a building under construction and a ship build store their `cost_g` and `rated_g_per_h` from when they started, plus a delivered `Quantity`. A later `rules_change` therefore does not alter a build in progress, for both cost and pace.
  - **Next event**: a planet stores `next_event` (time and version), so a restored snapshot needs no re-solve and `view` can give `valid_until` directly. The queue entry is its index. `QueueEntry` is ordered by time, kind rank (planet changes before fleet entries), entity ID, then version. Fleet arrivals and returns share rank 1 and go by fleet ID; for one fleet an arrival sorts before a return. Pushing and popping are P1.4's (D-023).
  - **Serde boundary**: `Quantity` deserializes through `from_parts`, so an out-of-range remainder is rejected. `World` deserializes through `check_links`. That checks IDs are unique and below `next_id`, map keys match, references resolve and are mirrored, homeworlds belong to their players, ship builds sit on operational Shipyards, fleets are non-empty and owned like their ships, stock checkpoints are not after world time, and every planet's next event matches its live queue entry. Each in-transit or returning fleet has exactly one live entry of the matching kind at its `arrives_at_ms`, a stationed fleet has none, no entry is newer than its entity's version, and every live entry (a planet's next event, a moving fleet's arrival) is after world time. Removing a fleet does not purge its queue entries, because the queue is never searched. So an entry for a fleet ID below `next_id` that no entity holds is stale and allowed. That happens when a fleet is recalled early, returns and is disbanded before its original arrival. An ID at or above `next_id`, or one held by another entity, is rejected. Unknown fields are rejected. It is not a gameplay validator.
  - **Testkit**: `WorldBuilder` creates every entity with rules defaults, keeps links in step and checks them in `build()`. `busy_world()` holds every variant. testkit depends on core and rules; core does not depend on testkit.
  - **Dependencies**: core takes serde at runtime (allowed by Project layering). serde_json is a dev-dependency of core and testkit for round-trip tests.

### D-023: Event queue, advance_to and the step seam

- Date and package: 2026-10-17, P1.4. Shape proposed by dev-build and agreed with orch-advisor, with three amendments from the advisor: the queue is private, a poisoned flag enforces discard-on-error, and the seam stays the production dispatch path.
- Decision:
  - **Time**: `advance_to(t)` sets `World.time_ms` to each instant as it reaches it, then to `t`. Every World method a step calls sees the instant being processed as now. A world between instants is a valid snapshot; a world mid-instant is not. Advancing to `t1` and then `t2` equals advancing to `t2`. `t` before world time is `TimeBeforeNow`, and `advance_to(World.time)` is valid.
  - **Seam**: the `Steps` trait has one method per queue kind (`planet_change`, `fleet_arrival`, `fleet_return`) and its own event type. `World::advance_with(t, steps)` is the one loop. `World::advance_to(t)` runs it with `Pending` steps, which return `StepNotBuilt` for any live entry rather than skipping it. `DomainEvent` is empty until the packages that produce events add variants. P1.6/P1.7 (the planet re-solve) and P1.8 (fleets) supply real steps, and `advance_to` then runs `advance_with` with them. The seam is the production dispatch path, not a test-only side door, so the tested loop is the production loop.
  - **Live or stale**: one predicate, `World::entry_status`, serves both the loop and `check_links`.
    - An entry older than its entity's version is stale.
    - An entry for a fleet ID that was allocated but that no entity holds is stale (D-022).
    - An entry at its entity's version is live.
    - An entry newer than its entity, for an unallocated ID, or for an ID held by a different kind of entity is an error. The loop stops on such an entry; it never skips it.
  - **Scheduling**: the queue is private to the state module, and `World::queue()` reads it. `schedule_planet(planet, Some(at) | None)` and `schedule_fleet(fleet)` are the only way to add entries.
    - Both bump the entity's version first, so any earlier entry goes stale. That includes a fleet's send, not only its recall.
    - `schedule_planet` sets or clears `next_event`.
    - `schedule_fleet` takes the kind and time from the fleet's state: in transit gives an arrival, returning gives a return. A stationed fleet is `FleetNotMoving`.
    - An `at` not strictly after world time is `NotAfterNow`. During an advance that is the architecture's loop guard ("a step never schedules work at its own instant").
    - A scheduling error is raised before anything changes. Inside an advance, the engine also latches the first one, and the loop stops with it after the step returns, even if the step ignored the error and returned success. The guard therefore does not depend on every step propagating it (review-r2 repair).
  - **No nested advance**: a step that calls `advance_to` or `advance_with` gets `NestedAdvance` before anything changes. The refusal is latched like a scheduling error, so the running advance stops and poisons even if the step ignores it. There is one loop, never a loop inside a step (review-r2 repair).
  - **Processing**: the loop pops entries in queue order up to `t`. A stale entry is dropped. A live entry before the reached instant is `LiveEntryBehind`. Before the planet step runs, a live planet entry clears the planet's `next_event`; the step reschedules if a next event exists.
  - **Post-step check**: after each step, the loop checks the processed entity's schedule, always and not only in debug builds. A planet's `next_event` must be in the queue and after now. A moving fleet's expected entry must be in the queue and after now. A failure is the architecture's stop-on-bug. Only the processed entity is checked. A fleet arrival re-solves its destination planet (Fleets), so P1.8 must extend the check to that planet; until then, a broken cross-entity schedule would surface only when a snapshot is loaded.
  - **Errors and poison**:
    - `TimeBeforeNow` is checked first and leaves the world unchanged.
    - Any other error from an advance can leave the world mid-instant. It sets a poisoned flag, after which advancing, scheduling and serializing return `Poisoned`. The caller discards the world; the engine host restores from the last snapshot and log (Persistence).
    - A scheduling error outside an advance changes nothing and does not poison.
    - The flag is not serialized: a deserialized world is never poisoned. `allocate_id` is not guarded, because a poisoned world can neither advance nor be saved.
  - **Out of scope**: the solver and the "step 2 repeats" guard (P1.6/P1.7), fleet missions (P1.8), commands, which are step 3 at an instant (P1.9+), and views (P1.11).

### D-024: Determinism suite, canonical bytes and the two-process run

- Date and package: 2026-10-17, P1.5. The comparison form and dependencies were agreed with orch-advisor. The advisor amended the slice SPEC to add a light two-process run, and orch-lead confirmed it.
- Decision:
  - **Where it lives**: the suite is testkit's `determinism` test binary. What its harnesses share lives in `voidforge_testkit::determinism`: `canonical_bytes`, `Scenario` and the `Churn` test steps, the run helpers, and `settle_stock`. `just determinism` and the blocking CI job `determinism` both run `.github/scripts/determinism.sh`, so they cannot drift. `test` runs the same binary as part of the workspace, except the two-process test.
  - **Harnesses and rules**:
    - Observation invariance enforces "One rounding rule" and "Pure reads": 100 hours in one advance against 1,000 advances with reads, over the event loop and over stocks.
    - Property tests check the invariants that apply before the solver: insertion-order-independent queue order, stale entries never run and live ones always run, new entries strictly after now, `check_links` after every advance, and quantities within their ranges.
    - The double run enforces "Total order": in one process, and in two.
  - **Engine steps**: production `advance_to` refuses live events until P1.6–P1.8, so the event-loop harnesses drive `advance_with` with `Churn`. `Churn` is deterministic test steps that reschedule planets and send, recall, station and return fleets, with every choice a function of seed, entity, version and instant. It invents no gameplay.
  - **Canonical bytes, no hash**: runs are compared on postcard bytes of the World and the event sequence. There is no state hash or BLAKE3 yet. P2.2's state hash MUST be BLAKE3 over exactly these postcard World bytes, header excluded, and P2.2 adds a test asserting store's World bytes equal `canonical_bytes`. That gives one byte format end to end and no migration of golden values.
  - **Two processes, lightly**: the script runs the ignored `two_process_double_run` twice as separate processes. The first writes the bytes of fixed scenarios under `target/determinism/`; the second recomputes them and requires identical bytes. A missing file, or a filter that matches no test, fails. The log-based two-process replay stays Phase 2's.
  - **Reproducibility**: every property has a fixed case count and a fixed seed, and no failure files, so a failure reproduces by rerunning (the P1.1 choice). Scenario gaps are at least 10 minutes, so runtime stays small and bounded.
  - **Dependencies**: postcard =1.1.3 (alloc, default features off) is a normal dependency of testkit, with cobs pulled in. Every consumer takes testkit as a dev-dependency only, so postcard is reachable only through dev-dependencies, and no non-test crate's `cargo tree -e normal` shows it. proptest stays a dev-dependency (core, and now testkit): as a normal testkit dependency it would put its rand 0.10 next to OpenTelemetry's rand 0.9 in cargo-deny's duplicate check. Both are allowlisted. insta is not added until its first use.
  - **Extension**: P1.6+ add their cases to these harnesses rather than writing parallel ones (testkit guide, "Extending the determinism suite").


### D-025: Per-planet solver part 1, its seam and the rules it reads

- Date and package: 2026-10-17, P1.6. Proposed by dev-build and agreed with orch-advisor, with one amendment: make the odd ingot demand unreachable.
- Decision:
  - **Rules at the call**: `World::advance_to(t, &Rules)`. A rules version other than `World.rules_version` is `RulesMismatch`, checked before anything changes and without poisoning ("Pinned rules and build").
  - **Production steps**: `advance_to` runs `advance_with` with `Solver`. `Solver::planet_change` re-solves the planet. Fleet arrivals and returns stay `StepNotBuilt` until P1.8. `Pending` is removed; tests have their own steps.
  - **Re-solving outside the loop**: `World::solve_planet(planet, &Rules)` re-solves at world time. It is the same function the step calls, and later Join game and planet commands use it. Building a world does not solve it. A deserialized world is taken as already solved, since rates and the next event are stored (D-022).
  - **The solve**, architecture steps 1 to 5:
    1. Bring every quantity to t: the stocks, and the delivered ingots of constructions and active ship builds. A quantity whose crossing is exactly t snaps to its boundary with remainder 0. A crossing before t is a missed event, a bug, and an error.
    2. Transitions are P1.7's. A build at its cost, or a demolition at or past its end, is `TransitionNotBuilt`, which stops and poisons the advance. Nothing is skipped or faked.
    3. Solve the flows (`solver::planet_flows`).
    4. Set the net rates with `with_rate` at t, carrying remainders.
    5. Schedule the earliest crossing or timer through `schedule_planet`, or clear it.
  - **Empty and full**: a stock is empty when `value_g <= 0` and full when `value_g >= cap`, whatever its remainder. Counting a stock at value 0 with a positive remainder as "not empty" would give it a negative net rate at value 0. Its falling crossing would then be now, which trips the "not strictly after t" guard.
  - **Ratios from the rules**: O = `ingots_per_ore` × A, and the ingot limit is floor(ingot demand / `ingots_per_ore`). The architecture's "2" and "half" are Rules v0's value.
    - With an ingot demand that is not a multiple of the ratio, a full ingot stock would get net 2·floor(D/2) − D = −1 g/h and drain away from its cap. Using ceiling division would overfill instead.
    - The floor stays, and `Rules::validate` now requires every build's rated ingot rate to be a multiple of `ingots_per_ore`, so every demand divides exactly. Rules v0 and v0-fast already satisfy this (Drill 50e6, Refinery 62.5e6, Shipyard 100e6, Colony Ship 80e6, Cargo Vessel 75e6 g/h, and ×3600 for fast).
  - **Even split**: water-filling. Each round gives an equal share, floor(S/n), to every remaining consumer; consumers at or below the share take their demand and leave. Once all remaining consumers want more than the share, the leftover S mod n goes 1 g/h at a time to the lowest IDs. Constructions and ship builds share one ID order (D-022). A is split across operational Refineries, and Drill actual across operational Drills, for the labels; the stocks use the totals.
  - **Labels** (`PlanetFlows`, derived, never stored): Running, Throttled or Halted for:
    - each operational Drill and Refinery (its share against its rated rate);
    - each construction and active ship build (its delivery against its rated rate);
    - each operational Shipyard (its active builds' deliveries against their rated sum; Halted with no active builds or no delivery).

    Demolishing buildings and waiting builds do no work and get no label.
  - **Next event**: the earliest of every stock with a nonzero rate reaching 0 or its cap, every build with a positive delivery rate reaching its cost, and every demolition end. Nothing is scheduled when nothing moves. A candidate not strictly after t is `NotAfterNow`.
  - **Determinism**:
    - Solved-planet scenarios join the D-024 harnesses through production `advance_to`, with real stock crossings, and with build costs out of reach so no transition fires.
    - New properties: no negative or over-cap stock; the next event strictly after t; mass accounted for within the allowance P1.1 already states (D-018), with no new tolerance. Each quantity is exact up to its carried remainder and read flooring. Each crossing snap moves it by less than one millisecond of flow plus one gram.

### D-026: Per-planet solver part 2: transitions, their order, events and the loop guard

- Date and package: 2026-10-17, P1.7. Proposed by dev-build and agreed with orch-advisor.
- Decision:
  - **Step 2** replaces P1.6's `TransitionNotBuilt`. It collects every transition due at t on the planet, applies them in ascending entity ID (one global counter, D-022), and repeats until a round finds none.
    - The transitions are a construction whose delivered ingots reach its cost, an active ship build that reaches its cost, and a demolition at its end time.
    - The source gives no order between them, and none affects another within a round, so ID order exists for determinism only.
    - Step 3's flows include whatever the transitions changed (cascade within the solve).
  - **Construction complete**: the building becomes `Operational`; its delivered ingots and start terms are dropped. Event `BuildingCompleted { planet, building, kind }`.
  - **Ship build complete**:
    - The build is removed. A new `Ship` takes the next ID from `World.next_id`, with the build's kind, owned by the Shipyard planet's owner, and joins that planet's roster. A Shipyard on an unowned planet is a typed error, since only owners build.
    - After the round, each Shipyard that completed a build starts its waiting builds in ascending ID order until `parallel_builds` are active. Each starts from its delivered ingots, normally 0, checkpointed at t.
    - Event `ShipCompleted { planet, shipyard, ship, kind }`. It is not named in the architecture, whose event list says "such as".
  - **Start terms are the order's terms**: a waiting build keeps the cost and rated rate stored when it was queued. Starting it does not re-read the rules: what a player queued is what they get, and a start needs no rules access. This is how D-022's "builds keep their start terms" is read.
  - **Demolition end**:
    - At its end time exactly, the building is removed from `buildings` and from its planet, which frees the slot.
    - An end time strictly before t is a missed event, `MissedCrossing`, under the same rule as stock crossings, because the planet always schedules its earliest demolition end.
    - A Shipyard being demolished with builds is a typed error.
    - Event `DemolitionCompleted { planet, building, kind }`.
  - **Loop guard**: step-2 rounds that apply transitions may not exceed the planet's buildings plus its ship builds, counted at the start of the solve. Exceeding it is `TransitionLoop`, which poisons inside an advance (D-023).
    - Every transition consumes an item, so consistent state never reaches the guard; it guards against bugs.
    - It is proven by an internal call with a lowered bound on a state that needs two rounds, and by a test pinning the production bound.
    - A next event not strictly after t remains `NotAfterNow`, proven through the internal seam that hands the scheduler its candidate.
  - **Events**: `solve_planet` returns its events in application order (round, then ID); `advance_to` concatenates them in queue order.
  - **Atomic solve**: a standalone solve that fails leaves the planet, its buildings and builds, the ship map and the ID counter as they were (restored from a copy taken at the start). Inside an advance, a failure poisons as before.
  - **Determinism**: `Economy` worlds run real completions, ship creation and demolitions. New properties: slots respected; each ship in exactly one place; active builds per Shipyard within `parallel_builds`; mass accounted for, with each completion moving its cost out of the delivered ingots.

### D-027: Fleets: derived values, send and recall, arrivals, the tie and their events

- Date and package: 2026-10-17, P1.8. Proposed by dev-build and agreed with orch-advisor, who ruled on four open questions (recall at the departure instant, the emptied fleet, a Colonize arrival without a Colony Ship, the event set).
- Decision:
  - **Colony Ship cargo = 0** (orch-advisor's ruling): `Rules::validate` rejects any other `ships.colony_ship.cargo_capacity_g`. Fleet capacity is the checked sum of every ship's spec capacity, with no ship-type special case; the architecture's "sum of Cargo Vessel capacities" is the same number.
  - **Derived values** (`voidforge_core::fleet`): speed is the slowest ship's; distance is the integer square root (rounding down) of the summed squared coordinate differences, in i128 (D-021 units); travel time is ceil(distance × 3,600,000 / speed) ms. All are checked: a value outside 64 bits is `FleetOutOfRange { fleet, what }`, never a panic or a wrapped value.
  - **Engine-side send and recall**, the entry points P1.10's commands call. They do no command validation (ownership, mission requirements, another planet are P1.10's reason codes), are atomic, refuse a poisoned world, and return no events.
    - `World::send_fleet(fleet, target, mission, &Rules)`: a stationed fleet only (`FleetNotStationed`). It departs now, and its arrival, now plus the travel time, is fixed here and queued through `schedule_fleet`. A zero distance gives a zero travel time, which the scheduler refuses as `NotAfterNow`.
    - `World::recall_fleet(fleet)`: a fleet in transit only (`FleetNotInTransit`), which makes it once only. It returns at now + (now − departure), and the version bump makes the old arrival stale. At the departure instant it has traveled 0 ms, so it is stationed at its origin at once with no entry and its version bumped (no `RecallAtDeparture` error).
  - **Arrival** (`Solver::fleet_arrival`): bring the target to t (solver step 1), run the mission, station the fleet at the target, then re-solve the target (`solve_planet`).
    - Colonize, target unowned: the lowest-ID Colony Ship in the fleet is consumed (it leaves `ships`), the fleet's owner owns the planet, cargo unloads as for Transport, and extra Colony Ships stay.
    - Colonize, target owned by anyone, including the sender (say, by their own lower-ID fleet at the same ms): it fails. Nothing is consumed and cargo stays aboard, for a later Trigger unload.
    - Transport: each resource unloads min(aboard, cap − stock at t); the rest stays aboard. No owner check: P1.10 refuses a Transport to another's planet, and ownership is permanent.
    - Move: nothing unloads.
    - An unloaded stock gains whole grams and keeps its carried remainder, at rate 0 until the re-solve sets it. A stock filled exactly to its cap may keep a remainder under 1 g; D-025 counts it as full. Destroying mass would be worse.
    - A Colonize that uses the fleet's last ship removes the emptied fleet in the same step (its entry was just popped, so nothing stale remains). Cargo left aboard a fleet with no ships is `CargoWithoutShips`, unreachable while a Colony Ship carries nothing, and never resolved by destroying it.
    - A Colonize arrival on an unowned planet with no Colony Ship aboard is `NoColonyShip`, a bug that poisons, not a failed colonization.
  - **Return** (`Solver::fleet_return`): the fleet is stationed at its origin, cargo and ships intact. The origin does not change, so it is not re-solved.
  - **Post-step check** (D-023's obligation): for a fleet arrival, the loop reads the fleet's target before the step and afterwards checks the target planet's schedule as well as the fleet's, even if the step removed the fleet. It is in the loop, so it holds for any `Steps`.
  - **Same-moment order**: two colony fleets reaching one planet at the same ms run by fleet ID (rank 1), so the lower ID claims it and the other fails as an owned target. IDs increase, so lower = assembled first. A planet change at the same ms runs before the arrival (rank 0). No extra rule.
  - **Events**, in this order within an arrival: `FleetArrived { fleet, planet, mission }` at every arrival; then `PlanetColonized { planet, owner, fleet, ship }` (the consumed Colony Ship) or `ColonizationFailed { planet, fleet }`; then `CargoUnloaded { planet, fleet, cargo }` with the grams unloaded, only when something unloaded; then `FleetRemoved { fleet, planet }` for an emptied fleet; then the target's re-solve events. A return gives `FleetReturned { fleet, planet }`. `StepNotBuilt` is removed: every queue kind has its production step.
  - **Determinism**: `Economy { fleets: true }` adds Bob, three unowned planets and seven fleets sent at time 0 through `send_fleet`: a same-ms colonization race decided by fleet ID, a Transport, a lone Colony Ship, a refused Colonize, a Move with cargo, and a Colonize recalled halfway. The recall is an intervention at a fixed time, at which both the straight and the observed runs stop. New properties: ships conserved (start + `ShipCompleted` − `PlanetColonized`), fleet cargo within 0 and capacity, and mass accounted for with cargo aboard (each arrival's target gains exactly what its fleet unloaded, as `CargoUnloaded` reports, and the cargo aboard closes exactly).

### D-028: Commands, part 1: `apply`, Join game, the planet and Shipyard commands, and their reason codes

- Date and package: 2026-10-17, P1.9. Proposed by dev-build and agreed with orch-advisor, who ruled on six questions (the idempotency key, the Cancel ship build payload, errors after validation, events from commands, Join game on an occupied candidate, the reason codes).
- Decision:
  - **What the core takes**: `voidforge_core::command::Command { player, action }`, where `Action` is `JoinGame`, `BuildBuilding { planet, kind }`, `CancelConstruction { building }`, `DemolishBuilding { building }`, `QueueShip { shipyard, kind }` or `CancelShipBuild { build }`. It serializes snake_case and externally tagged (`{"build_building": {...}}`), so a log payload is this value; the API's JSON mapping is Phase 3's.
    - The idempotency key stays with the host, which looks it up before `apply` and logs it; the core never reads it.
    - `game_time` is `apply`'s `t`, and `seq` is the order of calls.
    - Cancel ship build names the build alone, which determines its Shipyard.
  - **`World::apply(&Command, t, &Rules) -> Result<Applied, EngineError>`**, where `Applied { advanced, outcome: Result<Vec<DomainEvent>, Rejection> }`:
    1. `advance_to(t)`, so events at t run first. Its errors pass through unchanged.
    2. Validate against the state at t. A rejection returns `Ok` with `Err(rejection)`, and the world is byte-identical to one that only advanced.
    3. Apply the effect, then re-solve the touched planet; the command's events are that re-solve's.
    - After validation, the effect can fail only through `IdsExhausted` or an engine error in the re-solve (`RateOutOfRange`, the new `CommandOutOfRange`). Those poison the world, like errors inside an advance (D-023). A rejection never poisons.
  - **No events for commands**: the log record (command and outcome) is the record, as with P1.8's send and recall. `advanced` and the command's own events are kept apart.
  - **The six commands** (accepted when → effect; then re-solve):
    - Join game: a new player, and the first homeworld candidate in list order that nobody owns → a `Player`; the planet's owner; the rules' Drills then Refineries, operational, on fresh IDs; ore and ingot stocks set to the rules' amounts at t with remainder 0. A candidate's stocks are 0 in the MVP, so "fills" sets them. Other players' ships and fleets on the candidate (reachable by Move) stay where they are; there is no combat.
    - Build building: the player owns the planet, and its buildings, counting construction and demolition, are fewer than its slots → a construction with delivered 0 at t on the rules' current cost and rate (D-022 start terms).
    - Cancel construction: under construction on the player's planet → removed; delivered ingots lost; the slot frees at once.
    - Demolish building: operational on the player's planet; a Shipyard with no builds, active or waiting → `Demolishing { ends_at_ms: t + demolition time }`, with checked addition.
    - Queue ship: an operational Shipyard on the player's planet → a build on the rules' current terms, active below `parallel_builds`, otherwise waiting; delivered 0 at t.
    - Cancel ship build: the build's Shipyard is on the player's planet → removed with no refund. If it was active, waiting builds start in ID order up to the limit (P1.7's rule).
  - **Reason codes** (`Rejection`; `code()` equals the serialized name), checked in a fixed order, who → what → whose → state, with the first failure the code:
    - `already_joined`, `no_free_homeworld`, `not_joined`;
    - `unknown_planet`, `unknown_building`, `unknown_ship_build`;
    - `not_owner`;
    - `no_free_slot`, `not_under_construction`, `not_operational`, `shipyard_has_builds`, `not_a_shipyard`, `shipyard_not_operational`.

    An entity that existed and is gone is `unknown_*`, since IDs are never reused. These codes are the API contract. P1.10 keeps the architecture's examples verbatim (`insufficient_stock`, `cargo_aboard`) and adds `unknown_fleet` to the `unknown_*` family, so the list reads as one vocabulary.
  - **Determinism**: `Economy { commands: n }` adds two unowned homeworld candidates and a script of n commands at whole hours, often the instant a build completes. Each is chosen against the state at its instant, half the time a fitting target issued mostly by its owner, otherwise a mix that is rejected. Both runs stop at every command instant (`determinism::stops`).
    - New properties: equal events and outcomes in 1 against 1,000 advances; a rejected command leaves the world's bytes unchanged; every invariant holds after every command; each action type is both accepted and rejected in every case.
    - The mass audit through commands: a cancelled construction or build takes exactly its delivered ingots out of the ledger; Join game sets exactly the starting stocks on an empty candidate; nothing else moves mass.

### D-029: Commands, part 2: the five fleet commands and their reason codes

- Date and package: 2026-10-17, P1.10. Proposed by dev-build. Lead ruling overnight 2026-10-17/18, endorsed by the advisor 2026-10-18. orch-advisor was held at a hook prompt, and orch-lead ruled on the six API-shape questions as proposed (slice 16, `ruling-lead-overnight.md`). Any amendment from the advisor's review lands as a follow-up before Phase 3.
- Decision:
  - **Payloads** (new `Action` variants): `AssembleFleet { planet, ships: BTreeSet<ShipId>, cargo: Cargo }`, `SendFleet { fleet, target, mission }`, `RecallFleet { fleet }`, `DisbandFleet { fleet }`, `TriggerUnload { fleet }`.
    - Cargo is grams per resource; zero is "no cargo". The design's "spread across Cargo Vessels by tonnage" needs nothing more, since cargo is a fleet total (D-022).
    - `ships` is a set, so `Action` and `Command` are `Clone`, no longer `Copy`.
    - **Phase 3 obligation**: the API rejects duplicate ship IDs at its edge, so a client's duplicate never collapses silently.
  - **The rules**, checked in D-028's order (who → what → whose → state), with the first failure as the code:
    - **Assemble fleet**: `not_joined`; `unknown_planet`, `unknown_ship`, `no_ships`, `invalid_cargo` (a negative amount); `not_owner` (a ship not the player's, then cargo at a planet not the player's); `ship_not_on_roster`, `insufficient_stock` (either resource above its stock at t), `insufficient_capacity` (ore + ingots above the summed ship capacities, D-027).
      - Effect: a new fleet stationed there; its ships leave the roster. Cargo leaves storage exactly, each stock keeping its remainder at rate 0 (the mirror of D-027's unload), and the planet is re-solved only when cargo was loaded.
      - Without cargo, any planet's roster works, owned or not.
    - **Send fleet**: `not_joined`; `unknown_fleet`, `unknown_planet`; `not_owner`; `fleet_not_stationed`, `same_planet`; then by mission:
      - Colonize: `no_colony_ship`, `target_owned`;
      - Transport: `no_cargo_vessel`, `target_not_owned`;
      - Move: nothing further.

      Then `unreachable_target` (zero travel time, or an arrival outside 64 bits), so a player's command never poisons the world. Of the five fleet commands, only Send and Recall compute a time from player input; Assemble and Trigger unload bound every amount by stock, capacity or free storage, and Disband moves only IDs (review-r2's audit, confirmed). Effect: D-027's `send_fleet`.
    - **Recall fleet**: `not_joined`; `unknown_fleet`; `not_owner`; `fleet_not_in_transit` (stationed), `already_recalled` (returning), then `unreachable_target` (the return, now plus the time traveled, past the end of time; review-r2's repair, so a recall too can never poison). Effect: D-027's `recall_fleet`, including its departure-instant rule; its own checked error stays as the engine's defense.
      - The Commands table's recall row omits "the player's"; it is read as brevity, not permission, because fleets are visible only to their owner and every other fleet row says so.
      - A recall at the arrival instant is `fleet_not_in_transit`, because events at t run first.
    - **Disband fleet**: `not_joined`; `unknown_fleet`; `not_owner`; `fleet_not_stationed`, `cargo_aboard`. Effect: the ships join the station planet's roster and the fleet is removed (any stale entry stays stale, D-022). It works at any planet. The design's "Move to an owned planet and unload first" follows from `cargo_aboard` and Trigger unload's rule.
    - **Trigger unload**: `not_joined`; `unknown_fleet`; `not_owner`; `fleet_not_stationed`, `planet_not_owned`. Effect: D-027's unload (each resource up to free storage, the rest aboard, remainder kept), then re-solve. Nothing aboard, or no room, is accepted and moves nothing.
    - `planet_not_owned` and `target_not_owned` are state codes, not `not_owner`, because which planet matters is known only from the fleet's state. That keeps D-028's rule that `not_owner` always precedes state codes.
  - **The vocabulary** grows by 18 codes, all API contract: `unknown_fleet`, `unknown_ship`, `no_ships`, `invalid_cargo`, `ship_not_on_roster`, `insufficient_stock`, `insufficient_capacity`, `fleet_not_stationed`, `fleet_not_in_transit`, `already_recalled`, `same_planet`, `no_colony_ship`, `target_owned`, `no_cargo_vessel`, `target_not_owned`, `unreachable_target`, `cargo_aboard`, `planet_not_owned`. The architecture's `insufficient_stock` and `cargo_aboard` are kept verbatim.
  - **Ownership before state** (review-r1's carry from P1.9): another player's building or fleet in any state is `not_owner`. It is pinned for Cancel construction, Demolish, Queue ship and the fleet commands.
  - **Determinism**:
    - Command scripts cycle all eleven actions, 165 per script, on spread-out planets so trips take hours.
    - The mass audit's command step covers Assemble (the planet loses exactly the cargo, and the cargo aboard gains it) and Trigger unload (the reverse). The cargo aboard closes as start − unloaded + loaded.
    - Coverage (each action type accepted and rejected) is asserted over eight fixed economies rather than per generated case.
    - Ship conservation now allows a consumed Colony Ship that was completed during the run.
    - A planned recall leaves alone a fleet that a generated command already moved out of transit.

### D-030: Views and score

- Date and package: 2026-10-18, P1.11. Proposed by dev-build. Lead ruling overnight 2026-10-17/18, endorsed by the advisor 2026-10-18. (Slice 17, `ruling-lead-overnight.md`.) **Q4 is PROVISIONAL, parked for Tom as game intent.**
- Decision:
  - **Entry point**: `World::view(&self, &Query, t, &Rules) -> Result<View, ViewError>`.
    - It takes `&World`, so "Pure reads" holds by type, and the determinism suite also checks the bytes are unchanged.
    - It needs the rules for D-025's labels and for `ScoringSpecs`, so it checks the version (`RulesMismatch`).
    - Errors: `BeforeNow`; `Stale { valid_until }` (the caller must advance; a view never projects across an event); `Unknown`; `ScoreOutOfRange`; `Engine`.
  - **Queries and typed results** (JSON and ISO formatting are Phase 3's):
    - `Planet` → stocks (`amount_g`, `rate_g_per_h`, `cap_g`), buildings (status, label, construction progress), ship builds (status, label, progress) and roster;
    - `PlanetsNear { center, radius }` → summaries (id, position, owner, distance ≤ radius, inclusive);
    - `Player` → homeworld, owned planets, score;
    - `Fleets { viewer }` → the viewer's own fleets (ships, cargo, capacity, speed, state).

    Every view carries `as_of` and `valid_until`. Progress carries `delivered_g`, `cost_g`, `rate_g_per_h` and `eta_ms`: the exact rising crossing under current rates (D-018), or `None` while nothing flows.
  - **Validity** (`valid_until`; `None` when nothing is scheduled). A view is defined for t with world time ≤ t < `valid_until`.
    - A planet: the minimum of its `next_event` and the earliest arrival of a fleet in transit to it, since an arrival re-solves its target (D-027). This takes a scan of the fleets, O(fleets), because D-022's stored `next_event` alone would make the done-when false. A per-planet index of incoming fleets is a Phase 2 option if profiling asks for it.
    - Multi-entity queries take the minimum:
      - a player: their planets, as above, and their moving fleets;
      - `PlanetsNear`: the earliest Colonize arrival at a listed planet, the only event that changes a summary;
      - `Fleets`: the first arrival or return among them.
    - Commands are inputs, not events; the host republishes after each one.
  - **Score** (never stored; computed from the projection at t, in checked i128):
    - planets owned × `planet`;
    - operational buildings by kind;
    - buildings under construction and ship builds (active or waiting) at points × delivered ÷ cost, rounded down per build;
    - each ship the player owns counted once, by ID, wherever it is;
    - ore and ingots in the player's storage and aboard their fleets (stationed or moving): grams summed per resource, then `floor(total × points ÷ per_g)` once. That keeps score unchanged when cargo moves between storage and fleets.
  - **PROVISIONAL (Q4, parked for Tom):** a building being demolished counts in full until its demolition ends.
    - The source: "cancelled or demolished buildings do not count" (Game Design, Scoring).
    - Reading A, implemented: "demolished" means done, so it counts while it still exists and holds its slot.
    - Reading B: it stops counting when demolition begins, as it stops working.
    - The switch is one match arm in `World::score`. The test name contains "provisional".
  - **Visibility**: planets, `PlanetsNear` and players are public. Fleets are visible only to their owner: the query names the viewer, and no query returns another player's fleets. **Phase 3 obligation**: the host takes the viewer from the auth token, never from the request body.
  - **Determinism**: every observation in the invariance runs takes every view and checks the bytes are unchanged. The done-when (a view at any t before `valid_until` equals the same view after `advance_to(t)`, field for field, and is `Stale` at `valid_until`) is a named test and a property over `Economy` worlds with fleets and commands.

### D-031: World generation

- Date and package: 2026-10-18, P1.12. Proposed by dev-build. Lead ruling overnight 2026-10-17/18, endorsed by the advisor 2026-10-18. (Slice 18, `ruling-lead-overnight.md`.) **The placement option is PROVISIONAL, pending Tom (game intent).**
- Decision:
  - **Entry point**: `voidforge_worldgen::generate(seed, players, &Rules) -> Result<World, WorldgenError>`.
    - It produces the universe's first `World`: at time 0 (the host sets the epoch in P2.9), under the rules' version, with the seed recorded.
    - It holds `players × planets_per_expected_player` unowned planets with the rules' slots and caps and empty stocks, plus `players` homeworld candidates.
    - Every planet is solved (D-025), and the links hold.
  - **Placement, option B (PROVISIONAL, pending Tom)**:
    - Planets are uniform in the cube on whole world units (D-021), each at least `min_planet_spacing_du` from every other.
    - Candidates are chosen among the planets with at least `homeworld_min_neighbors` others within `homeworld_neighbor_radius_du` (inclusive). The qualifying planets are taken in a seeded shuffle, each kept if it is at least `homeworld_min_spacing_du` from those already kept, until `players` are kept. That order is the candidate order.
    - This is the Technical Plan's literal "spread uniformly". It works at the 100-player default (cube packing about 0.12).
    - Under the default rules small worlds fail: `Infeasible("too few planets qualify as homeworld candidates")`, because a fixed 1,000-unit cube at 20 planets per player has density proportional to players, and at 1 player essentially no planet has 5 neighbors within 100 units. Small worlds are tested under denser rules (the cube scaled to keep the 100-player density).
    - **Alternatives awaiting Tom**:
      - **A**: build each candidate's neighborhood (sites pairwise apart, `homeworld_min_neighbors` planets uniform within the radius of each), then fill the rest uniformly. Feasible at every size, but clustered.
      - **C**: scale the cube with the player count (side ∝ players^⅓), uniform and always feasible, but "a 1,000-unit cube" becomes the 100-player size.
  - **Randomness**: `ChaCha8Rng::seed_from_u64(seed)` is the only source, from `rand_chacha =0.9.0` (default features off).
    - That version is already in the tree through opentelemetry_sdk's rand 0.9, so the graph gains no crate or duplicate. The guardrail fixture's 0.10.0 is a lint control only.
    - A unit test pins the first outputs for seed 42, so an upgrade that changed the stream fails before it changes worlds.
    - If telemetry moves to rand 0.10, the pin is revisited (it would then become a duplicate).
  - **Integer-exact sampling**:
    - `below(n)` by rejection of the top partial range: unbiased, with expected draws under 2.
    - Spacing is checked on exact squared distances in i128 against a `BTreeMap` grid (cell = spacing, 27 cells per check).
    - Neighbor counts use a grid with cell = radius.
    - The shuffle is Fisher–Yates with `below`.
    - Termination: each planet tries at most `MAX_ATTEMPTS` (10,000) draws, then `Exhausted { placed }`, never a loop.
  - **Feasibility** (the P1.2 QA LOW carry; repaired after review-r2's M1 and M2 on 1325f22):
    - **`Rules::validate`** (independent of player count):
      - every worldgen distance is at most `MAX_WORLDGEN_DU` (10^12 distance units, a billion world units), the supported range that keeps every feasibility product exact in i128;
      - cube and spacings are whole world units;
      - homeworld spacing ≥ planet spacing, and ≤ the cube's diagonal;
      - with neighbors required, the neighbor radius ≥ planet spacing;
      - the neighborhood bound `(n + 1) × s³ ≤ (2r + s)³`: balls of radius s/2 around the candidate and its neighbors are disjoint inside the ball of radius r + s/2;
      - `planets_per_expected_player ≥ 1 + homeworld_min_neighbors`.
    - **`generate`** (count-dependent):
      - `NoPlayers`;
      - the packing bounds `n × s³ ≤ 2 × (L + s)³` for the planets and for the candidates (`Infeasible`). Balls of radius s/2 around points pairwise s apart are disjoint inside the cube widened by s/2, so n × (π/6) × s³ ≤ (L + s)³, and with π > 3 the stated bound is still necessary and integer-only. The first version used `n × s³ ≤ (L + s)³`, which is not necessary: separation does not imply disjoint s-cubes. review-r2's lattice of 4,000 points in a 160-unit cube at spacing 11 refutes it;
      - too few qualifying candidates (`Infeasible`);
      - anything subtler is caught by `Exhausted`, an honest result even for feasible but dense parameters.
    - Every left-hand product is checked, and an overflow counts as larger than the exact right side, so decisions are the same in debug and release.
  - **Determinism**:
    - The same seed gives identical canonical postcard bytes, the bytes P2.2's state hash covers (D-024).
    - worldgen's own `tests/determinism.rs` joins `just determinism`: an in-process double run over generation, a generated world with three players joined and played for 100 hours, and a two-process run (`double-run-worldgen.bin`).
    - Timing: the 100-player default (2,000 planets) generates in about 0.1 s in a debug build and 0.02 s in release.

### D-032: Checked arithmetic in the core, rules and worldgen

- Date and package: 2026-10-18, the Phase 1 gate repair. Proposed by dev-build and approved by orch-lead. It extends [D-010](#d-010-determinism-bans-in-core-rules-and-worldgen)'s determinism bans.
- Why: the same class reached review three times (P1.6 solver aggregates, P1.12 feasibility, P1.11 score). Rules that validate drove an integer operation past its range, which panicked in debug and wrapped silently in release, so the two profiles decided differently. That breaks "Integer arithmetic" and the typed-error contract.
- Decision:
  - **The ban**: `clippy::arithmetic_side_effects` is forbidden at crate level in voidforge-core, voidforge-rules and voidforge-worldgen. It is applied with `cfg_attr(not(test), ...)`, so in-crate unit tests are exempt. No library item can allow or expect it (the gate's "no lint exceptions"), exactly like the D-010 bans.
  - **The two allowed forms**:
    - `checked_*` with a typed error, wherever rules-valid input or reachable state can drive the operation (the score: `ScoreOutOfRange`; Recall's return time: `unreachable_target` and `FleetOutOfRange`);
    - `strict_*`, which panics in every profile, only where a bound is proven. Each proof is in a code comment tagged D-032 and in the gate repair's arithmetic audit, which reviewers check.
  - **The audit**: all 74 flagged operations at ab984fd are classified in `missions/voidforge-mvp/proof/phase1-gate-repair/arithmetic-audit.md`: 3 sites fixed with regressions, the rest strict with proofs.
  - **The guardrail fixtures** (D-012's harness):
    - `arithmetic.rs` must fail with the lint in each crate; `arithmetic.ok.rs` (checked and strict) must pass;
    - `exceptions.rs` refuses an `allow` of it (E0453);
    - the `floats.ok.rs` and `clocks.ok.rs` controls use checked operations.

    The harness covers 133 cases with these (136 with the addendum's `dependency_sections.ok.rs` harness control).
  - **Release overflow checks**: `overflow-checks = true` in the workspace's `[profile.release]`, so code outside the ban (the host and the binaries from Phase 2) panics on overflow instead of wrapping.
  - **New code** in these crates writes `checked_*` or `strict_*`; the lint enforces it. A strict site needs its bound argued in the review.

### D-033: Carried obligations from Phase 1

- Date and package: 2026-10-18, the Phase 1 gate repair (review-r1's gate verdict). These obligations lived only in mission files; they are recorded here with their target packages.
- Decision:
  - **P2.5** (snapshots and restore): restore must not rely on `World::check_links` for gameplay fields. It is a structural check only (D-022: "not a gameplay validator"; P1.3 LOW).
  - **P2.9** (the universe bootstrap):
    - It caps the total world size an operator can ask for (players × planets per player; P1.12 QA LOW).
    - Under D-031 option B (provisional), the default rules refuse worlds below roughly 31 to 40 expected players, because too few planets qualify as homeworld candidates. The bootstrap, and P3.6's end-to-end setup, must use at least about 40 players or denser rules, until Tom rules on D-031.
  - **P3.6** (the end-to-end scenario): dex configuration parity between local and CI becomes a mechanical check (a Phase 0 carry).

### D-034: Schema and migrations, sqlx, and the Database tests stage

- Date and package: 2026-10-18, P2.1. Proposed by dev-design, acting as builder (`slices/19-p2-1-schema/proposal-schema.md`). Lead ruling overnight 2026-10-17/18, endorsed by the advisor 2026-10-18. None of it is game intent.
- Decision:
  - **Tables**: `log_records` and `snapshots` match the [Persistence](Voidforge%20Architectural%20Design.md#persistence) tables column for column. `accounts` follows [Decision 9](Voidforge%20Technical%20Plan.md#decisions). They're created by three up-only sqlx migrations in `crates/voidforge-store/migrations/` (`0001_log_records`, `0002_snapshots`, `0003_accounts`), and `voidforge_store::migrate` applies them from Rust.
  - **Nullability is read literally**: only `log_records.player_id` and `log_records.idempotency_key` are nullable. An `advance` record writes JSON values in `payload` and `outcome` (for example JSON `null`), never SQL NULL. `wall_time` and both `created_at` columns default to `now()`.
  - **Constraints** (each one encodes a documented invariant):
    - `log_records`:
      - `kind` is one of `command`, `advance`, `rules_change` and `system`;
      - `UNIQUE (player_id, idempotency_key)` ("unique per player"; NULLs stay distinct, so keyless records never collide);
      - a key needs a player;
      - `seq >= 1`;
      - `rules_version >= 0`, since core's version is `u32` and P2.3 converts with `try_from`;
      - `events` is a JSON array.
    - `snapshots`:
      - `seq >= 0`, where 0 is a snapshot before any record (the P2.9 bootstrap);
      - `rules_version >= 0`;
      - `state_hash` is exactly 32 bytes (BLAKE3-256, D-024).
    - `accounts`:
      - `player_id` is the primary key and must be UUID version 7. The check is `coalesce(uuid_extract_version(player_id), 0) = 7`: the function returns NULL outside the RFC 9562 variant, and a NULL check would pass. A test found this.
      - issuer and subject are non-empty, and `UNIQUE (issuer, subject)`.
    - `game_time` is unconstrained, because core's `i64` time refuses no sign, and the table is never stricter than the engine.
    - Gap-free `seq` is the single writer's job (P2.3); there's no trigger.
  - **No foreign keys.** The log must replay into a database without accounts (P2.6 golden logs, P2.7 replay). A bootstrap snapshot has no record, and old records may be pruned.
  - **Migrations only add**:
    - in CI, `.github/scripts/check-migrations-only-add.sh` fails if a migration on the base revision is modified, renamed or deleted, or if a new one doesn't sort after the base's highest version. It has 15 regression cases, five of which also check the reason it gives;
    - at run time, sqlx refuses an applied migration whose checksum changed (a named test).
    - A destructive statement in a new migration is a review matter: no regex can judge SQL soundly.
  - **sqlx `=0.9.0`**, with no default features plus `postgres`, `runtime-tokio`, `macros` and `migrate`. It's a normal dependency of voidforge-store only.
    - No TLS, because Postgres is on localhost and a CI service. No `json` or `uuid`, which P2.3 and P3.3 add with their own entries.
    - `cargo tree -e normal,build` shows every new crate reachable from voidforge-store and nothing else. Core's, rules' and worldgen's trees are byte-identical to main's (`proof/dependency-reach.txt`).
    - New license: Zlib (foldhash).
    - The duplicates, each a `deny.toml` skip with its reason:
      - sqlx-core is still on the RustCrypto 0.10 generation, while sqlx-postgres moved to 0.11: sha2, digest, block-buffer, crypto-common and cpufeatures;
      - base64 0.22 (the telemetry stack uses 0.23);
      - hashbrown 0.16 (indexmap uses 0.17);
      - rand, rand_core and getrandom 0.10/0.4, used by sqlx-postgres for SCRAM nonces. Until now these were in the tree only through proptest as a dev-dependency (D-024), and cargo-deny leaves dev-only duplicates out by default. Now they're normal dependencies, reached only from voidforge-store.
    - 0.8.6 was compared: it brings 6 duplicates (including rand_chacha 0.3 beside worldgen's pinned 0.9) and two unlisted licenses, so 0.9.0 is kept.
  - **Offline query metadata**: `.sqlx/` at the workspace root, written by `cargo sqlx prepare --workspace -- --all-targets` against a migrated database. With `DATABASE_URL` unset, the macros read it, so `lint` and `test` build without Postgres.
  - **sqlx-cli `=0.9.0`**, a tool, built with `cargo install sqlx-cli@0.9.0 --locked --no-default-features --features postgres`. taiki-e/install-action has no manifest for it; rust-cache keeps `~/.cargo/bin`.
  - **Where the Postgres tests run**:
    - The store's tests are `#[sqlx::test]` with `#[ignore = "needs Postgres: just db-test"]`, so `lint` and `test` compile and lint them but don't run them.
    - `.github/scripts/database.sh` runs them with `--run-ignored only`, after applying the migrations to an empty database twice (the second run must apply nothing) and running `cargo sqlx prepare --check`.
    - A cargo feature would have hidden the tests from clippy. Skipping when `DATABASE_URL` is unset would have been a silent pass.
  - **The `database` CI job** uses a Postgres service with the compose file's image digest and database `voidforge_test`. It runs the guardrail, its cases and `database.sh` on every pull request and on main. **Making it a required check needs a ruleset change that agents can't make.** The prepared body is in the P2.1 proof, parked for the owner, as in P1.5.
  - **Local runs and the owner's data**: `just db-test` runs the same commands against the `just up` stack.
    - `database.sh` refuses any `DATABASE_URL` whose database isn't `voidforge_test`.
    - It only ever creates `voidforge_test`, and only ever drops databases named `_sqlx_test_*` (before and after each run, so nothing accumulates).
    - It never connects to `voidforge`.
    - `just migrate` has no default target: it requires an explicit `DATABASE_URL`. Applying the schema to the owner's `voidforge` database stays with the owner.
- Carried obligations:
  - **P2.4: a key is unique per player forever, but the host remembers keys only for a window.** A key reused after the window would look new to the host, then fail the append on the unique constraint, and a failed append is fatal. So one client could stop the host. P2.4 must refuse an expired key before the append, or keep the window covering the whole log.
  - **P2.3**: write `rules_version` with `i32::try_from`, and write JSON values (not SQL NULL) in an advance record's `payload` and `outcome`.
- Where it's enforced: the migrations; `crates/voidforge-store/tests/database.rs` (19 Postgres tests); `.github/scripts/{check-migrations-only-add.sh, test-check-migrations-only-add.sh, database.sh}`; the CI job `database`; `deny.toml`; and the [version table](versions.md).

### D-035: Snapshot codec, header, hash and engine build

- Date and package: 2026-10-18, P2.2. Proposed by dev-design, acting as builder (`slices/20-p2-2-snapshot-codec/proposal-codec.md`). Lead ruling overnight 2026-10-17/18, endorsed by the advisor 2026-10-18. None of it is game intent.
- Decision:
  - **One encoding, two calls, one guard.** The canonical encoding is postcard with its default configuration (D-024).
    - `voidforge_store::canonical::to_bytes` and testkit's `determinism::canonical_bytes` both call `postcard::to_allocvec`.
    - Store's `determinism` tests assert that they give identical bytes on every harness world, event sequence and command list (`canonical_encoding_is_the_determinism_suites`).
    - Any non-default postcard configuration (a flavor, a different integer form) must change both places, and the guard fails if one is missed.
    - **Declined: making store the one definition and testkit depend on it.** That would pull sqlx and tokio into the test builds of every crate that uses testkit, and change the Project layering table, all for a one-line function. testkit keeps depending on core and rules only.
  - **Snapshot bytes are a self-describing envelope**, `MAGIC ‖ header ‖ payload`:
    - `MAGIC` is `b"VFSNAP"`;
    - the header is the canonical encoding of `Header { format, rules_version, engine_build, game_time_ms, state_hash }`, and this build writes and reads format 1;
    - the payload is the World's canonical bytes, to the end of the input.
    - The Architectural Design's `snapshots.payload` ("Serialized World") is read as **this whole envelope**, so the format version and the hash travel with the World, also outside Postgres (P2.6, P2.7).
    - The other columns repeat the header (`state_hash`, `rules_version`, `engine_build`, `game_time`); P2.5 checks they agree, and the writer supplies `seq`.
  - **The hash covers the World payload only**: `state_hash = BLAKE3-256(payload)`, header excluded. The same World has the same hash under any build or format. That's D-024's promise, held as `state_hash_is_blake3_of_the_determinism_canonical_bytes`.
  - **Decoding goes through core's serde boundary, and only canonical bytes are accepted.** `decode(bytes, Expected { rules_version, engine_build })` checks, in order, each a typed `SnapshotError` and never a panic:
    1. the magic;
    2. the header (`Truncated`, `BadHeader`);
    3. the format (`UnknownFormat`);
    4. the expected rules version and build (`RulesMismatch`, `BuildMismatch`);
    5. the hash (`HashMismatch`), before any byte reaches serde;
    6. the World through `try_from` and `check_links` (`BadPayload`; `Rejected` when the World refuses);
    7. no trailing bytes (`TrailingBytes`);
    8. the header against the World (`HeaderWorldMismatch`);
    9. re-encoding gives the payload back (`NonCanonical`).
    - So encode, decode and re-encode is identical by construction.
    - `decode_any` skips only step 4, for tools.
    - `encode` refuses a poisoned World (`Poisoned`, D-023).
  - **Engine build identity**: `EngineBuild` is 1 to 64 bytes of printable ASCII without spaces, and the codec takes it as an input.
    - `ENGINE_BUILD` is `"<workspace version>+<git commit>"`, set by store's `build.rs` from `VOIDFORGE_GIT_SHA` when set (CI and images), else `git rev-parse --short=12 HEAD`, else `unknown`.
    - Local builds are identified by their HEAD commit, not by uncommitted edits.
    - The core never sees it. The tests and the determinism suite pass fixed builds, so neither depends on git state.
  - **Dependencies:**
    - **blake3 `=1.8.7`** with `default-features = false, features = ["pure"]`: portable Rust, no assembly or C compiled, and the same hash in every mode. Its tree adds arrayvec, constant_time_eq, and cc, find-msvc-tools and shlex as build-time only. No new license (Apache-2.0 is taken from its OR expression) and no duplicate.
    - **postcard `=1.1.3`** and serde become normal dependencies of store. D-024's "reachable only through dev-dependencies" no longer holds for postcard, which store now uses at run time.
    - Store's dev-dependencies are testkit, worldgen and proptest, for harness worlds; none reaches the library.
    - Core's, rules', worldgen's and testkit's `cargo tree -e normal,build` are identical to main's (`slices/20-p2-2-snapshot-codec/proof/dependency-proof.txt`).
  - **Tests**:
    - `crates/voidforge-store/tests/snapshot.rs`: round trips on the busy world, an economy mid-advance with fleets and commands, and a generated world before and after Join game; the layout and hash; each refusal by variant; truncation at every length and every single-byte flip;
    - `crates/voidforge-store/tests/determinism.rs` joins the determinism suite (as worldgen's binary does): the guard, property round trips over testkit's `Scenario` and `Economy`, generated worlds, and in-process and two-process double runs over snapshot bytes. `.github/scripts/determinism.sh` runs it.
- Where it's enforced: `crates/voidforge-store/src/{snapshot.rs, canonical.rs}`, `build.rs`, the two test binaries, the determinism job, `deny.toml`, and the [version table](versions.md).

### D-036: The engine host loop, its log store and the record format

- Date and package: 2026-10-18, P2.3. Proposed by dev-design, acting as builder (`slices/21-p2-3-host-loop/proposal-host.md`). Lead ruling overnight 2026-10-17/18, endorsed by the advisor 2026-10-18. None of it is game intent.
- Decision:
  - **One thread owns the World.** `voidforge_host::spawn` starts a dedicated OS thread running a current-thread tokio runtime with every driver enabled, and the host loop is its only task.
    - Callers hold a `HostHandle`: `submit` through a bounded inbox, gets a `Receipt` (seq, t, outcome), `Busy` when the inbox is full (the API's 503), or `Stopped`. They also read the `ReadModel` and the counters.
    - `HostTask::join` returns why the host stopped. It stops by itself once every handle is gone.
  - **The store is built on the host's thread.** `spawn` takes a store constructor, because a database connection belongs to the tokio runtime that opened it. `PgLog::lazy` opens its connection on first use, on the host's runtime. The Postgres round trip found this: the memory store can't show it.
  - **The write path**, in order:
    1. wait for a command or the next queue entry (`select!`, biased to the inbox, against a sleep until `epoch + at_ms`), never polling; an idle host doesn't wake (`HostStats::wakes`);
    2. idempotency is P2.4;
    3. stamp t = max(wall clock − epoch, last t), where the clock is injected (`SystemClock`, or `ManualClock` for tests) and the epoch is configuration until P2.9; take the next seq, gap-free from 1 and after existing records, and refuse a World behind the log's last record;
    4. `apply(command, t)`, or `advance_to(t)` when only time has passed;
    5. one durable `append` for the whole turn: commands already waiting join it, up to `max_batch` (64), with no added delay;
    6. reply, then publish.
  - **One record per step.**
    - A command record's `events` are `Applied.advanced` followed by the command's own events.
    - An advance record's `events` are what `advance_to` returned.
    - Replay calls the same core function, flattens the result the same way, and compares.
    - Rejected commands are logged with their reason.
  - **An advance that fires no events is not logged.** Why that's sound: D-024's observation invariance (one advance or many give identical bytes) guarantees that the next logged step, which advances from the last logged state through this instant, reaches the same state. The log is a complete history of events, not of wake-ups.
  - **The read model** (`seq`, `Arc<World>`, `Arc<Rules>`) is immutable, published through `watch` after every durable write **and after every step that changes the World without a record**. An eventless advance still changes rates and `valid_until` (for example a storage crossing), and without a publish, views would be `Stale` until the next command.
  - **Fatal errors:**
    - a failed append stops the host: nothing in the turn is acknowledged (its callers get `Stopped`), the queue is refused, and memory is dropped;
    - an engine error that poisons the World (D-023) stops it too, after the turn's earlier steps are appended and acknowledged; the poisoning command isn't logged.
  - **Typed records** (`voidforge_store::log::LogRecord`) behind the `LogStore` trait. `MemoryLog` (with injected append failures) and `PgLog` (one transaction per append) implement it.
    - Records are typed, never `serde_json::Value`, because a `PlayerId` is a `u128` that a `Value` can't hold exactly. `PgLog` writes the JSON columns as text and reads them back as text, and jsonb keeps big integers exactly.
    - The JSON forms (D-034: values, never SQL NULL):
      - `payload` is the command's serde JSON, or `null`;
      - `outcome` is `{"result":"accepted"}`, `{"result":"rejected","reason":"<code>"}`, or `null`;
      - `events` is a JSON array.
    - Columns: `player_id` is the UUID form, `wall_time` is the injected clock's milliseconds (exact both ways), and `rules_version` goes through `i32::try_from` (both stores refuse a larger value).
  - **Dependencies:**
    - tokio `=1.53.3` (rt, sync, time, macros), already in the tree. tokio-macros is newly allowlisted.
    - serde_json becomes a normal dependency of store.
    - No new sqlx features: UUIDs, JSON and timestamps bind as text or integers with SQL casts. The uuid crate stays with P3.3.
    - The host's dev-dependencies are testkit and sqlx (for `#[sqlx::test]`).
  - **Tests:**
    - `crates/voidforge-host/tests/host.rs`: one per write-path step and crash row, on the memory store with a manual clock;
    - `tests/postgres.rs`: the done-when, the same economy script through both stores giving identical records, receipts and World; extreme values; all-or-none appends; the documented JSON forms. It runs in the database stage, which now runs the `database` and `postgres` binaries by filter;
    - `tests/determinism.rs`: in-process and two-process double runs over the records, run by `determinism.sh`.
- Carried obligations:
  - **P2.5 and P2.7: replay must reproduce unlogged eventless advances implicitly.** A test must show that a snapshot taken after one, then replay of the later records, equals the uninterrupted run.
  - **P2.4: the key-reuse obligation (D-034) still stands.** Records carry `idempotency_key: None` until then.
- Where it's enforced: `crates/voidforge-host/src/{lib.rs, clock.rs}`, `crates/voidforge-store/src/log.rs`, the three host test binaries, `.github/scripts/{database.sh, determinism.sh}`, `deny.toml`, and the [version table](versions.md).

### D-037: Idempotency: keys at any age, the cache window, and key conflicts

- Date and package: 2026-10-18, P2.4. Proposed by dev-design, acting as builder (`slices/22-p2-4-idempotency/proposal-idempotency.md`). **Lead ruling; advisor review pending.** None of it is game intent. Q3 is API contract.
- Decision:
  - **A repeated (player, key) always gets its first receipt, at any age.** That's the original seq, t and outcome, with nothing applied or appended. This closes D-034's obligation by construction: no duplicate (player, key) can reach the append, where the table's unique constraint would make it fatal, so no client can stop the host with an old key.
  - **The window bounds only the cache.** `HostConfig::key_window_ms` (default 24 h of game time) is how long keys stay in memory. Each turn, the keyed submissions that miss the cache are looked up in the log in **one query** (`LogStore::find_keys`, on the unique `(player_id, idempotency_key)` index). At start, the cache is rebuilt from the window's keyed records (`LogStore::keyed_since`). The Architectural Design's sentence ("keys stay known for a configurable window") is amended in this PR.
  - **The cost:** a new key always misses the cache. So once the API sends a key with every command (Phase 3), every turn with new commands does one batched, indexed log read before applying. That's acceptable at MVP scale, and a **P5 load-test watch item**: if it shows, add a Bloom filter or a durable key index.
  - **A key conflict (API contract):** the same (player, key) with a different `Command` is **`HostError::KeyConflict { seq }`**, where seq is the key's first record. Nothing is applied or appended, and it's not a core `Rejection` (D-029's codes are game rules; this is a protocol error). **Phase 3 maps it to 422 Unprocessable Content with the code `idempotency_key_conflict`.**
  - **The key:** `IdempotencyKey` is 1 to 128 bytes of printable ASCII without spaces, per player. It's submitted with `HostHandle::submit_with_key`. `submit` stays keyless, and a keyless command is never deduplicated. The record stores the key with the player.
  - **Within one turn,** the cache is updated as each step is applied, so a repeat later in the same turn gets the first's receipt, and a conflicting reuse gets `KeyConflict`. Both are replied to only after the turn's append is durable. A replay or conflict isn't a step: it changes nothing and publishes nothing.
  - **A failed key lookup is fatal** (`Fatal::Lookup`): the host can't tell a new key from a used one, so it stops rather than guess.
  - **`own_events` amends D-036's outcome form.** An accepted command's outcome is `{"result":"accepted","own_events":n}`: the record's last n events are the command's own, and the ones before them came due in the apply's advance. A receipt rebuilt from the log (after a restart, or past the window) is then exact. Replay is unaffected; it still compares the flattened list.
  - **`MemoryLog` refuses a duplicate (player, key)** as the table does, so a duplicate reaching the append stops the host on the memory store too, and the tests see it.
- Where it's enforced: `crates/voidforge-host/src/lib.rs` (the lookup and cache in each turn); `crates/voidforge-store/src/log.rs` (`IdempotencyKey`, `find_keys`, `keyed_since`, `Outcome::Accepted { own_events }`); `crates/voidforge-host/tests/idempotency.rs` and `postgres.rs`; and the shared test driver, which retries every third command in the round trip and the determinism runs.

### D-038: Snapshots, recovery and the shared record→core function

- Date and package: 2026-10-18, P2.5. Proposed by dev-design, acting as builder (`slices/23-p2-5-snapshots-recovery/proposal-recovery.md`). **Lead ruling; advisor review pending.** None of it is game intent.
- Decision:
  - **The shared function lives in `voidforge_store::replay`.** Store is a crate both host and replay may depend on (Project layering), and it defines the record; core can't hold it, because it's pure and the record isn't a core type.
    - `step(world, Step::Command | Step::Advance, t, rules)` returns the flattened events, the record's outcome and the receipt's result.
    - `replay_record` runs `step` on a logged record and compares its events and outcome (`Diverged`). It refuses another build's record (`CrossBuildRecord`), an unsupported kind, and a malformed record.
    - `replay` runs a gap-free tail (`Gap`).
    - **The host's live path builds every record from `step`.** It never calls `apply` or `advance_to` itself, and a source-level test enforces that. So recovery and replay can't drift from the live path.
  - **Snapshots** are taken between turns, never mid-turn:
    - after a durable turn once `snapshot_every_records` (default 1,000) records were appended since the last one;
    - and at clean shutdown, unless the newest snapshot is already at the head.
    - The row follows D-035's mapping, with seq = the last record included. A snapshot after an eventless advance includes it.
    - **The read model names the log head from the start** (carry e, from dev-qa): a host spawned on a non-empty log publishes `seq` = the head, not 0.
    - **A failed write isn't fatal but is loud:** an error event and `HostStats::snapshot_failures`, and the next schedule point retries.
  - **Recovery** (`voidforge_host::recover`):
    1. **the newest snapshot, of any build, and the whole tail after it must be the running build's records** (replay refuses another build's record: `CrossBuildRecord`). This single rule covers:
       - a normal restart;
       - an engine upgrade (the old build's shutdown snapshot at the head);
       - a crash after an upgrade (the old build's snapshot plus the new build's tail);
       - a rollback from a clean shutdown (the newer build's snapshot at the head).

       It refuses any replay across builds, and a snapshot format this build can't read is a typed error. **Carry (f), from dev-qa.** Two earlier rules were rejected:
       - "a matching build first, else another build only at the log head" can't recover a crash after an upgrade;
       - "a matching build first" fails a rollback after a clean shutdown and can never do better than the newest of any build.

       No snapshot at all is `NoSnapshot`
    2. a strict `decode` (rules version in force, the row's build), with the row's columns cross-checked against the header (`ColumnMismatch`);
    3. the later records replayed with verification;
    4. `spawn`. The host catches up to now on its first wake and rebuilds the idempotency keys from the log (D-037).
  - **Verification is event replay plus the state hash** (D-033): every replayed record must reproduce its events and outcome, and byte identity is checked by canonical bytes and the state hash. `check_links`, which runs inside decoding, is structural only and doesn't validate gameplay fields.
  - **Unlogged eventless advances** (D-036's obligation) are reproduced implicitly: the next replayed step advances through the same instant to the same state (observation invariance, D-024). A snapshot taken after one includes it.
  - **The P2.2 review's LOWs:** the header must be its own canonical encoding (`NonCanonicalHeader`); `decode_any` is documented as inspection only, with build metadata unverified, and recovery never uses it.
- Where it's enforced:
  - `crates/voidforge-store/src/{replay.rs, log.rs, snapshot.rs}`;
  - `crates/voidforge-host/src/lib.rs` (`recover`, the schedule, the shutdown snapshot);
  - `crates/voidforge-host/tests/recovery.rs`: the crash table, byte identity (a named test and a property test), the carries, the snapshot choice, a tampered tail, another build's record, and the shared function;
  - `postgres.rs` (recovery through Postgres);
  - `determinism.rs` (a run through snapshot, restore and continue, in two processes).
