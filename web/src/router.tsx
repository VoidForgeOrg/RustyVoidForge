import { Button } from "@heroui/react";
import {
  createRootRoute,
  createRoute,
  createRouter,
  lazyRouteComponent,
  notFound,
  Outlet,
  redirect,
} from "@tanstack/react-router";
import { completeLogin, login } from "./auth/auth";
import { LOGIN_FAILED } from "./copy/messages";
import { failures, titles } from "./copy/voices";
import { AppShell } from "./shell/AppShell";
import { Notice } from "./shell/Failure";

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
  interface StaticDataRouteOption {
    title?: string;
  }
}

/** A planet or building number the API could take: a safe non-negative integer, else not found. */
export function number(text: string): number {
  const n = Number(text);
  if (!/^\d+$/.test(text) || !Number.isSafeInteger(n)) notFound({ throw: true });
  return n;
}

const Uncharted = () => <Notice heading={failures.uncharted} copy={failures.unchartedCopy} />;

const root = createRootRoute({ component: Outlet });

const callback = createRoute({
  getParentRoute: () => root,
  path: "/callback",
  // A loader runs once per navigation, so the one-time code is never redeemed twice.
  loader: async () => {
    redirect({ href: await completeLogin(), throw: true });
  },
  // oidc-client-ts's own message never reaches the page.
  errorComponent: () => (
    <main className="grid min-h-dvh place-items-center p-4">
      <Notice heading={failures.login} copy={LOGIN_FAILED}>
        <Button className="self-start" variant="primary" onPress={() => void login("/")}>
          Log in
        </Button>
      </Notice>
    </main>
  ),
});

const game = createRoute({
  getParentRoute: () => root,
  id: "game",
  component: AppShell,
  notFoundComponent: Uncharted,
});

const home = createRoute({
  getParentRoute: () => game,
  path: "/",
  beforeLoad: () => {
    redirect({ to: "/planets", throw: true });
  },
});

const planets = createRoute({
  getParentRoute: () => game,
  path: "/planets",
  staticData: { title: titles.planets },
  component: lazyRouteComponent(() => import("./screens/Planets"), "Planets"),
});

const planet = createRoute({
  getParentRoute: () => planets,
  path: "$planetId",
  params: {
    parse: (p) => ({ planetId: number(p.planetId) }),
    stringify: (p) => ({ planetId: p.planetId.toString() }),
  },
  component: lazyRouteComponent(() => import("./screens/Planets"), "Planet"),
});

const buildingRoute = createRoute({
  getParentRoute: () => planet,
  path: "buildings/$buildingId",
  params: {
    parse: (p) => ({ buildingId: number(p.buildingId) }),
    stringify: (p) => ({ buildingId: p.buildingId.toString() }),
  },
  component: lazyRouteComponent(() => import("./screens/Building"), "BuildingDrawer"),
});

const build = createRoute({
  getParentRoute: () => planet,
  path: "build",
  component: lazyRouteComponent(() => import("./screens/Building"), "BuildDrawer"),
});

const fleets = createRoute({
  getParentRoute: () => game,
  path: "/fleets",
  staticData: { title: titles.fleets },
  component: lazyRouteComponent(() => import("./screens/Fleets"), "Fleets"),
});

const universe = createRoute({
  getParentRoute: () => game,
  path: "/universe",
  staticData: { title: titles.universe },
  component: lazyRouteComponent(() => import("./screens/Universe"), "Universe"),
});

const leaderboard = createRoute({
  getParentRoute: () => game,
  path: "/leaderboard",
  staticData: { title: titles.leaderboard },
  component: lazyRouteComponent(() => import("./screens/Leaderboard"), "Leaderboard"),
});

export const routeTree = root.addChildren([
  callback,
  game.addChildren([
    home,
    planets.addChildren([planet.addChildren([buildingRoute, build])]),
    fleets,
    universe,
    leaderboard,
  ]),
]);

export const router = createRouter({ routeTree });
