import { expect, test } from "vitest";
import { ApiError } from "../api/client";
import { grown, ratedFor, viewNow } from "./live";
import { dueIn, FLOOR_MS, refetchDelay } from "./refresh";

// On the fast rules a view can be due again within milliseconds; only the floor stops a storm.
test("a refetch never comes sooner than the floor, and a refusal waits its Retry-After", () => {
  expect(refetchDelay(0, null)).toBe(FLOOR_MS);
  expect(refetchDelay(-5_000, null)).toBe(FLOOR_MS);
  expect(refetchDelay(30_000, null)).toBe(30_000);
  expect(refetchDelay(null, null)).toBe(false);
  const refused = (retryAfter: number | null) =>
    new ApiError({ kind: "refused", status: 429, code: "rate_limited", retryAfter });
  expect(refetchDelay(0, refused(5))).toBe(5_000);
  expect(refetchDelay(0, refused(1))).toBe(FLOOR_MS);
  expect(refetchDelay(0, refused(null))).toBe(false);
  expect(refetchDelay(0, new ApiError({ kind: "unreachable" }))).toBe(false);
});

test("a view's due time is measured from its own as_of", () => {
  const asOf = "2026-10-21T12:00:00.000Z";
  const due = Date.parse("2026-10-21T12:00:10.000Z");
  // Arrived at browser time 1,000 with a clock far off the server's: 4 s later, 6 s remain.
  expect(dueIn(asOf, due, 1_000, 5_000)).toBe(6_000);
});

test("a stock grows at its rate, rounded down, within its cap, and holds at valid_until", () => {
  const view = { as_of: "2026-10-21T12:00:00.000Z", valid_until: "2026-10-21T12:00:30.000Z" };
  expect(ratedFor(view, 0, 10_000)).toBe(10_000);
  expect(ratedFor(view, 0, 60_000)).toBe(30_000);
  expect(viewNow(view, 0, 60_000)).toBe(Date.parse("2026-10-21T12:01:00.000Z"));
  // 36 t/h for 10 s is 100 kg.
  expect(grown(1_000_000, 36_000_000, 10_000)).toBe(1_100_000);
  expect(grown(1_000_000, 36_000_000, 10_000, 1_050_000)).toBe(1_050_000);
  // A drain rounds down too, and never below empty.
  expect(grown(1_000, -1, 1)).toBe(999);
  expect(grown(1_000, -36_000_000, 10_000)).toBe(0);
});
