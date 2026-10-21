import { createRootRoute, createRoute, createRouter, redirect } from "@tanstack/react-router";
import { completeLogin, login } from "./auth/auth";
import { Overview } from "./pages/Overview";
import { AppShell } from "./shell/AppShell";

const root = createRootRoute({ component: AppShell });

const placeholder = (path: string, title: string) =>
  createRoute({ getParentRoute: () => root, path, component: () => <h1>{title}</h1> });

export const routeTree = root.addChildren([
  createRoute({ getParentRoute: () => root, path: "/", component: Overview }),
  createRoute({
    getParentRoute: () => root,
    path: "/callback",
    // A loader runs once per navigation, so the one-time code is never redeemed twice.
    loader: async () => {
      redirect({ href: await completeLogin(), throw: true });
    },
    errorComponent: ({ error }) => (
      <>
        <p>The login didn't complete: {error instanceof Error ? error.message : "unknown error"}</p>
        <button type="button" onClick={() => void login("/")}>
          Log in
        </button>
      </>
    ),
  }),
  placeholder("/planets/$planetId", "Planet"),
  placeholder("/fleets", "Fleets"),
  placeholder("/universe", "Universe"),
  placeholder("/leaderboard", "Leaderboard"),
]);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
