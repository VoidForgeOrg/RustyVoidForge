import { Button, Chip, Drawer, Popover } from "@heroui/react";
import { Link } from "@tanstack/react-router";
import { type ReactNode, useState } from "react";
import type { components } from "../api/schema";
import { failure } from "../copy/messages";
import { duration, mass, rate, thousands } from "../format";
import { type Empire, next, type Received, type Resource, total } from "../game/empire";
import { useLeaderboard, useRules } from "../game/queries";
import { useNow } from "../game/useNow";
import { IngotIcon, LeaderboardIcon, OreIcon, ShipyardIcon } from "../theme/icons";
import type { Layout } from "./useLayout";

type S = components["schemas"];

const item = "flex items-center gap-2 rounded px-2 py-1 whitespace-nowrap hover:bg-default";

/** The empire at a glance: resources, score and what completes next. */
export function TopBar({
  me,
  empire,
  layout,
  menu,
}: {
  me: S["PlayerView"];
  empire: Empire;
  layout: Layout;
  menu: ReactNode;
}) {
  const now = useNow();
  // Controlled, so following one of the sheet's links closes it.
  const [more, setMore] = useState(false);
  const board = useLeaderboard();
  const { fleets, planets, loaded, missing, partial } = empire;
  const rank = board.data?.entries.find((e) => e.player_id === me.player_id)?.rank;
  const soonest = loaded
    ? next(planets, fleets.data ? { view: fleets.data, receivedAt: fleets.dataUpdatedAt } : null)
    : null;

  const resources = loaded && (
    <>
      <ResourceItem
        resource="iron_ore"
        name="Iron Ore"
        icon={<OreIcon />}
        planets={planets}
        missing={missing}
        partial={partial}
        now={now}
      />
      <ResourceItem
        resource="iron_ingot"
        name="Iron Ingots"
        icon={<IngotIcon />}
        planets={planets}
        missing={missing}
        partial={partial}
        now={now}
      />
    </>
  );
  const nextItem = <NextItem soonest={soonest} partial={partial} now={now} />;
  const score = (onFollow?: () => void) => (
    <Link
      to="/leaderboard"
      className={item}
      onClick={onFollow}
      aria-label={`Score ${thousands(me.score)}${rank ? `, rank ${rank.toString()}` : ""}`}
    >
      <LeaderboardIcon />
      <span className="display text-muted">Score</span> {thousands(me.score)}
      {rank !== undefined && <span className="text-muted">#{rank}</span>}
    </Link>
  );

  return (
    <header className="flex flex-wrap items-center gap-x-1 gap-y-1 border-b border-separator px-2 py-2 sm:px-4">
      {menu}
      {resources}
      {layout === "phone" ? (
        <>
          {nextItem}
          <Drawer isOpen={more} onOpenChange={setMore}>
            <Button variant="ghost" className="ml-auto">
              More
            </Button>
            <Drawer.Backdrop>
              <Drawer.Content placement="bottom">
                <Drawer.Dialog aria-label="Empire">
                  <Drawer.Body className="flex flex-col items-start gap-2 p-4">
                    {score(() => {
                      setMore(false);
                    })}
                    <FastRules />
                  </Drawer.Body>
                </Drawer.Dialog>
              </Drawer.Content>
            </Drawer.Backdrop>
          </Drawer>
        </>
      ) : (
        <>
          {score()}
          {nextItem}
          <div className="ml-auto">
            <FastRules />
          </div>
        </>
      )}
    </header>
  );
}

/** Marks a figure that leaves out planets whose read failed. */
export function Partial({ label }: { label: string | null }) {
  return label ? (
    <span className="text-xs text-warning" title={label}>
      <span aria-hidden>partial</span>
      <span className="sr-only">{label}</span>
    </span>
  ) : null;
}

function ResourceItem({
  resource,
  name,
  icon,
  planets,
  missing,
  partial,
  now,
}: {
  resource: Resource;
  name: string;
  icon: ReactNode;
  planets: Received<S["PlanetView"]>[];
  missing: { id: number; error: unknown }[];
  partial: string | null;
  now: number;
}) {
  const t = total(planets, resource, now);
  // Controlled, so following one of its links closes it rather than leaving it over the planet.
  const [open, setOpen] = useState(false);
  return (
    <Popover isOpen={open} onOpenChange={setOpen}>
      <Button
        variant="ghost"
        className={item}
        aria-label={`${name}: ${mass(t.amountG)} in storage, ${rate(t.rateGPerH)}${partial ? `, ${partial}` : ""}`}
      >
        {icon}
        {mass(t.amountG)}
        <span className={t.rateGPerH > 0 ? "text-success" : t.rateGPerH < 0 ? "text-danger" : "text-muted"}>
          {rate(t.rateGPerH)}
        </span>
        <Partial label={partial} />
      </Button>
      <Popover.Content placement="bottom start">
        <Popover.Dialog>
          <Popover.Heading className="display mb-2">{name}</Popover.Heading>
          <ul className="flex flex-col gap-1">
            {t.planets.map(({ planetId, stock }) => (
              <li key={planetId}>
                <Link
                  to="/planets/$planetId"
                  params={{ planetId }}
                  className={item}
                  onClick={() => {
                    setOpen(false);
                  }}
                >
                  #{planetId} {mass(stock.amount_g)}{" "}
                  <span className="text-muted">
                    of {mass(stock.cap_g)}, {rate(stock.rate_g_per_h)}
                  </span>
                </Link>
              </li>
            ))}
            {missing.map(({ id, error }) => (
              <li key={id}>
                <Link
                  to="/planets/$planetId"
                  params={{ planetId: id }}
                  className={item}
                  onClick={() => {
                    setOpen(false);
                  }}
                >
                  #{id} <span className="text-warning">{failure(error, "page", { planet: id }).copy}</span>
                </Link>
              </li>
            ))}
          </ul>
        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
}

function NextItem({
  soonest,
  partial,
  now,
}: {
  soonest: ReturnType<typeof next>;
  partial: string | null;
  now: number;
}) {
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
      <Partial label={partial} />
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
    <Link to="/fleets" hash={`fleet-${soonest.fleetId.toString()}`} className={item}>
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
