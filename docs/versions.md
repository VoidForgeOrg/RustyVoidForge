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

## Guardrail fixtures

Used only by `tests/guardrails/run_fixtures.py`, inside a temporary copy of the workspace ([D-012](decision-log.md#d-012-guardrail-fixtures)). They are not workspace dependencies.

| Item | Version | Why |
| --- | --- | --- |
| uuid | 1.28.0 (features v1, v3 to v8) | Proves the UUID-generation and `Timestamp::now` bans |
| getrandom | 0.4.3 (feature sys_rng) | Proves the system-randomness bans |
| rand_chacha | 0.10.0 | Control: seeded generation stays allowed |
| toml | 1.1.8 | Proves the float check sees a float read from a dependency |
| Python | 3, standard library only; CI uses the runner image's `python3`, and the scripts also run on Python 3.9 | Runs the fixture harness and the web check |

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

## Tools: planned, not introduced

| Tool | Introduced by |
| --- | --- |
| cargo-chef | P5.2 |

## Crate dependencies

The first external crates arrived with P0.6. `deny.toml` allowlists every crate in the tree ([D-008](decision-log.md#d-008-cargo-deny-policy-and-dependency-allowlist), [D-017](decision-log.md#d-017-telemetry-dependencies-licenses-and-duplicates)). Each remaining Tech stack crate (anyhow, tokio, axum, tower-http, reqwest, sqlx, postcard, blake3, rand_chacha, uuid, clap, jsonwebtoken, openidconnect, maud, tower-sessions, utoipa, insta, scraper, tower_governor) gets its exact version here, with a decision-log entry, in the package that first adds it.

| Crate | Version | Used by | Added in | Decision |
| --- | --- | --- | --- | --- |
| tracing | =0.1.44 | telemetry, all four binaries | P0.6 | D-017 |
| tracing-subscriber | =0.3.23 (std, fmt, registry) | telemetry | P0.6 | D-017 |
| opentelemetry | =0.33.1 (trace) | telemetry | P0.6 | D-017 |
| opentelemetry_sdk | =0.33.1 (trace) | telemetry | P0.6 | D-017 |
| opentelemetry-otlp | =0.33.1 (http-proto, reqwest-blocking-client, trace) | telemetry | P0.6 | D-017 |
| tracing-opentelemetry | =0.34.0 (no default features) | telemetry | P0.6 | D-017 |
| thiserror | =2.0.21 | telemetry; core from P1.1; rules from P1.2 | P0.6 | D-017, D-018, D-020 |
| proptest | =1.12.0 (std only, no fork or timeout) | core and testkit, dev-dependency only | P1.1 | D-019, D-024 |
| postcard | =1.1.3 (alloc, default features off) | testkit (dev-only crate), for canonical bytes | P1.5 | D-024 |
| serde | =1.0.229 (std, derive) | rules; core from P1.3 | P1.2 | D-020, D-022 |
| serde_json | =1.0.154 (std) | core and testkit, dev-dependency only | P1.3 | D-022 |
| toml | =1.1.8 (std, parse, serde) | rules | P1.2 | D-020 |
