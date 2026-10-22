import { useQueries, useQuery } from "@tanstack/react-query";
import { api } from "../api/client";
import type { components } from "../api/schema";
import { useAuth } from "../auth/authStore";
import type { Timed } from "./live";
import { dueIn, refetchDelay } from "./refresh";

/** The leaderboard is rebuilt every 60 s (D-051), so it's due a minute after its `as_of`. */
const LEADERBOARD_EVERY_MS = 60_000;

interface QueryState<T> {
  state: { data: T | undefined; dataUpdatedAt: number; error: unknown };
}

/** Refetch at the time `due` names for the view, within the read budget's limits. */
function refetchAt<T extends Timed>(due: (view: T) => number | null) {
  return ({ state }: QueryState<T>) => {
    const at = state.data ? due(state.data) : null;
    const wait =
      state.data && at !== null ? dueIn(state.data.as_of, at, state.dataUpdatedAt, Date.now()) : null;
    return refetchDelay(wait, state.error);
  };
}

const atValidUntil = refetchAt<Timed>((v) => (v.valid_until ? Date.parse(v.valid_until) : null));

const useLoggedIn = () => useAuth((s) => s.user !== null);

export const useRules = () => api.useQuery("get", "/rules", {}, { staleTime: Infinity });

export const useMe = () =>
  api.useQuery("get", "/players/me", {}, { enabled: useLoggedIn(), refetchInterval: atValidUntil });

export const useFleets = () =>
  api.useQuery("get", "/players/me/fleets", {}, { enabled: useLoggedIn(), refetchInterval: atValidUntil });

const planetQuery = (id: number) =>
  api.queryOptions("get", "/planets/{id}", { params: { path: { id } } }, { refetchInterval: atValidUntil });

export function usePlanet(id: number) {
  return useQuery({ ...planetQuery(id), enabled: useLoggedIn() });
}

export function usePlanets(ids: readonly number[]) {
  const enabled = useLoggedIn();
  return useQueries({ queries: ids.map((id) => ({ ...planetQuery(id), enabled })) });
}

const useNear = (near: string, radius: number, ready: boolean) =>
  api.useQuery(
    "get",
    "/planets",
    { params: { query: { near, radius } } },
    {
      enabled: useLoggedIn() && ready,
      refetchInterval: atValidUntil,
    },
  );

/** Planets within `radiusDu` of the planet `center`, nearest first. */
export const nearestFirst = (planets: readonly components["schemas"]["PlanetSummary"][]) =>
  [...planets].sort((a, b) => a.distance - b.distance || a.planet_id - b.planet_id);

export function useChart(center: number, radiusDu: number) {
  const planet = usePlanet(center);
  const p = planet.data?.position;
  const near = useNear(
    p ? `${p.x.toString()},${p.y.toString()},${p.z.toString()}` : "",
    radiusDu,
    p !== undefined,
  );
  return { planet, near, rows: near.data && nearestFirst(near.data.planets) };
}

export const useLeaderboard = () =>
  api.useQuery(
    "get",
    "/leaderboard",
    {},
    {
      enabled: useLoggedIn(),
      refetchInterval: refetchAt((v) => Date.parse(v.as_of) + LEADERBOARD_EVERY_MS),
    },
  );
