import { expect, test } from "vitest";
import { returnPath } from "./auth";

test("the login returns only to a path on this origin", () => {
  expect(returnPath("/planets/26?tab=shipyard")).toBe("/planets/26?tab=shipyard");
  for (const offSite of ["//evil.test/x", "/\\evil.test", "https://evil.test/", "fleets", "", 7, undefined]) {
    expect(returnPath(offSite)).toBe("/");
  }
});
