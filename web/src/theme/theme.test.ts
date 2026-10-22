import { expect, test } from "vitest";
import css from "./theme.css?raw";

const tokens = new Map([...css.matchAll(/--([\w-]+):\s*(#[0-9a-f]{6});/g)].map((m) => [m[1], m[2]]));

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(token(a)), luminance(token(b))].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

function token(name: string): string {
  const hex = tokens.get(name);
  if (!hex) throw new Error(`theme.css has no hex --${name}`);
  return hex;
}

const surfaces = ["background", "surface", "surface-secondary", "surface-tertiary", "overlay"];

test("text meets WCAG AA on every surface", () => {
  for (const text of ["foreground", "muted", "accent", "success", "warning", "danger"]) {
    for (const surface of surfaces) {
      expect(contrast(text, surface), `${text} on ${surface}`).toBeGreaterThanOrEqual(4.5);
    }
  }
  for (const status of ["accent", "success", "warning", "danger"]) {
    expect(contrast(`${status}-foreground`, status), status).toBeGreaterThanOrEqual(4.5);
  }
  expect(contrast("field-foreground", "field-background")).toBeGreaterThanOrEqual(4.5);
  expect(contrast("field-placeholder", "field-background")).toBeGreaterThanOrEqual(4.5);
});

test("an unticked checkbox's edge meets 3:1 in a dialog", () => {
  expect(css).toMatch(/\.checkbox__control \{\s*border: 1px solid var\(--field-border\);/);
  expect(contrast("field-border", "overlay")).toBeGreaterThanOrEqual(3);
});

test("control edges and the focus ring meet 3:1 where controls sit", () => {
  for (const edge of ["border", "field-border", "focus"]) {
    for (const surface of ["background", "surface", "overlay", "field-background"]) {
      expect(contrast(edge, surface), `${edge} on ${surface}`).toBeGreaterThanOrEqual(3);
    }
  }
});
