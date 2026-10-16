<!-- Title: start with the work package ID, for example "P1.4: event queue and advance_to". -->

## Package

- Package ID and row: <!-- link to its row in docs/Voidforge Technical Plan.md -->
- Crates or areas touched:

## Done when

<!-- Quote the package's "Done when" and show the evidence: commands run, their exit codes and the tests that prove it. -->

## Checks

- [ ] `cargo build`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo fmt --all --check`
- [ ] `cargo nextest run` (with `--no-tests=warn` only while the workspace has no real test; see decision log D-004)

## Specs and decisions

- [ ] The change matches the game design, architectural design and Technical Plan. Any document change lands in this PR before or with the code.
- [ ] New dependencies, Decisions-table changes and document clarifications have a decision-log entry, and new versions are in `docs/versions.md`.
- [ ] Each crate touched still depends only on the crates its `AGENTS.md` allows.
- [ ] Golden files under `tests/golden/` changed only through `just bless`, and the changes are called out for review (from P2.6 on).

## Notes for the reviewer

<!-- Limitations, follow-ups, and anything not covered. -->
