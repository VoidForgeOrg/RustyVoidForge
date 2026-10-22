import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { render } from "@testing-library/react";
import { User } from "oidc-client-ts";
import { vi } from "vitest";
import { useAuth } from "./auth/authStore";
import { routeTree } from "./router";

// Test-only: the app at a path against canned API answers, in a wide window.

type Answer = Response | ((request: Request) => Response | Promise<Response>);

/** Answers each request by path, and 404 `not_found` for anything else. */
export function serve(fetchMock: ReturnType<typeof vi.fn<typeof fetch>>, answers: Record<string, Answer>) {
  fetchMock.mockImplementation((input) => {
    const request = input as Request;
    const answer =
      answers[new URL(request.url).pathname] ??
      Response.json({ code: "not_found", message: "" }, { status: 404 });
    return Promise.resolve(typeof answer === "function" ? answer(request) : answer.clone());
  });
}

export function renderAt(path: string) {
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: [path] }) });
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

export function logIn() {
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

/** A desktop-wide window, so every layout renders its widest form. */
export function wideWindow() {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: true,
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  }));
}
