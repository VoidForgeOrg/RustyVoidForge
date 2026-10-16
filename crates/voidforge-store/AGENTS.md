# voidforge-store

Postgres access, migrations and the snapshot codec: the append-only `log_records` table, `snapshots` and `accounts`, plus postcard encoding with a BLAKE3 state hash.

Library. It is a stub today; P2.1 is the first package that fills it. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-core`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): sqlx with its migrations, postcard and blake3.
- Never: Any workspace crate other than core.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- The log is append-only with gap-free `seq`. On disk the log is the source of truth, and Postgres is never queried for live game state.
- sqlx queries are checked at compile time, and their offline query metadata is committed so CI builds without a database.
- Snapshot bytes are deterministic: encode, decode and re-encode give identical bytes. The header names the rules version and engine build.
- Migrations only ever add. No destructive change ships in the same release as the code that stops using it ([CI/CD](../../docs/Voidforge%20Technical%20Plan.md#cicd)).

## Tests

```sh
cargo build -p voidforge-store
cargo clippy -p voidforge-store --all-targets -- -D warnings
cargo nextest run -p voidforge-store --no-tests=warn   # drop --no-tests=warn once the workspace has a real test (D-004)
```

P2.1: migrations apply cleanly to an empty database. P2.2: the snapshot codec round-trips byte for byte. The Database tests stage in [CI/CD](../../docs/Voidforge%20Technical%20Plan.md#cicd), which runs migrations, store tests and the sqlx metadata check against Postgres, is not in CI yet. P0.3 added only the lint, supply-chain and test jobs.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Persistence](../../docs/Voidforge%20Architectural%20Design.md#persistence), [Phase 2](../../docs/Voidforge%20Technical%20Plan.md#phase-2-host-and-persistence), [Decisions 3 and 4](../../docs/Voidforge%20Technical%20Plan.md#decisions)
