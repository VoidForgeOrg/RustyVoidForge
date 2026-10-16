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

## Tools: installed or planned, not pinned in the repository

| Tool | Status | Pinned by |
| --- | --- | --- |
| cargo-nextest | P0.1 was verified with 0.9.148, installed locally. The repository does not pin it. | P0.3, for the CI install |
| cargo-deny | Not introduced | P0.3 |
| just | Not introduced | P0.5 |
| Docker Compose, Postgres, dex, otel-lgtm images | Not introduced | P0.5 |
| cargo-chef | Not introduced | P5.2 |

## Crate dependencies

`Cargo.lock` holds only the twelve workspace crates; no external crate is a dependency yet. Each Tech stack crate (serde, serde_json, thiserror, anyhow, tokio, axum, tower-http, reqwest, sqlx, postcard, blake3, toml, rand_chacha, uuid, clap, jsonwebtoken, openidconnect, maud, tower-sessions, utoipa, tracing, opentelemetry, opentelemetry-otlp, tracing-opentelemetry, proptest, insta, scraper, tower_governor) gets its exact version here, with a decision-log entry, in the package that first adds it.

| Crate | Version | Used by | Added in | Decision |
| --- | --- | --- | --- | --- |
| _none yet_ | | | | |
