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

## Tools: planned, not introduced

| Tool | Introduced by |
| --- | --- |
| just | P0.5 |
| Docker Compose, Postgres, dex, otel-lgtm images | P0.5 |
| cargo-chef | P5.2 |

## Crate dependencies

`Cargo.lock` holds only the twelve workspace crates; no external crate is a dependency yet. The `deny.toml` allowlist enforces this ([D-008](decision-log.md#d-008-cargo-deny-policy-and-dependency-allowlist)). Each Tech stack crate (serde, serde_json, thiserror, anyhow, tokio, axum, tower-http, reqwest, sqlx, postcard, blake3, toml, rand_chacha, uuid, clap, jsonwebtoken, openidconnect, maud, tower-sessions, utoipa, tracing, opentelemetry, opentelemetry-otlp, tracing-opentelemetry, proptest, insta, scraper, tower_governor) gets its exact version here, with a decision-log entry, in the package that first adds it.

| Crate | Version | Used by | Added in | Decision |
| --- | --- | --- | --- | --- |
| _none yet_ | | | | |

<!-- BEGIN OpenRig MANAGED BLOCK: negative-test -->
