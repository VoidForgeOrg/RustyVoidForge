import { cleanup, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { useAuth } from "../auth/authStore";
import fleets from "../fixtures/fleets_bob.json";
import home from "../fixtures/planet_view_alice_home.json";
import { logIn, renderAt, serve as answer, wideWindow } from "../test-harness";

// openapi-fetch keeps the global fetch it finds at import, so the stub must exist first.
const fetchMock = vi.hoisted(() => {
  const mock = vi.fn<typeof fetch>();
  globalThis.fetch = mock;
  return mock;
});

const me = {
  as_of: "2026-10-21T12:00:00.000Z",
  valid_until: null,
  player_id: "01890a5d-ac96-774b-bcce-b302099a8057",
  homeworld_id: 63,
  planet_ids: [63, 26],
  score: 1_240,
};

const serve = (answers: Parameters<typeof answer>[1]) => {
  answer(fetchMock, answers);
};

beforeEach(() => {
  wideWindow();
  serve({ "/rules": Response.json({ name: "v0-fast" }) });
});

afterEach(() => {
  cleanup();
  useAuth.setState({ user: null });
  vi.unstubAllGlobals();
  globalThis.fetch = fetchMock;
});

test("logged out, the shell is the access panel alone, still naming the fast rules", async () => {
  renderAt("/planets");
  expect(await screen.findByRole("heading", { name: "Imperial Access" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Log in" })).toBeInTheDocument();
  expect(await screen.findByText("fast test rules")).toBeInTheDocument();
  expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
});

test("the nav has one Planets item, selected under a planet's panel too", async () => {
  logIn();
  serve({
    "/players/me": Response.json(me),
    "/players/me/fleets": Response.json({ as_of: me.as_of, valid_until: null, fleets: [] }),
  });
  renderAt("/planets/26/build");
  const nav = await screen.findByRole("navigation", { name: "Command", hidden: true });
  expect(within(nav).getByRole("link", { name: /^Planets/, hidden: true })).toHaveAttribute(
    "aria-current",
    "page",
  );
  expect(within(nav).queryByRole("link", { name: "#26", hidden: true })).not.toBeInTheDocument();
});

test("a busy server's read shows the retry frame with its wait", async () => {
  logIn();
  serve({
    "/players/me": () =>
      Response.json({ code: "busy", message: "" }, { status: 503, headers: { "Retry-After": "4" } }),
  });
  renderAt("/planets");
  expect(await screen.findByRole("heading", { name: "Transmission failed" })).toBeInTheDocument();
  expect(screen.getByText(/The game is busy\. Try again in 4 seconds\./)).toBeInTheDocument();
  expect(screen.getByText("(busy)")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
});

test("a planet number the API can't take is uncharted, before any call for it", async () => {
  logIn();
  serve({
    "/players/me": Response.json(me),
    "/players/me/fleets": Response.json({ as_of: me.as_of, valid_until: null, fleets: [] }),
  });
  renderAt("/planets/abc");
  expect(await screen.findByRole("heading", { name: "Uncharted coordinates" })).toBeInTheDocument();
  const asked = fetchMock.mock.calls.map(([r]) => new URL((r as Request).url).pathname);
  expect(asked.filter((p) => p.startsWith("/planets"))).toEqual([]);
});

test("the nav carries the holdings, the fleets and the account, and the top bar doesn't", async () => {
  logIn();
  serve({
    "/players/me": Response.json(me),
    "/players/me/fleets": Response.json(fleets),
    "/planets/63": Response.json({ ...home, planet_id: 63 }),
    "/planets/26": Response.json({ ...home, planet_id: 26 }),
  });
  renderAt("/leaderboard");
  const nav = await screen.findByRole("navigation", { name: "Command" });
  expect(
    await within(nav).findByRole("link", { name: "Planets: 2 planets, 6 of 16 slots used" }),
  ).toHaveTextContent("2 · 6/16");
  expect(within(nav).getByRole("link", { name: "Fleets: 1 in flight, 0 stationed" })).toHaveTextContent(
    "1 flying · 0 stationed",
  );
  expect(within(nav).getByText("alice@voidforge.test")).toBeInTheDocument();
  const bar = screen.getByRole("banner");
  expect(within(bar).getByRole("link", { name: /^Score 1,240/ })).toBeInTheDocument();
  expect(within(bar).queryByRole("link", { name: /^(Holdings|Fleets)/ })).not.toBeInTheDocument();
  expect(within(bar).queryByText("alice@voidforge.test")).not.toBeInTheDocument();
});
