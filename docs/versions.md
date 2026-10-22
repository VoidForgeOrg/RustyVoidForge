# Version table

What is pinned, what is only installed, and what has not been introduced yet. The [Tech stack](Voidforge%20Technical%20Plan.md#tech-stack) names the tools and crates; this table records the versions. A version change is its own pull request with a [decision log](decision-log.md) entry.

## Pinned in the repository

| Item | Version | Where | Decision |
| --- | --- | --- | --- |
| Rust toolchain | 1.99.1, minimal profile plus clippy and rustfmt | `rust-toolchain.toml` | [D-001](decision-log.md#d-001-rust-toolchain-1991-edition-2024) |
| Minimum Rust version | 1.99.1 | `rust-version` in `Cargo.toml` `[workspace.package]` | [D-001](decision-log.md#d-001-rust-toolchain-1991-edition-2024) |
| Edition | 2024 | `Cargo.toml` `[workspace.package]` | [D-001](decision-log.md#d-001-rust-toolchain-1991-edition-2024) |
| Cargo resolver | 3 | `Cargo.toml` `[workspace]` | [D-001](decision-log.md#d-001-rust-toolchain-1991-edition-2024) |
| Workspace crates | 0.1.0, unpublished | `Cargo.toml` `[workspace.package]` | [D-003](decision-log.md#d-003-crate-shapes-and-workspace-registration) |

Upgrading the toolchain also means re-running `python3 tests/guardrails/run_fixtures.py`. The float check reads rustc's MIR text, which is not a stable interface, and the clippy bans depend on clippy's lint names ([D-013](decision-log.md#d-013-type-aware-float-check)).

## CI: runner, actions and tools

Pinned in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) ([D-007](decision-log.md#d-007-ci-baseline-and-required-checks)). Actions are pinned to the commit of their release tag. Local runs should use the same tool versions.

| Item | Version | Pin |
| --- | --- | --- |
| Runner image | `ubuntu-24.04` | `runs-on` |
| actions/checkout | v7.0.1 | `3d3c42e5aac5ba805825da76410c181273ba90b1` |
| Swatinem/rust-cache | v2.9.2 | `6323deb102c322ba6fcbdcafc7e3dddab59af2b6` |
| taiki-e/install-action | v2.87.27 | `ab6895351a88b908e96a4697860d4ffea5e57dbe` |
| cargo-nextest | 0.9.148 | `tool: cargo-nextest@0.9.148` |
| cargo-deny | 0.20.2 | `tool: cargo-deny@0.20.2` ([D-008](decision-log.md#d-008-cargo-deny-policy-and-dependency-allowlist)) |
| actions/setup-node | v7.1.0 | `949feb2413d6458794dcd2491c4babbbce0c15c1`; Node from `web/.nvmrc` ([D-066](decision-log.md#d-066-the-web-frontend-is-a-static-react-app)) |
| sqlx-cli | 0.9.0 (postgres only, no TLS) | `cargo install sqlx-cli@0.9.0 --locked --no-default-features --features postgres` in the `database` job; no install-action manifest exists ([D-034](decision-log.md#d-034-schema-and-migrations-sqlx-and-the-database-tests-stage)) |
| Postgres service (`database` job) | 18.6, Alpine image | The same digest as the local stack's Postgres, below ([D-034](decision-log.md#d-034-schema-and-migrations-sqlx-and-the-database-tests-stage)) |

## Guardrail fixtures

Used only by `tests/guardrails/run_fixtures.py`, inside a temporary copy of the workspace ([D-012](decision-log.md#d-012-guardrail-fixtures)). They are not workspace dependencies.

| Item | Version | Why |
| --- | --- | --- |
| uuid | 1.28.0 (features v1, v3 to v8) | Proves the UUID-generation and `Timestamp::now` bans |
| getrandom | 0.4.3 (feature sys_rng) | Proves the system-randomness bans |
| rand_chacha | 0.9.0 (default features off) | Control: seeded generation stays allowed; the same pin worldgen uses (D-031) |
| toml | 1.1.8 | Proves the float check sees a float read from a dependency |
| Python | 3, standard library only; CI uses the runner image's `python3`, and the scripts also run on Python 3.9 | Runs the fixture harness and the bots' dependency check |

## Local stack

Pinned in [`deploy/compose.yaml`](../deploy/compose.yaml) and [`deploy/dex/Dockerfile`](../deploy/dex/Dockerfile) by tag and multi-platform digest ([D-014](decision-log.md#d-014-local-stack)).

| Item | Version | Pin |
| --- | --- | --- |
| Postgres | 18.6, Alpine image | `postgres:18.6-alpine@sha256:77f585114c32fbca283dc835b0596f4e52b51b4c6662d7810b2f4084f60a1873` |
| dex | v2.46.0 | `ghcr.io/dexidp/dex:v2.46.0@sha256:933fcd3f523338c847b88ef84a7145fb2fcb7b9917f08418612afeaaa2001777` |
| otel-lgtm | 0.36.0 | `grafana/otel-lgtm:0.36.0@sha256:57c04cbd8e7022f32bb84c4ac005d79baf8b08e2c5be44facf11f5f1f9944ba2` |
| Discovery probe (curl) | 8.16.0 | `curlimages/curl:8.16.0@sha256:463eaf6072688fe96ac64fa623fe73e1dbe25d8ad6c34404a669ad3ce1f104b6` |
| Docker Compose | 5.6.0 verified (needs `up --wait`, Compose 2.17 or later) | Not pinned; installed per machine |
| just | 1.59.0 verified | Not pinned; installed per machine |

## Images

Pinned in [`dockerfiles/Dockerfile`](../dockerfiles/Dockerfile) by tag and multi-platform digest (an OCI index: amd64 and arm64 both resolve); `.github/scripts/check-images.py --static` refuses a base without a digest, and a builder that isn't the toolchain above ([D-065](decision-log.md#d-065-container-images-serve-as-pid-1-and-the-watched-lock)).

| Item | Version | Pin |
| --- | --- | --- |
| Build base | Rust 1.99.1, Debian trixie slim | `rust:1.99.1-slim-trixie@sha256:aab42391a986424e22d6a3ec14f37bc6dd3dacff4d457ca7cccd781e02297cc6` |
| Runtime base | distroless cc, Debian 13, nonroot (65532:65532) | `gcr.io/distroless/cc-debian13:nonroot@sha256:83b9b50586253821cf63ffcec87d3d94265a093ad30da232039af962dd8f4edf` |
| cargo-chef | 0.1.78, build stage only | `cargo install cargo-chef --locked --version 0.1.78`; never in a runtime image |
| Docker | 29.5.2 verified locally (colima; the legacy builder, no buildx); the runner image's Docker in CI | Not pinned; installed per machine |

## Tools: planned, not introduced

None.

## Crate dependencies

The first external crates arrived with P0.6. `deny.toml` allowlists every crate in the tree ([D-008](decision-log.md#d-008-cargo-deny-policy-and-dependency-allowlist), [D-017](decision-log.md#d-017-telemetry-dependencies-licenses-and-duplicates)). Each remaining Tech stack crate (anyhow) gets its exact version here, with a decision-log entry, in the package that first adds it. Two were dropped for code of our own: tower_governor for a rate limiter ([D-054](decision-log.md#d-054-rate-limits-per-principal-gcra-read-and-write-budgets)), and openidconnect for the web login's OIDC client ([D-057](decision-log.md#d-057-web-login-our-own-code-flow-with-pkce-and-the-session)).

| Crate | Version | Used by | Added in | Decision |
| --- | --- | --- | --- | --- |
| tracing | =0.1.44 | telemetry, all four binaries | P0.6 | D-017 |
| tracing-subscriber | =0.3.23 (std, fmt, registry) | telemetry; server, dev-dependency only (the privacy test's capture, D-047) | P0.6 | D-017 |
| opentelemetry | =0.33.1 (trace, metrics) | telemetry | P0.6 | D-017, D-064 |
| opentelemetry_sdk | =0.33.1 (trace, metrics; testing behind telemetry's dev-only `testing` feature) | telemetry | P0.6 | D-017, D-064 |
| opentelemetry-otlp | =0.33.1 (http-proto, reqwest-blocking-client, trace, metrics) | telemetry | P0.6 | D-017, D-064 |
| tracing-opentelemetry | =0.34.0 (no default features) | telemetry | P0.6 | D-017 |
| thiserror | =2.0.21 | telemetry; core from P1.1; rules from P1.2 | P0.6 | D-017, D-018, D-020 |
| proptest | =1.12.0 (std only, no fork or timeout) | core and testkit, dev-dependency only | P1.1 | D-019, D-024 |
| postcard | =1.1.3 (alloc, default features off) | testkit (dev-only crate), for canonical bytes; store from P2.2, a runtime dependency (the snapshot codec) | P1.5 | D-024, D-035 |
| blake3 | =1.8.7 (pure, default features off) | store, the state hash | P2.2 | D-035 |
| rand_chacha | =0.9.0 (default features off) | worldgen, the only randomness source, seeded from the universe seed; already in the tree through opentelemetry_sdk's rand 0.9, so no new crate or duplicate | P1.12 | D-031 |
| serde | =1.0.229 (std, derive) | rules; core from P1.3; api-types from P3.1; bots from P3.6, dex's token response | P1.2 | D-020, D-022, D-045, D-052 |
| serde_json | =1.0.154 (std) | core and testkit, dev-dependency only; store from P2.3, the log's JSON columns; api-types from P3.1; bots from P3.6, a reply decoded after reading its text | P1.3 | D-022, D-036, D-045, D-052 |
| sqlx | =0.9.0 (postgres, runtime-tokio, macros, migrate; default features off) | store | P2.1 | D-034 |
| toml | =1.1.8 (std, parse, serde) | rules | P1.2 | D-020 |
| tokio | =1.53.3 (rt, sync, time, macros; default features off) | host, the loop and its runtime; already in the tree through sqlx and reqwest. The server adds net, rt-multi-thread and signal for `serve` (P3.4s, D-050); signal brings one crate, signal-hook-registry 1.4.8 (MIT OR Apache-2.0). The bots use it from P3.6 (D-052), with test-util for paused time in their tests (no new crate) | P2.3 | D-036, D-050, D-052 |
| clap | =4.6.7 (std, derive, help, usage, error-context, env; default features off, so no color crates) | replay, its command line; the server's from P2.9; the bots' from P3.6 | P2.7 | D-041, D-052 |
| utoipa | =6.0.0 (macros; default features off) | api-types, the OpenAPI document | P3.1 | D-045 |
| axum | =0.8.9 (http1, tokio, json, query, matched-path since P5.1 for the request span's route, D-064; default features off) | server: the token extractor, the routes (query since P3.4, D-049); the tests' local servers | P3.2 | D-046, D-049 |
| jsonwebtoken | =11.1.0 (aws_lc_rs; default features off, so no PEM) | server, token signatures (RS256) | P3.2 | D-046 |
| reqwest | =0.13.5 (json; default features off, no TLS until P5) | server, dex's discovery document and keys; already in the tree through telemetry's OTLP exporter. The bots' HTTP client from P3.6: dex's token endpoint and the API | P3.2 | D-046, D-052 |
| aws-lc-rs | =1.18.2 | server, dev-dependency only: generates the token tests' RSA keys (also jsonwebtoken's backend) | P3.2 | D-046 |
| jsonschema | =0.59.0 (default features off: no HTTP or TLS) | api-types, dev-dependency only: validates examples against the OpenAPI schemas (draft 2020-12), with exact integer bounds | P3.1 | D-045 |
| uuid | =1.28.0 (std, v7; default features off) | server: draws each new player's UUIDv7 | P3.3 | D-047 |
| insta | =1.46.1 (json; default features off) | server, dev-dependency only: the routes' contract snapshots | P3.4 | D-049, D-060 |
| log | =0.4.34 (std; default features off) | server, dev-dependency only: the privacy test captures sqlx's `log` events | P3.3 | D-047 |
| tower-http | =0.6.11 (cors; default features off) | server, CORS for the web app's origin; already in the tree through reqwest, so no new version | P4.6 | D-066 |

## Web app

Pinned in [`web/`](../web/AGENTS.md) ([D-066](decision-log.md#d-066-the-web-frontend-is-a-static-react-app)). Every direct dependency is exact in `package.json`, and `package-lock.json` locks the tree. `package.json` overrides openapi-typescript's TypeScript peer to the project's TypeScript.

| Item | Version | Pin |
| --- | --- | --- |
| Node | 26.12.0 | `web/.nvmrc`, `engines` |
| npm | 11.20.0 | `packageManager` |

| Package | Version | Kind | Used for |
| --- | --- | --- | --- |
| @fontsource/chakra-petch | 5.3.0 | runtime | The display face (OFL-1.1), bundled |
| @heroui/react | 3.2.6 | runtime | Components |
| @heroui/styles | 3.2.6 | runtime | HeroUI's theme and component styles |
| @kedar42/perseus | 0.3.0 (GitHub release tarball) | runtime | The owner's HeroUI components (NavMenu) |
| @tanstack/react-query | 5.104.5 | runtime | Server reads and command mutations |
| @tanstack/react-router | 1.170.45 | runtime | Code routes |
| openapi-fetch | 0.17.0 | runtime | The typed API client |
| openapi-react-query | 0.5.4 | runtime | Binds the client to TanStack Query |
| oidc-client-ts | 3.5.0 | runtime | The browser login: discovery, PKCE, state, nonce and the ID token checks |
| react | 19.3.1 | runtime | UI |
| react-aria-components | 1.22.1 | runtime | RouterProvider, so HeroUI links navigate in-app |
| react-dom | 19.3.1 | runtime | UI |
| zustand | 5.0.15 | runtime | Client state |
| @eslint/js | 10.0.1 | dev | eslint's recommended rules |
| @playwright/test | 1.64.0 | dev | The web walk in Chromium (`web/e2e`), run by `e2e.sh` |
| @tailwindcss/vite | 4.3.3 | dev | Tailwind's Vite plugin |
| @testing-library/jest-dom | 7.0.1 | dev | DOM matchers |
| @testing-library/react | 16.3.3 | dev | Component tests |
| @types/node | 26.6.5 | dev | Node's types, for the web walk's environment |
| @types/react | 19.3.0 | dev | React's types |
| @types/react-dom | 19.3.0 | dev | React DOM's types |
| @vitejs/plugin-react | 6.1.2 | dev | React in Vite |
| eslint | 10.13.0 | dev | Lint |
| eslint-plugin-react-hooks | 7.1.1 | dev | Hooks rules |
| globals | 17.13.0 | dev | Browser globals for eslint |
| jsdom | 30.1.3 | dev | The DOM for Vitest |
| openapi-typescript | 7.13.0 | dev | Generates `src/api/schema.ts` |
| tailwindcss | 4.3.3 | dev | Styles |
| typescript | 6.0.3 | dev | Types; 6.0 because typescript-eslint supports below 6.1 |
| typescript-eslint | 8.72.0 | dev | Strict type-checked lint |
| vite | 8.3.5 | dev | Dev server and build |
| vitest | 5.0.5 | dev | Unit and component tests |
