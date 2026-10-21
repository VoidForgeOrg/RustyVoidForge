import { api } from "../api/client";
import { useAuth } from "../auth/authStore";

export function Overview() {
  const loggedIn = useAuth((s) => s.user !== null);
  const me = api.useQuery("get", "/players/me", {}, { enabled: loggedIn });
  return (
    <>
      <h1>Overview</h1>
      {loggedIn && me.data && <p>Player {me.data.player_id}</p>}
    </>
  );
}
