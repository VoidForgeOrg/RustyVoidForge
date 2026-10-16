# Voidforge

A persistent space strategy MMO: players build empires across a universe of planets, manage physical economies and expand through exploration and colonization. The server is a headless, deterministic discrete-event engine behind an open API, written in Rust.

The project is in Phase 0 (Foundations) of the [Technical Plan](docs/Voidforge%20Technical%20Plan.md#phase-0-foundations). The workspace holds the twelve crates as stubs; nothing runs yet.

## Specifications

- [MVP Game Design](docs/Voidforge%20MVP%20Game%20Design.md): the game rules.
- [Architectural Design](docs/Voidforge%20Architectural%20Design.md): engine, persistence and API behavior.
- [Technical Plan](docs/Voidforge%20Technical%20Plan.md): stack, crate layering, phases and work packages.

## Conventions

- [AGENTS.md](AGENTS.md): how work lands, the crate layering, commands and guardrails. Each crate under `crates/` has its own `AGENTS.md`.
- [Decision log](docs/decision-log.md): every team decision and new dependency.
- [Version table](docs/versions.md): pinned toolchain and versions.
- [Pull request template](.github/pull_request_template.md).

## Building

Install [rustup](https://rustup.rs); `rust-toolchain.toml` selects the pinned toolchain. Tests need [cargo-nextest](https://nexte.st) and the supply-chain check needs [cargo-deny](https://github.com/EmbarkStudios/cargo-deny), at the versions in the [version table](docs/versions.md). CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs the same commands.

```sh
cargo build
.github/scripts/check-no-managed-blocks.sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo deny check                    # needs cargo-deny
cargo nextest run --workspace --locked --no-tests=warn   # plain `cargo nextest run` once real tests exist (decision log D-004)
```
