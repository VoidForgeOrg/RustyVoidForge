# voidforge-server

The game API process. It checks tokens, maps accounts to players, runs the routes and the leaderboard job, and has the universe bootstrap subcommand. It serves `/openapi.json` on localhost:8080 locally.

Binary. It is a stub today; P2.9 is the first package that fills it. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-host`, `voidforge-store`, `voidforge-rules`, `voidforge-worldgen`, `voidforge-api-types`, `voidforge-telemetry`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): The API stack from the Tech stack: tokio, axum, tower-http, jsonwebtoken, uuid (v7), utoipa, tower_governor, clap and anyhow.
- Never: voidforge-core directly. Engine state changes only through the host.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Telemetry: `main` calls `voidforge_telemetry::init` once, first, and `Telemetry::shutdown` before exit, reporting a failed shutdown with a non-zero exit ([D-016](../../docs/decision-log.md#d-016-telemetry-bootstrap)). It never configures its own exporter or subscriber.
- Nothing in the engine knows about auth. The API turns a dex ID token into a player UUID before any command reaches the host, and never sees a password ([Auth](../../docs/Voidforge%20Technical%20Plan.md#auth)).
- Tokens are checked against dex's cached signing keys, issuer, the audience allowlist and expiry. Anything else gets 401.
- All writes go through `POST /commands` with a required `Idempotency-Key` header. The player comes from the token, never from the body.
- Status mapping: 200 accepted, 400, 401, 422 for a rule violation with its reason code, 503 with Retry-After when the inbox is full, and 429 for the rate limit (P3.8).
- An `accounts` row maps issuer and subject to a player UUIDv7. The first authenticated call creates it and issues Join game, exactly once.
- The leaderboard is rebuilt every 60 seconds from the read model and served with its `as_of`.

## Tests

```sh
cargo build -p voidforge-server
cargo clippy -p voidforge-server --all-targets -- -D warnings
cargo nextest run --workspace --locked   # this crate has no tests yet, so `-p voidforge-server` alone exits 4
```

P2.9: on an empty database, the bootstrap always yields the same state hash. P3.2 to P3.5 and P3.8: token, account, route (insta contract snapshots), leaderboard and rate-limit tests.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Read side and API](../../docs/Voidforge%20Architectural%20Design.md#read-side-and-api), [Auth](../../docs/Voidforge%20Technical%20Plan.md#auth), [Phase 3](../../docs/Voidforge%20Technical%20Plan.md#phase-3-api-and-auth)
