import createFetchClient from "openapi-fetch";
import createClient from "openapi-react-query";
import { logout } from "../auth/auth";
import { useAuth } from "../auth/authStore";
import type { paths } from "./schema";

export const fetchClient = createFetchClient<paths>({ baseUrl: import.meta.env.VITE_API_URL });

// The only place the ID token is read. A 401 means dex's token is no longer accepted, so the
// login ends and the player is offered a new one.
fetchClient.use({
  onRequest({ request }) {
    const user = useAuth.getState().user;
    if (user?.expired) void logout();
    else if (user?.id_token) request.headers.set("Authorization", `Bearer ${user.id_token}`);
    return request;
  },
  onResponse({ response }) {
    if (response.status === 401) void logout();
    return response;
  },
});

export const api = createClient(fetchClient);
