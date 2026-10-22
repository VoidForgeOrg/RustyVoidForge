import { expect, test } from "vitest";
import type { components } from "../api/schema";
import type { Order } from "../game/commands";
import { doneLine, eventLine } from "./outcomes";

type Cargo = components["schemas"]["Cargo"];
const cargo = (ore: number, ingots: number): Cargo => ({ iron_ore_g: ore, iron_ingot_g: ingots });
const unload = (shown: Cargo): Order => ({
  type: "trigger_unload",
  payload: { fleet_id: 8 },
  planet: 4,
  shown,
});

test("an unload says what moved: shown minus what's aboard after it", () => {
  const neutral = "Fleet #8 unloaded what fit into Planet #4's storage.";
  const full = "Nothing was unloaded: the planet's storage is full. The cargo stays aboard.";
  const table: [Cargo, Cargo | null, string][] = [
    [cargo(0, 500_000_000), cargo(0, 0), "Unloaded 500.0 t of ingots into Planet #4."],
    [
      cargo(250_000_000, 500_000_000),
      cargo(0, 0),
      "Unloaded 250.0 t of ore and 500.0 t of ingots into Planet #4.",
    ],
    [
      cargo(0, 500_000_000),
      cargo(0, 20_000_000),
      "Unloaded 480.0 t of ingots into Planet #4. 20.0 t of ingots stays aboard: the storage is full.",
    ],
    [
      cargo(250_000_000, 500_000_000),
      cargo(250_000_000, 0),
      "Unloaded 500.0 t of ingots into Planet #4. 250.0 t of ore stays aboard: the storage is full.",
    ],
    [cargo(0, 500_000_000), cargo(0, 500_000_000), full],
    [cargo(0, 500_000_000), null, neutral],
    [cargo(0, 500_000_000), cargo(1, 500_000_000), neutral],
    [cargo(0, 500_000_000), cargo(0, 500_000_001), neutral],
    [cargo(0, 0), cargo(0, 0), neutral],
  ];
  for (const [shown, now, line] of table) expect(doneLine(unload(shown), now)).toBe(line);
});

test("each order's line is §5.3's", () => {
  const table: [Order, string][] = [
    [
      { type: "build_building", payload: { planet_id: 4, building_type: "shipyard" } },
      "Construction of a Shipyard started on Planet #4. It draws ingots until it's complete.",
    ],
    [
      { type: "cancel_construction", payload: { building_id: 15 }, planet: 4, building: "shipyard" },
      "Construction of Shipyard #15 cancelled. Its slot is free; the ingots delivered are lost.",
    ],
    [
      { type: "demolish_building", payload: { building_id: 12 }, planet: 4, building: "drill" },
      "Drill #12 is being demolished. It has stopped working; its slot frees when demolition ends.",
    ],
    [
      { type: "queue_ship", payload: { shipyard_id: 15, ship_type: "colony_ship" }, planet: 4 },
      "A Colony Ship was queued at Shipyard #15.",
    ],
    [
      { type: "cancel_ship_build", payload: { ship_build_id: 20 }, planet: 4 },
      "Build #20 cancelled. The ingots delivered are lost.",
    ],
    [
      { type: "assemble_fleet", payload: { planet_id: 4, ship_ids: [1, 2], cargo: cargo(0, 0) } },
      "A fleet of 2 ships was assembled at Planet #4.",
    ],
    [
      { type: "assemble_fleet", payload: { planet_id: 4, ship_ids: [1], cargo: cargo(0, 0) } },
      "A fleet of 1 ship was assembled at Planet #4.",
    ],
    [
      { type: "send_fleet", payload: { fleet_id: 7, target_planet_id: 12, mission: "transport" } },
      "Fleet #7 left for Planet #12 (Transport).",
    ],
    [{ type: "recall_fleet", payload: { fleet_id: 8 }, origin: 4 }, "Fleet #8 turned back to Planet #4."],
    [
      { type: "disband_fleet", payload: { fleet_id: 8 }, planet: 9 },
      "Fleet #8 disbanded. Its ships are on Planet #9's roster.",
    ],
  ];
  for (const [order, line] of table) expect(doneLine(order)).toBe(line);
});

test("every event has its line", () => {
  const events: components["schemas"]["Event"][] = [
    { type: "building_completed", planet_id: 4, building_id: 15, building_type: "shipyard" },
    { type: "ship_completed", planet_id: 4, shipyard_id: 15, ship_id: 30, ship_type: "colony_ship" },
    { type: "demolition_completed", planet_id: 4, building_id: 12, building_type: "drill" },
    { type: "fleet_arrived", fleet_id: 8, planet_id: 12, mission: "colonize" },
    {
      type: "planet_colonized",
      planet_id: 12,
      owner: "01890a5d-ac96-774b-bcce-b302099a8057",
      fleet_id: 8,
      ship_id: 30,
    },
    { type: "colonization_failed", planet_id: 12, fleet_id: 8 },
    { type: "cargo_unloaded", planet_id: 12, fleet_id: 8, cargo: cargo(250_000_000, 500_000_000) },
    { type: "fleet_removed", fleet_id: 8, planet_id: 12 },
    { type: "fleet_returned", fleet_id: 9, planet_id: 4 },
  ];
  expect(events.map(eventLine)).toEqual([
    "Shipyard #15 on Planet #4 is complete.",
    "A Colony Ship (#30) is ready on Planet #4's roster.",
    "Drill #12 on Planet #4 is demolished; its slot is free.",
    "Fleet #8 arrived at Planet #12 (Colonize).",
    "Planet #12 is now yours.",
    "Planet #12 was taken first. Fleet #8 is stationed there with its cargo.",
    "Unloaded 250.0 t of ore and 500.0 t of ingots into Planet #12.",
    "Fleet #8 used its last ship and is gone.",
    "Fleet #9 is back at Planet #4.",
  ]);
});
