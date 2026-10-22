import { Button, Chip, Spinner, Toast } from "@heroui/react";
import { NavMenu } from "@kedar42/perseus";
import { Outlet, useLocation, useMatches } from "@tanstack/react-router";
import { useState } from "react";
import { login, logout } from "../auth/auth";
import { useAuth } from "../auth/authStore";
import { failure, ORDER_LOST } from "../copy/messages";
import { access } from "../copy/voices";
import type { components } from "../api/schema";
import { homeworldFirst } from "../game/empire";
import { useFleets, useMe } from "../game/queries";
import {
  FleetsIcon,
  LeaderboardIcon,
  LogoutIcon,
  MarkIcon,
  MenuIcon,
  PlanetsIcon,
  UniverseIcon,
} from "../theme/icons";
import { Failure, Notice } from "./Failure";
import { useNavStore } from "./navStore";
import { FastRules, TopBar } from "./TopBar";
import { useLayout } from "./useLayout";

const modes = { desktop: "inline", tablet: "compact", phone: "overlay" } as const;

export function AppShell() {
  const user = useAuth((s) => s.user);
  return user ? <Command /> : <Gate />;
}

/** Logged out: the access panel alone, with no navigation. */
function Gate() {
  const here = useLocation({ select: (l) => l.href });
  const lostOrder = useAuth((s) => s.lostOrder);
  return (
    <div className="flex min-h-dvh flex-col">
      <Bar />
      <main className="flex flex-1 items-center justify-center p-4">
        <section className="frame flex max-w-md flex-col gap-4 border border-separator bg-surface p-6">
          <h1 className="text-2xl">{access.heading}</h1>
          <p className="text-muted">{lostOrder ? ORDER_LOST : access.prompt}</p>
          <Button className="self-start" variant="primary" onPress={() => void login(here)}>
            Log in
          </Button>
        </section>
      </main>
    </div>
  );
}

/** Logged in: the first `/players/me` joins the game, so nothing else shows until it answers. */
function Command() {
  const me = useMe();
  if (me.isPending) {
    return (
      <div className="grid min-h-dvh place-items-center">
        <Spinner aria-label="Loading" />
      </div>
    );
  }
  if (me.isError) {
    const message = failure(me.error, "page");
    return (
      <div className="flex min-h-dvh flex-col">
        <Bar />
        <main className="flex flex-1 items-center justify-center p-4">
          {message.kind === "universe-full" ? (
            <Notice heading={access.full} copy={message.copy} code={message.code}>
              <Button className="self-start" variant="secondary" onPress={() => void logout()}>
                Log out
              </Button>
            </Notice>
          ) : (
            <Failure error={me.error} onRetry={() => void me.refetch()} />
          )}
        </main>
      </div>
    );
  }
  return <Frame me={me.data} />;
}

function Frame({ me }: { me: components["schemas"]["PlayerView"] }) {
  const { homeworld_id: homeworld, planet_ids: planets } = me;
  const layout = useLayout();
  const { desktopOpen, setDesktopOpen } = useNavStore();
  const [drawerOpen, setDrawerOpen] = useState(false);
  const pathname = useLocation({ select: (l) => l.pathname });
  const email = useAuth((s) => s.user?.profile.email ?? s.user?.profile.name);
  const fleets = useFleets();
  const flying = fleets.data?.fleets.filter((f) => f.state !== "stationed").length ?? 0;
  const open =
    layout === "desktop"
      ? { isOpen: desktopOpen, onOpenChange: setDesktopOpen }
      : layout === "phone"
        ? { isOpen: drawerOpen, onOpenChange: setDrawerOpen }
        : {};

  return (
    <div className="flex h-dvh">
      <NavMenu
        aria-label="Command"
        mode={modes[layout]}
        selectedValue={navValue(pathname)}
        selectedCategoryValue={pathname.split("/")[1] ?? ""}
        {...open}
      >
        <NavMenu.Header>
          <NavMenu.Toggle />
          <NavMenu.App icon={<MarkIcon />}>
            <span className="display">Voidforge</span>
          </NavMenu.App>
        </NavMenu.Header>
        <NavMenu.Content>
          <NavMenu.Category value="planets" label="Planets" icon={<PlanetsIcon />} defaultExpanded>
            <NavMenu.Item href="/planets" value="/planets">
              All holdings
            </NavMenu.Item>
            {homeworldFirst(planets, homeworld).map((id) => (
              <NavMenu.Item key={id} href={`/planets/${id.toString()}`} value={`/planets/${id.toString()}`}>
                {planetName(id, homeworld)}
              </NavMenu.Item>
            ))}
          </NavMenu.Category>
          <NavMenu.Item
            href="/fleets"
            value="/fleets"
            icon={<FleetsIcon />}
            aria-label={`Fleets, ${flying.toString()} in flight`}
            textValue={`Fleets, ${flying.toString()} in flight`}
            endContent={flying > 0 && <Chip size="sm">{flying}</Chip>}
          >
            Fleets
          </NavMenu.Item>
          <NavMenu.Item href="/universe" value="/universe" icon={<UniverseIcon />}>
            Universe
          </NavMenu.Item>
          <NavMenu.Item href="/leaderboard" value="/leaderboard" icon={<LeaderboardIcon />}>
            Leaderboard
          </NavMenu.Item>
        </NavMenu.Content>
        <NavMenu.Footer>
          <NavMenu.Item icon={<LogoutIcon />} onPress={() => void logout()}>
            Log out
          </NavMenu.Item>
        </NavMenu.Footer>
      </NavMenu>
      <div className="flex min-w-0 flex-1 flex-col">
        <TopBar
          me={me}
          email={email}
          layout={layout}
          menu={
            layout === "phone" && (
              <Button
                isIconOnly
                variant="ghost"
                aria-label="Open navigation"
                onPress={() => {
                  setDrawerOpen(true);
                }}
              >
                <MenuIcon />
              </Button>
            )
          }
        />
        <main className="min-h-0 flex-1 overflow-y-auto p-4 sm:p-6">
          <Title />
          <Outlet />
        </main>
      </div>
      <Toast.Provider placement="bottom end" />
    </div>
  );
}

/** The bar before the game is reachable: the mark and the rules when they're the fast ones. */
function Bar() {
  return (
    <header className="flex items-center gap-3 border-b border-separator px-4 py-3">
      <MarkIcon />
      <span className="display">Voidforge</span>
      <FastRules />
    </header>
  );
}

/** The screen's in-universe title, from its route. */
function Title() {
  const title = useMatches({ select: (m) => m.findLast((r) => r.staticData.title)?.staticData.title });
  return title ? <h1 className="mb-4 text-2xl">{title}</h1> : null;
}

const planetName = (id: number, homeworld: number) =>
  id === homeworld ? `#${id.toString()} Home` : `#${id.toString()}`;

/** The nav item a path selects: a planet's own item for anything under it. */
function navValue(pathname: string): string {
  const [, section, id] = pathname.split("/");
  return section === "planets" && id ? `/planets/${id}` : `/${section ?? ""}`;
}
