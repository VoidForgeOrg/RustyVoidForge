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
  - **Amended by D-049 (S1, the advisor; the lead's Addendum 3):** fleets and ships are visible only to their owner, so **another player's fleet or ship is unknown**. It fails the existence check itself, as `unknown_fleet` or `unknown_ship`, and **at every check a foreign one is indistinguishable from one that doesn't exist**. `not_owner` is now only ever about a planet.

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

      Then `unreachable_target` (zero travel time, or an arrival outside 64 bits; **amended by D-050:** or a trip longer than the 1,000-year travel horizon), so a player's command never poisons the world. Of the five fleet commands, only Send and Recall compute a time from player input; Assemble and Trigger unload bound every amount by stock, capacity or free storage, and Disband moves only IDs (review-r2's audit, confirmed). Effect: D-027's `send_fleet`.
    - **Recall fleet**: `not_joined`; `unknown_fleet`; `not_owner`; `fleet_not_in_transit` (stationed), `already_recalled` (returning), then `unreachable_target` (the return, now plus the time traveled, past the end of time; review-r2's repair, so a recall too can never poison; **amended by D-050:** or a time traveled longer than the travel horizon). Effect: D-027's `recall_fleet`, including its departure-instant rule; its own checked error stays as the engine's defense.
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

- Date and package: 2026-10-18, P1.11. Proposed by dev-build. Lead ruling overnight 2026-10-17/18, endorsed by the advisor 2026-10-18. (Slice 17, `ruling-lead-overnight.md`.) Q4 was provisional; **Tom resolved it on 2026-10-18 ([D-039](#d-039-a-building-being-demolished-stops-scoring-when-demolition-begins)).**
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
  - **Q4, resolved by Tom on 2026-10-18 (reading B; [D-039](#d-039-a-building-being-demolished-stops-scoring-when-demolition-begins)):** a building being demolished does not count. It was provisional here as reading A (counting until demolition ended).
  - **Visibility**: planets, `PlanetsNear` and players are public. Fleets are visible only to their owner: the query names the viewer, and no query returns another player's fleets. **Phase 3 obligation**: the host takes the viewer from the auth token, never from the request body.
  - **Determinism**: every observation in the invariance runs takes every view and checks the bytes are unchanged. The done-when (a view at any t before `valid_until` equals the same view after `advance_to(t)`, field for field, and is `Stale` at `valid_until`) is a named test and a property over `Economy` worlds with fleets and commands.

### D-031: World generation

- Date and package: 2026-10-18, P1.12. Proposed by dev-build. Lead ruling overnight 2026-10-17/18, endorsed by the advisor 2026-10-18. (Slice 18, `ruling-lead-overnight.md`.) **Placement: Tom chose option A on 2026-10-18** (slice 25), replacing the provisional option B.
- Decision:
  - **Entry point**: `voidforge_worldgen::generate(seed, players, &Rules) -> Result<World, WorldgenError>`.
    - It produces the universe's first `World`: at time 0, under the rules' version, with the seed recorded. The epoch isn't in the World: it's the host's configuration. **Amended at the Phase 2 gate** (D-044): this once said the host sets the epoch in P2.9, which P2.9 didn't do. Phase 3 owns a stable epoch source.
    - It holds `players × planets_per_expected_player` unowned planets with the rules' slots and caps and empty stocks, plus `players` homeworld candidates.
    - Every planet is solved (D-025), and the links hold.
  - **Placement, option A** (Tom's decision, 2026-10-18; the brief is `missions/voidforge-mvp/tom-decision-worldgen.md`):
    1. `players` homeworld candidates, uniform in the cube on whole world units (D-021), pairwise at least `homeworld_min_spacing_du`.
    2. Around each candidate in turn, planets uniform within `homeworld_neighbor_radius_du` (inclusive, the bounding box clipped to the cube, points outside the ball rejected) until it has `homeworld_min_neighbors`, counting planets already there.
    3. The rest uniform in the cube, up to `players × planets_per_expected_player`. Every planet is at least `min_planet_spacing_du` from every other.
    4. The candidate order is a seeded Fisher–Yates shuffle.
    - Planet IDs are allocated in coordinate order, so an ID doesn't reveal which planets are candidates.
    - **The player limit** in the default cube is where random placement runs out of room for candidates: 241 to 254 players over seeds 0 to 11 (the packing bound allows 901). Past it, generation stops with `Exhausted { what: "homeworld candidate", .. }`.
    - **Why A:** Tom: "I don't want to grow the cube with players, as worldgen will definitely be rewritten at some point. You may increase the size of the cube or fix the generation." So option C (scaling the cube with players) is out. A bigger fixed cube only makes small worlds sparser. A guarantees every homeworld its neighborhood by construction, at any player count. It's deliberately minimal, since worldgen is temporary.
    - **Superseded, option B** (P1.12, provisional): planets uniform in the cube, candidates chosen among the planets that happened to have enough neighbors. Under the default rules it refused small worlds (`Infeasible("too few planets qualify as homeworld candidates")`, below roughly 31 to 40 players), so it is removed, along with the denser test rules that worked around it.
  - **Randomness**: `ChaCha8Rng::seed_from_u64(seed)` is the only source, from `rand_chacha =0.9.0` (default features off).
    - That version is already in the tree through opentelemetry_sdk's rand 0.9, so the graph gains no crate or duplicate. The guardrail fixture's 0.10.0 is a lint control only.
    - A unit test pins the first outputs for seed 42, so an upgrade that changed the stream fails before it changes worlds.
    - If telemetry moves to rand 0.10, the pin is revisited (it would then become a duplicate).
  - **Integer-exact sampling**:
    - `below(n)` by rejection of the top partial range: unbiased, with expected draws under 2.
    - Spacing is checked on exact squared distances in i128 against a `BTreeMap` grid (cell = spacing, 27 cells per check).
    - A candidate's neighbors are counted with the same inclusive `within` the neighborhood placement uses, by a scan of the planets placed so far.
    - The shuffle is Fisher–Yates with `below`.
    - Termination: each candidate and each planet tries at most `MAX_ATTEMPTS` (10,000) draws, then `Exhausted { what, placed }`, never a loop.
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
    - It caps the total world size an operator can ask for (players × planets per player; P1.12 QA LOW). **Done in D-043** (`MAX_PLANETS = 100_000`).
    - **Superseded on 2026-10-18** by D-031 option A (Tom's decision): any player count up to about 240 generates under the default rules, so the bootstrap and P3.6 need no minimum player count or denser rules. (It read: under option B, worlds below roughly 31 to 40 expected players were refused, so P2.9 and P3.6 had to use at least about 40 players or denser rules.)
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
    - **Amended at the Phase 2 gate** (D-044, QA O1): it also creates and drops its lock database `voidforge_test_lock`, so one run at a time uses a server, and its cleanup never terminates a session.
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
    3. stamp t = max(wall clock − epoch, last t), where the clock is injected (`SystemClock`, or `ManualClock` for tests) and the epoch is configuration (`HostConfig::epoch_unix_ms`; **amended at the Phase 2 gate**: this once said "until P2.9", but the bootstrap records no epoch, and Phase 3 owns a stable source, D-044); take the next seq, gap-free from 1 and after existing records, and refuse a World behind the log's last record;
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
- **Bounded repair 1** (dev-qa's findings, confirmed by review-r1):
  - **M1:** `recover` refuses a snapshot whose seq is past the log head (`SnapshotPastHead { snapshot_seq, head }`). Recovering from one would make the next acknowledged commands vanish at the following recovery. Tested through both stores.
  - **L1:** `replay_record` refuses a record whose `rules_version` isn't the supplied rules' (`RulesMismatch`), per Determinism rules.
  - **L2:** it refuses a command record whose `player` column isn't the command's player (`Malformed`).
  - **L3:** QA's two cases are in the suite: a record whose events were emptied diverges, and another build's *advance* record is refused.
- Where it's enforced:
  - `crates/voidforge-store/src/{replay.rs, log.rs, snapshot.rs}`;
  - `crates/voidforge-host/src/lib.rs` (`recover`, the schedule, the shutdown snapshot);
  - `crates/voidforge-host/tests/recovery.rs`: the crash table, byte identity (a named test and a property test), the carries, the snapshot choice, a tampered tail, another build's record, and the shared function;
  - `postgres.rs` (recovery through Postgres);
  - `determinism.rs` (a run through snapshot, restore and continue, in two processes).

### D-039: A building being demolished stops scoring when demolition begins

- Date and package: 2026-10-18, a follow-up after P2.5 (slice 24). **Tom's decision** (game intent), resolving D-030's provisional Q4; the brief is `missions/voidforge-mvp/tom-decision-score-demolition.md`.
- Decision:
  - A building counts in the score from the moment it is operational until its demolition begins. Buildings being demolished, demolished or cancelled count nothing; a building under construction still counts in proportion to the ingots delivered.
  - So a building's points leave the score at the exact instant `DemolishBuilding` is applied, not when the demolition ends.
- Why: demolition can't be cancelled, the building stops working at once, and nothing is refunded, so it is already lost to the player. D-030's reading A (counting until the demolition ended) kept points for a building that does no work.
- The [Game Design's Scoring table](Voidforge%20MVP%20Game%20Design.md#scoring) and the Architectural Design's score bullet now say this explicitly.
- Where it's enforced: `World::score` in `crates/voidforge-core/src/view.rs` (one match arm); `crates/voidforge-testkit/tests/views.rs` (`score_a_building_being_demolished_counts_nothing` and `score_drops_at_the_instant_demolition_begins`).

### D-040: Golden logs: scenarios, recording, the fixture build and bless

- Date and package: 2026-10-18, P2.6. Proposed by dev-build (`slices/26-p2-6-golden-logs/proposal-golden.md`). **Lead ruling; advisor review pending** (`ruling-lead.md`). None of it is game intent.
- Decision:
  - **Where the code lives (the plan's "testkit, host").** The scenarios are in testkit (`voidforge_testkit::golden`); the recording and the replay are in the host's tests (`tests/golden.rs`, `tests/common/golden.rs`). testkit stays below the host, with no new crate edge (D-035's reason). Store gains `LogRecord::{to_line, from_line}`, a record's one-line JSON form, which P2.7 can read too.
  - **The fixture build.** Goldens are recorded and replayed under `0.1.0+golden`, a fixed, test-only build identity, as every test passes a fixed build (D-035). A golden must replay on every commit, so the git-commit build can't be in it. Production recovery keeps D-038's real-build rule, and nothing relaxes the pin. `the_fixture_build_is_never_a_real_build` asserts `ENGINE_BUILD` is never the fixture. The rules version stays pinned, so a rules change fails the goldens until they are blessed again ("refreshed on rules changes").
  - **A golden** is `tests/golden/<scenario>/`:
    - `start.vfsnap`: the start World as a snapshot envelope (D-035);
    - `records.jsonl`: the host's log records, one per line. The JSON columns are values in D-036's forms, so a reviewer reads commands, outcomes and events directly;
    - `manifest.json`: the scenario and its description, the rules version, the build, the start hash, **the state hash after every record**, the end time and hash, and every player's score at the end.
  - **Recording**: the real host (`spawn`, `MemoryLog`, a `ManualClock`). The clock moves through each due event's own time, so advance records are stamped when their events came due, as with the system clock. At each cue the action is computed from the published World. Each cue states the outcome it must get (accepted, or rejected with a reason code), so a scenario can't quietly stop doing what it says. A keyed cue can be retried at once: it must get its first receipt and append nothing (D-037). The recording is then replayed, and it must equal the host's own World at the end, byte for byte.
  - **Replay** (`verify`):
    - the start is decoded strictly under the fixture build and the rules;
    - each record goes through `store::replay` (events, outcome, gap, build and rules checked), then its state hash is checked;
    - then the end hash and the scores.
    - The first failure names its record: a tampered event is `Diverged` at its seq, a tampered hash is `Hash { seq }`, and a dropped record is a gap at the next seq. That's the groundwork for P2.7.
  - **Only through `just bless`.** The freshness check, `the_goldens_are_what_a_fresh_recording_gives`, records every scenario again and compares each file byte for byte, and also checks one golden per scenario. A hand edit, or a behavior change that wasn't blessed, fails it and names the file and line. `just bless` runs the ignored `bless` test with `VOIDFORGE_BLESS=1`, prints new, unchanged or CHANGED per golden, and refuses goldens with no scenario. A PR that blesses calls out the output for review.
  - **Where they run**:
    - the golden binary in `.github/scripts/determinism.sh` (the CI table's golden replays) and in `test`;
    - `every_golden_recovers_through_postgres` in the database stage: each golden's start and records go into Postgres, read back exactly through jsonb, and `recover` reaches the last checkpoint and then the end hash (the Phase 2 gate).
  - **The first goldens**, all on Rules v0:
    - `start-and-build`: joins, builds, a cancellation, a demolition to its end;
    - `starve-and-resume`: starved constructions with an even split, full storage halting and resuming;
    - `ships-and-colonize`: three ships at once and a cancelled waiting build, and a same-millisecond colonization race with a removed fleet and a failure;
    - `transport-and-recall`: Transport, Move with Trigger unload, a recall, a return and a disband;
    - `rejections-and-retries`: a rejection per command family, and keyed retries;
    - `whole-loop`: the Phase 1 gate's hand-derived loop. Its recorded score, 3,239, is asserted against QA's hand derivation.
    - A coverage guard (exhaustive matches) fails unless the goldens accept every command, produce every event, and hold a rejection and an advance.
- Where it's enforced: `crates/voidforge-testkit/src/golden.rs`; `crates/voidforge-host/tests/{golden.rs, common/golden.rs, postgres.rs}`; `crates/voidforge-store/src/log.rs` (`to_line`, `from_line`) and `tests/line.rs`; `tests/golden/`; `justfile` (`bless`); `.github/scripts/determinism.sh`.

### D-041: The replay CLI: one verification path, the build pin and exit codes

- Date and package: 2026-10-18, P2.7. Proposed by dev-build (`slices/27-p2-7-replay-cli/proposal-replay-cli.md`). **Lead ruling; advisor review pending** (`ruling-lead.md`). None of it is game intent.
- Decision:
  - **One verification path, in store.**
    - `voidforge_store::golden` now holds the golden format and `verify_golden`, moved from the host's tests. This amends D-040: the host's tests still record, and they verify through store.
    - `voidforge_store::verify` holds `verify_log`. It works over any `LogStore`: the start snapshot is decoded strictly and cross-checked against its row, every later record goes through `replay::replay_record`, and every later snapshot row is a checkpoint. A Postgres log carries no per-record hashes, so its snapshots are its checkpoints.
      - Each later snapshot is decoded strictly and cross-checked against its row, like the start.
      - The replayed World is brought to the snapshot's own game time, since a snapshot may follow unlogged eventless advances (D-036); any event on the way is a divergence. It must then have the snapshot's hash.
      - Any snapshot past the log head is refused, as recovery refuses it.
      - These rules come from dev-qa's QA repair of c29210c: F1, a shutdown snapshot after an eventless advance was reported diverged; F2, a corrupt, past-head or inconsistent later snapshot verified.
    - Both report a `VerifyError`: `Diverged`, `Refused` (another build or rules version) or `Input`.
    - `ReplayError::Diverged` now carries what the record holds and what replay produced, in the log's JSON form.
    - `LogStore` gains `snapshot_index` (snapshot rows without payloads) and `snapshot_at`. `PgLog` gains `lazy_url`.
  - **The command line** (`voidforge-replay`, clap):
    - `golden <PATH>...`: goldens under the fixture build, with `rules/v<N>.toml` from the nearest ancestor or `--rules`;
    - `postgres --rules <FILE> [--database-url] [--from] [--to] [--build]`;
    - `inspect golden|postgres`: metadata through `decode_any`, labeled unverified in text and JSON (D-035's caveat);
    - `--json` on all of them.
    - There's no interactive mode, and the database URL is never printed.
  - **The build pin.** **Amended at the Phase 2 gate** (D-044, R1 LOW): the pin is on the records, as in recovery (D-038). Every snapshot row, the start's included, decodes under the build its row names, so an upgraded build's records verify on an earlier build's snapshot. The rules version is pinned everywhere. The expected build is the binary's own `ENGINE_BUILD`. `--build <ID>` asserts another, such as a test build or an earlier deploy's. **A pass under `--build` means this binary's code reproduces that log, not that the named build's code was verified.** A production incident log is verified by its own build's binary. `--help`, the crate guide and the output say so, and the output names the build and whether the flag gave it. Goldens always use the fixture build (D-040).
  - **Exit codes (a stable contract):**
    - 0: verified;
    - 1: diverged;
    - 2: usage (clap);
    - 3: refused (build or rules pin);
    - 4: input error;
    - 5: telemetry failed to start, or failed to shut down after a pass (a failing verification keeps its own code; D-016).

    With several sources, the exit code is the first failing source's.
  - **Output.** Text by default, or one JSON document with `--json`. The first divergence gives its `seq`, `what`, `expected` and `found`. For a gap (a removed or reordered record) the seq is the one expected there; otherwise it's the record's own. Stdout holds no times, so the same input gives byte-identical output, and the trace ID goes to stderr.
  - **Dependencies:**
    - **clap `=4.6.7`**, with std, derive, help, usage, error-context and env, and default features off (so no color crates). New crates: clap, clap_builder, clap_derive, clap_lex and anstyle. The license is MIT OR Apache-2.0; there's no new license and no duplicate.
    - anyhow, also named in the Tech stack, isn't added: the errors are typed and nothing needs it.
    - The replay crate's normal dependencies are core, rules, store, telemetry, tracing, tokio, serde, serde_json and clap.
  - **Where it runs:**
    - `tests/cli.rs` (the binary as a process, on every golden and on tampered copies) runs in `test` and in `determinism.sh`;
    - `tests/postgres.rs` runs in the database stage.
    - `just replay` runs the tool.
- Where it's enforced: `crates/voidforge-store/src/{golden.rs, verify.rs, replay.rs, log.rs}`, `crates/voidforge-replay/`, `crates/voidforge-host/tests/{golden.rs, common/golden.rs}`, `.sqlx/`, `deny.toml`, the [version table](versions.md), `justfile`, `.github/scripts/determinism.sh`.

### D-042: The crash-and-retry harness

- Date and package: 2026-10-18, P2.8. Proposed by dev-build (`slices/28-p2-8-crash-retry/proposal-crash.md`). **Lead ruling; advisor review pending** (`ruling-lead.md`). None of it is game intent.
- Decision:
  - **A kill is a real process death at a seeded point.**
    - `crates/voidforge-host/tests/crash.rs` re-invokes its own test binary as a child (the ignored `crash_child`). The child recovers from the store (D-038), runs the real host with a `ManualClock`, and talks JSON lines over stdin and stdout, prefixed `@crash `.
    - The child calls `std::process::abort()` at the Nth hit of a named kill point, with the point and N chosen from the seed. So no destructors, flush, reply or snapshot happen after it.
    - The kill points live in the child's store wrapper and protocol loop, which are test code. **No production code has a hook.**
  - **The kill points and their crash-table rows:**

    | Point | Crash-table row |
    | --- | --- |
    | `append.before` | before the append |
    | `append.during` | before the append (atomicity) |
    | `append.after` | after the append, before the reply |
    | `reply.after` | after the reply |
    | `snapshot.before`, `snapshot.during` (file store only), `snapshot.after` | between the reply and the snapshot |
    | `recover.during` | during recovery |

    - A kill inside a multi-command turn is asserted as hit.
    - Per batch, the parent checks by effect that a batch killed before or during its append is absent after the death, and one killed after it is durable.
  - **Postgres's `append.during` dies inside PgLog's own open transaction.**
    - A test-only trigger on `log_records`, armed through a `crash_hold` table, holds PgLog's first INSERT in `pg_sleep`. The child aborts once it sees that backend sleeping.
    - The test database's `client_connection_check_interval` is 20 ms, set with `ALTER DATABASE` because it must be in place before a statement starts. The server notices the dead client and rolls the transaction back.
    - This is PgLog's exact statements, with no copied SQL and no hook. A Postgres snapshot is one autocommit INSERT, so Postgres has no separate `snapshot.during`.
    - The harness's control SQL (the trigger, the hold, `pg_stat_activity`) is test-only and uses `sqlx::raw_sql`, not checked queries.
  - **The memory variant is a file-backed test store,** `FileLog` (`tests/common/filelog.rs`): JSONL records, each append ending in a commit line and `sync_all`, with a torn tail discarded. That way a death keeps exactly what was durable.
  - **Clients retry every unacknowledged command with its key** in the next life. The last life has no kill, so it completes the script and shuts down cleanly.
  - **The checks, after every seed:**
    - exactly one record per (player, key), and none unplanned;
    - each client's final receipt equals its record (seq, t, outcome, own events);
    - `verify_log` from the seq-0 snapshot (D-041);
    - gap-free seqs;
    - the log and the World at the end equal a reference host that never died, given the logged commands at their logged times.
  - **Determinism:** each life's kill point and N are functions of the seed. How a turn's commands are batched depends on scheduling, so the records inside a multi-command turn may differ between runs of one seed. The checks don't depend on it, and a failure names its seed.
  - **Bounds:**
    - per PR, 12 seeds of 40 commands on the file store in `test` (about 5 s), and 4 seeds of 30 commands on Postgres in the database stage (about 6 s; the filter gains the crash binary's Postgres test);
    - the nightly shape (P5.3 schedules it) is the ignored `crash_nightly`, with `VOIDFORGE_CRASH_SEED_BASE`, `VOIDFORGE_CRASH_SEEDS` and `VOIDFORGE_CRASH_COMMANDS`.
- Where it's enforced: `crates/voidforge-host/tests/{crash.rs, common/filelog.rs}`, `.github/scripts/database.sh`.

### D-043: The universe bootstrap, the world-size cap and `just seed`

- Date and package: 2026-10-18, P2.9. Proposed by dev-build (`slices/29-p2-9-bootstrap/proposal-seed.md`). **Lead ruling; advisor review pending** (`ruling-lead.md`). None of it is game intent.
- Decision:
  - **The world-size cap** (D-033's carry, the P1.12 QA LOW): `voidforge_worldgen::MAX_PLANETS = 100_000` on players × planets per expected player.
    - It's checked before anything is allocated, as a typed `WorldgenError::TooLarge { planets, max }`, and the error names the limit.
    - Measured in release (generation plus canonical bytes): 2,000 planets in 6 ms (74 KB); 20,000 in 47 ms (0.8 MB); 100,000 in 0.26 s (4.1 MB). Linear growth.
    - At Rules v0 the cap is 5,000 players, so it binds only for custom, larger cubes. Below it, the default cube refuses earlier, in two ways (**amended at the Phase 2 gate**, D-044; measured by dev-qa at P2.9):
      - from about 241 to 256 players, depending on the seed, homeworld candidates run out of room: `Exhausted`;
      - from 902 to 5,000 players, the packing pre-check refuses before any draw: `Infeasible`. The pre-check's bound (about 901) is a necessary condition, so the `Exhausted` limit binds first.
      - Both are world-generation errors: `voidforge-server seed` exits 4 for them (only `TooLarge` and no players are refusals, exit 3).
  - **`voidforge-server seed --seed --players --rules [--database-url] [--json]`:**
    1. generate (option A, D-031);
    2. encode the snapshot under this binary's `ENGINE_BUILD`;
    3. `voidforge_store::bootstrap`: connect, apply the migrations, then, in one transaction with both tables locked (`EXCLUSIVE`), refuse unless `snapshots` and `log_records` are empty, and write the snapshot at seq 0.
    - **A universe is never overwritten:** the refusal is `NotEmpty` and names the existing seq-0 state hash.
    - The output is the state hash (or one JSON document). The database URL is never printed.
    - Exit codes as D-041's: 0 seeded, 2 usage, 3 refused (not empty, too large, or no players), 4 input or database error, 5 telemetry.
    - The state hash covers the World only (D-035), so the same seed, players and rules give the same hash under any build.
  - **`just seed` and owner safety:**
    - `.github/scripts/seed.sh` targets only `voidforge_dev` on the local stack (created if missing). It never names or migrates the owner's `voidforge` database (D-034 stands).
    - `just seed` refuses (exit 3) when `voidforge_dev` holds a universe.
    - `just seed-reset`, the only reset, drops and recreates `voidforge_dev` (the name is fixed in the script), then seeds.
    - The defaults are `SEED=20261018` and `PLAYERS=100` on Rules v0; `RULES=fast` uses `rules/v0-fast.toml`. Its state hash under Rules v0 is `63185376ae382891972632b6f15eb52cf87816dbce27a20a48c2efa23ea487f5`, pinned in the database-stage test.
    - The script builds with `SQLX_OFFLINE=true`, so queries are checked against `.sqlx/`, never against the dev database.
    - Phase 3's local server runs against `voidforge_dev` too.
  - **Dependencies:** the server gains clap (D-041), serde, serde_json, tokio, rules, store and worldgen; its tests gain host, core, testkit and sqlx. There's no new crate in the tree.
  - **Tests:**
    - `crates/voidforge-server/tests/cli.rs` covers the cap and no players (exit 3, before any connection), a bad URL (exit 4, never echoed), and usage (exit 2);
    - `tests/postgres.rs` (database stage) covers the pinned hash on two fresh databases, equal to `generate` in-process; a second seed refused with the database unchanged; and the host recovering from seq 0 (D-038), serving a Join game, and recovering again with the player.
- Carried from P2.8 (R2 LOW): `CRASH_DEBUG` keeps the crash harness's child stderr piped and echoes it, so the kill marker is always parsed.
- Where it's enforced: `crates/voidforge-worldgen/src/lib.rs`, `crates/voidforge-store/src/lib.rs` (`bootstrap`), `crates/voidforge-server/`, `.github/scripts/seed.sh`, `justfile`, `.sqlx/`.

### D-044: Phase 2 gate repair: verification failures, the record pin, the database-stage lock and the epoch

- Date and package: 2026-10-18, the Phase 2 gate repair (the lead's gate brief on 145daee: QA PASS, review-r1 CLEAR, review-r2 REQUIRES REPAIR). Proposed by dev-build. None of it is game intent.
- Decision:
  - **A failure is never a value** (review-r2 L1). `verify_golden` once turned a core scoring error into `i64::MIN`, so a manifest naming that number verified while the core returned `ScoreOutOfRange`.
    - Now a scoring error is `VerifyError::Diverged` (`scores`), naming the player and the core's error. `voidforge-replay golden` exits 1.
    - The sweep of the verify paths (store's `golden`, `verify` and `replay`, and the replay CLI) found one more: the CLI's golden search skipped a directory entry it couldn't read, so a directory of goldens could pass on the readable rest. An unreadable entry is now an input error (exit 4).
    - Round 2 (dev-qa L1, confirmed by review-r2): a directory holding a golden's snapshot or records but no `manifest.json` was also passed over, and the sweep reported verified. It's now an incomplete golden: an input error naming it (exit 4), reported beside every other source found. A directory with none of a golden's files is still passed over.
    - The rest are display fallbacks for report text only (`events_json`, `outcome_text`, the CLI's text for a pass). Every comparison is typed.
  - **The build pin is on the records** (review-r1 LOW, the shared-path principle). `verify_log` once decoded every snapshot row under the pinned build, while recovery decodes the row under its own build (D-038). So after an engine upgrade, recovery accepted a tail that `voidforge-replay postgres` refused.
    - Now every snapshot row, the start's included, decodes under the build its row names, and the pin applies to records only. The rules version stays pinned everywhere.
    - A row naming another build than its payload's header is a divergence at that snapshot, like any other column mismatch.
    - The host's upgrade matrix (`upgrade_cases`, on memory and Postgres) now checks that `verify_log` agrees with recovery: from A's snapshot, B's records verify under B's pin, and A's pin refuses B's first record.
  - **The database stage holds a lock, and its cleanup never ends a session** (dev-qa O1). On the shared local stack, one seat's `just db-test` force-dropped every `_sqlx_test_*` database and killed another seat's live run.
    - `database.sh` takes `voidforge_test_lock` (CREATE DATABASE is atomic) before anything else, through `.github/scripts/database-lock.sh`. A second run waits up to `VOIDFORGE_DB_LOCK_WAIT` seconds (default 1,800), then fails naming the holder and the command that clears the lock.
    - The lock's comment names its holder's host and pid. **A lock whose holder is no longer running is refused, never taken over**, with the exact command that clears it. Round 1 took such a lock over, and review-r2 (M1) showed two waiters that both saw the dead holder could both enter, the second dropping the first's live lock. A run killed before its exit trap is the only way to leave one, so clearing it by hand is rare.
    - A release drops the lock only while its comment still names that run.
    - `.github/scripts/test-database-lock.sh` is the regression. It drives the same functions against the real server on a scratch lock of its own run, `voidforge_test_lock_selftest_<pid>`, dropped on exit, with review-r2's two-contender interleaving made deterministic: both runs refuse, neither enters, and the stale lock is untouched. It also covers a free lock, a live holder and a release of another holder's lock. Every worker runs under a hard timeout (`LOCK_TEST_TIMEOUT`, default 60 s), so a lock that deadlocks or loops fails its case instead of hanging the run. An earlier version used bare waits and hung under the takeover mutant. `just db-test` and the CI database job run it before `database.sh`.
    - Round 3 (dev-qa M1, confirmed by review-r2): the self-test once used one fixed scratch name. Two `just db-test` runs started together dropped each other's scratch lock and both failed before reaching the real lock. Now each run has its own, and a fifth case runs two copies of the self-test at once, which must both pass.
    - Cleanup drops only idle `_sqlx_test_*` databases, without `FORCE`. Before a run, a test database in use is an error (exit 2): another test run is using the server. After a run, one in use is left with a warning. sqlx's bookkeeping rows are cleared only for databases that are gone.
    - It doesn't help a direct `cargo nextest` against `voidforge_test`, which takes no lock, and sqlx's per-test database names are fixed, so two runs of the same test still collide (the later one fails, and nothing is killed).
    - Checked on the local stack: a live holder makes a run wait, then fail; an in-use test database stops a run and survives it; a stale lock is refused. Two `just db-test` runs started together (self-test, then `database.sh`) run one after the other, both green: the second waits for the real lock.
  - **The universe epoch** (review-r2 INFO). D-031 and D-036 said P2.9 would record or set the epoch. It doesn't: the bootstrap writes a time-0 World, and `HostConfig::epoch_unix_ms` is supplied by the caller. Those promises are amended.
    - **Obligation for Phase 3, P3.4** (the first package that starts the host in the server, projects views to now and returns UTC times): the server's startup takes a stable epoch, the same value on every start, from configuration or from a durable record written at seeding. A changed epoch would move game time for every player.
  - **D-043's wording**: the default cube's limits are `Exhausted` from about 241 to 256 players (seed-dependent) and `Infeasible` from 902 to 5,000, below the 5,000-player cap.
- Carried: `crash_nightly` shares the per-PR `file-{seed}` scratch prefix; it gets its own before P5.3 schedules it (review-r1 INFO).
- Where it's enforced: `crates/voidforge-store/src/{golden.rs, verify.rs}`, `crates/voidforge-replay/src/main.rs`, `crates/voidforge-store/tests/verify.rs`, `crates/voidforge-host/tests/{golden.rs, common/mod.rs}`, `crates/voidforge-replay/tests/{cli.rs, postgres.rs}`, `.github/scripts/{database.sh, database-lock.sh, test-database-lock.sh}`, `.github/workflows/ci.yml`, `justfile`.

### D-045: The public API wire contract

- Date and package: 2026-10-18, P3.1. Proposed by dev-build (`slices/31-p3-1-api-types/proposal-api-types.md`). Lead ruling, with an addendum on the dependency audit and the 401 codes (`ruling-lead.md`). None of it is game intent; all of it is API contract. The advisor may review it, and any amendment lands as a follow-up before Phase 4 consumes the contract.
- Decision:
  - **`voidforge-api-types` holds the contract** as plain serde types with no workspace dependency (`command`, `event`, `view`, `error`, `wire`), plus `openapi()` for the OpenAPI 3.1 document. The server maps between them and the core (P3.4).
  - **Wire values:**
    - entity IDs are JSON integers (`u64`). They come from one engine counter, so leaving JavaScript's safe range (2^53) would take about 9×10^15 entities;
    - player IDs are lowercase hyphenated UUID strings (`format uuid`, with a pattern);
    - game times are RFC 3339 UTC with milliseconds, always (`2026-10-01T21:00:00.000Z`, with a pattern): game time is in milliseconds, and an `eta` cut to seconds would be off by up to 999 ms;
    - amounts are integer grams (`_g`), rates grams per hour (`_g_per_h`), positions and distances distance units (D-021);
    - enums are snake_case strings, and the resources are `iron_ore` and `iron_ingot`, in stocks and in cargo (`iron_ore_g`, `iron_ingot_g`).
  - **Commands:**
    - The body is `{"type", "payload"}` with one of the **ten player actions**. **Join game isn't public**: the server issues it on a player's first call (Decision 9, P3.3), so `join_game` is 400 `malformed_body`. The Architectural Design's "eleven command types" sentence is amended.
    - Unknown fields are refused, at the envelope and in each payload, by serde and by the schema.
    - `assemble_fleet.ship_ids` is a list with `uniqueItems`. The API refuses a duplicate at its edge with 400 `duplicate_ship_ids` (`AssembleFleet::check_unique_ships`), which closes D-029 Q1's obligation for P3.4. An empty list and negative cargo are well-formed, so they reach the core's 422 `no_ships` and `invalid_cargo`.
    - `IdempotencyKey::parse` carries D-037's rule: missing is 400 `missing_idempotency_key`, invalid is 400 `invalid_idempotency_key`.
    - 200 is `{"seq", "game_time", "events"}`. A repeated key returns the first response, at any age (D-037).
  - **The response's events are the command's own events only. This is a privacy requirement, not tidiness.** The log record also holds the events of advancing to the command's time, and those can be other players' owner-only information (a fleet arrival, D-030). P3.4 must never add them back. The record's `own_events` count (D-037) rebuilds the same list for a repeated key.
  - **No answer tells a player that another player's fleet or ship exists** (D-049, S1): a foreign fleet or ship is refused exactly as an unknown one, byte for byte. The coarse creation volume visible through the shared sequential ID counter is accepted for the MVP.
  - **Events** are internally tagged by `type`: `building_completed`, `ship_completed`, `demolition_completed`, `fleet_arrived`, `planet_colonized`, `colonization_failed`, `cargo_unloaded`, `fleet_removed` and `fleet_returned`, with `_id` and `_type` names.
  - **Views:** every view has `as_of` and `valid_until` (always present, null when nothing is scheduled). **`valid_until` counts only the viewer's own fleets and the entity's own scheduled events** (D-049, the advisor's ruling): it means "valid until, unless another player's fleet arrives", so a view never tells one player when another's fleet lands. Every continuous value has its `rate_g_per_h`, every stock its `cap_g`, and every progress its exact `eta` (null while nothing flows).
    - The planet view is the architecture's example, extended:
      - `rate_g_per_h` on progress, which the every-continuous-value invariant requires;
      - `position`, `slots`, `ship_builds` and `roster`;
      - buildings tagged by `state` (`under_construction`, `operational`, `demolishing` with `demolition_ends_at`), with an optional flow `status` (`running`, `throttled`, `halted`). Each state is a closed variant listing every field it has (repair round 1).
    - The architecture's example is updated to the contract (milliseconds, the rate) and marked abridged.
    - Planets near, the player (`player_id`, `homeworld_id`, `planet_ids`, `score`) and the player's fleets (tagged by `state`: `stationed`, `in_transit`, `returning`).
    - **There's no display name**: the accounts table has none. Adding an optional one later is backward compatible.
    - The leaderboard (P3.5) is `{as_of, entries: [{rank, player_id, score}]}`, ordered by score descending, then player ID. `rank` is competition ranking, so ties share a rank.
  - **Errors:** one body for every non-2xx status, `{"code", "message"}`. `code` is a closed enum (`ErrorCode`, with `status()`); `message` is for people.

    | Status | Codes |
    | --- | --- |
    | 400 | `malformed_body`, `missing_idempotency_key`, `invalid_idempotency_key`, `duplicate_ship_ids`, `invalid_query` |
    | 401 | `missing_token` (no Authorization header; it reveals nothing); `invalid_token` for **every** verification failure (expired, foreign issuer or audience, bad signature), so the API never says which check failed |
    | 404 | `not_found`; `GET /planets/{id}` answers `unknown_planet` with 404 |
    | 422 | the core's **31 reason codes verbatim** (D-028, D-029), plus `idempotency_key_conflict` (D-037) |
    | 429 | `rate_limited` (P3.8) |
    | 500 | `internal` |
    | 503 | `busy` (a full inbox, with Retry-After), `unavailable` |

    The 422 codes are kept verbatim mechanically: `crates/voidforge-server/tests/api_codes.rs` (the server's tests may use the core, D-043) maps every `Rejection` to its `ErrorCode` in an exhaustive match. So a new core reason code fails to compile until the API names it, and every API game code must deserialize as a `Rejection` with the same `code()`. The API's codes and the command-line tools' exit codes (D-041) are separate contracts.
  - **`openapi.json` is checked in** beside the crate. A freshness test fails on any byte of difference from `openapi()`, and `just openapi` rewrites it, as `just bless` does for goldens (D-040). **A reviewer checks every change to it.** The document has the six paths (described in api-types on doc-only functions, so P3.4's handlers must match), the bearer scheme (P3.2) and the required `Idempotency-Key` header. A modifier closes each variant of the command and event unions, because utoipa leaves a tagged variant's object open while serde refuses unknown fields there.
  - **Dependencies:**
    - **utoipa `=6.0.0`** (macros only, default features off) adds 2 crates (utoipa, utoipa-gen), with no new license and no duplicate. It reaches the web client and the bots through api-types.
    - **jsonschema `=0.59.0`** (dev-only, default features off, so no HTTP or TLS) validates against draft 2020-12, the dialect of OpenAPI 3.1. It adds 33 crates and no duplicate. boon `=0.6.1` (10 crates) was used first; repair round 1 replaced it with the ruled fallback because boon compares every number as an f64, so it can't tell 2^63 - 1 from 2^63 or 2^64 - 1 from 2^64. jsonschema compares integers exactly (num-cmp).
    - jsonschema pulls in borrow-or-share, licensed **MIT-0** (MIT without the attribution clause, OSI-approved), which joins `deny.toml`'s license allow list (D-008).
- **Repair round 1** (review-r2 on 9775536: F1 MEDIUM, L1 LOW; QA PASS). Rule: **the schema and serde agree.** Every value one refuses, the other refuses too.
  - **F1:** a building and a fleet were an `allOf` of a flattened state and the common fields, so the schema accepted a misspelled field or another state's field, and so did serde (flatten defeats `deny_unknown_fields`). Each is now one internally tagged union whose variants list every field of that state, closed in serde (`deny_unknown_fields`) and in the schema (`additionalProperties: false`, the `Closed` modifier).
  - **L1:** every integer schema now carries its Rust type's exact bounds as JSON integers: i64 is -2^63 to 2^63 - 1, u32 is 0 to 2^32 - 1, and u64 (the IDs, keeping the full domain) is 0 to 2^64 - 1. This covers referenced IDs and the inline path and query parameter schemas.
    - `openapi()` now returns the document as JSON, after `bound_integers`, which panics on an integer kind it doesn't know, and the tests call it.
    - The checked-in file's keys are therefore sorted. The proof's `openapi-semantic-diff-r1.txt` lists the 54 real changes.
  - **The same sweep, beyond the findings:**
    - player IDs and timestamps are checked on deserialization exactly as the schema's patterns check them (the timestamp also against the calendar, with no leap second, and the pattern bounds hours, minutes and seconds);
    - the nullable fields that are always sent are required by serde too (`deserialize_with = "Option::deserialize"`), as the schema requires them;
    - the `Idempotency-Key` header carries `IdempotencyKey::parse`'s rule as a pattern;
    - `near` carries a pattern of three integers. A coordinate beyond i64 that fits the pattern is P3.4's `invalid_query`, the one remaining gap; a regular expression can't bound i64 exactly.
    - **Two deliberate differences, and only two:**
      - a duplicate ship ID is well-formed to serde (a list), so that `check_unique_ships` answers 400 `duplicate_ship_ids`;
      - an **integral float** (`1042.0` where an integer goes) is an integer in JSON Schema 2020-12, so the schema accepts it, but serde refuses it for an integer field. In a request, that's 400 `malformed_body`, which P3.4 confirms. The server never writes one. (Repair round 2, dev-qa's I1.)
  - **Tests:** each composed view refuses a typo and another state's field (schema and serde), while every legitimate state validates. Each integer kind is checked at its valid edge and one past it, through a component, a referenced ID, the path ID and the `radius` query parameter.
    - JSON past i64's minimum or u64's maximum parses as a float, so the first value past i64::MIN is the nearest float, -2^63 - 2048.
    - An agreement table checks UUID forms, times (leap days, month ends, 24:00, :60, no milliseconds, offsets), missing and null fields, idempotency keys and `near`.
- **Repair round 2** (QA's Q1 and I1 on 5571ab7; review-r2 PASS): tests and docs only, with no product change.
  - The agreement tests now hold dev-qa's cases:
    - century leap years;
    - every month's last day and the next, in a common and a leap year;
    - a wrong byte at each timestamp separator;
    - every always-sent nullable field missing, one by one;
    - every other state's field in every building and fleet state, directly and inside the whole view, against each state's field set pinned as contract (so a field added to a state fails even when serde and the schema agree on it).
  - dev-qa's 11 drift mutants, of which 4 were caught before, are now all caught.
  - The integral float is named as the second deliberate difference, above.
- Where it's enforced: `crates/voidforge-api-types/` (`src/`, `openapi.json`, `tests/contract.rs`, `tests/openapi_file.rs`), `crates/voidforge-server/tests/api_codes.rs`, `justfile` (`openapi`), `deny.toml`, the [version table](versions.md), the Architectural Design's Read side and API.

### D-046: Token checks

- Date and package: 2026-10-19, P3.2. Proposed by dev-build (`slices/32-p3-2-token-checks/proposal-token-checks.md`). Lead ruling with five amendments (`ruling-lead.md`). None of it is game intent.
- Decision:
  - **`voidforge_server::auth`.** The server crate gains a library target. It's still one of D-003's binary crates and still not in `[workspace.dependencies]`; only its own tests use the library.
    - Two web-isolation guardrail fixtures relied on the server having no library ("cargo ignores a path dependency on a binary-only crate"). They now use voidforge-replay, which is still binary-only, to keep testing that behaviour.
    - An api-types → server path dependency would now be a cycle, because the server depends on api-types.
    - Through `voidforge_server::auth`, a request's `Authorization: Bearer <dex ID token>` becomes a verified `Principal { issuer, subject }`, through an axum extractor over `Arc<TokenVerifier>`, or a refusal. P3.3 maps the principal to a player, and P3.4 puts it on every route. No placeholder route is added to the binary.
  - **The order, and nothing ever turns into an accept:**
    1. **The header, before any key state** (amendment 2):
       - no `Authorization` header is 401 `missing_token`;
       - anything but exactly one well-formed `Bearer <token>` of JWT characters, at most 8 KiB, is 401 `invalid_token`. That covers another scheme, an empty token, two headers, extra spaces and non-ASCII, and holds even while keys are unavailable. The scheme is case-insensitive.
    2. **The token's header:** `alg` must be **RS256**, before any key is touched, so never `none`, HS* (no confusion with a public key used as a secret), ES* or PS*. A `kid` of 1 to 128 printable ASCII bytes is required; we never try every key.
    3. **The key, by `kid`.** When no key set is usable, or an unknown `kid`'s refresh fails, the answer is **503 `unavailable` with `Retry-After`**: the token may be valid and can't be checked now.
       - `Retry-After` is the seconds until the next fetch may run, from 1 to 30 (amendment 3).
       - A fresh set without the `kid` is 401: it's unknown.
    4. **The signature**, then **the claims**, checked by us rather than by jsonwebtoken (its claim checks are off, so the edges are explicit):
       - `iss` equals the configured issuer exactly;
       - **every** `aud` value (a string or an array, never empty) is in the allowlist. A token minted for anyone else too is refused; jsonwebtoken's own check would accept any overlap;
       - `exp` is required, with the leeway;
       - `nbf`, if present, is checked with the leeway;
       - `iat` is required, and not past now plus the leeway;
       - `sub` is 1 to 255 bytes, the accounts table's `subject`.
       - Numeric claims are non-negative integers within u64. Negative, float, string and beyond-u64 values don't decode, so the token is invalid. The comparisons saturate, so `exp` at u64::MAX is a far-future token, not a wrap.
  - **The key cache:**
    - Discovery comes from the issuer alone: the document's `issuer` must equal the configured one exactly, or the fetch fails. A mismatch is never trusted.
    - Only JWKs with `kty` RSA, `alg` RS256 or absent, `use` sig or absent, a well-formed `kid`, and an `n` and `e` that make a key are loaded. A set with none is a failed fetch.
    - Each fetch has a 5 s timeout, a 64 KiB body cap, and no redirects.
    - Refresh runs single-flight, so concurrent requests share one fetch. On an unknown `kid` it runs **at most once per floor** (10 s), and a `kid` that isn't well-formed never triggers one.
    - It refreshes periodically, every 15 min (`refresh_forever`), which also drops keys dex no longer publishes.
    - A failed fetch keeps the last good keys. Keys older than 24 h without a successful fetch are dropped, which means 503.
    - At startup with dex down, the server starts, and the refresher retries from 1 s doubling to 30 s.
    - **The accepted cost:** a token signed with a key dex rotated in within the floor of a previous fetch gets 401 until the next allowed fetch, at most 10 s.
  - **Responses:**
    - The body is D-045's `ErrorBody`. The message never names the check, and the token's text never appears in a body or a log.
    - The failing check is logged at debug level.
    - `WWW-Authenticate: Bearer realm="voidforge"` comes with `missing_token`, plus `, error="invalid_token"` with `invalid_token` (RFC 6750).
    - The signature check is aws-lc's, and the early refusals depend only on public header bytes.
  - **Configuration** (`AuthArgs`, clap with env fallbacks; P3.4's `serve` flattens it):

    | Setting | Env | Default | Bound at startup (amendment 4) |
    | --- | --- | --- | --- |
    | `--issuer` | `VOIDFORGE_ISSUER` | `http://localhost:5556/dex` | an http(s) URL |
    | `--audience` | `VOIDFORGE_AUDIENCES` | `voidforge-web,voidforge-cli` | at least one, none empty |
    | `--token-leeway-secs` | `VOIDFORGE_TOKEN_LEEWAY_SECS` | 30 | at most 300 |
    | `--jwks-refresh-secs` | `VOIDFORGE_JWKS_REFRESH_SECS` | 900 | 60 to 86,400 |
    | `--jwks-min-refresh-secs` | `VOIDFORGE_JWKS_MIN_REFRESH_SECS` | 10 | 1 to 3,600, at most the refresh |
    | `--jwks-max-stale-secs` | `VOIDFORGE_JWKS_MAX_STALE_SECS` | 86,400 | from the refresh to 604,800 |

    A setting out of range is a `ConfigError`, and the server refuses to start.
  - **Dependencies** (measured before committing):
    - **jsonwebtoken `=11.1.0`** on **aws_lc_rs** (default features off, so no PEM), **axum `=0.8.9`** (http1, tokio, json) and **reqwest `=0.13.5`** (adds json; no TLS). Together they add 18 crates, with no duplicate in the graph, and `cargo deny` advisories are ok.
    - **The rust_crypto backend was rejected:** 44 crates, 6 duplicate families, and RUSTSEC-2023-0071 (the `rsa` crate's Marvin timing attack) would have needed an advisory ignore.
    - **aws-lc-rs `=1.18.2`** is also a dev-dependency: the tests generate their RSA keys at run time, so no private key is committed.
    - **ISC and BSD-3-Clause** join `deny.toml`'s license allow list (D-008): aws-lc-rs, aws-lc-sys and untrusted. reqwest joins the server's AGENTS.md allowlist (docs first).
    - **What aws-lc-sys needs to build** (amendment 1): a C compiler. It builds with the system compiler (clang on macOS), with no cmake installed: measured at 19 s on macOS. CI's Linux runners build it in the `lint`, `test` and `database` jobs.
  - **Tests:**
    - **`tests/auth.rs`** (in `test`, over real HTTP) runs a local dex stand-in (discovery and keys, a fetch counter, failure on demand) and a test app guarded by the extractor. It covers the cases in the slice's proof map, including:
      - rotation through an unknown `kid`;
      - the floor under a flood of random `kid`s;
      - single-flight under 20 concurrent requests;
      - the periodic refresh dropping a key;
      - dex down at start and later;
      - stale keys;
      - a discovery issuer mismatch;
      - unusable and oversized key sets;
      - the configuration bounds.

      Every 401 is checked for its code and `WWW-Authenticate`, for an echoed token, and for any check named in the body.
    - **`tests/dex.rs`** checks the real dex, `#[ignore]`d and run by **`just dex-test`**: discovery and keys load (any dex config), and a password-grant token for alice verifies while a tampered copy doesn't. The second test needs `DEX_CONFIG=config.ci.yaml just up`. **QA runs it** (amendment 5).
- **Repair round 1** (QA PASS and review-r1 CLEAR on 4427641, with no accept path found; the follow-ups were folded in):
  - **The floor counts from when a fetch finishes** (review-r1's R1-1). It was stamped when a fetch started, so with a dex slower than the floor, every queued unknown-`kid` request made its own fetch: review-r1 measured 53 fetches for 53 requests. Now the floor, and whether that fetch succeeded, are recorded when it ends.
    - A request that waited for the lock looks at the cache again before fetching.
    - Inside the floor, an unknown `kid` is 401 after a successful fetch and 503 after a failed one, so a down dex gets one fetch per floor too (dev-qa's Q9).
    - The second look matters (corrected in repair round 2: round 1 called it equivalent, and dev-qa showed otherwise). In dev-qa's schedule, a waiter that gets the lock just past the floor would fetch again for a key another request had just fetched: one extra fetch, still within one per floor, and seconds of delay. `a_waiter_finds_a_key_another_request_just_fetched` pins it.
  - **`crit` is refused** before any key is touched (RFC 7515 §4.1.11: a critical extension this verifier doesn't understand must be refused, and it understands none; dex never sets one).
  - **The clock is never a sentinel** (R1-2): a system clock before 1970 is a 503 `unavailable`, logged loudly, not a time of 0.
  - **The clock is read after the key lookup** (repair round 2, dev-qa's F1). Round 1 read it before, and the lookup can wait on the lock and on fetches (5 s each), so `exp`, `nbf` and `iat` were judged against the request's arrival. A token expiring during a 4 s wait was accepted. `expiry_is_judged_after_the_wait_for_keys` pins it.
  - **The configured issuer is taken verbatim** (R1-3), trailing slash and all, so a mismatch with discovery or with `iss` fails loudly at the exact match instead of being trimmed away.
  - **Decided, not missed** (dev-qa's I2): `iat` has no maximum age, because `exp` bounds a token's lifetime. `exp` at u64::MAX is a far-future token.
  - **Tests added:** dev-qa's Q8 to Q11 (a malformed `kid` never fetches; a down dex gets one fetch per floor; an audience in another case or with a trailing space is refused; discovery's issuer with a trailing slash is another issuer); review-r1's slow-dex case (20 waiting requests, one fetch); `crit`; and the clock conversion.
  - **A web → server guardrail fixture is restored** (dev-qa's Q2). With the server's library, cargo resolves the dependency, and the checker reports the declaration, the resolved path and every crate the server reaches. The fixture lists them exactly, so it changes when the server's dependencies do.
- **Carried obligations:**
  - **P5** (deployment):
    - add a TLS feature to reqwest for an `https` issuer (until then, an https issuer means 503, never an accept);
    - confirm aws-lc-sys builds in the cargo-chef image, and on musl if the image uses it.
  - **P3.6:** bring `tests/dex.rs` into CI with the e2e job's dex.
  - **P3.4:** put the extractor on every route, and run `refresh_forever` beside the server.
- Where it's enforced: `crates/voidforge-server/src/{lib.rs, auth.rs}`, `crates/voidforge-server/tests/{auth.rs, dex.rs}`, `crates/voidforge-server/AGENTS.md`, `justfile` (`dex-test`), `deny.toml`, the [version table](versions.md).

### D-047: Accounts: principal to player, and Join game exactly once

- Date and package: 2026-10-19, P3.3. Proposed by dev-build (`slices/33-p3-3-accounts/proposal-accounts.md`). Lead ruling (`ruling-lead.md`): approved, with Q4, Q5 and one amendment (look up before inserting). Advisor rulings on Q1 to Q3: agreed, with the privacy rule widened. The answer to a refused join is game intent the advisor ruled on (Q1).
- Decision:
  - **Where the code lives:**
    - `voidforge_store::accounts`: the `AccountStore` trait (`lookup_or_create`), `PgAccounts`, and `AccountError`. All SQL stays in the crate that owns sqlx and the migrations. The trait exists so tests can wrap the store without a hook in production code (D-042's rule).
    - `voidforge_server::accounts`: `Players<A>` over an account store and a `HostHandle`, `Player { id }`, `PlayerError`, and a `Player` axum extractor built on `Principal`'s, over `Arc<Players<PgAccounts>>`. P3.4's routes take `Player` and never see a subject.
    - `voidforge_server::codes::game_code`: the core's reason code as the API's `ErrorCode`, exhaustive with no wildcard. It moved from `tests/api_codes.rs`, which now checks this mapping.
  - **The host names the core's types for the server** (Q5). `voidforge-host` re-exports `Command`, `Action`, `Rejection`, `DomainEvent`, `PlayerId` and `World`, the vocabulary `Receipt` and `ReadModel` already expose. The server still never depends on core, and the live World still changes only through the host; the read model's World is behind an `Arc`. Both AGENTS.md files say so.
  - **`World::free_homeworld` is public** (Q4): a read, and the one Join game's check calls, so the server's full-universe answer and the core can't drift.
  - **The account row.** Look up first: one indexed read, the common case on every request (the lead's amendment). Only on a miss, `INSERT … ON CONFLICT (issuer, subject) DO NOTHING RETURNING player_id`, then the read again if the insert yielded.
    - At READ COMMITTED a concurrent insert of the same pair waits for the first transaction, so N first calls end with one row and one ID.
    - The new ID is a UUIDv7 the server draws (`uuid`, below); it's used only if the insert wins.
    - The issuer is part of the key: one subject under two issuers is two players.
    - A read that finds nothing after the insert yielded is `AccountError::Vanished`, never a fresh ID. A primary-key collision is outside the `ON CONFLICT` target, so it's a database error, not a silent merge.
    - **An issuer or subject holding a NUL byte** can't be stored in Postgres `text`. It's `AccountError::Unstorable`, a 500 `internal` with no Retry-After, because a retry can never succeed. Without this check it would be a 503 forever. (Found while planning the tests; dex subjects never hold one.) **It stays a 500 at P3.3, not a 401 at P3.2** (lead, on dev-qa's argument). The token is valid, so a 401 would be false and invite a login loop. P3.2 shouldn't encode this table's storage limit. And Postgres stores every other control character, so nothing else is refused.
    - No in-process principal → player cache (the ruling left it optional): the indexed read is cheap, and a cache would be one more thing to keep honest.
  - **Join game, on every call, after the row:**
    1. If the read model shows the player, done. Both checks read **one** snapshot: a second read could see this player's own join land between them and answer a joined player `no_free_homeworld` (dev-qa's I1; no schedule reaches it today, because there's no await between the checks). The read model is published only after what it shows is durable, so a join that could still be lost is never reported.
    2. If the read model shows no free homeworld candidate, answer `no_free_homeworld` and append nothing.
    3. Otherwise submit Join game **without an idempotency key**, and read the receipt:
       - accepted is joined;
       - `already_joined` is also joined: another call joined this player first, at a durable record;
       - any other rejection is `PlayerError::Refused`, never joined;
       - `Busy` and `Stopped` are errors, never joined; a `KeyConflict` (impossible without a key) is 500.
  - **Why no key** (the departure from the slice's suggested shape, accepted by the lead and the advisor): D-037 returns a key's first receipt at any age, and a receipt can be a rejection. Under a fixed join key, the first `no_free_homeworld` would answer every later attempt for good, so a refused account could never join, against the slice's "a later call retries". A fixed key would also take one key value away from the player's own commands (`KeyConflict` after joining). The core already refuses a second join with `already_joined` (D-028), and the host applies commands one at a time, so the core is the deduplicator: two homeworlds for one player ID can't happen, whatever the server does.
  - **The row before the join, on purpose.** A crash between them leaves an account without a homeworld, which that user's next call repairs, so there is no startup sweep: the gap is visible only to its owner, and only through a call that repairs it. The reverse order could strand a homeworld under an ID no account names, and a retry would take a second homeworld for the same person.
  - **A refused join** (Q1, advisor): **422 `no_free_homeworld`**, through D-045's existing mapping, not 503. No production path frees a homeworld in the MVP (an owner is only ever set), so 503's promise that a retry may succeed would be false. The row stays, and each later call checks again, appending nothing. **Intent consequence:** in the MVP a refused account gets that 422 on every route that takes a `Player`, reads included. There is no spectator mode. **Owner-confirmed:** Tom confirmed this, through the advisor, on 2026-10-19.
  - **Answers:** `Refused(r)` is 422 with `r`'s code. `Busy` is 503 `busy` with `Retry-After: 1`. `Stopped`, and an accounts database that can't be reached, are 503 `unavailable` with `Retry-After: 5`. `Unstorable` and `Internal` are 500 `internal`. A database error's body says only that accounts can't be read; it never carries the database's text.
  - **The seed carry** (D-043; Q2, advisor): an accounts-only database is not empty. `voidforge_store::bootstrap` takes `accounts` into the same `EXCLUSIVE` lock as `snapshots` and `log_records`, and refuses with `BootstrapError::HasAccounts { count }`: exit 3, with the count and "clear them by hand before seeding". Accounts without a universe mean the universe was lost, and seeding over them would quietly give returning users a fresh start under their old identities. Nothing is deleted automatically.
  - **Privacy** (Q3, advisor, the stricter rule): the subject is personal data. It's never logged at any level, and never appears in an error body, a panic message or a `Debug` output that can reach a log. `Principal`'s `Debug` prints the issuer and the subject's length only. Spans and errors carry the player UUID and the issuer; `PlayerError` and `AccountError` never hold the subject. **A database refusal is redacted** (dev-qa's L1): Postgres's message and DETAIL can quote the row ("Failing row contains (…)"), so `AccountError::Database` holds a `DbFailure` with only the SQLSTATE and the constraint name. Other failures, such as a connection or a pool timeout, keep sqlx's text, which has no row data.
  - **Dependencies:**
    - **uuid `=1.28.0`** (std, v7; default features off) in the server: it draws UUIDv7s through getrandom 0.4.3, already in the graph under sqlx (its `deny.toml` skip reason is updated). The core, rules and worldgen bans on UUID generation (D-010) don't cover the server.
    - **voidforge-host** becomes a normal dependency of the server (it was a dev-dependency), as the layering table allows.
    - **log `=0.4.34`** and **tracing-subscriber** (D-017's pin) are server dev-dependencies: the privacy test captures every `tracing` and `log` event in the process, because sqlx emits through both.
  - **Tests** (`crates/voidforge-server/tests/accounts.rs`, in the database stage: `database.sh`'s filter gains the `accounts` binary). Each test drives the real host over Postgres on a seeded universe, and checks effects: `accounts` rows, accepted and refused `join_game` records, the total record count, and the World recovered from Postgres.
    - A new principal; the same principal again (nothing written, and no INSERT issued: sqlx's statement log is counted, which pins the lookup-first amendment); 32 concurrent first calls for each of 12 principals, racing freely; the same race made certain, with the first join held in its append until every call has submitted (exactly 31 `already_joined`, all counted as joined); a full universe (refused, the row kept, nothing appended); 16 principals racing for 4 homeworlds, held the same way (exactly 12 `no_free_homeworld` from the core's receipts); two issuers with one subject; subjects differing only in case (two players: the match is exact); a database refusal (an empty issuer breaks the check), whose error and body carry nothing of the row; an account without a homeworld; a stopped host and the next host; a busy host; a restart; an unstorable subject; each error's code, status and Retry-After; `Principal`'s `Debug`; and the subject never logged, at TRACE, sqlx included.
    - **Kill points**, as P2.8's harness: the binary re-runs itself as a child that recovers, serves one call and aborts at a named point in the test's own wrappers. Then a fresh child serves the call, and the test asserts one row, one accepted join, and the reported player in the World. The points are `account.before`, `account.after`, `append.before` and `append.after`. A death inside the insert statement reduces to before or after, since a statement commits whole or not at all, and P2.8 already proved Postgres rolls back an open transaction on a dead client.
    - `tests/postgres.rs`: `seed` on an accounts-only database refuses with exit 3 and writes nothing. An account inserted but not yet committed when `seed` starts makes it wait on the lock and then refuse, so the refusal holds under a race.
- Carried:
  - **P3.4:** take `Player` on every route that acts as a player, and build `Players<PgAccounts>` in `serve` with the host it starts (the epoch is D-044's).
  - **P3.8:** a refused account's calls append nothing, but each costs an indexed read; the rate limit bounds them.
- Where it's enforced: `crates/voidforge-store/src/{accounts.rs, lib.rs}`, `crates/voidforge-server/src/{accounts.rs, codes.rs, auth.rs, main.rs}`, `crates/voidforge-host/src/lib.rs`, `crates/voidforge-core/src/command.rs`, the core's, host's, server's and store's AGENTS.md, `crates/voidforge-server/tests/{accounts.rs, postgres.rs, api_codes.rs}`, `.github/scripts/database.sh`, `deny.toml`, and the [version table](versions.md).

### D-048: `serve` is its own package, P3.4s

- Date and package: 2026-10-19, planning (P3.4). Proposed by dev-build as T8 of `slices/34-p3-4-routes/proposal-routes.md`; approved by the lead (`ruling-lead.md`); the Technical Plan row added by the advisor.
- Decision: running the server for real (`voidforge-server serve`) becomes package P3.4s, after P3.4 and before P3.5. It covers host recovery from Postgres, a durable universe epoch that `seed` records through a new migration (D-044's carry, taking over the "Phase 3 owns a stable epoch source" amendments above), the bind address, and a clean shutdown with a final snapshot. It flattens D-046's `AuthArgs` for dex configuration and runs `TokenVerifier::refresh_forever` beside the server (D-046's carry). It builds `Players<PgAccounts>` with the host it starts (D-047's carry). P3.5's leaderboard job wires into it and P3.6 runs against it.
- Reasons: it is a separate design with its own failure modes, and folding it into P3.4 would double that package. Until it lands, P3.4's QA probes an in-process test server over a seeded `voidforge_dev` with real dex. The Phase 3 gate's "QA has tried the API by hand" uses `serve`.
- Carries moved: these earlier P3.4 carries now belong to P3.4s: D-044's stable epoch, D-046's `AuthArgs` and `refresh_forever`, and the `Players<PgAccounts>` half of D-047's. P3.4 keeps `Principal` and `Player` on every route.
- For the P3.4s SPEC: a universe seeded before the epoch migration (today's `voidforge_dev`) has no epoch record. `serve` must not invent one, since a changed epoch moves game time (D-044); the SPEC decides the refusal and its next step.
- Where it's recorded: the [Technical Plan's Phase 3 table](Voidforge%20Technical%20Plan.md#phase-3-api-and-auth).

### D-049: Routes

- Date and package: 2026-10-19, P3.4. Proposed by dev-build (`slices/34-p3-4-routes/proposal-routes.md`). The lead ruled on T1 to T8, approving all with additions (`ruling-lead.md`, and an addendum on the core viewer filter). The advisor ruled on S1 to S3, agreeing with all three: planet privacy is game intent.
- Decision:
  - **`voidforge_server::api`.** One route table, `routes()`, builds the router, and the parity tests read it. `AppState` holds the verifier, `Players`, the host, its clock and its epoch.
    - **Mapping:** `api::wire` maps the core and the wire both ways, exhaustively, with no wildcard arm.
    - **Times:** `api::time` formats them.
    - **Errors:** `api::error` holds `ApiError`, the one host-error mapping, the refusing extractors and the panic layer.
    - **Players:** every handler that acts as a player takes `Player`, never an ID from the body, query or path.
  - **The host re-exports the read vocabulary** (T1): `voidforge_core::view` and the read-only state names the mapping needs. These are names only; nothing mutates. Both AGENTS.md files say so.
  - **What a viewer may see** (S1 to S3, the advisor; Game Design amended):
    - every planet's properties are public, including stocks, buildings, construction, Shipyard queues and slots;
    - **roster ships are owner-only**: each caller sees only their own, on any planet;
    - **`valid_until` counts only the caller's own fleets and the entity's own events**, so it means "valid until, unless another player's fleet arrives". That applies to the planet, planets-near and player views. A null whenever something is hidden would still leak, so it's not that.
    - **In the core** (the lead's addendum): `World::view_for(query, viewer, t, rules)` projects through the unchanged `World::view`, so staleness is still judged by the true validity, which counts every fleet. It then filters the roster and replaces `valid_until` with the viewer's. `planet_public_valid_until` is public. `World::view` and its tests are unchanged, and no golden file or hash moves.
  - **`POST /commands`:**
    - the key comes through the store's `IdempotencyKey::new`: missing is 400 `missing_idempotency_key`; empty, too long, a space, non-ASCII or two headers is 400 `invalid_idempotency_key`;
    - the body comes through `ApiJson`, with a 64 KiB limit. `join_game`, an unknown type or field, broken JSON, a wrong or missing content type, an integral float, an ID out of range or negative, and an oversized body are all 400 `malformed_body`. A repeated ship is 400 `duplicate_ship_ids`;
    - accepted is 200 with **the receipt's own events only**. A repeated key is the first answer exactly, refusals included. Refused is 422 with the core's code, and `KeyConflict` is 422 `idempotency_key_conflict`;
    - **one host-error mapping**, `api::error::host_error`, shared with `PlayerError`: busy is 503 `busy` with Retry-After 1, stopped 503 `unavailable` with Retry-After 5.
  - **Views projected to now:**
    - `t = max(clock − epoch, world time)`, through `view_for` on the published `Arc<World>`. No mutation, no append, no inbox.
    - A `Stale` view waits up to 1 s for the host's next publish (`changed()`). Still stale is 503 `unavailable` with Retry-After 1; a projection never crosses an event.
    - `GET /planets/{id}`: an unknown planet is **404** `unknown_planet` (an explicit status; the code's own is 422), and an ID that isn't a u64 is 400 `invalid_query`.
    - `GET /planets?near&radius` (T4):
      - each coordinate must fit the pattern and i64, or it's 400 `invalid_query`, and so is any missing or extra part;
      - **a negative radius is 400 `invalid_query`**, and 0 is valid;
      - **more than `MAX_NEAR_RESULTS` = 1,000 matches is 400 `invalid_query`, never a truncated list.** The cap bounds the response, not the scan: the scan is bounded by `MAX_PLANETS`, and P3.8 bounds frequency.
      - **The extremes are safe:** the core's `distance` widens to i128 and returns `None` only past i64, which is beyond every radius, so excluding the planet is the right answer. A test pins radius i64::MAX, centres at i64::MIN and MAX, and a planet at exactly i64::MAX. No core change was needed.
    - `GET /players/me` and `GET /players/me/fleets`: the caller's own.
    - **`GET /leaderboard` until P3.5 (T5):** 503 `unavailable`, "the leaderboard isn't built yet", with **no** Retry-After so no client polls. The path and schema stay. **Amended by D-051:** the 503 now comes only before the first build, with `Retry-After: 1`.
  - **`GET /openapi.json` is public** (T2): no `Principal`, no `Player`, so reading it joins nobody. It's in the document as a path without security, and `openapi.json` is re-blessed.
  - **The document now declares 422 `no_free_homeworld` on every GET route** that takes a player (D-047, owner-confirmed). Without it, T7's status parity would fail.
  - **Every error is a D-045 body:**
    - an unknown route, **or a known route with another method, is 404 `not_found`** (T3: the code list stays closed, so there's no 405);
    - a panicking handler is 500 `internal` with a generic body. A small layer of our own runs each request as a task and answers a panic's `JoinError`, so no tower-http is needed.
  - **Game times** (T6) use a days-from-civil formatter. A year outside 0000 to 9999, or an epoch plus time that overflows, is a checked error, a 500 `internal`. The tests cover:
    - fixed vectors: 1970-01-01, 2000-02-29, 2100-03-01 and the day before it, and a time of day;
    - both year edges and the overflows;
    - every day from 1600 to 2400, each accepted by the contract's own `Timestamp` check and strictly increasing.
  - **An inconsistent view is a 500, never a made-up value:** a building under construction without its progress is `MapError::MissingProgress`.
  - **Parity** (T7):
    - the route table equals the document's methods and paths;
    - a route is public exactly when the document has no security;
    - every route is served, not `not_found`;
    - every test records each (route, status) it sees, and its harness checks the document declares them, so a handler can't answer a status the contract doesn't name.
  - **`serve` is its own package, P3.4s** (T8; the advisor's D-048). P3.4's QA runs an in-process server over a seeded `voidforge_dev` with real dex.
  - **Dependencies:**
    - **axum gains its `query` feature:** two new crates, `serde_urlencoded` and `ryu`, which it uses.
    - **insta `=1.46.1`** (dev; default features off, plus `json`) for the contract snapshots. It adds seven crates: insta, similar, tempfile, fastrand, rustix, errno and linux-raw-sys.
- **Repair round 1** (folded in after QA PASS on 4169425 and R1's verdict):
  - **S1, a core change** (the advisor ruled to change it; the lead's Addendum 3 took the core option over a server remap):
    - another player's fleet or ship fails the existence check itself, as `unknown_fleet` or `unknown_ship` (`command/fleet.rs`). D-028 is amended, and D-045's privacy text gains the line.
    - **Why not a server remap.** It couldn't be certain, and it wasn't byte-identical. Walking the core's order:
      - `send_fleet` checked the target planet between the fleet's existence and its ownership, so a foreign fleet sent to an unknown planet answered `unknown_planet` while an unknown fleet answered `unknown_fleet`;
      - `assemble_fleet` checked `no_ships` and `invalid_cargo` before ship ownership;
      - assemble's `not_owner` had two sources, a foreign ship or a foreign planet to load from.
    - Recall, disband and unload already checked existence, then ownership, then state.
    - **Planet `not_owner` is unchanged.** It's now only ever the planet: assemble loading from a planet that isn't the player's.
    - **Tests:**
      - eight `fleet_commands` expectations change from `NotOwner` to `UnknownFleet` or `UnknownShip`;
      - paired tests compare a foreign fleet or ship with an unknown one, with the same other inputs, for every fleet and ship action, stationed and in transit, including both cases from the walk;
      - the assemble split is pinned;
      - `another_players_fleet_or_ship_answers_byte_for_byte_as_an_unknown_one` compares the status, the headers and the body bytes over HTTP, fresh and for a repeated key.
    - **Goldens:** `just bless` moves nothing, and `voidforge-replay golden` verifies all six (the goldens' only `not_owner` records are planet refusals).
  - **L1** (dev-qa): the `radius` schema's minimum is 0, set in `openapi()` after the integer bounds, since utoipa's f64 bounds can't hold i64::MAX and bound_integers would read a minimum of 0 as a u64.
    - It's re-blessed, and a contract test pins 0 to i64::MAX.
    - This is a third deliberate place where the schema and serde disagree (−1 fits an i64), alongside D-045's two. It's a query parameter, and the server answers −1 as 400 `invalid_query`.
  - **dev-qa's test gaps:** a clock behind the World's time answers every view at the World's time, never 500 (Q1); a non-ASCII key byte is 400 (Q2); the body limit's exact boundary (Q3).
- Tests:
  - **`crates/voidforge-server/tests/routes.rs`**, in the database stage: `database.sh` gains the `routes` binary. It runs the real router over HTTP, a real host recovered from Postgres, `Players` and a dex stand-in (shared with `tests/auth.rs` through `tests/common/dex.rs`), on a universe from testkit's `WorldBuilder` with fixed UUIDv7 players.
  - **The privacy test** holds the host so that another player's event fires inside the caller's own command record, asserts that it's there, and asserts that it never reaches the answer, fresh or repeated.
  - **The contract snapshots** are in `tests/snapshots/`.
  - **The core:** `crates/voidforge-testkit/tests/views.rs` checks the roster and validity for an owner and a non-owner with a hidden inbound fleet, through the planet, near, player and fleets queries, and checks that staleness stays the true validity's.
- Carried:
  - **P3.4s:** `serve`, the durable epoch (D-044), recovery, bind, and clean shutdown. With it go D-046's serve-only carries (`serve` flattens `AuthArgs`, and `refresh_forever` runs beside the server) and D-047's (build `Players<PgAccounts>` in `serve`). P3.4 provides `api::router(AppState)` for it.
  - **P3.5:** the leaderboard replaces the 503.
  - **P3.8:** the rate limit, the 429 every path already declares.
- Where it's enforced: `crates/voidforge-server/src/{api/, accounts.rs, lib.rs}`, `crates/voidforge-core/src/view.rs` (`view_for`), `crates/voidforge-host/src/lib.rs`, `crates/voidforge-api-types/src/lib.rs` and `openapi.json`, `crates/voidforge-server/tests/{routes.rs, common/, snapshots/}`, `crates/voidforge-testkit/tests/views.rs`, `.github/scripts/database.sh`, the Game Design (Universe and planets), the Architectural Design (Read side), D-045, `deny.toml`, the [version table](versions.md), and the core's, host's and server's AGENTS.md.

### D-050: `serve`, the durable epoch and the travel horizon

- Date and package: 2026-10-19, P3.4s. Proposed by dev-build (`slices/35-p3-4s-serve/proposal-serve.md`). Lead ruling on T1 to T9, approved with four amendments (`ruling-lead.md`). Advisor rulings on S1 to S3 (`ruling-advisor.md`): the travel horizon and the missing-epoch refusal are game intent.
- Decision:
  - **The durable epoch** (D-044's carry, D-048):
    - Migration `0004_universe.sql` adds a one-row table, `universe (id boolean PRIMARY KEY DEFAULT true CHECK (id), epoch_unix_ms bigint NOT NULL, created_at)`. The constraint `universe_epoch_in_window` repeats the code's window, so the database holds nothing the code would refuse.
    - `voidforge_store::bootstrap(url, snapshot, epoch)` writes it in the seq-0 snapshot's transaction, with `universe` in the same `EXCLUSIVE` lock, so a universe never exists without its epoch. A `universe` row counts as "used".
    - `seed --epoch-unix-ms` is optional. The default is the wall clock, read once. The printed result and the JSON give the epoch in ms and in RFC 3339 form.
    - `serve` reads it through `voidforge_store::open_universe` and **never takes it from a flag or the clock**.
  - **The epoch window: [1970-01-01, 2100-01-01)**, that is [0, 4,102,444,800,000) ms (`EPOCH_MIN_UNIX_MS`, `EPOCH_END_UNIX_MS`, `epoch_in_window`).
    - `seed` refuses an epoch outside it before anything is read, generated or connected (exit 3).
    - `serve` refuses a recorded one (exit 3), separately from the CHECK.
    - **It closes the P3.4 QA's I2 completely.** A stamp is max(wall − epoch, last stamp), and every earlier stamp came from an earlier wall clock, so epoch + stamp ≤ max(wall now, epoch). A `game_time` can be formatted whenever the epoch and the system clock are within years 0000 to 9999. I2 needed an epoch within a second of 9999-12-31.
    - The QA's `wall_time` carry: that column takes the wall clock, never the epoch. A test pins the record path at the window's last millisecond: game time is held at 0, `wall_time` is the real clock, and `as_of` is the epoch, formatted.
  - **The travel horizon** (advisor S2, option A; **a core change**, amending D-029):
    - `voidforge_core::fleet::MAX_TRAVEL_MS` is 1,000 Julian years (31,557,600,000,000 ms). Send refuses a longer trip as `unreachable_target`.
    - Recall refuses a time traveled longer than the horizon with the same code. A sent trip can't reach that, but the invariant is checked locally.
    - It closes the P3.4 QA's I1 for every time a player chooses. Rules v0's longest trip is about 173 h (a 1,000-unit cube at 10 units/h), so it never constrains play.
    - Core tests pin both edges: exactly the horizon is accepted and one ms past it is refused (Send at a 1 ms-per-unit speed, and under Rules v0 at 240 ms per unit), and the same for Recall. `just bless` moves no golden.
    - The Game Design's Travel and recall section says so, in one sentence.
    - **Residue, accepted** (advisor): a solver `valid_until` past 9999 under extreme custom rules is still a 500 (D-049 T6). It can't be reached under Rules v0. If a rules change makes it reachable, it comes back as a contract question.
  - **`voidforge-server serve`** (`voidforge_server::serve`):
    - **Configuration:**
      - `--database-url` / `VOIDFORGE_DATABASE_URL`: required, never printed, and **deliberately not `DATABASE_URL`**, since a stray one set for sqlx's tooling could point `serve` at the owner's database;
      - `--rules` / `VOIDFORGE_RULES`, whose version must be the universe's;
      - `--bind` / `VOIDFORGE_BIND`, default `127.0.0.1:8080` (P3.6 sets `0.0.0.0:8080`);
      - `--drain-secs` / `VOIDFORGE_DRAIN_SECS`, from 0 to 60, default 5;
      - D-046's `AuthArgs`, flattened.
    - **`seed` stays on `DATABASE_URL`** (the lead, T7). Unlike `serve`, `seed` *can* write to an empty database. So the owner's empty `voidforge` is protected only by `just seed` naming `voidforge_dev`, not by `seed`'s refusal of a non-empty database.
    - **Startup:**
      1. The settings and the rules file are checked.
      2. **A database without a universe is refused before anything is migrated** (the lead's T2 amendment): no `snapshots` table, or an empty one, is exit 3 ("seed it first") and the database is untouched. `serve` never creates a schema; `seed` does.
      3. Otherwise the migrations run (they only add, D-034) and the epoch is read.
      4. **No recorded epoch is exit 3 with the next step, "re-seed it (for voidforge_dev: just seed-reset)"** (advisor S1). There's no command to record an epoch by hand, because a hand-entered epoch is an invented one, and a changed epoch moves game time (D-044). Only dev and test databases predate the migration.
      5. `voidforge_host::recover` (D-038), then `spawn` on the system clock.
      6. `refresh_forever` runs beside the server. **With dex down at startup the server starts, and tokens get 503 `unavailable` with Retry-After until the keys load, never an accept** (D-046, confirmed). An info line says so.
      7. The bind, then one line on stdout: `ready: listening on <addr>, epoch <RFC 3339>, seq <head>`.
    - **Shutdown** on the first SIGINT or SIGTERM, or when the host stops on its own:
      1. Stop accepting, and drain in-flight requests for `--drain-secs`.
      2. Shut the runtime down. The router, its `AppState` and any connection the drain cut off go with it, and so does every `HostHandle`. `serve` keeps none past the drain (it watches the host through one handle until then). So the host stops and writes its clean-shutdown snapshot.
      3. Join the host, bounded at 10 s, so a surviving handle is exit 6, never a hang.
      4. **Read Postgres: exit 0 only if the newest snapshot is at the log head.** `HostTask::join` doesn't report a failed shutdown snapshot (it's only a counter), and the database is the truth anyway.
    - **Unclean stops are exit 6:**
      - a second signal, which exits at once;
      - a drain timeout (the host still stops and snapshots once the cut-off requests are gone);
      - the host stopping on its own, the host not stopping within its bound, or no snapshot at the head.
      - Recovery then replays the log from the last snapshot: every acknowledged command's record is durable (D-036).
    - **A departure from the P3.4 QA's I4, for the process only** (the lead, T5): a serving process whose host has died exits 6 rather than answer 503 to every write forever, so compose's restart runs recovery. The in-process router behaves as before.
    - **Exit codes**, in D-041's style and listed in `serve --help`:
      - 0: clean;
      - 2: usage, including `AuthArgs` out of range;
      - 3: refused (no universe, no recorded epoch, an epoch out of the window, or another rules version);
      - 4: input or database error (connecting, migrating, recovery, the bind);
      - 5: telemetry;
      - 6: unclean.

      `seed`'s 3 gains "the epoch is out of range".
    - **Readiness** (advisor S3): there's no new route. Compose checks the public `GET /openapi.json`, which answers only once the router serves, after recovery. Clients treat 503 `unavailable` as "not ready yet", as D-046 already gives while dex's keys load.
    - Logs at info: the build, the rules version, the epoch, the recovered seqs, the issuer, the bind address, the signal, the drain and the final snapshot. Never the database URL, a token or a subject.
  - **`just serve`** (`.github/scripts/serve.sh`) serves `voidforge_dev` only, and `exec`s the binary so Ctrl-C reaches it.
  - `PgAccounts::lazy_url` builds the accounts pool on the runtime that uses it.
  - **Dependencies:** tokio gains `net`, `rt-multi-thread` and `signal` on the server's own dependency only. `signal` adds one crate, **signal-hook-registry 1.4.8** (MIT OR Apache-2.0), which joins `deny.toml`'s crate allowlist. There's no other new crate.
- **Repair round 1** (QA PASS and review-r2 REPAIR REQUIRED on 6707d85, folded with the lead's addition; `repair-assignment.md`):
  - **M1 (review-r2, MEDIUM) and D1 (dev-qa): a second signal was lost after the join and between the phases.**
    - In 6707d85, the shutdown ran on two runtimes, each with its own signal stream. A signal between the first runtime's end and the second's start was swallowed: dev-qa measured exit 0 for a second signal 5 to 40 ms after the drain.
    - The final snapshot check was awaited after the join's select had chosen its branch, so neither the signal nor a bound applied. review-r2 held that SELECT behind a lock: the process lived 11 s past a second SIGTERM, then exited 0.
    - **The fix:** one **control** runtime (current-thread) owns the only signal stream, the universe's lock and every shutdown step. The **server** runtime (multi-thread) runs the API and its connections, and it shuts down in the background (`shutdown_background`, allowed from async code). Its connection tasks, and the host handles they hold, go with it. Every step from the first signal to the exit is selected against that one stream: the drain, the join (bounded at 10 s) and **the final check, now bounded at 5 s** (`CHECK_BOUND`). A second signal at any of them is exit 6 at once.
    - My brief had called the transition window "microseconds" without measuring it. That was wrong.
  - **One serve per universe** (dev-qa's I4; the lead's addition): `voidforge_store::lock_universe` takes `pg_try_advisory_lock(SERVE_LOCK_KEY)` on a connection of its own, held for the process's life. Postgres frees it when the connection closes, on a clean stop or a crash. An advisory lock is scoped to its database, and a database holds one universe. A second `serve` refuses with exit 3 ("another serve already holds this universe") after the universe checks and before recovering or binding.
  - **D2 (dev-qa; review-r2's L1):** `serve.sh` hard-coded `target/debug`, so with `CARGO_TARGET_DIR` set it failed with exit 127 (or ran a stale binary). It now runs `exec cargo run`. Cargo finds the binary wherever it builds, and on Unix it execs it in its own place: checked by effect, since with `CARGO_TARGET_DIR` set, the launcher's pid became the binary and a SIGINT to it stopped cleanly with exit 0.
  - **L1 (dev-qa; review-r2's L2):** `serve --help` now names every exit-6 cause (a join past 10 s, no snapshot at the head, a check past 5 s) and the new exit-3 cause.
  - **I5 (dev-qa), for P3.6:** compose execs the binary, never `just`. `just` exits 130 on Ctrl-C, forwards a group SIGTERM so serve sees it twice (an unclean stop), and doesn't forward a SIGINT sent to its own pid.
  - `voidforge_host` re-exports `Rules`, a name only, so `serve` can pass a parsed rules file between its runtimes without depending on the core.
  - **Tests added to `tests/serve.rs`:**
    - review-r2's probe as a regression: the final SELECT held behind a snapshots lock, then a second SIGTERM is exit 6 within 2 s;
    - the same hold with no second signal is exit 6 after 4 to 8 s;
    - dev-qa's D1 as a sweep: SIGINT, then SIGTERM after 0, 2, 5, 10, 20, 40, 80, 200 and 1,000 ms, with the shutdown snapshot slowed to 2 s and a new record each round so a snapshot is due. Every delay is exit 6 **within 1 s of the second signal**, inside the 2 s the snapshot holds the stop. (Without the timing, a join that ignored the signal survived the table: the stream kept the signal pending, and the next step's select exited 6 after the snapshot.);
    - dev-qa's Q2: the join bound, exit 6 between 9 and 13 s;
    - dev-qa's Q3: seed's default epoch lies between wall-clock reads taken before and after;
    - I4: two serves, where the second is exit 3 without recovering or binding, the first serves on and stops with exit 0, and a later serve starts.
    - dev-qa's Q1 (a second signal during the join) is inside the sweep, whose later delays land in the slowed snapshot.
- Tests:
  - **`crates/voidforge-server/tests/serve.rs`**, in the database stage (`database.sh` gains `binary(=serve)`). It runs the real binary over a seeded `_sqlx_test_*` database and a dex stand-in, both on threads of their own, with real signals (`kill -s`). Every wait is bounded, and a failing test kills its child.
    - **SIGINT and SIGTERM restarts:** exit 0, the snapshot at the head, the epoch row unchanged. The next start prints the same epoch and seq, with nothing left to replay; a repeated key gets its first answer; and game time continues.
    - **The refusals:**
      - an empty database: exit 3, and still 0 tables;
      - no recorded epoch: exit 3 with the next step, and nothing written;
      - epochs out of the window: at seed, exit 3 and 0 tables; at serve, past a dropped CHECK, exit 3.
      - The CHECK itself is probed by effect: SQLSTATE 23514 with the constraint's name, and 23505 for a second row.
    - **The window's last millisecond** serves, as above.
    - **dex down at startup:** 503 with Retry-After and `/openapi.json` 200, then 200 once the stand-in comes up on that port.
    - **A request in flight at shutdown**, made certain with no hook in the code: the test holds `LOCK TABLE log_records`, and `pg_stat_activity` shows the host's INSERT waiting on it. Released within the drain, the request is answered, its record is durable and the stop is clean. Held past `--drain-secs 1`, the connection is cut, the exit is 6, the host still snapshots once released, and a retry with the same key after a restart appends nothing.
    - **A second signal** during a 60 s drain: exit 6 within 5 s. The unacknowledged command is absent, and a restart applies the retry once.
    - **SIGKILL:** no shutdown snapshot; the restart replays to the head, and the acknowledged command is in the log.
    - **The host dying** (the lead's T8 correction): a `BEFORE INSERT` trigger that raises, because the local and CI role is a superuser and bypasses a revoked privilege. The command is 503, the process exits 6, and stderr carries the trigger's message, so the failure is proven to have fired. Without the trigger, a restart recovers.
  - **`tests/cli.rs`:**
    - `serve --help` lists the exit codes;
    - no `--database-url` is exit 2 even with `DATABASE_URL` set;
    - `--drain-secs 61` and a leeway of 301 are exit 2 before any connection;
    - a bad or unreachable URL is exit 4 and never printed;
    - `seed`'s out-of-range epoch is exit 3 even with a URL that isn't Postgres, so it's checked before connecting.
  - **The core:** `crates/voidforge-testkit/tests/fleet_commands.rs` pins the horizon's value and both edges for Send and Recall.
- Where it's enforced: `crates/voidforge-core/src/{fleet.rs, command.rs, command/fleet.rs}`, `crates/voidforge-store/{migrations/0004_universe.sql, src/lib.rs, src/accounts.rs}`, `crates/voidforge-server/src/{serve.rs, main.rs, lib.rs}`, `crates/voidforge-server/tests/{serve.rs, cli.rs, accounts.rs, routes.rs}`, `crates/voidforge-testkit/tests/fleet_commands.rs`, `.sqlx/`, `.github/scripts/{database.sh, serve.sh}`, `justfile`, `deny.toml`, `crates/voidforge-host/src/lib.rs` (the `Rules` re-export), the host's AGENTS.md, the Game Design (Travel and recall), the Architectural Design (Persistence, Operations), D-029, the [version table](versions.md), and the server's and store's AGENTS.md.

### D-051: The leaderboard

- Date and package: 2026-10-20, P3.5. Proposed by dev-build (`slices/36-p3-5-leaderboard/proposal-leaderboard.md`). Lead ruling on T1 to T6, approved with four conditions (`ruling-lead.md`). The advisor settled the intent beforehand (SPEC items 1 and 5).
- Decision:
  - **Who's on it** (advisor): every joined player, with no top N; the Game Design says the board is visible to all. A zero score can't occur, because a homeworld alone is 1,000 points. The ship value inside a total is public by design.
  - **`voidforge_server::leaderboard`:**
    - `build(read, t, epoch)` is a pure function;
    - `rank` gives D-045's order (score descending, then player ID ascending) and competition ranks (1, 2, 2, 4), with the rank a checked `u32`. The ID order is the `u128`'s, which is the wire's text order, because the hyphenated lowercase UUID has fixed-width hex;
    - `build_now` and `run`, the job;
    - `Built` (the game time `t` and the JSON bytes, serialized once per build), published on a `tokio::sync::watch`. `AppState` gains `leaderboard`.
  - **One computation with `/players/me`:** a build reads **one** published read model at **one** `t = max(clock − epoch, world time)`, the routes' projection. It takes each player's score through **the route's own call**, `view_for(&Query::Player { player }, player, t, rules)`. So the board and a player's own view at the same time are one function on one World.
  - **A stale read model** (behind a due event): the build waits up to 1 s for the host's next publish (`STALE_WAIT`, as the routes do), then fails as `Stale`. **A failed build keeps the previous board** and logs at error level. A half-built board is never published, because publishing is one `send` of a complete build.
  - **The job:** it waits, then builds and publishes. It waits `--leaderboard-secs` once a board exists, and 1 s until then. Each wait starts after a build ends, which is the effect of `MissedTickBehavior::Delay`: a slow build never bunches builds up.
  - **In `serve`:** the first build runs before `ready:`, so the board exists from the first request. If it fails, `serve` still starts, logs it, and **`GET /leaderboard` answers 503 `unavailable` with `Retry-After: 1`** until a build succeeds (D-049 T5 amended). The document already declared the 503, and `openapi.json` doesn't change.
    - `--leaderboard-secs` / `VOIDFORGE_LEADERBOARD_SECS`: default 60 (the Game Design's), from 1 to 3,600, exit 2 outside.
    - The job is a task on the **server runtime**, so D-050's shutdown drops it, and its host handle, with the connections. There's no new mechanism. The CPU part of a build holds only the `Arc<ReadModel>`.
  - **Logging** (repair round 1, the lead's option on dev-qa's I2): the first failure in a run logs at error level, later ones at debug with a count, and the next success at info with the count. A host stuck for minutes gives one ERROR, not about 30 a minute. No test pins the levels; they're log-only.
  - **The route** serves the latest build's bytes, with the build's own `as_of`: up to the interval old by design, while a player's own score is current through `/players/me`.
  - **Measured** (the lead's condition; release build, solved homeworlds, at `t` = 1 h):

    | Players | Build | Board |
    | --- | --- | --- |
    | 250 (the default cube's practical limit) | 0.25 ms | 18.8 KB |
    | 5,000 (the cap) | 113 ms | 375 KB |

    Under the ruling's 1 s threshold, so the build stays inline on the server runtime. **Scaling** (review-r1's R1-2): a build is O(players × (planets + fleets)), because each player's view walks the World. That gives 0.24 ms at 250 players, 113 to 132 ms at 5,000, and about 1 s near 14,000. **Rerun `build_time` before any rise in the player cap**, and move the build to `spawn_blocking` if it nears 1 s.
  - **Dependencies:** `jsonschema` (D-045's pin, already in the graph) becomes a server dev-dependency, for the schema check below. There's no new crate.
- Tests:
  - **`src/leaderboard.rs`:** the ranking table (ties in the middle and at the end; all tied; strictly ordered; empty), and a property test over 500 generated boards (each rank is 1 plus the count strictly ahead; the order; every player once). Also the ignored measurement, `build_time`.
  - **`tests/routes.rs`** (the database stage, the real router and host on a `ManualClock`):
    - **the board equals every player's own `/players/me` at the same time**: five players (three newcomers tied on identical homeworlds; Alice's drill under construction for partial credit; Bob's ships), at 1 minute and again at 5 hours, after the drill completes and Bob's fleet arrives. Each check covers the ranks, the order, the tie, the same `as_of`, and the `as_of` advancing;
    - **every board read is validated against the document's `Leaderboard` schema** (Draft 2020-12, formats on) and deserialized into `api::Leaderboard` (the lead's T1 condition);
    - **a failed build keeps the previous board:** the host's append held past Bob's arrival, the build `Stale` after its wait, the route serving the previous board unchanged, then a rebuild once released;
    - the job on a 50 ms interval rebuilding as the clock moves;
    - **the job's own error path keeps the board** (added for the mutation table's one survivor, "a failed build clears the board": the direct-build test never ran the job): on a 50 ms interval with the host held stale for 2.5 s, the route serves the previous board unchanged, and the job's next build advances it once released;
    - the build's `as_of` kept when read later (not the request's time), and the job retrying every second until its first board;
    - **the interval pinned** (repair round 1, dev-qa's Q1, in `tests/serve.rs`): with `--leaderboard-secs 1`, every gap between new `as_of`s over 6 s lies between 0.8 and 1.6 s;
    - before any build, 503 with `Retry-After: 1`.
  - **`tests/serve.rs`** (the real binary):
    - with `--leaderboard-secs 1`, the board is 200 at `ready:` and its `as_of` advances;
    - **the host joins with the job mid-wait** (the lead's T3 condition): with `--leaderboard-secs 3600` the job is asleep holding its handle, and SIGINT is exit 0 with the snapshot at the head in under 5 s, well inside the 10 s join bound.
    - **Honest scope:** this pins the job's sleep, its usual wait. The other wait, the up-to-1 s stale wait inside a build, is an await point of the same task, dropped the same way, but no test parks a real build in it.
  - **`tests/cli.rs`:** `--leaderboard-secs` 0 and 3,601 are exit 2.
- Where it's enforced: `crates/voidforge-server/src/{leaderboard.rs, api/mod.rs, serve.rs, lib.rs}`, `crates/voidforge-server/tests/{routes.rs, serve.rs, cli.rs}`, `crates/voidforge-server/Cargo.toml`, D-049, and the server's AGENTS.md.
