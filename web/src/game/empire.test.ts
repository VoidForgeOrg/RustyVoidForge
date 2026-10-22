import { expect, test } from "vitest";
import fleets from "../fixtures/fleets_bob.json";
import home from "../fixtures/planet_view_alice_home.json";
import type { components } from "../api/schema";
import { next, total } from "./empire";

type S = components["schemas"];
const planet = home as S["PlanetView"];
const second: S["PlanetView"] = {
  ...planet,
  planet_id: 26,
  buildings: [],
  stocks: {
    iron_ore: { amount_g: 1_000_000_000, cap_g: 10_000_000_000, rate_g_per_h: 0 },
    iron_ingot: { amount_g: 0, cap_g: 5_000_000_000, rate_g_per_h: 0 },
  },
};

test("the bar's totals sum every planet ticked to now, and Next is the soonest completion", () => {
  const arrived = 1_000_000;
  const planets = [
    { view: planet, receivedAt: arrived },
    { view: second, receivedAt: arrived },
  ];
  // An hour on: home's ore +2 t and ingots −34 t.
  const ore = total(planets, "iron_ore", arrived + 3_600_000);
  expect([ore.amountG, ore.capG, ore.rateGPerH]).toEqual([3_002_000_000, 20_000_000_000, 2_000_000]);
  expect(total(planets, "iron_ingot", arrived + 3_600_000).amountG).toBe(1_466_000_000);

  // The Drill completes three hours after as_of; bob's fleet lands after one.
  expect(next(planets, null)).toEqual({
    kind: "building",
    label: "Drill on #1",
    at: arrived + 3 * 3_600_000,
    planetId: 1,
    buildingId: 8,
  });
  expect(next(planets, { view: fleets as S["FleetsView"], receivedAt: arrived })).toEqual({
    kind: "fleet",
    label: "Fleet 11 at #7",
    at: arrived + 3_600_000,
    fleetId: 11,
  });
});
