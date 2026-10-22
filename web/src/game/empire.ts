import type { components } from "../api/schema";
import { building, ship } from "../format";
import { grown, ratedFor } from "./live";

type S = components["schemas"];
type Stock = S["Stock"];

/** A view with the browser time it arrived (TanStack's dataUpdatedAt). */
export interface Received<T> {
  view: T;
  receivedAt: number;
}

export type Resource = "iron_ore" | "iron_ingot";

interface PlanetStock {
  planetId: number;
  stock: Stock;
}

interface Total {
  amountG: number;
  capG: number;
  rateGPerH: number;
  planets: PlanetStock[];
}

/** A resource summed over the owned planets, each ticked to now within its cap. */
export function total(planets: Received<S["PlanetView"]>[], resource: Resource, now: number): Total {
  const each = planets.map(({ view, receivedAt }) => {
    const s = view.stocks[resource];
    const amount = grown(s.amount_g, s.rate_g_per_h, ratedFor(view, receivedAt, now), s.cap_g);
    return { planetId: view.planet_id, stock: { ...s, amount_g: amount } };
  });
  const sum = (f: (s: Stock) => number) => each.reduce((n, p) => n + f(p.stock), 0);
  return {
    amountG: sum((s) => s.amount_g),
    capG: sum((s) => s.cap_g),
    rateGPerH: sum((s) => s.rate_g_per_h),
    planets: each,
  };
}

/** The player's planets as every list shows them: the homeworld, then by number. */
export const homeworldFirst = (planets: readonly number[], homeworld: number) =>
  [...planets].sort((a, b) => Number(b === homeworld) - Number(a === homeworld) || a - b);

export type Next =
  | { kind: "building"; label: string; at: number; planetId: number; buildingId: number }
  | { kind: "fleet"; label: string; at: number; fleetId: number };

/** A game time as browser time, through the view it came from. */
const local = (t: string, view: { as_of: string }, receivedAt: number) =>
  Date.parse(t) - Date.parse(view.as_of) + receivedAt;

/** The soonest completion across the empire: a building, a demolition, a ship build or an arrival. */
export function next(
  planets: Received<S["PlanetView"]>[],
  fleets: Received<S["FleetsView"]> | null,
): Next | null {
  const all: Next[] = [];
  for (const { view, receivedAt } of planets) {
    const on = `#${view.planet_id.toString()}`;
    const at = (label: string, t: string, buildingId: number) => {
      all.push({
        kind: "building",
        label,
        at: local(t, view, receivedAt),
        planetId: view.planet_id,
        buildingId,
      });
    };
    for (const b of view.buildings) {
      if (b.state === "under_construction" && b.eta) at(`${building(b.type)} on ${on}`, b.eta, b.id);
      if (b.state === "demolishing") at(`Demolition on ${on}`, b.demolition_ends_at, b.id);
    }
    for (const s of view.ship_builds) {
      if (s.state === "active" && s.eta) at(`${ship(s.ship_type)} on ${on}`, s.eta, s.shipyard_id);
    }
  }
  for (const f of fleets?.view.fleets ?? []) {
    if (!fleets || f.state === "stationed") continue;
    const to = f.state === "in_transit" ? f.target_planet_id : f.origin_planet_id;
    const label = `Fleet ${f.fleet_id.toString()} at #${to.toString()}`;
    all.push({
      kind: "fleet",
      label,
      at: local(f.arrives_at, fleets.view, fleets.receivedAt),
      fleetId: f.fleet_id,
    });
  }
  return all.reduce<Next | null>((soonest, n) => (soonest === null || n.at < soonest.at ? n : soonest), null);
}
