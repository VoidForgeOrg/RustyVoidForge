import { expect, test } from "vitest";
import doc from "../fixtures/copy.json";
import { ApiError } from "../api/client";
import { copy, type ErrorCode, failure } from "./messages";

const fill = { planet: 4, target: 9 };
const filled = (s: string) => s.replace("{p}", "4").replace("{t}", "9");
const refused = (code: ErrorCode, retryAfter: number | null = null) =>
  new ApiError({ kind: "refused", status: 422, code, retryAfter });

test("every code has §6.1's copy for a screen and an order", () => {
  for (const [code, want] of Object.entries(doc.codes)) {
    expect(copy(code as ErrorCode, "page", fill), code).toBe(filled(want.page));
    expect(copy(code as ErrorCode, "command", fill), code).toBe(filled(want.command));
  }
  for (const [code, want] of Object.entries(doc.without_number)) {
    expect(copy(code as ErrorCode, "command"), code).toBe(want);
  }
});

test("the app's own states have no code and say whether an order went through", () => {
  for (const error of [new ApiError({ kind: "unreachable" }), new SyntaxError("bad body")]) {
    const page = failure(error, "page");
    const order = failure(error, "command");
    expect([page.code, order.code, page.kind]).toEqual([null, null, "retry"]);
    expect(Object.values(doc.web).flatMap((w) => [w.page, w.command])).toContain(page.copy);
    expect(order.copy).toContain("may or may not have gone through");
  }
  expect(failure(refused("future_code" as ErrorCode), "page").copy).toBe(doc.web.unexpected.page);
});

test("each failure has its kind, and a wait over 2 s is named", () => {
  expect(failure(refused("invalid_token"), "page").kind).toBe("login");
  expect(failure(refused("no_free_homeworld"), "page").kind).toBe("universe-full");
  expect(failure(refused("no_free_homeworld"), "command").kind).toBe("inline");
  expect(failure(refused("no_free_slot"), "command").kind).toBe("inline");
  const busy = (wait: number | null) => failure(refused("busy", wait), "page");
  expect(busy(4).kind).toBe("retry");
  expect(busy(2).copy).toBe("The game is busy. Try again in a moment.");
  expect(busy(3).copy).toBe("The game is busy. Try again in 3 seconds.");
});
