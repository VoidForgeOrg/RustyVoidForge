import { cleanup, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { useAuth } from "../auth/authStore";
import fleets from "../fixtures/fleets_bob.json";
import home from "../fixtures/planet_view_alice_home.json";
import { logIn, renderAt, serve, wideWindow } from "../test-harness";

// openapi-fetch keeps the global fetch it finds at import, so the stub must exist first.
const fetchMock = vi.hoisted(() => {
  const mock = vi.fn<typeof fetch>();
  globalThis.fetch = mock;
  return mock;
});

const me = {
  as_of: fleets.as_of,
  valid_until: null,
  player_id: "01890a5d-ac96-774b-bcce-b302099a8057",
  homeworld_id: 4,
  planet_ids: [4],
  score: 1_240,
};

beforeEach(() => {
  wideWindow();
  logIn();
});

afterEach(() => {
  cleanup();
  useAuth.setState({ user: null });
  vi.unstubAllGlobals();
  globalThis.fetch = fetchMock;
});

const [flying] = fleets.fleets;

test.each([
  ["stationed", [{ ...flying, state: "stationed", at_planet_id: flying?.target_planet_id }]],
  ["gone, its only ship consumed by the colony", []],
])("a fleet's arrival, %s, refetches its target planet's view", async (_, after) => {
  const target = flying?.target_planet_id ?? 0;
  const path = `/planets/${target.toString()}`;
  const targetReads = () =>
    fetchMock.mock.calls.filter(([r]) => new URL((r as Request).url).pathname === path).length;
  let reads = 0;
  let before = Infinity;
  serve(fetchMock, {
    "/players/me": Response.json(me),
    "/rules": Response.json({ name: "v0" }),
    "/planets/4": Response.json({ ...home, planet_id: 4, owner: me.player_id, valid_until: null }),
    [path]: Response.json({ ...home, planet_id: target, owner: null, valid_until: null }),
    "/players/me/fleets": () => {
      reads += 1;
      if (reads === 2) before = targetReads();
      return Response.json(
        reads === 1
          ? { ...fleets, valid_until: new Date(Date.parse(fleets.as_of) + 1).toISOString() }
          : { as_of: fleets.as_of, valid_until: null, fleets: after },
      );
    },
  });
  renderAt(path);
  await waitFor(
    () => {
      expect(targetReads()).toBeGreaterThan(before);
    },
    { timeout: 5_000 },
  );
});
