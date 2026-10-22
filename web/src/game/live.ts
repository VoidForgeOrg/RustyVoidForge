// Values between reads: a view holds at `as_of` and changes at the rates it gives until its
// `valid_until`, when the next read is due. Integer arithmetic on BigInt, as in format.ts.

const MS_PER_H = 3_600_000n;

export interface Timed {
  as_of: string;
  valid_until?: string | null;
}

/** The game time a view stands for now: its `as_of` plus the time since it arrived. */
export const viewNow = (view: Timed, receivedAt: number, now: number) =>
  Date.parse(view.as_of) + Math.max(0, now - receivedAt);

/** Time the view's rates have run since `as_of`, held at `valid_until`. */
export function ratedFor(view: Timed, receivedAt: number, now: number): number {
  const ran = Math.max(0, now - receivedAt);
  if (!view.valid_until) return ran;
  return Math.min(ran, Date.parse(view.valid_until) - Date.parse(view.as_of));
}

/** An amount after `ms` at a rate, rounded down and kept within 0 and the cap. */
export function grown(amountG: number, rateGPerH: number, ms: number, capG?: number): number {
  if (![amountG, rateGPerH, ms].every(Number.isSafeInteger)) return amountG;
  const delta = BigInt(rateGPerH) * BigInt(ms);
  const step = delta / MS_PER_H - (delta < 0n && delta % MS_PER_H !== 0n ? 1n : 0n);
  const next = Number(BigInt(amountG) + step);
  return Math.max(0, capG === undefined ? next : Math.min(next, capG));
}
