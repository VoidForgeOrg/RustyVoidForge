# voidforge-web

A server-rendered HTML frontend, so people can test and evaluate the game in a browser. It logs players in through dex and calls the public API over HTTP like any other client. It serves on localhost:8081 locally.

A binary over a library (`src/lib.rs`), so tests drive the router directly. P4.1 brought the login (D-057); P4.2 to P4.4 bring the API client, the pages and the forms. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-api-types` and `voidforge-telemetry` only.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): axum, maud, tower-sessions (in-memory store), reqwest, clap, jsonwebtoken, aws-lc-rs and base64 (the OIDC client, our own: [D-057](../../docs/decision-log.md#d-057-web-login-our-own-code-flow-with-pkce-and-the-session)), plus tower-http when it's needed and scraper for smoke tests. Errors are typed with thiserror; there's no anyhow.
- Never: core, host, store or server, directly or anywhere in its dependency tree. `tests/guardrails/check_web_deps.py` fails CI on any workspace crate other than api-types and telemetry in this crate's resolved dependencies, or in the declarations of this crate or any workspace crate it reaches, of any kind ([D-011](../../docs/decision-log.md#d-011-web-dependency-isolation-check)).
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Telemetry: `main` calls `voidforge_telemetry::init` once, first, and `Telemetry::shutdown` before exit, reporting a failed shutdown with a non-zero exit ([D-016](../../docs/decision-log.md#d-016-telemetry-bootstrap)). It never configures its own exporter or subscriber.
- Plain HTML from maud: tables, forms and links with browser default styles. No JavaScript and no CSS framework.
- It is a confidential OIDC client using the authorization code flow with PKCE, with its redirect at `http://localhost:8081/callback`. The ID token stays in the server-side session; the browser holds only a session cookie (HttpOnly, SameSite=Lax, Secure with `--secure-cookie`).
- `src/oidc.rs` is security code: endpoints only from discovery and on the issuer's origin, no redirects followed, CSPRNG state, nonce and verifier, and an ID token check at least as strict as the server's (D-046). Each check has a test that fails without it; keep it that way (D-057).
- The ID token, the code, state, nonce, the verifier and the client secret never reach a page, a log, a URL the app builds or a `Debug`. The secret comes from `VOIDFORGE_WEB_CLIENT_SECRET` only, never a flag.
- A login attempt is used once, and the session gets a new id at login. Logout is a POST.
- Logging in joins the game: the first `/players/me` creates the account (D-047).
- Every form carries a fresh idempotency key in a hidden field, and posts to the web app, which sends one `POST /commands` and redirects back. A double submit runs once.
- A rejected command shows its reason code at the top of the page.

## Tests

```sh
cargo build -p voidforge-web
cargo clippy -p voidforge-web --all-targets -- -D warnings
cargo nextest run -p voidforge-web --locked
```

The login against a real dex runs in `just e2e` (`.github/scripts/web-login.py`, step 5c of `e2e.sh`), which needs localhost:8081 free. Locally, run the web app with `VOIDFORGE_WEB_CLIENT_SECRET=voidforge-web-local-test-secret cargo run -p voidforge-web` against the stack's dex and a running `serve`.

P4.1 to P4.4: login and logout, API errors shown as messages, pages rendered from live data, and every MVP action working from the browser. P4.5: cookie-jar smoke tests on the fast test rules.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Web frontend](../../docs/Voidforge%20Technical%20Plan.md#web-frontend), [Auth](../../docs/Voidforge%20Technical%20Plan.md#auth), [Phase 4](../../docs/Voidforge%20Technical%20Plan.md#phase-4-web-frontend), [Read side and API](../../docs/Voidforge%20Architectural%20Design.md#read-side-and-api)
