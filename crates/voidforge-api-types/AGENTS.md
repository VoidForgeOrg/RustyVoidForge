# voidforge-api-types

The wire types for the public API (commands, views and errors) and their OpenAPI schemas. The server, the web frontend and the bots share them.

Library. P3.1 filled it ([D-045](../../docs/decision-log.md#d-045-the-public-api-wire-contract)): `command`, `event`, `view`, `error` and `wire`, and `openapi()` for the document. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: None.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): serde, serde_json and utoipa. Dev-only: jsonschema, to validate examples against the generated schemas (it compares integers exactly, which the bounds at 2^63 and 2^64 need).
- Never: Any workspace crate. The web frontend and the bots link this crate, so it must not pull the engine in.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- The wire format is JSON. Game times leave the API as RFC 3339 UTC timestamps with milliseconds, always (universe epoch plus game time); amounts as integer grams; entity IDs as integers; player IDs as lowercase hyphenated UUIDs (D-045).
- Every continuous value carries `rate_g_per_h`, `as_of` and `valid_until`, and stocks carry `cap_g`.
- A command body names one of the **ten player actions** and never names the player; the player comes from the auth token. Join game isn't public: the server issues it on a player's first call (Decision 9), so `join_game` is a 400 `malformed_body`.
- **A command's response carries its own events only**, never the events of advancing time to it: those can be other players' owner-only information (D-030). This is a privacy requirement (D-045), not a choice the server may revisit.
- Every error is `{"code", "message"}`, and `code` is a closed list (`ErrorCode`, with its status). The 31 game codes are the core's reason codes verbatim; the server's `tests/api_codes.rs` checks that.
- **The schema and serde agree**: every value one refuses, the other refuses (D-045, repair round 1).
  - A tagged union whose serde form refuses unknown fields gets a closed schema (the `Closed` modifier). A state-dependent view is one union whose variants list every field of their state; never `#[serde(flatten)]`, which defeats `deny_unknown_fields`.
  - Every integer schema carries its Rust type's exact bounds (`bound_integers`; a new integer kind needs adding there).
  - A pattern in the schema has the same check in the type's deserialization (`PlayerId`, `Timestamp`).
  - The only deliberate difference: duplicate ship IDs, refused at the edge by `check_unique_ships`.
- **`openapi.json` beside this crate is the public contract**: it changes only through `just openapi`, its freshness test fails on any difference, and a reviewer checks every change, as with goldens.

## Tests

```sh
cargo build -p voidforge-api-types
cargo clippy -p voidforge-api-types --all-targets -- -D warnings
cargo nextest run -p voidforge-api-types --locked
just openapi   # rewrites openapi.json after a contract change; call the change out in the PR body
```

- `tests/contract.rs`: every command, response, view, event and error example validates against its schema in the generated document (jsonschema, draft 2020-12, formats asserted), deserializes, and serializes back to itself; every tag and code the document declares has an example; invalid payloads (`join_game`, unknown or mistyped fields, duplicate ship IDs, times without milliseconds, uppercase UUIDs, a dropped view field, an unknown code) are refused by the schema and by serde; the ruled status of every code; the six endpoints and the bearer scheme; the closed per-state views; each integer kind at its edge and one past it (components and parameters); and the schema/serde agreement table.
- `tests/openapi_file.rs`: the freshness test, and the `bless` that `just openapi` runs.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Read side and API](../../docs/Voidforge%20Architectural%20Design.md#read-side-and-api), [Commands](../../docs/Voidforge%20Architectural%20Design.md#commands), [Phase 3](../../docs/Voidforge%20Technical%20Plan.md#phase-3-api-and-auth)
