import { describe, expect, test } from "vitest";
import * as f from "./format";

const MAX = Number.MAX_SAFE_INTEGER;
const cases = <T>(fn: (x: T) => string, table: [T, string][]) => {
  for (const [input, want] of table) expect(fn(input), String(input)).toBe(want);
};

test("whole numbers group their thousands", () => {
  cases(f.thousands, [
    [0, "0"],
    [999, "999"],
    [1_000, "1,000"],
    [-1_234_567, "−1,234,567"],
    [MAX, "9,007,199,254,740,991"],
    [MAX + 1, "—"],
  ]);
});

test("mass rounds down to 0.1 t", () => {
  cases(f.mass, [
    [0, "0 t"],
    [1, "< 0.1 t"],
    [99_999, "< 0.1 t"],
    [100_000, "0.1 t"],
    [199_999, "0.1 t"],
    [1_234_567_890, "1,234.5 t"],
    [5_000_000_000, "5,000.0 t"],
    [-1, "−0.1 t"],
    [-100_000, "−0.1 t"],
    [-100_001, "−0.2 t"],
    [MAX, "9,007,199,254.7 t"],
  ]);
});

test("rates round toward zero and keep their sign", () => {
  cases(f.rate, [
    [0, "0 t/h"],
    [1, "+0.0 t/h"],
    [-1, "−0.0 t/h"],
    [100_000, "+0.1 t/h"],
    [-199_999, "−0.1 t/h"],
    [36_000_000_000, "+36,000.0 t/h"],
    [-8_000_000, "−8.0 t/h"],
  ]);
});

test("stocks are full only at the exact cap", () => {
  expect(f.stock(1_234_500_000, 5_000_000_000)).toBe("1,234.5 t of 5,000.0 t");
  expect(f.stock(5_000_000_000, 5_000_000_000)).toBe("5,000.0 t of 5,000.0 t (full)");
  expect(f.stock(4_999_999_999, 5_000_000_000)).toBe("4,999.9 t of 5,000.0 t");
  expect(f.stock(5_000_000_000, 5_000_050_000)).toBe("5,000.0 t of 5,000.0 t");
  expect(f.stock(0, 5_000_000_000)).toBe("0 t of 5,000.0 t (empty)");
});

test("distances, positions and speeds round toward zero", () => {
  cases(f.distance, [
    [0, "0.0 u"],
    [99, "0.0 u"],
    [123_456, "123.4 u"],
    [1_000_000, "1,000.0 u"],
  ]);
  expect(f.position({ x: 150_000, y: -20_500, z: 999_999 })).toBe("(150.0, −20.5, 999.9)");
  expect(f.position({ x: -50, y: -20_550, z: 0 })).toBe("(0.0, −20.5, 0.0)");
  expect(f.speed(15_000)).toBe("15.0 u/h");
});

test("durations take the two largest units, rounded up to the second", () => {
  const S = 1_000;
  cases(f.duration, [
    [-5, "0 s"],
    [0, "0 s"],
    [1, "1 s"],
    [1_001, "2 s"],
    [60 * S, "1 min"],
    [250 * S, "4 min 10 s"],
    [3_600 * S - 1, "1 h"],
    [3_599 * S, "59 min 59 s"],
    [(2 * 3_600 + 14 * 60 + 59) * S, "2 h 14 min"],
    [(3 * 86_400 + 4 * 3_600 + 59 * 60) * S, "3 d 4 h"],
    [(2 * 365 + 40) * 86_400 * S, "2 y 40 d"],
  ]);
});

describe("times", () => {
  const now = Date.parse("2026-10-20T18:04:12.000Z");

  test("a clock shows the date only on another day, and rounds up only ahead of now", () => {
    expect(f.clock("2026-10-20T18:04:15.999Z", now)).toBe("18:04:16 UTC");
    expect(f.clock("2026-10-20T18:04:11.999Z", now)).toBe("18:04:11 UTC");
    expect(f.clock("2026-10-21T06:00:00.000Z", now)).toBe("2026-10-21 06:00:00 UTC");
    expect(f.clock("2026-10-19T23:59:59.000Z", now)).toBe("2026-10-19 23:59:59 UTC");
  });

  test("a countdown agrees with its clock and never says in 0 s", () => {
    expect(f.at("2026-10-20T18:04:12.000Z", now)).toBe("18:04:12 UTC");
    expect(f.at("2026-10-20T18:04:12.001Z", now)).toBe("18:04:13 UTC (in 1 s)");
    expect(f.at("2026-10-20T18:04:13.001Z", now)).toBe("18:04:14 UTC (in 2 s)");
    expect(f.at("2026-10-21T06:00:00.000Z", now)).toBe("2026-10-21 06:00:00 UTC (in 11 h 55 min)");
    const r1 = Date.parse("2026-10-21T08:21:03.400Z");
    expect(f.at("2026-10-21T08:21:23.100Z", r1)).toBe("08:21:24 UTC (in 21 s)");
    const late = Date.parse("2026-10-20T23:59:59.500Z");
    expect(f.at("2026-10-20T23:59:59.600Z", late)).toBe("2026-10-21 00:00:00 UTC (in 1 s)");
  });
});

test("progress floors its percent", () => {
  expect(f.progress(63_000_000, 150_000_000)).toBe("42% (63.0 of 150.0 t)");
  expect(f.progress(149_999_999, 150_000_000)).toBe("99% (149.9 of 150.0 t)");
  expect(f.progress(150_000_000, 150_000_000)).toBe("100% (150.0 of 150.0 t)");
  expect(f.percent(5, 0)).toBe(100);
});

test("players and names", () => {
  const me = "01890a5d-ac96-774b-bcce-b302099a8057";
  const other = "01928c5e-7a3b-7c2d-9e4f-5a6b7c8d9e0f";
  expect(f.player(me, me)).toBe("you");
  expect(f.player(other, me)).toBe("player 7c8d9e0f");
  expect(f.owner(null, me)).toBe("unowned");
  expect(f.ships(1, "colony_ship")).toBe("1 Colony Ship");
  expect(f.ships(2, "cargo_vessel")).toBe("2 Cargo Vessels");
});

test("flow words are §3's table", () => {
  const table: [f.FlowStatus, f.Flow, string][] = [
    ["running", "drill", "running"],
    ["throttled", "drill", "slowed: ore storage is full"],
    ["halted", "drill", "halted: ore storage is full"],
    ["throttled", "refinery", "slowed: short of ore, or ingot storage is full"],
    ["halted", "refinery", "halted: no ore, or ingot storage is full"],
    ["running", "shipyard", "running"],
    ["throttled", "shipyard", "slowed: short of ingots"],
    ["halted", "shipyard", "halted: no ingots"],
    ["running", "receiving", "receiving ingots"],
    ["halted", "receiving", "halted: no ingots"],
  ];
  for (const [status, of, want] of table) expect(f.flow(status, of)).toBe(want);
});
