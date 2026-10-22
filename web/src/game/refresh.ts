import { ApiError } from "../api/client";

/** No query refetches sooner than this after its last answer (the read budget is 10/s, D-054). */
export const FLOOR_MS = 2_000;

/** How long until a view's due time, measured from the view's own `as_of`, not this clock. */
export function dueIn(asOf: string, due: number, receivedAt: number, now: number): number {
  return due - Date.parse(asOf) - (now - receivedAt);
}

/**
 * When a query refetches next: at its view's due time, a refusal's Retry-After, or never (false)
 * for a failure without one, which waits for the player's Try again. Never under the floor.
 */
export function refetchDelay(dueInMs: number | null, error: unknown): number | false {
  if (error) {
    const wait =
      error instanceof ApiError && error.failure.kind === "refused" ? error.failure.retryAfter : null;
    return wait === null ? false : Math.max(wait * 1_000, FLOOR_MS);
  }
  return dueInMs === null ? false : Math.max(dueInMs, FLOOR_MS);
}
