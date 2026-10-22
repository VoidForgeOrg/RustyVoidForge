import createFetchClient from "openapi-fetch";
import createClient from "openapi-react-query";
import { logout } from "../auth/auth";
import { useAuth } from "../auth/authStore";
import type { components, paths } from "./schema";

export type Failure =
  | {
      kind: "refused";
      status: number;
      code: components["schemas"]["ErrorCode"];
      retryAfter: number | null;
    }
  | { kind: "unreachable" }
  | { kind: "unexpected"; status: number };

/** Every failed call rejects with this, so a screen handles one shape. */
export class ApiError extends Error {
  constructor(readonly failure: Failure) {
    super(failure.kind);
  }
}

async function refusal(response: Response): Promise<Failure> {
  const body: unknown = await response.json().catch(() => null);
  const code = typeof body === "object" && body !== null && "code" in body ? body.code : null;
  if (typeof code !== "string") return { kind: "unexpected", status: response.status };
  const wait = Number(response.headers.get("Retry-After"));
  return {
    kind: "refused",
    status: response.status,
    code: code as components["schemas"]["ErrorCode"],
    retryAfter: Number.isSafeInteger(wait) && wait > 0 ? wait : null,
  };
}

export const fetchClient = createFetchClient<paths>({ baseUrl: import.meta.env.VITE_API_URL });

// The only place the ID token is read. A 401 means dex's token is no longer accepted, so the
// login ends and the player is offered a new one. Every POST is an order.
fetchClient.use({
  onRequest({ request }) {
    const user = useAuth.getState().user;
    if (user?.expired) void logout(request.method === "POST");
    else if (user?.id_token) request.headers.set("Authorization", `Bearer ${user.id_token}`);
    return request;
  },
  async onResponse({ request, response }) {
    if (response.ok) return response;
    if (response.status === 401) void logout(request.method === "POST");
    throw new ApiError(await refusal(response));
  },
  onError({ error }) {
    const aborted = error instanceof DOMException && error.name === "AbortError";
    return aborted ? error : new ApiError({ kind: "unreachable" });
  },
});

export const api = createClient(fetchClient);
