# voidforge-bots

Scripted players for end-to-end and nightly runs, plus a terminal client with device code login. Bots play only through the public API.

Binary. It is a stub today; P3.6 is the first package that fills it. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-api-types`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): reqwest, tokio, clap and anyhow.
- Never: Any workspace crate other than api-types. Bots reach the server over HTTP alone.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Terminal login uses the device code grant as the public `voidforge-cli` dex client (P3.7). The CI end-to-end tests use the password grant, for the static test users only.
- End-to-end and nightly runs use the fast test rules.

## Tests

```sh
cargo build -p voidforge-bots
cargo clippy -p voidforge-bots --all-targets -- -D warnings
cargo nextest run -p voidforge-bots --no-tests=warn   # drop --no-tests=warn once the workspace has a real test (D-004)
```

P3.6: bots log in through dex, then join, build, race to colonize and transport cargo, against the compose stack in CI. P3.7: a device code login yields a token the API accepts. P5.3: the nightly long simulation.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Auth](../../docs/Voidforge%20Technical%20Plan.md#auth), [Phase 3](../../docs/Voidforge%20Technical%20Plan.md#phase-3-api-and-auth), [Testing strategy](../../docs/Voidforge%20Architectural%20Design.md#testing-strategy)
