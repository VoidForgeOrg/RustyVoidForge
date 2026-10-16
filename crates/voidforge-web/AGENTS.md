# voidforge-web

A server-rendered HTML frontend, so people can test and evaluate the game in a browser. It logs players in through dex and calls the public API over HTTP like any other client. It serves on localhost:8081 locally.

Binary. It is a stub today; P4.1 is the first package that fills it. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-api-types` and `voidforge-telemetry` only.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): axum, tower-http, maud, openidconnect, tower-sessions (in-memory store), reqwest, clap and anyhow, plus scraper for smoke tests.
- Never: core, host, store or server, directly or anywhere in its dependency tree. `tests/guardrails/check_web_deps.py` fails CI on any workspace crate other than api-types and telemetry in this crate's resolved dependencies, or in the declarations of this crate or any workspace crate it reaches, of any kind ([D-011](../../docs/decision-log.md#d-011-web-dependency-isolation-check)).
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Plain HTML from maud: tables, forms and links with browser default styles. No JavaScript and no CSS framework.
- It is a confidential OIDC client using the authorization code flow with PKCE, with its redirect at `http://localhost:8081/callback`. The ID token stays in the server-side session; the browser holds only a session cookie.
- Every form carries a fresh idempotency key in a hidden field, and posts to the web app, which sends one `POST /commands` and redirects back. A double submit runs once.
- A rejected command shows its reason code at the top of the page.

## Tests

```sh
cargo build -p voidforge-web
cargo clippy -p voidforge-web --all-targets -- -D warnings
cargo nextest run -p voidforge-web --no-tests=warn   # drop --no-tests=warn once the workspace has a real test (D-004)
```

P4.1 to P4.4: login and logout, API errors shown as messages, pages rendered from live data, and every MVP action working from the browser. P4.5: cookie-jar smoke tests on the fast test rules.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Web frontend](../../docs/Voidforge%20Technical%20Plan.md#web-frontend), [Auth](../../docs/Voidforge%20Technical%20Plan.md#auth), [Phase 4](../../docs/Voidforge%20Technical%20Plan.md#phase-4-web-frontend), [Read side and API](../../docs/Voidforge%20Architectural%20Design.md#read-side-and-api)
