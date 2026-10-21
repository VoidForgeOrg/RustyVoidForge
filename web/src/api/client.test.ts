import { User } from "oidc-client-ts";
import { afterEach, expect, test, vi } from "vitest";
import { useAuth } from "../auth/authStore";
import { fetchClient } from "./client";

// openapi-fetch keeps the global fetch it finds at import, so the stub must exist first.
const fetchMock = vi.hoisted(() => {
  const mock = vi.fn<typeof fetch>();
  globalThis.fetch = mock;
  return mock;
});

function loggedIn(idToken: string, expiresAt: number) {
  const profile = { sub: "alice", iss: "http://dex", aud: "voidforge-spa", exp: expiresAt, iat: 0 };
  const user = new User({
    id_token: idToken,
    access_token: "",
    token_type: "Bearer",
    session_state: null,
    profile,
    expires_at: expiresAt,
  });
  useAuth.setState({ user });
}

const inAnHour = () => Math.floor(Date.now() / 1000) + 3600;

afterEach(() => {
  useAuth.setState({ user: null });
  fetchMock.mockReset();
});

test("a logged-in call carries the ID token as its bearer token", async () => {
  loggedIn("alice-id-token", inAnHour());
  fetchMock.mockResolvedValue(Response.json({}));
  await fetchClient.GET("/players/me");
  const request = fetchMock.mock.calls[0]?.[0] as Request;
  expect(request.headers.get("Authorization")).toBe("Bearer alice-id-token");
});

test("a 401 ends the login", async () => {
  loggedIn("stale-id-token", inAnHour());
  fetchMock.mockResolvedValue(Response.json({ code: "unauthenticated" }, { status: 401 }));
  await fetchClient.GET("/players/me");
  await vi.waitFor(() => {
    expect(useAuth.getState().user).toBeNull();
  });
});

test("an expired login sends no token and ends", async () => {
  loggedIn("expired-id-token", Math.floor(Date.now() / 1000) - 1);
  fetchMock.mockResolvedValue(Response.json({}));
  await fetchClient.GET("/players/me");
  const request = fetchMock.mock.calls[0]?.[0] as Request;
  expect(request.headers.get("Authorization")).toBeNull();
  await vi.waitFor(() => {
    expect(useAuth.getState().user).toBeNull();
  });
});
