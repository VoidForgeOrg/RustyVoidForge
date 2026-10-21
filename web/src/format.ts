import type { components } from "./api/schema";

// Every value a screen shows, formatted the one documented way (phase4-ux §3). Screens format
// nothing themselves. Arithmetic runs on BigInt, so no quotient is ever a float; an input that
// isn't a safe integer was already rounded by JSON parsing and shows as UNKNOWN.

type S = components["schemas"];
export type BuildingType = S["BuildingType"];
export type ShipType = S["ShipType"];
export type Mission = S["Mission"];
export type FlowStatus = S["FlowStatus"];

export const MINUS = "−";
export const UNKNOWN = "—";

const G_PER_TENTH_T = 100_000n;
const DU_PER_TENTH_U = 100n;

const big = (n: number): bigint | null => (Number.isSafeInteger(n) ? BigInt(n) : null);
const abs = (n: bigint) => (n < 0n ? -n : n);

/** Floor division, as Rust's `div_euclid` for a positive divisor. */
function floorDiv(a: bigint, b: bigint): bigint {
  const q = a / b;
  return a % b !== 0n && a < 0n ? q - 1n : q;
}

function grouped(n: bigint): string {
  return n.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

function tenths(n: bigint): string {
  return `${grouped(n / 10n)}.${(n % 10n).toString()}`;
}

/** A whole number with thousands separated: `3,450`, `−1,200`. */
export function thousands(n: number): string {
  const b = big(n);
  if (b === null) return UNKNOWN;
  return `${b < 0n ? MINUS : ""}${grouped(abs(b))}`;
}

/** Tonnes without the unit, rounded down to 0.1 t, so storage never shows more than it holds. */
export function tonnes(g: number): string {
  const b = big(g);
  if (b === null) return UNKNOWN;
  if (b === 0n) return "0";
  if (b > 0n && b < G_PER_TENTH_T) return "< 0.1";
  const t = floorDiv(b, G_PER_TENTH_T);
  return `${t < 0n ? MINUS : ""}${tenths(abs(t))}`;
}

export const mass = (g: number) => `${tonnes(g)} t`;

/** A signed rate in t/h, toward zero; under 0.1 t/h it keeps its sign, so a slow drain shows. */
export function rate(gPerH: number): string {
  const b = big(gPerH);
  if (b === null) return UNKNOWN;
  if (b === 0n) return "0 t/h";
  return `${b > 0n ? "+" : MINUS}${tenths(abs(b / G_PER_TENTH_T))} t/h`;
}

/** `1,234.5 t of 5,000.0 t`, marked full only at the exact cap and empty only at 0. */
export function stock(amountG: number, capG: number): string {
  const note = amountG === 0 ? " (empty)" : amountG === capG ? " (full)" : "";
  return `${mass(amountG)} of ${mass(capG)}${note}`;
}

function units(du: number): string {
  const b = big(du);
  if (b === null) return UNKNOWN;
  const t = b / DU_PER_TENTH_U;
  return `${t < 0n ? MINUS : ""}${tenths(abs(t))}`;
}

export const distance = (du: number) => `${units(du)} u`;
export const position = (p: S["Position"]) => `(${units(p.x)}, ${units(p.y)}, ${units(p.z)})`;
export const speed = (duPerH: number) => `${units(duPerH)} u/h`;

const DURATION_UNITS: [number, string][] = [
  [365 * 86_400, "y"],
  [86_400, "d"],
  [3_600, "h"],
  [60, "min"],
  [1, "s"],
];

/** The two largest units, rounded up to the second and the smaller one cut: `2 h 14 min`. */
export function duration(ms: number): string {
  if (!Number.isSafeInteger(ms)) return UNKNOWN;
  const secs = ms <= 0 ? 0 : Math.floor(ms / 1_000) + (ms % 1_000 === 0 ? 0 : 1);
  const i = DURATION_UNITS.findIndex(([size]) => secs >= size);
  const unit = DURATION_UNITS[i];
  if (!unit) return "0 s";
  const [size, name] = unit;
  let out = `${Math.floor(secs / size).toString()} ${name}`;
  const smaller = DURATION_UNITS[i + 1];
  if (smaller) {
    const n = Math.floor((secs % size) / smaller[0]);
    if (n > 0) out += ` ${n.toString()} ${smaller[1]}`;
  }
  return out;
}

/** The whole second a time is shown at: after now it rounds up, so it never shows early. */
function shownSecs(tMs: number, nowMs: number): number {
  const secs = Math.floor(tMs / 1_000);
  return tMs > nowMs && tMs % 1_000 !== 0 ? secs + 1 : secs;
}

function civil(secs: number): [string, string] {
  const iso = new Date(secs * 1_000).toISOString();
  return [iso.slice(0, 10), iso.slice(11, 19)];
}

/** `18:04:15 UTC`, with the date only when it isn't now's date. */
export function clock(t: string, nowMs: number): string {
  const tMs = Date.parse(t);
  if (Number.isNaN(tMs)) return t;
  const [day, time] = civil(shownSecs(tMs, nowMs));
  const [today] = civil(Math.floor(nowMs / 1_000));
  return day === today ? `${time} UTC` : `${day} ${time} UTC`;
}

/** A time with how long until it: `18:04:15 UTC (in 3 s)`; a past time is the clock alone. */
export function at(t: string, nowMs: number): string {
  const shown = clock(t, nowMs);
  const tMs = Date.parse(t);
  if (Number.isNaN(tMs) || tMs <= nowMs) return shown;
  const secs = shownSecs(tMs, nowMs) - Math.floor(nowMs / 1_000);
  return `${shown} (in ${duration(secs * 1_000)})`;
}

/** Percent of a cost delivered, rounded down, so it says 100 only when done. */
export function percent(deliveredG: number, costG: number): number {
  const d = big(deliveredG);
  const c = big(costG);
  if (d === null || c === null || c <= 0n) return 100;
  const p = (d * 100n) / c;
  return Number(p < 0n ? 0n : p > 100n ? 100n : p);
}

/** `42% (63.0 of 150.0 t)`. */
export function progress(deliveredG: number, costG: number): string {
  return `${percent(deliveredG, costG).toString()}% (${tonnes(deliveredG)} of ${tonnes(costG)} t)`;
}

/** The public short form of a player's ID: the UUID's last 8 hex digits. */
export const shortName = (playerId: string) => `player ${playerId.slice(-8)}`;

export const player = (id: string, me: string) => (id === me ? "you" : shortName(id));

export const owner = (id: string | null | undefined, me: string) =>
  id ? player(id, me) : "unowned";

export function building(t: BuildingType): string {
  switch (t) {
    case "drill":
      return "Drill";
    case "refinery":
      return "Refinery";
    case "shipyard":
      return "Shipyard";
  }
}

export function ship(t: ShipType): string {
  switch (t) {
    case "colony_ship":
      return "Colony Ship";
    case "cargo_vessel":
      return "Cargo Vessel";
  }
}

export const ships = (n: number, t: ShipType) => `${n.toString()} ${ship(t)}${n === 1 ? "" : "s"}`;

export function mission(m: Mission): string {
  switch (m) {
    case "colonize":
      return "Colonize";
    case "transport":
      return "Transport";
    case "move":
      return "Move";
  }
}

/** What a flow belongs to, which decides its words. */
export type Flow = "drill" | "refinery" | "shipyard" | "receiving";

/** §3's flow words. The view doesn't say which input limits a flow, so they name the likely causes. */
export function flow(status: FlowStatus, of: Flow): string {
  if (status === "running") return of === "receiving" ? "receiving ingots" : "running";
  const slowed = status === "throttled";
  switch (of) {
    case "drill":
      return slowed ? "slowed: ore storage is full" : "halted: ore storage is full";
    case "refinery":
      return slowed
        ? "slowed: short of ore, or ingot storage is full"
        : "halted: no ore, or ingot storage is full";
    case "shipyard":
    case "receiving":
      return slowed ? "slowed: short of ingots" : "halted: no ingots";
  }
}
