import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import near from "../../../crates/voidforge-web/tests/fixtures/planets_near.json";
import home from "../../../crates/voidforge-web/tests/fixtures/planet_view_alice_home.json";
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
const rules = {
  name: "v0",
  buildings: {
    drill: { cost_g: 150_000_000, build_time_ms: 10_800_000, demolition_time_ms: 3_600_000 },
    refinery: { cost_g: 250_000_000, build_time_ms: 14_400_000, demolition_time_ms: 3_600_000 },
    shipyard: { cost_g: 1_200_000_000, build_time_ms: 43_200_000, demolition_time_ms: 7_200_000 },
  },
  ships: {
    colony_ship: {
      cost_g: 800_000_000,
      build_time_ms: 36_000_000,
      speed_du_per_h: 10_000,
      cargo_capacity_g: 0,
    },
    cargo_vessel: {
      cost_g: 300_000_000,
      build_time_ms: 14_400_000,
      speed_du_per_h: 15_000,
      cargo_capacity_g: 500_000_000,
    },
  },
};
const shipyard = { id: 15, type: "shipyard", state: "operational", status: "running" };
const accepted = () => Response.json({ seq: 5, game_time: home.as_of, events: [] });
const refused = (status: number, code: string) => Response.json({ code, message: "" }, { status });

/** The orders sent: each one's body and idempotency key. */
const sent: { body: { type: string; payload: unknown }; key: string | null }[] = [];

function answer(extra: Record<string, Response | ((r: Request) => Response | Promise<Response>)> = {}) {
  serve(fetchMock, {
    "/players/me": Response.json(me),
    "/players/me/fleets": Response.json({ as_of: home.as_of, valid_until: null, fleets: [] }),
    "/planets/1": Response.json({ ...home, buildings: [...home.buildings, shipyard] }),
    "/rules": Response.json(rules),
    ...extra,
  });
}

/** Answers each order in turn, recording what was sent. */
function orders(...answers: (() => Response | Promise<Response>)[]) {
  return async (request: Request) => {
    sent.push({
      body: (await request.json()) as { type: string; payload: unknown },
      key: request.headers.get("Idempotency-Key"),
    });
    const next = answers.shift();
    return next ? next() : accepted();
  };
}

const gets = (path: string) =>
  fetchMock.mock.calls.filter(
    ([r]) => (r as Request).method === "GET" && new URL((r as Request).url).pathname === path,
  ).length;

beforeEach(() => {
  wideWindow();
  logIn();
  sent.length = 0;
});

afterEach(() => {
  cleanup();
  useAuth.setState({ user: null });
  vi.unstubAllGlobals();
  globalThis.fetch = fetchMock;
});

test("an order keeps its key for a retry, and takes a fresh one once the order is settled", async () => {
  answer({
    "/commands": orders(
      () => refused(503, "unavailable"),
      accepted,
      () => refused(409, "shipyard_not_operational"),
      accepted,
    ),
  });
  renderAt("/planets/1/buildings/15");
  const drawer = await screen.findByRole("dialog", { name: "#1 Home · Shipyard" });
  fireEvent.click(await within(drawer).findByRole("button", { name: "Queue Colony Ship" }));
  fireEvent.click(await within(drawer).findByRole("button", { name: "Try again" }));
  expect(await screen.findByText("A Colony Ship was queued at Shipyard #15.")).toBeInTheDocument();
  const before = gets("/planets/1");
  fireEvent.click(within(drawer).getByRole("button", { name: "Queue Colony Ship" }));
  expect(await within(drawer).findByText(/That Shipyard isn't working/)).toBeInTheDocument();
  fireEvent.click(within(drawer).getByRole("button", { name: "Queue Colony Ship" }));
  await waitFor(() => {
    expect(sent).toHaveLength(4);
  });
  const keys = sent.map((s) => s.key);
  expect(keys[1]).toBe(keys[0]);
  expect(new Set(keys).size).toBe(3);
  expect(sent[0]?.body).toEqual({
    type: "queue_ship",
    payload: { shipyard_id: 15, ship_type: "colony_ship" },
  });
  await waitFor(() => {
    expect(gets("/planets/1")).toBeGreaterThan(before);
  });
});

test("an order in flight can't be sent again", async () => {
  let release = () => undefined as unknown;
  answer({
    "/commands": orders(
      () =>
        new Promise<Response>((resolve) => {
          release = () => {
            resolve(accepted());
          };
        }),
    ),
  });
  renderAt("/planets/1/build");
  const drawer = await screen.findByRole("dialog", { name: "#1 Home · Free slot" });
  const build = await within(drawer).findByRole("button", { name: "Build Drill" });
  fireEvent.click(build);
  await waitFor(() => {
    expect(sent).toHaveLength(1);
  });
  fireEvent.click(build);
  release();
  await screen.findByText(/Construction of a Drill started on Planet #1/);
  expect(sent).toHaveLength(1);
});

test("an irreversible order asks first, and Keep it sends nothing", async () => {
  answer({ "/commands": orders() });
  renderAt("/planets/1/buildings/2");
  const drawer = await screen.findByRole("dialog", { name: "#1 Home · Drill" });
  fireEvent.click(await within(drawer).findByRole("button", { name: "Demolish Drill #2" }));
  let ask = await screen.findByRole("alertdialog", { name: "Demolish Drill #2?" });
  expect(within(ask).getByText(/The slot frees in 1 h, when demolition ends\./)).toBeInTheDocument();
  fireEvent.click(within(ask).getByRole("button", { name: "Keep it" }));
  expect(sent).toHaveLength(0);
  // jsdom never ends the closing animation, so the dialog closed above stays in the tree.
  fireEvent.click(within(drawer).getByRole("button", { name: "Demolish Drill #2" }));
  ask = (await screen.findAllByRole("alertdialog", { name: "Demolish Drill #2?" })).at(-1) as HTMLElement;
  fireEvent.click(within(ask).getByRole("button", { name: "Demolish Drill #2" }));
  await screen.findByText(/Drill #2 is being demolished/);
  expect(sent.map((s) => s.body)).toEqual([{ type: "demolish_building", payload: { building_id: 2 } }]);
});

test("a Shipyard with builds offers no Demolish, and says why", async () => {
  const build = {
    id: 20,
    shipyard_id: 15,
    ship_type: "colony_ship",
    state: "waiting",
    cost_g: 800_000_000,
    delivered_g: 0,
    rate_g_per_h: 0,
    eta: null,
    status: null,
  };
  answer({
    "/planets/1": Response.json({ ...home, buildings: [...home.buildings, shipyard], ship_builds: [build] }),
  });
  renderAt("/planets/1/buildings/15");
  const drawer = await screen.findByRole("dialog", { name: "#1 Home · Shipyard" });
  expect(await within(drawer).findByRole("button", { name: "Demolish Shipyard #15" })).toBeDisabled();
  expect(within(drawer).getByText(/Cancel them before demolishing it\./)).toBeInTheDocument();
});

test("the mission stepper checks its ships and cargo, assembles, and retries only a failed send", async () => {
  const roster = [
    { ship_id: 30, ship_type: "colony_ship", owner: alice },
    { ship_id: 31, ship_type: "cargo_vessel", owner: alice },
  ];
  let assembled = false;
  const fleet = {
    state: "stationed",
    fleet_id: 40,
    at_planet_id: 1,
    ships: roster.map(({ ship_id, ship_type }) => ({ ship_id, ship_type })),
    cargo: { iron_ore_g: 0, iron_ingot_g: 1_000_000 },
    capacity_g: 500_000_000,
    speed_du_per_h: 10_000,
  };
  answer({
    "/planets/1": Response.json({ ...home, roster }),
    "/planets": Response.json(near),
    "/players/me/fleets": () =>
      Response.json({ as_of: home.as_of, valid_until: null, fleets: assembled ? [fleet] : [] }),
    "/commands": orders(
      () => {
        assembled = true;
        return accepted();
      },
      () => refused(503, "unavailable"),
    ),
  });
  renderAt("/planets/1?mission=true");
  const stepper = await screen.findByRole("dialog", { name: "New mission" });
  const next = await within(stepper).findByRole("button", { name: "Next" });
  expect(next).toBeDisabled();
  expect(within(stepper).getByText("Pick at least one ship.")).toBeInTheDocument();

  fireEvent.click(within(stepper).getByRole("checkbox", { name: "Colony Ship #30" }));
  fireEvent.change(within(stepper).getByRole("textbox", { name: "Ingots (t)" }), {
    target: { value: "1.5x" },
  });
  expect(within(stepper).getByText("Enter cargo in tonnes, like 250 or 12.5.")).toBeInTheDocument();
  fireEvent.change(within(stepper).getByRole("textbox", { name: "Ingots (t)" }), { target: { value: "1" } });
  expect(within(stepper).getByText(/Those ships can't carry that much/)).toBeInTheDocument();
  fireEvent.click(within(stepper).getByRole("checkbox", { name: "Cargo Vessel #31" }));
  expect(next).toBeEnabled();
  fireEvent.click(next);

  expect(await within(stepper).findByRole("radio", { name: /Colonize/ })).toBeEnabled();
  fireEvent.click(await within(stepper).findByRole("option", { name: /^#7 · unowned/ }));
  fireEvent.click(within(stepper).getByRole("button", { name: "Next" }));
  fireEvent.click(await within(stepper).findByRole("button", { name: "Send" }));

  expect(await within(stepper).findByText(/The game is starting up or catching up/)).toBeInTheDocument();
  expect(within(stepper).getByText("Fleet #40 is assembled and stationed at Planet #1.")).toBeInTheDocument();
  fireEvent.click(within(stepper).getByRole("button", { name: "Try again" }));
  await waitFor(() => {
    expect(sent).toHaveLength(3);
  });
  expect(sent.map((s) => s.body.type)).toEqual(["assemble_fleet", "send_fleet", "send_fleet"]);
  expect(sent[0]?.body.payload).toEqual({
    planet_id: 1,
    ship_ids: [30, 31],
    cargo: { iron_ore_g: 0, iron_ingot_g: 1_000_000 },
  });
  expect(sent[2]?.body.payload).toEqual({ fleet_id: 40, target_planet_id: 7, mission: "colonize" });
  expect(sent[2]?.key).toBe(sent[1]?.key);
});
