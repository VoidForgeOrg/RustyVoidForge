# voidforge-api-types

The wire types for the public API (commands, views and errors) and their OpenAPI schemas. The server, the web frontend and the bots share them.

Library. It is a stub today; P3.1 is the first package that fills it. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: None.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): serde, serde_json and utoipa.
- Never: Any workspace crate. The web frontend and the bots link this crate, so it must not pull the engine in.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- The wire format is JSON. Game times leave the API as ISO 8601 UTC timestamps (universe epoch plus game time), and amounts as integer grams.
- Every continuous value carries `rate_g_per_h`, `as_of` and `valid_until`, and stocks carry `cap_g`.
- A command body names one of the eleven command types and never names the player; the player comes from the auth token.

## Tests

```sh
cargo build -p voidforge-api-types
cargo clippy -p voidforge-api-types --all-targets -- -D warnings
cargo nextest run --workspace --locked   # this crate has no tests yet, so `-p voidforge-api-types` alone exits 4
```

P3.1: `/openapi.json` builds, and the schemas for every command and view validate.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Read side and API](../../docs/Voidforge%20Architectural%20Design.md#read-side-and-api), [Commands](../../docs/Voidforge%20Architectural%20Design.md#commands), [Phase 3](../../docs/Voidforge%20Technical%20Plan.md#phase-3-api-and-auth)
