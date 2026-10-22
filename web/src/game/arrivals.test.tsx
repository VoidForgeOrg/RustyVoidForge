import { cleanup, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { useAuth } from "../auth/authStore";
import fleets from "../fixtures/fleets_bob.json";
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

test("a fleet's arrival refetches the player, whose valid_until doesn't foresee a new colony", async () => {
  const [flying] = fleets.fleets;
  const meReads = () =>
    fetchMock.mock.calls.filter(([r]) => new URL((r as Request).url).pathname === "/players/me").length;
  let reads = 0;
  let meBefore = Infinity;
  serve(fetchMock, {
    "/players/me": Response.json(me),
    "/rules": Response.json({ name: "v0" }),
    "/players/me/fleets": () => {
      reads += 1;
      if (reads === 2) meBefore = meReads();
      return Response.json(
        reads === 1
          ? { ...fleets, valid_until: new Date(Date.parse(fleets.as_of) + 1).toISOString() }
          : {
              as_of: fleets.as_of,
              valid_until: null,
              fleets: [{ ...flying, state: "stationed", at_planet_id: flying?.target_planet_id }],
            },
      );
    },
  });
  renderAt("/leaderboard");
  await waitFor(
    () => {
      expect(meReads()).toBeGreaterThan(meBefore);
    },
    { timeout: 5_000 },
  );
});
