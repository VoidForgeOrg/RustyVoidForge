import { buttonVariants, Spinner } from "@heroui/react";
import { Link, Navigate, Outlet, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { useEffect, useRef } from "react";
import { empties } from "../copy/voices";
import { building as buildingName, duration, owner, percent, ship, shipCounts, ships } from "../format";
import { homeworldFirst } from "../game/empire";
import { grown } from "../game/live";
import { useFleets, useMe, usePlanet } from "../game/queries";
import { dueIn } from "../game/refresh";
import { useNow } from "../game/useNow";
import { Failure } from "../shell/Failure";
import { useLayout } from "../shell/useLayout";
import {
  CargoVesselIcon,
  ColonyShipIcon,
  DrillIcon,
  EmptySlotIcon,
  RefineryIcon,
  ShipyardIcon,
} from "../theme/icons";
import { MissionModal } from "./Mission";
import { Meter, Panel, type PlanetView, planetName, StatusWord, statusOf, useLive } from "./parts";

const buildingIcons = { drill: <DrillIcon />, refinery: <RefineryIcon />, shipyard: <ShipyardIcon /> };
const shipIcons = { colony_ship: <ColonyShipIcon />, cargo_vessel: <CargoVesselIcon /> };

/** The holdings grid. A planet in the URL expands in place; on a phone it takes the screen. */
export function Planets() {
  const me = useMe().data;
  const { planetId } = useParams({ strict: false });
  const layout = useLayout();
  if (!me) return null;
  const owned = homeworldFirst(me.planet_ids, me.homeworld_id);
  const only = owned[0];
  if (planetId === undefined && owned.length === 1 && only !== undefined) {
    return <Navigate to="/planets/$planetId" params={{ planetId: only }} replace />;
  }
  const foreign = planetId !== undefined && !owned.includes(planetId);
  const shown =
    layout === "phone" && planetId !== undefined ? [planetId] : foreign ? [planetId, ...owned] : owned;
  return (
    <>
      {layout === "phone" && planetId !== undefined && (
        <Link to="/planets" className="mb-3 inline-block text-accent">
          ← All holdings
        </Link>
      )}
      <div className="grid grid-cols-[repeat(auto-fill,minmax(16rem,1fr))] gap-4">
        {shown.map((id) => (
          <PlanetCard key={id} id={id} homeworld={me.homeworld_id} me={me.player_id} open={id === planetId} />
        ))}
      </div>
      <Outlet />
    </>
  );
}

/** The planet route adds only the stepper over it: the grid shows it expanded. */
export function Planet() {
  const { planetId } = useParams({ from: "/game/planets/$planetId" });
  const { mission } = useSearch({ from: "/game/planets/$planetId" });
  const navigate = useNavigate();
  return (
    <>
      <Outlet />
      {mission && (
        <MissionModal
          planet={planetId}
          onClose={() => void navigate({ to: "/planets/$planetId", params: { planetId } })}
        />
      )}
    </>
  );
}

function PlanetCard({
  id,
  homeworld,
  me,
  open,
}: {
  id: number;
  homeworld: number;
  me: string;
  open: boolean;
}) {
  const planet = usePlanet(id);
  const ref = useRef<HTMLElement>(null);
  useEffect(() => {
    if (open) ref.current?.scrollIntoView({ block: "nearest" });
  }, [open]);
  const body = planet.isPending ? (
    <Spinner aria-label={`Loading planet ${id.toString()}`} />
  ) : planet.isError ? (
    <Failure error={planet.error} fill={{ planet: id }} onRetry={() => void planet.refetch()} />
  ) : (
    <Holding view={planet.data} receivedAt={planet.dataUpdatedAt} homeworld={homeworld} me={me} open={open} />
  );
  return (
    <article ref={ref} className={open ? "col-span-full" : ""} aria-label={planetName(id, homeworld)}>
      {body}
    </article>
  );
}

function Holding({
  view,
  receivedAt,
  homeworld,
  me,
  open,
}: {
  view: PlanetView;
  receivedAt: number;
  homeworld: number;
  me: string;
  open: boolean;
}) {
  const { ran } = useLive(view, receivedAt);
  const phone = useLayout() === "phone";
  // A lone planet always opens expanded, so collapsing it would lead straight back here.
  const lone = useMe().data?.planet_ids.length === 1;
  const mine = view.owner === me;
  const building = view.ship_builds.filter((s) => s.state === "active").length;
  const waiting = view.ship_builds.length - building;
  const title = (
    <h2 className="flex items-baseline gap-2 text-lg">
      {planetName(view.planet_id, homeworld)}
      {!mine && <span className="text-sm text-muted">{owner(view.owner, me)}</span>}
      <span className="ml-auto text-sm text-muted">
        {view.buildings.length}/{view.slots} slots
      </span>
    </h2>
  );
  return (
    <Panel className={open ? "border-accent" : "hover:border-accent"}>
      {open ? (
        <div className="flex items-baseline gap-3">
          <div className="flex-1">{title}</div>
          {!phone && !lone && (
            <Link to="/planets" className="text-sm text-accent">
              Collapse
            </Link>
          )}
        </div>
      ) : (
        <Link to="/planets/$planetId" params={{ planetId: view.planet_id }} className="block">
          {title}
        </Link>
      )}
      <Meter name="Iron Ore" stock={view.stocks.iron_ore} ran={ran} tone="ore" />
      <Meter name="Iron Ingots" stock={view.stocks.iron_ingot} ran={ran} tone="ingot" />
      {view.ship_builds.length > 0 && (
        <p className="text-xs text-muted">
          Shipyard: {building} building · {waiting} waiting
        </p>
      )}
      <Arrivals planetId={view.planet_id} />
      {open && <Slots view={view} ran={ran} mine={mine} />}
      {open && mine && <Roster view={view} me={me} />}
    </Panel>
  );
}

function Arrivals({ planetId }: { planetId: number }) {
  const fleets = useFleets();
  const now = useNow();
  if (!fleets.data) return null;
  const view = fleets.data;
  return view.fleets.map(
    (f) =>
      f.state === "in_transit" &&
      f.target_planet_id === planetId && (
        <p key={f.fleet_id} className="text-xs text-muted">
          Fleet {f.fleet_id} arriving in{" "}
          {duration(dueIn(view.as_of, Date.parse(f.arrives_at), fleets.dataUpdatedAt, now))}
        </p>
      ),
  );
}

const tile = "relative grid h-19 w-19 place-items-center border bg-field text-2xl";

/**
 * The buildings in id order, then one "+" per free slot: slots are interchangeable. Only the
 * player's own free slots open the build picker.
 */
function Slots({ view, ran, mine }: { view: PlanetView; ran: number; mine: boolean }) {
  const { buildingId: selected } = useParams({ strict: false });
  const buildings = [...view.buildings].sort((a, b) => a.id - b.id);
  const free = Math.max(0, view.slots - buildings.length);
  return (
    <ul className="my-3 grid grid-cols-[repeat(auto-fill,4.75rem)] gap-2.5" aria-label="Building slots">
      {buildings.map((b) => {
        const status = statusOf(b, view.ship_builds);
        const done =
          b.state === "under_construction"
            ? ` ${percent(grown(b.delivered_g, b.rate_g_per_h, ran, b.cost_g), b.cost_g).toString()}%`
            : "";
        const current = selected === b.id;
        return (
          <li key={b.id}>
            <Link
              to="/planets/$planetId/buildings/$buildingId"
              params={{ planetId: view.planet_id, buildingId: b.id }}
              aria-label={`${buildingName(b.type)}, ${status}${done}`}
              aria-current={current ? "true" : undefined}
              className={`${tile} border-border ${current ? "border-accent ring-2 ring-accent/40" : "hover:border-accent"}`}
            >
              {buildingIcons[b.type]}
              <span className="absolute inset-x-0 bottom-1 text-center text-[10px]">
                <StatusWord status={status} />
              </span>
            </Link>
          </li>
        );
      })}
      {Array.from({ length: free }, (_, i) => (
        <li key={`free-${i.toString()}`}>
          {mine ? (
            <Link
              to="/planets/$planetId/build"
              params={{ planetId: view.planet_id }}
              aria-label="Free slot"
              className={`${tile} border-dashed border-border text-muted hover:border-accent`}
            >
              <EmptySlotIcon />
            </Link>
          ) : (
            <span
              role="img"
              aria-label="Free slot"
              className={`${tile} border-dashed border-separator text-muted`}
            >
              <EmptySlotIcon />
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}

function Roster({ view, me }: { view: PlanetView; me: string }) {
  const counts = shipCounts(view.roster);
  const ours = view.roster.some((s) => s.owner === me);
  return (
    <div className="flex flex-wrap items-center gap-3 border-t border-separator pt-3">
      <span className="display text-muted">Ships here</span>
      {counts.length === 0 && <span className="text-muted">{empties.roster}</span>}
      {counts.map(([type, n]) => (
        <span key={type} className="inline-flex items-center gap-1" title={ship(type)}>
          {shipIcons[type]}
          {ships(n, type)}
        </span>
      ))}
      {ours && (
        <Link
          to="/planets/$planetId"
          params={{ planetId: view.planet_id }}
          search={{ mission: true }}
          className={`ml-auto ${buttonVariants({ variant: "primary", size: "sm" })}`}
        >
          New mission
        </Link>
      )}
    </div>
  );
}
