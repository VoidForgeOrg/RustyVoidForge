import { Label, NumberField } from "@heroui/react";
import type { ReactNode } from "react";
import type { components } from "../api/schema";
import { flow, type Flow, mass, rate, stock as stockText } from "../format";
import { grown, ratedFor, viewNow } from "../game/live";
import { useNow } from "../game/useNow";
import { ConstructingIcon, DemolishingIcon, HaltedIcon, WorkingIcon } from "../theme/icons";

type S = components["schemas"];
export type PlanetView = S["PlanetView"];
export type BuildingView = S["BuildingView"];

/** A view and when it arrived, ticked to now: what every live value is computed from. */
export function useLive(view: { as_of: string; valid_until?: string | null }, receivedAt: number) {
  const now = useNow();
  return { ran: ratedFor(view, receivedAt, now), gameNow: viewNow(view, receivedAt, now) };
}

export const DU_PER_U = 1_000;
export const DEFAULT_RADIUS_U = 50;

/** A search radius in whole world units, for the star charts and the mission's targets. */
export function RadiusField({ value, onChange }: { value: number; onChange: (radiusU: number) => void }) {
  return (
    <NumberField
      className="w-40"
      value={value}
      minValue={0}
      maxValue={1_000_000}
      step={10}
      formatOptions={{ maximumFractionDigits: 0 }}
      onChange={(next) => {
        if (Number.isSafeInteger(next)) onChange(next);
      }}
    >
      <Label>Radius (u)</Label>
      <NumberField.Group>
        <NumberField.DecrementButton />
        <NumberField.Input />
        <NumberField.IncrementButton />
      </NumberField.Group>
    </NumberField>
  );
}

export const planetName = (id: number, homeworld?: number) =>
  id === homeworld ? `#${id.toString()} Home` : `#${id.toString()}`;

/** A framed panel, the console's card. */
export function Panel({ children, className = "" }: { children: ReactNode; className?: string }) {
  return (
    <section className={`frame border border-separator bg-surface p-4 ${className}`}>{children}</section>
  );
}

/** A stock as a bar against its cap, with amount, cap and rate in words beside it. */
export function Meter({
  name,
  stock,
  ran,
  tone,
}: {
  name: string;
  stock: S["Stock"];
  ran: number;
  tone: "ore" | "ingot";
}) {
  const amount = grown(stock.amount_g, stock.rate_g_per_h, ran, stock.cap_g);
  const percent = stock.cap_g > 0 ? Math.min(100, Math.floor((amount * 100) / stock.cap_g)) : 0;
  return (
    <div className="my-1.5">
      <div className="flex justify-between gap-2 text-xs text-muted">
        <span className="display">{name}</span>
        <span>
          {stockText(amount, stock.cap_g)} · {rate(stock.rate_g_per_h)}
        </span>
      </div>
      <div
        className="h-1.5 overflow-hidden rounded-full bg-default"
        role="meter"
        aria-label={name}
        aria-valuemin={0}
        aria-valuemax={stock.cap_g}
        aria-valuenow={amount}
        aria-valuetext={`${mass(amount)} of ${mass(stock.cap_g)}, ${rate(stock.rate_g_per_h)}`}
      >
        <i
          className={`block h-full ${tone === "ore" ? "bg-accent" : "bg-warning"}`}
          style={{ width: `${percent.toString()}%` }}
        />
      </div>
    </div>
  );
}

export type Status = "working" | "constructing" | "demolishing" | "slowed" | "halted" | "idle";

/** What a building is doing, in one word, with the planet's builds for a Shipyard's idleness. */
export function statusOf(b: BuildingView, shipBuilds: S["ShipBuildView"][]): Status {
  if (b.state === "under_construction") return "constructing";
  if (b.state === "demolishing") return "demolishing";
  if (b.type === "shipyard" && !shipBuilds.some((s) => s.shipyard_id === b.id && s.state === "active"))
    return "idle";
  if (b.status === "throttled") return "slowed";
  return b.status === "halted" ? "halted" : "working";
}

/** The flow words for a building's full status line (§3). */
export function flowWords(b: BuildingView): string | null {
  if (!b.status) return null;
  const of: Flow = b.state === "under_construction" ? "receiving" : b.type;
  return flow(b.status, of);
}

const statusIcons: Record<Status, ReactNode> = {
  working: <WorkingIcon />,
  constructing: <ConstructingIcon />,
  demolishing: <DemolishingIcon />,
  slowed: <HaltedIcon />,
  halted: <HaltedIcon />,
  idle: <HaltedIcon />,
};

const statusTones: Record<Status, string> = {
  working: "text-success",
  constructing: "text-warning",
  demolishing: "text-danger",
  slowed: "text-warning",
  halted: "text-danger",
  idle: "text-muted",
};

/** A status as icon and word, never colour alone. */
export function StatusWord({ status, detail }: { status: Status; detail?: string }) {
  return (
    <span className={`inline-flex items-center gap-1 ${statusTones[status]}`}>
      {statusIcons[status]}
      {detail ? `${status} ${detail}` : status}
    </span>
  );
}
