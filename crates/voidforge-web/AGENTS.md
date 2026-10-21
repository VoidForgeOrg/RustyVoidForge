# voidforge-web

A server-rendered HTML frontend, so people can test and evaluate the game in a browser. It logs players in through dex and calls the public API over HTTP like any other client. It serves on localhost:8081 locally.

A binary over a library (`src/lib.rs`), so tests drive the router directly. P4.1 brought the login (D-057), P4.2 the API client and its messages (D-059), P4.3 the game's pages (D-060); P4.4 brings the forms. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: `voidforge-api-types` and `voidforge-telemetry` only.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): axum, maud, tower-sessions (with our own bounded store, `src/store.rs`), reqwest, clap, jsonwebtoken, aws-lc-rs and base64 (the OIDC client, our own: [D-057](../../docs/decision-log.md#d-057-web-login-our-own-code-flow-with-pkce-and-the-session)), plus tower-http when it's needed and scraper for smoke tests. Dev-only: tracing-subscriber (the log tests) and insta (the pages' HTML snapshots, [D-060](../../docs/decision-log.md#d-060-the-games-pages-one-formatting-module-and-concurrent-reads)). Errors are typed with thiserror; there's no anyhow.
- Never: core, host, store or server, directly or anywhere in its dependency tree. `tests/guardrails/check_web_deps.py` fails CI on any workspace crate other than api-types and telemetry in this crate's resolved dependencies, or in the declarations of this crate or any workspace crate it reaches, of any kind ([D-011](../../docs/decision-log.md#d-011-web-dependency-isolation-check)).
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Telemetry: `main` calls `voidforge_telemetry::init` once, first, and `Telemetry::shutdown` before exit, reporting a failed shutdown with a non-zero exit ([D-016](../../docs/decision-log.md#d-016-telemetry-bootstrap)). It never configures its own exporter or subscriber.
- Plain HTML from maud: tables, forms and links with browser default styles. No JavaScript and no CSS framework.
- It is a confidential OIDC client using the authorization code flow with PKCE, with its redirect at `http://localhost:8081/callback`. The ID token stays in the server-side session; the browser holds only a session cookie (HttpOnly, SameSite=Lax, Secure with `--secure-cookie`).
- `src/oidc.rs` is security code: endpoints only from discovery and on the issuer's origin, no redirects followed, CSPRNG state, nonce and verifier, and an ID token check at least as strict as the server's (D-046). Each check has a test that fails without it; keep it that way (D-057).
- The ID token, the code, state, nonce, the verifier and the client secret never reach a page, a log, a URL the app builds or a `Debug`. The secret comes from `VOIDFORGE_WEB_CLIENT_SECRET` only, never a flag.
- A login attempt is used once, and the session gets a new id at login. Logout is a POST.
- The session store is bounded (D-057): an anonymous session lives 10 minutes, and a handler that changes a logged-in session calls `keep_expiry`, or the save cuts it to that default.
- Logging in joins the game: the first `/players/me` creates the account (D-047).
- The API is reached only through `src/api.rs`'s `Api` (D-059): never hand-rolled HTTP. Authenticated methods take the session's token; `rules` takes none. Every call ends in a view or an `ApiError`, and nothing on the path panics.
- A failed call is shown through `src/messages.rs`: phase4-ux §6.1's copy for every `ErrorCode`, matched with no wildcard arm, and the code beside it; the API's `message` is never shown. 401 ends the session; 429, 500, 503 and an unreachable or unexpected API are the retry page with the API's status (502 for the last two). `tests/fixtures/copy.json` is §6.1 and §6.2 extracted from dev-design's document by `tests/fixtures/extract_copy.py`: when the copy changes there, rerun it.
- Every page reads the public `/rules` for the fast-rules line, beside its own reads and within its own 2 s bound; a failed read only drops the line. HEAD is 404 on every path, before any handler.
- Every value a page shows is formatted by `src/format.rs`, on integers only, the one documented way (phase4-ux §3, D-060): never format a number in a page. Times are relative to the view's `as_of`, never the web's clock.
- Every logged-in page starts through `routes::begin` and makes all its reads at once, with `/rules` and `/players/me`; the first failing read decides the page, through `routes::failed`. A planet number or a search the web can't use is refused before any call.
- The pages render no form control until P4.4 wires its forms: never a form that posts nowhere, and no placeholder.
- The frame's one-shot banner comes from the session's notice, taken (and the expiry kept) by the page that shows it; nothing in a URL sets it.
- Tests that read the log go in `tests/logs.rs` (one global subscriber, one test at a time), so they hold under plain `cargo test` too.
- Every form carries a fresh idempotency key in a hidden field, and posts to the web app, which sends one `POST /commands` and redirects back. A double submit runs once.
- A rejected command shows its reason code at the top of the page.

## Tests

```sh
cargo build -p voidforge-web
cargo clippy -p voidforge-web --all-targets -- -D warnings
cargo nextest run -p voidforge-web --locked
```

The login against a real dex runs in `just e2e` (`.github/scripts/web-login.py`, step 5c of `e2e.sh`), which needs localhost:8081 free. Locally, run the web app with `VOIDFORGE_WEB_CLIENT_SECRET=voidforge-web-local-test-secret cargo run -p voidforge-web` against the stack's dex and a running `serve`.

P4.1 to P4.4: login and logout, API errors shown as messages, pages rendered from live data, and every MVP action working from the browser. The pages' HTML snapshots are in `tests/snapshots/` (insta, a dev-dependency); each sits beside by-effect assertions, so review a changed snapshot by reading it, never by accepting it blind. e2e step 5c walks every page after the bots play. P4.5: cookie-jar smoke tests on the fast test rules.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Web frontend](../../docs/Voidforge%20Technical%20Plan.md#web-frontend), [Auth](../../docs/Voidforge%20Technical%20Plan.md#auth), [Phase 4](../../docs/Voidforge%20Technical%20Plan.md#phase-4-web-frontend), [Read side and API](../../docs/Voidforge%20Architectural%20Design.md#read-side-and-api)
