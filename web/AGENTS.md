# web

The browser frontend, so people can test and evaluate the game. A static TypeScript React app built with Vite. It logs players in through dex and calls the public API over HTTP like any other client. It is not part of the cargo workspace ([D-063](../docs/decision-log.md#d-063-the-web-frontend-is-a-static-react-app)). The [Web frontend](../docs/Voidforge%20Technical%20Plan.md#web-frontend) section is the authority, and the [root AGENTS.md](../AGENTS.md) covers repository-wide rules.

## Stack

- React, Vite and TypeScript in strict mode.
- TanStack Router with code routes (`src/router.tsx`), TanStack Query for every server read, Zustand for client state.
- HeroUI with Tailwind, and Perseus (`@kedar42/perseus`, the owner's HeroUI components, from its GitHub release tarball).
- Vitest with Testing Library; eslint with typescript-eslint's strict type-checked rules.
- Playwright for the web walk in `e2e/`, with its own `tsconfig.json` for Node's types.

Every dependency is pinned exactly in `package.json` and locked in `package-lock.json`. A new one needs a [decision log](../docs/decision-log.md) entry and a row in the [version table](../docs/versions.md).

## Invariants

- Node and npm are the versions in `.nvmrc`, `engines` and `packageManager`. Install with `npm ci`, never a bare `npm install` that rewrites the lockfile by accident.
- The dev server and `vite preview` both serve `http://localhost:8081` with a strict port, so dex has one redirect URI and the API one CORS origin.
- The API is reached only through `src/api/client.ts`: `api` for queries and mutations, `fetchClient` for the rest. Never a hand-rolled `fetch`.
- `src/api/schema.ts` is generated from `../crates/voidforge-api-types/openapi.json` by `npm run gen:api`, and never edited by hand. Regenerate it whenever that file changes; CI fails on a diff.
- The app names the fast test rules whenever `/rules` says `v0-fast`.
- Every failed call rejects with `ApiError` from `src/api/client.ts`, and a screen shows it through `src/copy/messages.ts`: §6.1's copy for every `ErrorCode` in a switch whose default takes `never`, with the code beside it. The API's `message` and oidc-client-ts's messages never reach a page. `messages.test.ts` checks the copy against `src/fixtures/copy.json`.
- In-universe words (titles, headings, empty states) live in `src/copy/voices.ts`; verbs, numbers and §6.1's copy stay plain (D-063).
- Every value a screen shows is formatted by `src/format.ts`, on integers only (BigInt inside); a value outside JavaScript's safe integers shows as `—`. Never format a number in a component.
- Values between reads come from `src/game/live.ts`: a view's `as_of` plus the time since it arrived, at its rates, held at `valid_until`. Never the browser's clock alone.
- Reads go through the hooks in `src/game/queries.ts`. A view refetches at its `valid_until` (the leaderboard at `as_of` plus 60 s) and on focus, never sooner than `FLOOR_MS` (2 s) after its last answer, and not while the tab is hidden. Nothing retries on its own; a refusal waits its Retry-After (D-054's read budget).
- Every order goes through `useOrder` in `src/game/commands.tsx`. A form keeps its idempotency key until an answer settles the order (a 200, or a refusal), so a retry after an unknown outcome reuses it. A 200 refetches the views the order changed at once, then shows a toast with `src/copy/outcomes.ts`'s lines. Submit is disabled while an order is in flight.
- Cancel construction, Demolish, Cancel ship build and Turn back ask first, through `Confirm` in `src/screens/Orders.tsx`. A control the view already shows the API would refuse is disabled with that code's copy beside it.
- The top bar (`src/shell/TopBar.tsx`) shows empire-wide figures from reads the screens already make, through `src/game/empire.ts`, never an endpoint of its own. It never scrolls sideways: it wraps, and on phones the figures past Ore, Ingots and Next move into the More sheet.
- A planet opens in one side panel on `/planets/:id` (`src/screens/PlanetPanel.tsx`), whose child routes name its state: a building by its id (`/planets/:id/buildings/:buildingId`), never by slot position, since the API numbers no slots and a position would shift after a demolition; and the build picker on `/planets/:id/build`, which every "+" tile opens, since free slots are interchangeable. The panel stays mounted while its state changes. Back steps out to the planet, and Close or Esc go to the grid. One dialog at a time: the mission stepper stands in for the panel. Tile labels name the building and its state ("Shipyard, working") or say "Free slot".
- Screens live in `src/screens/`, one lazily loaded module per route, with shared pieces (meters, status words) in `parts.tsx`. A status is always an icon and a word, never colour alone.
- The theme is `src/theme/theme.css`, one dark theme over HeroUI's tokens in hex; `theme.test.ts` holds its text and edge pairs to WCAG AA. Icons come from `src/theme/icons.tsx` and are always beside a word.
- `VITE_API_URL` names the API. `.env` holds the local default; override it in `.env.local` or the environment.

## Commands

Run from `web/`, or use `just web` from the repository root, which runs the CI job.

```sh
npm ci
npm run dev          # http://localhost:8081
npm run gen:api
npm run typecheck
npm run lint
npm test
npm run build
npm run preview      # serves dist/ on http://localhost:8081
npm run walk         # the Playwright walk; .github/scripts/e2e.sh runs it with the API, dex and preview up
```
