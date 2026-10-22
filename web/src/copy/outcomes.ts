import type { components } from "../api/schema";
import { building, mass, mission, ship } from "../format";
import type { Order } from "../game/commands";

// What an accepted order did, in words (phase4-ux §5.3, §6.4). The first line comes from the
// order the player sent and never from an event: a 200's events are the re-solve's, possibly
// another fleet's.

type S = components["schemas"];

const no = (n: number) => `#${n.toString()}`;

/** Cargo as §6.4 reads it, leaving out a zero resource. */
function cargoWords(c: S["Cargo"]): string {
  const parts = [
    [c.iron_ore_g, "ore"],
    [c.iron_ingot_g, "ingots"],
  ] as const;
  const shown = parts.filter(([g]) => g !== 0).map(([g, what]) => `${mass(g)} of ${what}`);
  return shown.length > 0 ? shown.join(" and ") : `${mass(0)} of ore and ${mass(0)} of ingots`;
}

/** One line per event the command returned. */
export function eventLine(e: S["Event"]): string {
  switch (e.type) {
    case "building_completed":
      return `${building(e.building_type)} ${no(e.building_id)} on Planet ${no(e.planet_id)} is complete.`;
    case "ship_completed":
      return `A ${ship(e.ship_type)} (${no(e.ship_id)}) is ready on Planet ${no(e.planet_id)}'s roster.`;
    case "demolition_completed":
      return `${building(e.building_type)} ${no(e.building_id)} on Planet ${no(e.planet_id)} is demolished; its slot is free.`;
    case "fleet_arrived":
      return `Fleet ${no(e.fleet_id)} arrived at Planet ${no(e.planet_id)} (${mission(e.mission)}).`;
    case "planet_colonized":
      return `Planet ${no(e.planet_id)} is now yours.`;
    case "colonization_failed":
      return `Planet ${no(e.planet_id)} was taken first. Fleet ${no(e.fleet_id)} is stationed there with its cargo.`;
    case "cargo_unloaded":
      return `Unloaded ${cargoWords(e.cargo)} into Planet ${no(e.planet_id)}.`;
    case "fleet_removed":
      return `Fleet ${no(e.fleet_id)} used its last ship and is gone.`;
    case "fleet_returned":
      return `Fleet ${no(e.fleet_id)} is back at Planet ${no(e.planet_id)}.`;
  }
}

const NOTHING_UNLOADED = "Nothing was unloaded: the planet's storage is full. The cargo stays aboard.";

/**
 * What moved is what the form showed aboard minus what the fleet has after the unload; the
 * neutral line when there's no read after it, or the numbers don't fit.
 */
function unloadLine(fleet: number, planet: number, shown: S["Cargo"], now: S["Cargo"] | null): string {
  const neutral = `Fleet ${no(fleet)} unloaded what fit into Planet ${no(planet)}'s storage.`;
  if (!now || now.iron_ore_g > shown.iron_ore_g || now.iron_ingot_g > shown.iron_ingot_g) return neutral;
  const moved = {
    iron_ore_g: shown.iron_ore_g - now.iron_ore_g,
    iron_ingot_g: shown.iron_ingot_g - now.iron_ingot_g,
  };
  const none = (c: S["Cargo"]) => c.iron_ore_g === 0 && c.iron_ingot_g === 0;
  if (none(moved)) return none(shown) ? neutral : NOTHING_UNLOADED;
  const line = `Unloaded ${cargoWords(moved)} into Planet ${no(planet)}.`;
  return none(now) ? line : `${line} ${cargoWords(now)} stays aboard: the storage is full.`;
}

/** The success line for an order; an unload also needs what its fleet has aboard after it. */
export function doneLine(order: Order, aboardNow: S["Cargo"] | null = null): string {
  switch (order.type) {
    case "build_building":
      return `Construction of a ${building(order.payload.building_type)} started on Planet ${no(order.payload.planet_id)}. It draws ingots until it's complete.`;
    case "cancel_construction":
      return `Construction of ${building(order.building)} ${no(order.payload.building_id)} cancelled. Its slot is free; the ingots delivered are lost.`;
    case "demolish_building":
      return `${building(order.building)} ${no(order.payload.building_id)} is being demolished. It has stopped working; its slot frees when demolition ends.`;
    case "queue_ship":
      return `A ${ship(order.payload.ship_type)} was queued at Shipyard ${no(order.payload.shipyard_id)}.`;
    case "cancel_ship_build":
      return `Build ${no(order.payload.ship_build_id)} cancelled. The ingots delivered are lost.`;
    case "assemble_fleet": {
      const n = order.payload.ship_ids.length;
      return `A fleet of ${n.toString()} ${n === 1 ? "ship" : "ships"} was assembled at Planet ${no(order.payload.planet_id)}.`;
    }
    case "send_fleet":
      return `Fleet ${no(order.payload.fleet_id)} left for Planet ${no(order.payload.target_planet_id)} (${mission(order.payload.mission)}).`;
    case "recall_fleet":
      return `Fleet ${no(order.payload.fleet_id)} turned back to Planet ${no(order.origin)}.`;
    case "disband_fleet":
      return `Fleet ${no(order.payload.fleet_id)} disbanded. Its ships are on Planet ${no(order.planet)}'s roster.`;
    case "trigger_unload":
      return unloadLine(order.payload.fleet_id, order.planet, order.shown, aboardNow);
  }
}
