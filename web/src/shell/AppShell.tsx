import { Button, Spinner, Toast } from "@heroui/react";
import { NavMenu, useNavMenu } from "@kedar42/perseus";
import { Outlet, useLocation, useMatches } from "@tanstack/react-router";
import { type ReactNode, useState } from "react";
import { login, logout } from "../auth/auth";
import { useAuth } from "../auth/authStore";
import { failure, ORDER_LOST } from "../copy/messages";
import { access } from "../copy/voices";
import type { components } from "../api/schema";
import { useEmpire } from "../game/empire";
import { useMe } from "../game/queries";
import {
  FleetsIcon,
  LeaderboardIcon,
  LogoutIcon,
  MarkIcon,
  MenuIcon,
  PlanetsIcon,
  UniverseIcon,
} from "../theme/icons";
import { Failure, Notice, Stale } from "./Failure";
import { useNavStore } from "./navStore";
import { FastRules, Partial, TopBar } from "./TopBar";
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
  if (me.data) return <Frame me={me.data} stale={<Stale query={me} />} />;
  if (!me.isError) {
    return (
      <div className="grid min-h-dvh place-items-center">
        <Spinner aria-label="Loading" />
      </div>
    );
  }
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

function Frame({ me, stale }: { me: components["schemas"]["PlayerView"]; stale: ReactNode }) {
  const layout = useLayout();
  const { desktopOpen, setDesktopOpen } = useNavStore();
  const [drawerOpen, setDrawerOpen] = useState(false);
  const pathname = useLocation({ select: (l) => l.pathname });
  const email = useAuth((s) => s.user?.profile.email ?? s.user?.profile.name);
  const empire = useEmpire(me);
  const { loaded, partial, used, slots, flying, stationed } = empire;
  const owned = me.planet_ids.length;
  const holdings = loaded
    ? `Planets: ${owned.toString()} ${owned === 1 ? "planet" : "planets"}, ${used.toString()} of ${slots.toString()} slots used${partial ? `, ${partial}` : ""}`
    : "Planets";
  const fleetsLabel = empire.fleets.data
    ? `Fleets: ${flying.toString()} in flight, ${stationed.toString()} stationed`
    : "Fleets";
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
        selectedValue={`/${pathname.split("/")[1] ?? ""}`}
        {...open}
      >
        <NavMenu.Header>
          <NavMenu.Toggle />
          <NavMenu.App icon={<MarkIcon />}>
            <span className="display">Voidforge</span>
          </NavMenu.App>
        </NavMenu.Header>
        <NavMenu.Content>
          <NavMenu.Item
            href="/planets"
            value="/planets"
            icon={<PlanetsIcon />}
            aria-label={holdings}
            textValue={holdings}
            endContent={
              loaded && (
                <span className="text-xs text-muted">
                  {owned} · {used}/{slots} <Partial label={partial} />
                </span>
              )
            }
          >
            Planets
          </NavMenu.Item>
          <NavMenu.Item
            href="/fleets"
            value="/fleets"
            icon={<FleetsIcon />}
            aria-label={fleetsLabel}
            textValue={fleetsLabel}
            endContent={
              empire.fleets.data && (
                <span className="text-xs text-muted">
                  {flying} flying · {stationed} stationed
                </span>
              )
            }
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
          <Account email={email} />
          <NavMenu.Item
            icon={<LogoutIcon />}
            textValue={email ? `Log out ${email}` : "Log out"}
            onPress={() => void logout()}
          >
            Log out
          </NavMenu.Item>
        </NavMenu.Footer>
      </NavMenu>
      <div className="flex min-w-0 flex-1 flex-col">
        <TopBar
          me={me}
          empire={empire}
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
          {stale}
          <Title />
          <Outlet />
        </main>
      </div>
      <Toast.Provider placement="bottom end" />
    </div>
  );
}

/** Who is signed in, above Log out. The rail has no room for it, so Log out's tooltip names them. */
function Account({ email }: { email: string | undefined }) {
  const { isCompact } = useNavMenu();
  return email && !isCompact ? (
    <p className="truncate px-3 py-1 text-xs text-muted" title={email}>
      {email}
    </p>
  ) : null;
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

