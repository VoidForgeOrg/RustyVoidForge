# voidforge-store

Postgres access, migrations and the snapshot codec: the append-only `log_records` table, `snapshots` and `accounts`, plus postcard encoding with a BLAKE3 state hash.

Library. P2.1 added the schema: three migrations in `migrations/`, `MIGRATOR` and `migrate` ([D-034](../../docs/decision-log.md#d-034-schema-and-migrations-sqlx-and-the-database-tests-stage)). The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-core`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): sqlx with its migrations, postcard and blake3.
- Never: Any workspace crate other than core.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- The log is append-only with gap-free `seq`. On disk the log is the source of truth, and Postgres is never queried for live game state.
- sqlx queries are checked at compile time, and their offline query metadata is committed so CI builds without a database.
- Snapshot bytes are deterministic: encode, decode and re-encode give identical bytes. The header names the rules version and engine build.
- Migrations only ever add. No destructive change ships in the same release as the code that stops using it ([CI/CD](../../docs/Voidforge%20Technical%20Plan.md#cicd)). CI's `database` job refuses an edited, renamed or deleted migration ([D-034](../../docs/decision-log.md#d-034-schema-and-migrations-sqlx-and-the-database-tests-stage)).
- No foreign key from the log to `accounts`: the log must replay into a database without accounts (D-034).

## Tests

```sh
cargo build -p voidforge-store
cargo clippy -p voidforge-store --all-targets -- -D warnings
cargo nextest run --workspace --locked   # compiles the Postgres tests; they are #[ignore]d here
just db-test                             # needs `just up`: runs them, like the CI database job
```

- `tests/database.rs` holds the Postgres tests. Each is `#[sqlx::test]` with `#[ignore = "needs Postgres: just db-test"]`, so it gets its own fresh database. They cover: migrations on an empty database, a no-op re-run, a refused changed migration, every column and constraint introspected against the Architectural Design, and each constraint probed by effect.
- `.github/scripts/database.sh` is the Database tests stage ([CI/CD](../../docs/Voidforge%20Technical%20Plan.md#cicd)). It's the CI `database` job, and `just db-test` locally. It runs only against the `voidforge_test` database, and drops only `_sqlx_test_*` databases.
- A new or changed `sqlx::query!` needs fresh metadata. Run `cargo sqlx prepare --workspace -- --all-targets` with `DATABASE_URL` pointing at a migrated database, and commit `.sqlx/`. The CI check fails on stale metadata.
- A schema change is a new migration that sorts after the last. Never edit one that exists on main; CI refuses it.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Persistence](../../docs/Voidforge%20Architectural%20Design.md#persistence), [Phase 2](../../docs/Voidforge%20Technical%20Plan.md#phase-2-host-and-persistence), [Decisions 3 and 4](../../docs/Voidforge%20Technical%20Plan.md#decisions)
