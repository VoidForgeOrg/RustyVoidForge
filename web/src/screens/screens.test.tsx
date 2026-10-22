import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import near from "../fixtures/planets_near.json";
import home from "../fixtures/planet_view_alice_home.json";
import { useAuth } from "../auth/authStore";
import { logIn, renderAt, serve, wideWindow } from "../test-harness";

// openapi-fetch keeps the global fetch it finds at import, so the stub must exist first.
const fetchMock = vi.hoisted(() => {
  const mock = vi.fn<typeof fetch>();
  globalThis.fetch = mock;
  return mock;
});

const alice = home.owner;
const me = {
  as_of: home.as_of,
  valid_until: null,
  player_id: alice,
  homeworld_id: 1,
  planet_ids: [1],
  score: 0,
};
const noFleets = { as_of: home.as_of, valid_until: null, fleets: [] };

function answer(extra: Record<string, Response> = {}) {
  serve(fetchMock, {
    "/players/me": Response.json(me),
    "/players/me/fleets": Response.json(noFleets),
    "/planets/1": Response.json(home),
    "/rules": Response.json({ name: "v0" }),
    ...extra,
  });
}

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

test("a lone planet opens expanded: its buildings by type and status, then its free slots", async () => {
  answer();
  renderAt("/planets");
  const slots = await screen.findByRole("list", { name: "Building slots" });
  const names = within(slots)
    .getAllByRole("link")
    .map((a) => a.getAttribute("aria-label"));
  expect(names).toEqual([
    "Drill, working",
    "Refinery, working",
    "Drill, constructing 0%",
    ...Array<string>(5).fill("Free slot"),
  ]);
  expect(within(slots).getByRole("link", { name: "Drill, constructing 0%" })).toHaveAttribute(
    "href",
    "/planets/1/buildings/8",
  );
  for (const free of within(slots).getAllByRole("link", { name: "Free slot" })) {
    expect(free).toHaveAttribute("href", "/planets/1/build");
  }
});

test("a demolishing building's drawer counts down to the slot freeing", async () => {
  const demolishing = {
    ...home,
    buildings: [
      home.buildings[0],
      {
        id: 3,
        type: "refinery",
        state: "demolishing",
        status: null,
        demolition_ends_at: "2025-10-09T08:55:20.000Z",
      },
    ],
  };
  answer({ "/planets/1": Response.json(demolishing) });
  renderAt("/planets/1/buildings/3");
  const drawer = await screen.findByRole("dialog", { name: "#1 Home · Refinery" });
  expect(await within(drawer).findByText("demolishing")).toBeInTheDocument();
  expect(within(drawer).getByText(/08:55:20 UTC \(in 2 min\)/)).toBeInTheDocument();
});

test("a building the planet doesn't have, or a build on a full planet, is uncharted", async () => {
  const full = { ...home, slots: 3 };
  answer({ "/planets/1": Response.json(full) });
  renderAt("/planets/1/buildings/99");
  expect(await screen.findByRole("heading", { name: "Uncharted coordinates" })).toBeInTheDocument();
  cleanup();
  renderAt("/planets/1/build");
  const drawer = await screen.findByRole("dialog", { name: "#1 Home · Free slot" });
  expect(await within(drawer).findByRole("heading", { name: "Uncharted coordinates" })).toBeInTheDocument();
});

test("another player's planet is read-only, with no roster", async () => {
  answer({
    "/planets/4": Response.json({ ...home, planet_id: 4, owner: near.planets[1]?.owner, roster: [] }),
  });
  renderAt("/planets/4");
  const card = await screen.findByRole("article", { name: "#4" });
  expect(await within(card).findByText("player 7c8d9e10")).toBeInTheDocument();
  expect(within(card).queryByText("Ships here")).not.toBeInTheDocument();
});

test("the star charts list planets nearest first and name the unowned", async () => {
  answer({ "/planets": Response.json(near) });
  renderAt("/universe");
  const table = await screen.findByRole("grid", { name: "Planets in range, nearest first" });
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows.map((r) => [...r.children].map((c) => c.textContent))).toEqual([
    ["#7", "unowned", "(0.5, 0.0, 0.0)", "0.0 u"],
    ["#1", "you", "(0.0, 0.0, 0.0)", "0.5 u"],
    ["#4", "player 7c8d9e10", "(1.0, 0.0, 0.0)", "0.5 u"],
  ]);
});

test("a nav click navigates in the app, keeping what's already read", async () => {
  answer();
  renderAt("/planets");
  await screen.findByRole("list", { name: "Building slots" });
  const reads = () =>
    fetchMock.mock.calls.filter(([r]) => new URL((r as Request).url).pathname === "/players/me").length;
  const before = reads();
  const nav = screen.getByRole("navigation", { name: "Command" });
  fireEvent.click(within(nav).getByRole("link", { name: /^Fleets/ }));
  expect(await screen.findByRole("heading", { name: "Fleet Command" })).toBeInTheDocument();
  expect(reads()).toBe(before);
});

test("a planet that doesn't answer leaves the top bar partial, not blank", async () => {
  answer({ "/players/me": Response.json({ ...me, planet_ids: [1, 26] }) });
  renderAt("/fleets");
  const bar = await screen.findByRole("banner");
  const partial = "partial: Planet #26 didn't answer";
  expect(
    await within(bar).findByRole("button", { name: `Iron Ore: 2,000.0 t in storage, +2.0 t/h, ${partial}` }),
  ).toBeInTheDocument();
  expect(
    within(bar).getByRole("link", { name: new RegExp(`^Holdings: 2 planets, .*, ${partial}$`) }),
  ).toBeInTheDocument();
});
