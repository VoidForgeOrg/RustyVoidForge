# voidforge-bots

Scripted players for end-to-end and nightly runs, plus a terminal client with device code login. Bots play only through the public API.

Binary. P3.6 added its first subcommand, `e2e`, the end-to-end scenario ([D-052](../../docs/decision-log.md#d-052-the-end-to-end-scenario-the-bots-and-the-e2e-job)). P3.7 added the terminal: `login` (dex's device code flow), `logout`, `me`, `get <path>` and `command '<json>'` ([D-053](../../docs/decision-log.md#d-053-terminal-login-the-device-code-flow-and-the-saved-login)). The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-api-types`, `voidforge-telemetry`.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): reqwest, tokio, clap and anyhow; tracing for spans. Also serde (derive), for dex's token response, and serde_json, to decode a reply after reading its text so a failure prints what came back (D-052). anyhow isn't used yet.
- Never: Any workspace crate other than api-types and telemetry. Bots reach the server over HTTP alone.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Telemetry: `main` calls `voidforge_telemetry::init` once, first, and `Telemetry::shutdown` before exit, reporting a failed shutdown with a non-zero exit ([D-016](../../docs/decision-log.md#d-016-telemetry-bootstrap)). It never configures its own exporter or subscriber.
- Terminal login uses the device code grant as the public `voidforge-cli` dex client (P3.7). The CI end-to-end tests use the password grant, for the static test users only.
- The terminal's login is a credential: saved only in the user-only token file (0600, in a 0700 directory, replaced atomically), refused when others can read it, and never printed or logged, nor are its subject or the device code (D-053). A login lasts dex's 24 h with no refresh token; an expired or refused one says "log in again" with the command, never a raw 401.
- The terminal writes answers as JSON to stdout and everything for the person to stderr. `get` takes only API paths, never a URL. One exit-code table covers every subcommand (`--help`).
- End-to-end and nightly runs use the fast test rules.
- The bots never read the rules file. Every wait's deadline comes from the views' own `eta`, `arrives_at` and `valid_until`, and every wait is bounded: no blind sleeps (D-052).
- Every check is on a view read back over the API, never on a command's 200 alone. A check that could pass with nothing in place is preceded by one proving the case is there (for example, that alice's fleet exists before bob is shown not to see it).

## Tests

```sh
cargo build -p voidforge-bots
cargo clippy -p voidforge-bots --all-targets -- -D warnings
cargo nextest run -p voidforge-bots --locked   # the waits, the device flow against a dex stand-in, the token file
just e2e   # the scenario and the terminal login; needs DEX_CONFIG=config.ci.yaml just up
```

P3.6: bots log in through dex, then join, build, race to colonize and transport cargo, against the compose stack in CI (the `e2e` job, `.github/scripts/e2e.sh`). P3.7: a device code login yields a token the API accepts. P5.3: the nightly long simulation.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Auth](../../docs/Voidforge%20Technical%20Plan.md#auth), [Phase 3](../../docs/Voidforge%20Technical%20Plan.md#phase-3-api-and-auth), [Testing strategy](../../docs/Voidforge%20Architectural%20Design.md#testing-strategy)
