import { Button, Chip, Drawer, Popover } from "@heroui/react";
import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import type { components } from "../api/schema";
import { duration, mass, rate, thousands } from "../format";
import { homeworldFirst, next, type Received, type Resource, total } from "../game/empire";
import { useFleets, useLeaderboard, usePlanets, useRules } from "../game/queries";
import { useNow } from "../game/useNow";
import { FleetsIcon, IngotIcon, LeaderboardIcon, OreIcon, PlanetsIcon, ShipyardIcon } from "../theme/icons";
import type { Layout } from "./useLayout";

type S = components["schemas"];

const item = "flex items-center gap-2 rounded px-2 py-1 whitespace-nowrap hover:bg-default";

/** The empire at a glance: every figure comes from reads the screens already make. */
export function TopBar({
  me,
  email,
  layout,
  menu,
}: {
  me: S["PlayerView"];
  email: string | undefined;
  layout: Layout;
  menu: ReactNode;
}) {
  const now = useNow();
  const planetReads = usePlanets(homeworldFirst(me.planet_ids, me.homeworld_id));
  const fleets = useFleets();
  const board = useLeaderboard();
  const planets: Received<S["PlanetView"]>[] = planetReads.flatMap((q) =>
    q.data ? [{ view: q.data, receivedAt: q.dataUpdatedAt }] : [],
  );
  const loaded = planets.length === me.planet_ids.length;
  const flying = fleets.data?.fleets.filter((f) => f.state !== "stationed").length ?? 0;
  const stationed = (fleets.data?.fleets.length ?? 0) - flying;
  const rank = board.data?.entries.find((e) => e.player_id === me.player_id)?.rank;
  const soonest = loaded
    ? next(planets, fleets.data ? { view: fleets.data, receivedAt: fleets.dataUpdatedAt } : null)
    : null;
  const used = planets.reduce((n, p) => n + p.view.buildings.length, 0);
  const slots = planets.reduce((n, p) => n + p.view.slots, 0);

  const resources = loaded && (
    <>
      <ResourceItem resource="iron_ore" name="Iron Ore" icon={<OreIcon />} planets={planets} now={now} />
      <ResourceItem
        resource="iron_ingot"
        name="Iron Ingots"
        icon={<IngotIcon />}
        planets={planets}
        now={now}
      />
    </>
  );
  const nextItem = <NextItem soonest={soonest} now={now} />;
  const rest = (labels: string) => (
    <>
      <Link
        to="/planets"
        className={item}
        aria-label={`Holdings: ${me.planet_ids.length.toString()} planets, ${used.toString()} of ${slots.toString()} slots used`}
      >
        <PlanetsIcon />
        {me.planet_ids.length} <span className={labels}>planets</span> · {used}/{slots}{" "}
        <span className={labels}>slots</span>
      </Link>
      <Link
        to="/fleets"
        className={item}
        aria-label={`Fleets: ${flying.toString()} in flight, ${stationed.toString()} stationed`}
      >
        <FleetsIcon />
        {flying} <span className={labels}>flying</span> · {stationed}{" "}
        <span className={labels}>stationed</span>
      </Link>
      <Link
        to="/leaderboard"
        className={item}
        aria-label={`Score ${thousands(me.score)}${rank ? `, rank ${rank.toString()}` : ""}`}
      >
        <LeaderboardIcon />
        <span className="display text-muted">Score</span> {thousands(me.score)}
        {rank !== undefined && <span className="text-muted">#{rank}</span>}
      </Link>
    </>
  );
  const ends = (
    <>
      <FastRules />
      <span className="truncate text-xs text-muted">{email}</span>
    </>
  );

  return (
    <header className="flex flex-wrap items-center gap-x-1 gap-y-1 border-b border-separator px-2 py-2 sm:px-4">
      {menu}
      {resources}
      {layout === "phone" ? (
        <>
          {nextItem}
          <Drawer>
            <Button variant="ghost" className="ml-auto">
              More
            </Button>
            <Drawer.Backdrop>
              <Drawer.Content placement="bottom">
                <Drawer.Dialog aria-label="Empire">
                  <Drawer.Body className="flex flex-col items-start gap-2 p-4">
                    {rest("text-muted")}
                    {ends}
                  </Drawer.Body>
                </Drawer.Dialog>
              </Drawer.Content>
            </Drawer.Backdrop>
          </Drawer>
        </>
      ) : (
        <>
          {rest("hidden text-muted xl:inline")}
          {nextItem}
          <div className="ml-auto flex min-w-0 items-center gap-3">{ends}</div>
        </>
      )}
    </header>
  );
}

function ResourceItem({
  resource,
  name,
  icon,
  planets,
  now,
}: {
  resource: Resource;
  name: string;
  icon: ReactNode;
  planets: Received<S["PlanetView"]>[];
  now: number;
}) {
  const t = total(planets, resource, now);
  return (
    <Popover>
      <Button
        variant="ghost"
        className={item}
        aria-label={`${name}: ${mass(t.amountG)} in storage, ${rate(t.rateGPerH)}`}
      >
        {icon}
        {mass(t.amountG)}
        <span className={t.rateGPerH > 0 ? "text-success" : t.rateGPerH < 0 ? "text-danger" : "text-muted"}>
          {rate(t.rateGPerH)}
        </span>
      </Button>
      <Popover.Content placement="bottom start">
        <Popover.Dialog>
          <Popover.Heading className="display mb-2">{name}</Popover.Heading>
          <ul className="flex flex-col gap-1">
            {t.planets.map(({ planetId, stock }) => (
              <li key={planetId}>
                <Link to="/planets/$planetId" params={{ planetId }} className={item}>
                  #{planetId} {mass(stock.amount_g)}{" "}
                  <span className="text-muted">
                    of {mass(stock.cap_g)}, {rate(stock.rate_g_per_h)}
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
}

function NextItem({ soonest, now }: { soonest: ReturnType<typeof next>; now: number }) {
  if (!soonest) {
    return (
      <span className={item}>
        <ShipyardIcon />
        <span className="text-muted">Nothing scheduled</span>
      </span>
    );
  }
  const content = (
    <>
      <ShipyardIcon />
      <span className="display text-muted">Next</span> {soonest.label}{" "}
      <span className="text-muted">in {duration(soonest.at - now)}</span>
    </>
  );
  return soonest.kind === "building" ? (
    <Link
      to="/planets/$planetId/buildings/$buildingId"
      params={{ planetId: soonest.planetId, buildingId: soonest.buildingId }}
      className={item}
    >
      {content}
    </Link>
  ) : (
    <Link to="/fleets" className={item}>
      {content}
    </Link>
  );
}

export function FastRules() {
  const rules = useRules();
  return rules.data?.name === "v0-fast" ? (
    <Chip color="warning" variant="soft">
      fast test rules
    </Chip>
  ) : null;
}
