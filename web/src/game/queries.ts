import { useQueries } from "@tanstack/react-query";
import { api } from "../api/client";
import { useAuth } from "../auth/authStore";
import type { Timed } from "./live";
import { dueIn, refetchDelay } from "./refresh";

/** The leaderboard is rebuilt every 60 s (D-051), so it's due a minute after its `as_of`. */
const LEADERBOARD_EVERY_MS = 60_000;

interface QueryState<T> {
  state: { data: T | undefined; dataUpdatedAt: number; error: unknown };
}

/** Refetch at the time `due` names for the view, within D14's limits. */
function refetchAt<T extends Timed>(due: (view: T) => number | null) {
  return ({ state }: QueryState<T>) => {
    const at = state.data ? due(state.data) : null;
    const wait = state.data && at !== null ? dueIn(state.data.as_of, at, state.dataUpdatedAt, Date.now()) : null;
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

export const usePlanet = (id: number) => api.useQuery("get", "/planets/{id}", { params: { path: { id } } }, {
  enabled: useLoggedIn(),
  refetchInterval: atValidUntil,
});

export function usePlanets(ids: readonly number[]) {
  const enabled = useLoggedIn();
  return useQueries({ queries: ids.map((id) => ({ ...planetQuery(id), enabled })) });
}

export const useNear = (near: string, radius: number) =>
  api.useQuery("get", "/planets", { params: { query: { near, radius } } }, {
    enabled: useLoggedIn(),
    refetchInterval: atValidUntil,
  });

export const useLeaderboard = () =>
  api.useQuery("get", "/leaderboard", {}, {
    enabled: useLoggedIn(),
    refetchInterval: refetchAt((v) => Date.parse(v.as_of) + LEADERBOARD_EVERY_MS),
  });
