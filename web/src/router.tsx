import { Button } from "@heroui/react";
import {
  createRootRoute,
  createRoute,
  createRouter,
  lazyRouteComponent,
  notFound,
  Outlet,
  redirect,
  useRouter,
} from "@tanstack/react-router";
import { RouterProvider } from "react-aria-components";
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

/** HeroUI's and Perseus's links navigate through the router, so a click never reloads the page. */
function Root() {
  const router = useRouter();
  return (
    <RouterProvider
      navigate={(href) => void router.navigate({ href })}
      useHref={(href) => router.buildLocation({ to: href }).href}
    >
      <Outlet />
    </RouterProvider>
  );
}

const root = createRootRoute({ component: Root });

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

// Any path no route names: the Uncharted notice inside the console.
const uncharted = createRoute({
  getParentRoute: () => game,
  path: "$",
  component: Uncharted,
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
  // `?mission=true` opens the New mission stepper over the planet.
  validateSearch: (s): { mission?: true } => (s.mission === true ? { mission: true } : {}),
  component: lazyRouteComponent(() => import("./screens/PlanetPanel"), "PlanetPanel"),
});

// The panel's other states: it reads which one matched, so these render nothing themselves.
const buildingRoute = createRoute({
  getParentRoute: () => planet,
  path: "buildings/$buildingId",
  params: {
    parse: (p) => ({ buildingId: number(p.buildingId) }),
    stringify: (p) => ({ buildingId: p.buildingId.toString() }),
  },
});

const build = createRoute({
  getParentRoute: () => planet,
  path: "build",
});

const fleets = createRoute({
  getParentRoute: () => game,
  path: "/fleets",
  staticData: { title: titles.fleets },
  // `?send=<fleet>` opens the New mission stepper at the target, for a stationed fleet.
  validateSearch: (s): { send?: number } =>
    typeof s.send === "number" && Number.isSafeInteger(s.send) ? { send: s.send } : {},
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
    uncharted,
  ]),
]);

export const router = createRouter({ routeTree });
