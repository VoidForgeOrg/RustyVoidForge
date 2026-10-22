import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { useAuth } from "../auth/authStore";
import { logIn, serve } from "../test-harness";
import { type Order, useOrder } from "./commands";
import { useMe } from "./queries";

// openapi-fetch keeps the global fetch it finds at import, so the stub must exist first.
const fetchMock = vi.hoisted(() => {
  const mock = vi.fn<typeof fetch>();
  globalThis.fetch = mock;
  return mock;
});

const asOf = "2026-10-21T12:00:00.000Z";
const me = { as_of: asOf, valid_until: null, player_id: "p", homeworld_id: 4, planet_ids: [4], score: 0 };
const accepted = () => Response.json({ seq: 5, game_time: asOf, events: [] });
const send: Order = {
  type: "send_fleet",
  payload: { fleet_id: 11, target_planet_id: 7, mission: "colonize" },
};

const sent = () =>
  fetchMock.mock.calls
    .map(([r]) => r as Request)
    .filter((r) => new URL(r.url).pathname === "/commands")
    .map((r) => ({ key: r.headers.get("Idempotency-Key") ?? "", trace: r.headers.get("traceparent") ?? "" }));
const meReads = () =>
  fetchMock.mock.calls.filter(([r]) => new URL((r as Request).url).pathname === "/players/me").length;

function hooks() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return renderHook(() => ({ order: useOrder(), me: useMe() }), { wrapper }).result;
}

beforeEach(() => {
  logIn();
});

afterEach(() => {
  cleanup();
  useAuth.setState({ user: null });
  globalThis.fetch = fetchMock;
});

test("an order's attempts share one trace, its key, and the next order starts another", async () => {
  const unavailable = () => Response.json({ code: "unavailable", message: "" }, { status: 503 });
  const answers = [unavailable, accepted, accepted];
  serve(fetchMock, {
    "/players/me": Response.json(me),
    "/players/me/fleets": Response.json({ as_of: asOf, valid_until: null, fleets: [] }),
    "/commands": () => (answers.shift() ?? accepted)(),
  });
  const result = hooks();
  for (let i = 0; i < 3; i++) {
    await act(() => result.current.order.send(send));
  }
  const [first, retry, next] = sent();
  for (const s of [first, retry, next]) {
    expect(s?.trace).toMatch(/^00-[0-9a-f]{32}-[0-9a-f]{16}-01$/);
    expect(s?.trace.split("-")[1]).toBe(s?.key.replaceAll("-", ""));
  }
  expect(retry?.key).toBe(first?.key);
  expect(next?.key).not.toBe(first?.key);
});

test("an accepted send reads the player again, whose valid_until moves to the arrival", async () => {
  serve(fetchMock, {
    "/players/me": Response.json(me),
    "/players/me/fleets": Response.json({ as_of: asOf, valid_until: null, fleets: [] }),
    "/commands": accepted,
  });
  const result = hooks();
  await waitFor(() => {
    expect(result.current.me.data).toBeDefined();
  });
  const before = meReads();
  await act(() => result.current.order.send(send));
  expect(meReads()).toBe(before + 1);
});
