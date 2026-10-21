import { useLocation } from "@tanstack/react-router";
import { login, logout } from "./auth";
import { useAuth } from "./authStore";

export function AuthControls() {
  const user = useAuth((s) => s.user);
  const here = useLocation({ select: (l) => l.href });
  if (!user) {
    return (
      <button type="button" onClick={() => void login(here)}>
        Log in
      </button>
    );
  }
  return (
    <span>
      Logged in as {user.profile.email ?? user.profile.name}{" "}
      <button type="button" onClick={() => void logout()}>
        Log out
      </button>
    </span>
  );
}
