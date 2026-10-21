# web

The browser frontend, so people can test and evaluate the game. A static TypeScript React app built with Vite. It logs players in through dex and calls the public API over HTTP like any other client. It is not part of the cargo workspace ([D-063](../docs/decision-log.md#d-063-the-web-frontend-is-a-static-react-app)). The [Web frontend](../docs/Voidforge%20Technical%20Plan.md#web-frontend) section is the authority, and the [root AGENTS.md](../AGENTS.md) covers repository-wide rules.

## Stack

- React, Vite and TypeScript in strict mode.
- TanStack Router with code routes (`src/router.tsx`), TanStack Query for every server read, Zustand for client state.
- HeroUI with Tailwind, and Perseus (`@kedar42/perseus`, the owner's HeroUI components, from its GitHub release tarball).
- Vitest with Testing Library; eslint with typescript-eslint's strict type-checked rules.

Every dependency is pinned exactly in `package.json` and locked in `package-lock.json`. A new one needs a [decision log](../docs/decision-log.md) entry and a row in the [version table](../docs/versions.md).

## Invariants

- Node and npm are the versions in `.nvmrc`, `engines` and `packageManager`. Install with `npm ci`, never a bare `npm install` that rewrites the lockfile by accident.
- The dev server and `vite preview` both serve `http://localhost:8081` with a strict port, so dex has one redirect URI and the API one CORS origin.
- The API is reached only through `src/api/client.ts`: `api` for queries and mutations, `fetchClient` for the rest. Never a hand-rolled `fetch`.
- `src/api/schema.ts` is generated from `../crates/voidforge-api-types/openapi.json` by `npm run gen:api`, and never edited by hand. Regenerate it whenever that file changes; CI fails on a diff.
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
```
