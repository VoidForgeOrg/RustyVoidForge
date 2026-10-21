import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, screen } from "@testing-library/react";
import { User } from "oidc-client-ts";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { useAuth } from "../auth/authStore";
import { routeTree } from "../router";

// openapi-fetch keeps the global fetch it finds at import, so the stub must exist first.
const fetchMock = vi.hoisted(() => {
  const mock = vi.fn<typeof fetch>();
  globalThis.fetch = mock;
  return mock;
});

const me = {
  as_of: "2026-10-21T12:00:00.000Z",
  valid_until: null,
  player_id: "01890a5d-ac96-774b-bcce-b302099a8057",
  homeworld_id: 63,
  planet_ids: [63, 26],
  score: 1_240,
};

type Answer = Response | (() => Response);

function serve(answers: Record<string, Answer>) {
  fetchMock.mockImplementation((input) => {
    const path = new URL((input as Request).url).pathname;
    const answer = answers[path] ?? Response.json({ code: "not_found", message: "" }, { status: 404 });
    return Promise.resolve(typeof answer === "function" ? answer() : answer.clone());
  });
}

function renderAt(path: string) {
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: [path] }) });
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

function logIn() {
  const exp = Math.floor(Date.now() / 1000) + 3600;
  useAuth.setState({
    user: new User({
      id_token: "t",
      access_token: "",
      token_type: "Bearer",
      session_state: null,
      profile: { sub: "alice", iss: "i", aud: "a", exp, iat: 0, email: "alice@voidforge.test" },
      expires_at: exp,
    }),
  });
}

beforeEach(() => {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: true,
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  }));
  serve({ "/rules": Response.json({ name: "v0-fast" }) });
});

afterEach(() => {
  cleanup();
  useAuth.setState({ user: null });
  vi.unstubAllGlobals();
  globalThis.fetch = fetchMock;
});

test("logged out, the shell is the access panel alone, still naming the fast rules", async () => {
  renderAt("/planets");
  expect(await screen.findByRole("heading", { name: "Imperial Access" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Log in" })).toBeInTheDocument();
  expect(await screen.findByText("fast test rules")).toBeInTheDocument();
  expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
});

test("the nav selects the planet being shown", async () => {
  logIn();
  serve({ "/players/me": Response.json(me), "/players/me/fleets": Response.json({ as_of: me.as_of, valid_until: null, fleets: [] }) });
  renderAt("/planets/26");
  expect(await screen.findByRole("link", { name: "#26" })).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("link", { name: "#63 Home" })).not.toHaveAttribute("aria-current");
});

test("a busy server's read shows the retry frame with its wait", async () => {
  logIn();
  serve({
    "/players/me": () =>
      Response.json({ code: "busy", message: "" }, { status: 503, headers: { "Retry-After": "4" } }),
  });
  renderAt("/planets");
  expect(await screen.findByRole("heading", { name: "Transmission failed" })).toBeInTheDocument();
  expect(screen.getByText(/The game is busy\. Try again in 4 seconds\./)).toBeInTheDocument();
  expect(screen.getByText("(busy)")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
});

test("a planet number the API can't take is uncharted, before any call for it", async () => {
  logIn();
  serve({ "/players/me": Response.json(me), "/players/me/fleets": Response.json({ as_of: me.as_of, valid_until: null, fleets: [] }) });
  renderAt("/planets/abc");
  expect(await screen.findByRole("heading", { name: "Uncharted coordinates" })).toBeInTheDocument();
  const asked = fetchMock.mock.calls.map(([r]) => new URL((r as Request).url).pathname);
  expect(asked.filter((p) => p.startsWith("/planets"))).toEqual([]);
});
