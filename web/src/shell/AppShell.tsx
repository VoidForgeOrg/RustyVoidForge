import { Chip } from "@heroui/react";
import { Outlet, useRouter } from "@tanstack/react-router";
import { RouterProvider } from "react-aria-components";
import { api } from "../api/client";

export function AppShell() {
  const router = useRouter();
  return (
    <RouterProvider navigate={(href) => void router.navigate({ href })}>
      <header>
        <RulesChip />
      </header>
      <main>
        <Outlet />
      </main>
    </RouterProvider>
  );
}

// The fast rules turn an hour into a second, so a player must always see them.
function RulesChip() {
  const { data } = api.useQuery("get", "/rules", {}, { staleTime: Infinity });
  if (data?.name !== "v0-fast") return null;
  return (
    <Chip color="warning" variant="soft">
      fast test rules
    </Chip>
  );
}
