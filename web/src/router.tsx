import { createRootRoute, createRoute, createRouter, Outlet } from "@tanstack/react-router";

const root = createRootRoute({ component: Outlet });

const placeholder = (path: string, title: string) =>
  createRoute({ getParentRoute: () => root, path, component: () => <h1>{title}</h1> });

export const routeTree = root.addChildren([
  placeholder("/", "Overview"),
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
