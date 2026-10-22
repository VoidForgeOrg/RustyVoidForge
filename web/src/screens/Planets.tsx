import { Link, Outlet } from "@tanstack/react-router";
import { duration, rate, stock } from "../format";
import { homeworldFirst, next } from "../game/empire";
import { grown } from "../game/live";
import { useFleets, useMe, usePlanet } from "../game/queries";
import { dueIn } from "../game/refresh";
import { useNow } from "../game/useNow";
import { Read } from "../shell/Failure";
import { Meter, Panel, type PlanetView, planetName, useLive } from "./parts";

/** The holdings grid: one card per planet, each opening its side panel. */
export function Planets() {
  const me = useMe().data;
  if (!me) return null;
  return (
    <>
      <div className="grid grid-cols-[repeat(auto-fill,minmax(16rem,1fr))] gap-4">
        {homeworldFirst(me.planet_ids, me.homeworld_id).map((id) => (
          <PlanetCard key={id} id={id} homeworld={me.homeworld_id} />
        ))}
      </div>
      <Outlet />
    </>
  );
}

function PlanetCard({ id, homeworld }: { id: number; homeworld: number }) {
  const planet = usePlanet(id);
  return (
    <article aria-label={planetName(id, homeworld)}>
      <Read query={planet} fill={{ planet: id }} loading={`Loading planet ${id.toString()}`}>
        {(view) => <Summary view={view} receivedAt={planet.dataUpdatedAt} homeworld={homeworld} />}
      </Read>
    </article>
  );
}

/** A planet at a glance: stocks, slots, what's running and what finishes first. */
function Summary({
  view,
  receivedAt,
  homeworld,
}: {
  view: PlanetView;
  receivedAt: number;
  homeworld: number;
}) {
  const { ran } = useLive(view, receivedAt);
  const now = useNow();
  const name = planetName(view.planet_id, homeworld);
  const constructing = view.buildings.filter((b) => b.state === "under_construction").length;
  const demolishing = view.buildings.filter((b) => b.state === "demolishing").length;
  const running = [
    constructing > 0 && `${constructing.toString()} constructing`,
    demolishing > 0 && `${demolishing.toString()} demolishing`,
    view.ship_builds.length > 0 &&
      `${view.ship_builds.length.toString()} ship build${view.ship_builds.length === 1 ? "" : "s"}`,
  ].filter((part) => part !== false);
  const stocks = (["iron_ore", "iron_ingot"] as const).map((r) => {
    const s = view.stocks[r];
    const amount = grown(s.amount_g, s.rate_g_per_h, ran, s.cap_g);
    return `${r === "iron_ore" ? "Iron Ore" : "Iron Ingots"} ${stock(amount, s.cap_g)}, ${rate(s.rate_g_per_h)}`;
  });
  const soonest = next([{ view, receivedAt }], null);
  const fleets = useFleets();
  const arriving = (fleets.data?.fleets ?? []).flatMap((f) =>
    f.state === "in_transit" && f.target_planet_id === view.planet_id && fleets.data
      ? [
          {
            id: f.fleet_id,
            text: `Fleet ${f.fleet_id.toString()} arriving in ${duration(dueIn(fleets.data.as_of, Date.parse(f.arrives_at), fleets.dataUpdatedAt, now))}`,
          },
        ]
      : [],
  );
  const slots = `${view.buildings.length.toString()}/${view.slots.toString()} slots`;
  const finishing = soonest && `${soonest.label} in ${duration(soonest.at - now)}`;
  return (
    <Link
      to="/planets/$planetId"
      params={{ planetId: view.planet_id }}
      aria-label={[name, slots, ...stocks, ...running, finishing, ...arriving.map((a) => a.text)]
        .filter(Boolean)
        .join(", ")}
      className="block"
    >
      <Panel className="hover:border-accent">
        <h2 className="flex items-baseline gap-2 text-lg">
          {name}
          <span className="ml-auto text-sm text-muted">{slots}</span>
        </h2>
        <Meter name="Iron Ore" stock={view.stocks.iron_ore} ran={ran} tone="ore" />
        <Meter name="Iron Ingots" stock={view.stocks.iron_ingot} ran={ran} tone="ingot" />
        {running.length > 0 && <p className="text-xs text-muted">{running.join(" · ")}</p>}
        {finishing && <p className="text-xs text-muted">Next: {finishing}</p>}
        {arriving.map((a) => (
          <p key={a.id} className="text-xs text-muted">
            {a.text}
          </p>
        ))}
      </Panel>
    </Link>
  );
}
