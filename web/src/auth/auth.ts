import { type User, UserManager, WebStorageStateStore } from "oidc-client-ts";
import { useAuth } from "./authStore";

// dex issues no refresh token without offline_access and has no end-session endpoint, so a login
// lasts as long as its ID token and logging out only forgets it.
const manager = new UserManager({
  authority: import.meta.env.VITE_OIDC_ISSUER,
  client_id: import.meta.env.VITE_OIDC_CLIENT_ID,
  redirect_uri: `${window.location.origin}/callback`,
  response_type: "code",
  scope: "openid email profile",
  userStore: new WebStorageStateStore({ store: window.sessionStorage }),
  automaticSilentRenew: false,
});

function show(user: User | null) {
  const shown = user && !user.expired ? user : null;
  useAuth.setState(shown ? { user: shown, lostOrder: false } : { user: null });
}

manager.events.addUserLoaded(show);
manager.events.addUserUnloaded(() => {
  show(null);
});

export async function restoreLogin() {
  show(await manager.getUser());
}

export function login(returnTo: string) {
  return manager.signinRedirect({ state: returnTo });
}

/** Forgets the login; `lostOrder` says it ended on an order that wasn't sent. */
export function logout(lostOrder = false) {
  if (lostOrder) useAuth.setState({ lostOrder });
  return manager.removeUser();
}

/** Completes the redirect from dex and answers where to go next. */
export async function completeLogin(): Promise<string> {
  const user = await manager.signinRedirectCallback();
  return returnPath(user.state);
}

/** Only a path on this origin, so the login can never redirect off-site. */
export function returnPath(state: unknown): string {
  if (typeof state !== "string" || !state.startsWith("/")) return "/";
  return state.startsWith("//") || state.startsWith("/\\") ? "/" : state;
}
