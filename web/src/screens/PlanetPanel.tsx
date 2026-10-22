import { Button, buttonVariants, Drawer } from "@heroui/react";
import { Link, useMatch, useNavigate, useParams, useRouter, useSearch } from "@tanstack/react-router";
import { type ReactNode, useEffect, useRef } from "react";
import type { components } from "../api/schema";
import { copy } from "../copy/messages";
import { empties, failures } from "../copy/voices";
import {
  at,
  building,
  duration,
  mass,
  owner,
  percent,
  progress,
  rate,
  ship,
  shipCounts,
  ships,
  stock,
} from "../format";
import { grown } from "../game/live";
import { useMe, usePlanet, useRules } from "../game/queries";
import { Notice, Read } from "../shell/Failure";
import { useLayout } from "../shell/useLayout";
import {
  CargoVesselIcon,
  ColonyShipIcon,
  DrillIcon,
  EmptySlotIcon,
  RefineryIcon,
  ShipyardIcon,
} from "../theme/icons";
import { MissionModal } from "./Mission";
import { Actions, Confirm, OrderButton, Unavailable } from "./Orders";
import {
  type BuildingView,
  flowWords,
  Meter,
  type PlanetView,
  planetName,
  StatusWord,
  statusOf,
  useLive,
} from "./parts";

const buildingIcons = { drill: <DrillIcon />, refinery: <RefineryIcon />, shipyard: <ShipyardIcon /> };
const shipIcons = { colony_ship: <ColonyShipIcon />, cargo_vessel: <CargoVesselIcon /> };

type S = components["schemas"];

/**
 * The planet's side panel, on the planet route and its children, which name its state: the planet
 * itself, one of its buildings, or the build picker. It stays mounted while the state changes, so a
 * tile or Back switches its body without closing it.
 */
export function PlanetPanel() {
  const { planetId } = useParams({ from: "/game/planets/$planetId" });
  const { mission } = useSearch({ from: "/game/planets/$planetId" });
  const atBuilding = useMatch({ from: "/game/planets/$planetId/buildings/$buildingId", shouldThrow: false });
  const atBuild = useMatch({ from: "/game/planets/$planetId/build", shouldThrow: false });
  const navigate = useNavigate();
  const layout = useLayout();
  const me = useMe().data;
  const planet = usePlanet(planetId);
  const name = planetName(planetId, me?.homeworld_id);
  const buildingId = atBuilding?.params.buildingId;
  const type = planet.data?.buildings.find((b) => b.id === buildingId)?.type;
  const what = buildingId !== undefined ? (type ? building(type) : "Building") : atBuild ? "Free slot" : null;
  const title = what ? `${name} · ${what}` : name;
  const toPlanet = () => void navigate({ to: "/planets/$planetId", params: { planetId } });

  // Back never adds a history entry: it pops the planet entry this panel stepped in from, and after
  // a deep link, where there is none, it replaces the current one.
  const router = useRouter();
  const inner = what !== null;
  const steppedIn = useRef(false);
  const wasInner = useRef(inner);
  const lastBuilding = useRef<number | null>(null);
  useEffect(() => {
    if (inner && !wasInner.current) steppedIn.current = true;
    if (!inner) {
      steppedIn.current = false;
      if (lastBuilding.current !== null) {
        document.querySelector<HTMLElement>(`[data-building="${lastBuilding.current.toString()}"]`)?.focus();
      }
    }
    lastBuilding.current = buildingId ?? null;
    wasInner.current = inner;
  }, [inner, buildingId]);
  const back = () => {
    if (steppedIn.current) router.history.back();
    else void navigate({ to: "/planets/$planetId", params: { planetId }, replace: true });
  };

  // One dialog at a time: the stepper stands in for the panel, and closing it brings the panel back.
  if (mission) return <MissionModal planet={planetId} onClose={toPlanet} />;
  return (
    <Drawer.Backdrop
      isOpen
      onOpenChange={(open) => {
        if (!open) void navigate({ to: "/planets" });
      }}
    >
      <Drawer.Content placement={layout === "phone" ? "bottom" : "right"}>
        <Drawer.Dialog aria-label={title} className={layout === "phone" ? "h-dvh" : ""}>
          <Drawer.Header>
            {what && (
              <Button variant="ghost" size="sm" aria-label={`Back to ${name}`} onPress={back}>
                ← Back
              </Button>
            )}
            <Drawer.Heading className="display text-lg">{title}</Drawer.Heading>
            <Drawer.CloseTrigger />
          </Drawer.Header>
          <Drawer.Body className="flex flex-col gap-3">
            <Read query={planet} fill={{ planet: planetId }} loading="Loading">
              {(view) => {
                const receivedAt = planet.dataUpdatedAt;
                if (buildingId !== undefined) {
                  const b = view.buildings.find((x) => x.id === buildingId);
                  return b ? <BuildingBody view={view} receivedAt={receivedAt} b={b} /> : <Uncharted />;
                }
                if (atBuild) {
                  return view.buildings.length < view.slots ? (
                    <BuildPicker view={view} receivedAt={receivedAt} />
                  ) : (
                    <Uncharted />
                  );
                }
                return <PlanetBody view={view} receivedAt={receivedAt} me={me?.player_id} />;
              }}
            </Read>
          </Drawer.Body>
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
  );
}

const Uncharted = () => <Notice heading={failures.uncharted} copy={failures.unchartedCopy} />;

/** The planet's own view: its stocks, its building tiles and, on the player's own, its roster. */
function PlanetBody({
  view,
  receivedAt,
  me,
}: {
  view: PlanetView;
  receivedAt: number;
  me: string | undefined;
}) {
  const { ran } = useLive(view, receivedAt);
  const mine = view.owner === me;
  return (
    <>
      {!mine && me && <p className="text-sm text-muted">Owner: {owner(view.owner, me)}</p>}
      <Meter name="Iron Ore" stock={view.stocks.iron_ore} ran={ran} tone="ore" />
      <Meter name="Iron Ingots" stock={view.stocks.iron_ingot} ran={ran} tone="ingot" />
      <Slots view={view} ran={ran} mine={mine} />
      {mine && me && <Roster view={view} me={me} />}
    </>
  );
}

function BuildingBody({ view, receivedAt, b }: { view: PlanetView; receivedAt: number; b: BuildingView }) {
  const live = useLive(view, receivedAt);
  const status = statusOf(b, view.ship_builds);
  const mine = view.owner === useMe().data?.player_id;
  const builds = view.ship_builds.filter((s) => s.shipyard_id === b.id);
  return (
    <>
      <Row label="Status">
        <StatusWord status={status} />
      </Row>
      {status !== "idle" && flowWords(b) && <p className="text-sm text-muted">{flowWords(b)}</p>}
      {b.state === "under_construction" && (
        <>
          <Row label="Delivered">
            {progress(grown(b.delivered_g, b.rate_g_per_h, live.ran, b.cost_g), b.cost_g)}
          </Row>
          <Row label="Done">{b.eta ? at(b.eta, live.gameNow) : "waiting for ingots"}</Row>
        </>
      )}
      {b.state === "demolishing" && <Row label="Slot frees">{at(b.demolition_ends_at, live.gameNow)}</Row>}
      {b.type === "shipyard" && b.state === "operational" && (
        <ShipBuilds
          shipyard={b.id}
          builds={builds}
          ran={live.ran}
          now={live.gameNow}
          planet={mine ? view.planet_id : null}
        />
      )}
      {mine && <BuildingOrders planet={view.planet_id} b={b} hasBuilds={builds.length > 0} ran={live.ran} />}
    </>
  );
}

/** Cancel construction or Demolish, each behind its confirm. */
function BuildingOrders({
  planet,
  b,
  hasBuilds,
  ran,
}: {
  planet: number;
  b: BuildingView;
  hasBuilds: boolean;
  ran: number;
}) {
  const navigate = useNavigate();
  const back = () => void navigate({ to: "/planets/$planetId", params: { planetId: planet } });
  const rules = useRules().data;
  const name = `${building(b.type)} #${b.id.toString()}`;
  const ref = { building_id: b.id };
  if (b.state === "under_construction") {
    return (
      <Actions>
        <Confirm
          order={{ type: "cancel_construction", payload: ref, planet, building: b.type }}
          label={`Cancel construction of ${name}`}
          question={`Cancel construction of ${name}?`}
          consequence={`Planet #${planet.toString()}. The ${mass(grown(b.delivered_g, b.rate_g_per_h, ran, b.cost_g))} of ingots delivered so far are lost, and the slot frees now.`}
          onDone={back}
        />
      </Actions>
    );
  }
  if (b.state !== "operational") return null;
  if (hasBuilds) {
    return (
      <Actions>
        <Unavailable reason={copy("shipyard_has_builds", "command") ?? ""}>Demolish {name}</Unavailable>
      </Actions>
    );
  }
  const frees = rules
    ? `The slot frees in ${duration(rules.buildings[b.type].demolition_time_ms)}, when demolition ends.`
    : "The slot frees when demolition ends.";
  return (
    <Actions>
      <Confirm
        order={{ type: "demolish_building", payload: ref, planet, building: b.type }}
        label={`Demolish ${name}`}
        question={`Demolish ${name}?`}
        consequence={`Planet #${planet.toString()}. It stops working now, and nothing is refunded. ${frees}`}
        onDone={back}
      />
    </Actions>
  );
}

/** A Shipyard's lanes and queue; on the player's own planet, with Queue and Cancel. */
function ShipBuilds({
  shipyard,
  builds,
  ran,
  now,
  planet,
}: {
  shipyard: number;
  builds: S["ShipBuildView"][];
  ran: number;
  now: number;
  planet: number | null;
}) {
  const rules = useRules().data;
  const active = builds.filter((s) => s.state === "active");
  const waiting = builds.filter((s) => s.state === "waiting");
  const cancel = (s: S["ShipBuildView"]) =>
    planet !== null && (
      <Confirm
        order={{ type: "cancel_ship_build", payload: { ship_build_id: s.id }, planet }}
        label={`Cancel build #${s.id.toString()}`}
        question={`Cancel build #${s.id.toString()} (${ship(s.ship_type)}) at Shipyard #${s.shipyard_id.toString()}?`}
        consequence={`Planet #${planet.toString()}. The ${mass(grown(s.delivered_g, s.rate_g_per_h, ran, s.cost_g))} of ingots delivered so far are lost.`}
      />
    );
  return (
    <>
      <h3 className="display mt-2 text-muted">Build lanes</h3>
      {active.length === 0 && <p className="text-sm text-muted">No ships building.</p>}
      {active.map((s) => (
        <Row key={s.id} label={ship(s.ship_type)}>
          {progress(grown(s.delivered_g, s.rate_g_per_h, ran, s.cost_g), s.cost_g)}
          {s.eta && <span className="block text-xs text-muted">done {at(s.eta, now)}</span>}
          {cancel(s)}
        </Row>
      ))}
      {waiting.length > 0 && (
        <>
          <h3 className="display mt-2 text-muted">Queue</h3>
          {waiting.map((s) => (
            <Row key={s.id} label={ship(s.ship_type)}>
              {cancel(s)}
            </Row>
          ))}
        </>
      )}
      {planet !== null && rules && (
        <>
          <h3 className="display mt-2 text-muted">Queue a ship</h3>
          {(["colony_ship", "cargo_vessel"] as const).map((type) => (
            <Row key={type} label={ship(type)}>
              {mass(rules.ships[type].cost_g)} · {duration(rules.ships[type].build_time_ms)}
              <span className="mt-1 block">
                <OrderButton
                  order={{ type: "queue_ship", payload: { shipyard_id: shipyard, ship_type: type }, planet }}
                >
                  Queue {ship(type)}
                </OrderButton>
              </span>
            </Row>
          ))}
        </>
      )}
    </>
  );
}

function BuildPicker({ view, receivedAt }: { view: PlanetView; receivedAt: number }) {
  const rules = useRules().data;
  const navigate = useNavigate();
  const { ran } = useLive(view, receivedAt);
  const planet = view.planet_id;
  const mine = useMe().data?.planet_ids.includes(planet) ?? false;
  const ingots = view.stocks.iron_ingot;
  return (
    <>
      <p className="text-muted">{empties.slot}</p>
      <Row label="Ingots in storage">
        {stock(grown(ingots.amount_g, ingots.rate_g_per_h, ran, ingots.cap_g), ingots.cap_g)} ·{" "}
        {rate(ingots.rate_g_per_h)}
      </Row>
      <p className="text-sm text-muted">
        Ingots are drawn as it builds; construction halts when they run out.
      </p>
      {rules &&
        (["drill", "refinery", "shipyard"] as const).map((type) => (
          <Row key={type} label={building(type)}>
            {mass(rules.buildings[type].cost_g)} · {duration(rules.buildings[type].build_time_ms)}
            {mine && (
              <span className="mt-1 block">
                <OrderButton
                  variant="primary"
                  order={{ type: "build_building", payload: { planet_id: planet, building_type: type } }}
                  onDone={() => void navigate({ to: "/planets/$planetId", params: { planetId: planet } })}
                >
                  Build {building(type)}
                </OrderButton>
              </span>
            )}
          </Row>
        ))}
    </>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-4 border-b border-separator py-2">
      <span className="display text-muted">{label}</span>
      <span className="text-right">{children}</span>
    </div>
  );
}

const tile = "relative grid h-19 w-19 place-items-center border bg-field text-2xl";

/**
 * The buildings in id order, then one "+" per free slot: slots are interchangeable. Only the
 * player's own free slots open the build picker.
 */
function Slots({ view, ran, mine }: { view: PlanetView; ran: number; mine: boolean }) {
  const buildings = [...view.buildings].sort((a, b) => a.id - b.id);
  const free = Math.max(0, view.slots - buildings.length);
  return (
    <ul className="my-3 grid grid-cols-[repeat(auto-fill,4.75rem)] gap-2.5" aria-label="Building slots">
      {buildings.map((b) => {
        const status = statusOf(b, view.ship_builds);
        const done =
          b.state === "under_construction"
            ? ` ${percent(grown(b.delivered_g, b.rate_g_per_h, ran, b.cost_g), b.cost_g).toString()}%`
            : "";
        return (
          <li key={b.id}>
            <Link
              to="/planets/$planetId/buildings/$buildingId"
              params={{ planetId: view.planet_id, buildingId: b.id }}
              data-building={b.id}
              aria-label={`${building(b.type)}, ${status}${done}`}
              className={`${tile} border-border hover:border-accent`}
            >
              {buildingIcons[b.type]}
              <span className="absolute inset-x-0 bottom-1 text-center text-[10px]">
                <StatusWord status={status} />
              </span>
            </Link>
          </li>
        );
      })}
      {Array.from({ length: free }, (_, i) => (
        <li key={`free-${i.toString()}`}>
          {mine ? (
            <Link
              to="/planets/$planetId/build"
              params={{ planetId: view.planet_id }}
              aria-label="Free slot"
              className={`${tile} border-dashed border-border text-muted hover:border-accent`}
            >
              <EmptySlotIcon />
            </Link>
          ) : (
            <span
              role="img"
              aria-label="Free slot"
              className={`${tile} border-dashed border-separator text-muted`}
            >
              <EmptySlotIcon />
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}

function Roster({ view, me }: { view: PlanetView; me: string }) {
  const counts = shipCounts(view.roster);
  const ours = view.roster.some((s) => s.owner === me);
  return (
    <div className="flex flex-wrap items-center gap-3 border-t border-separator pt-3">
      <span className="display text-muted">Ships here</span>
      {counts.length === 0 && <span className="text-muted">{empties.roster}</span>}
      {counts.map(([type, n]) => (
        <span key={type} className="inline-flex items-center gap-1" title={ship(type)}>
          {shipIcons[type]}
          {ships(n, type)}
        </span>
      ))}
      {ours && (
        <Link
          to="/planets/$planetId"
          params={{ planetId: view.planet_id }}
          search={{ mission: true }}
          className={`ml-auto ${buttonVariants({ variant: "primary", size: "sm" })}`}
        >
          New mission
        </Link>
      )}
    </div>
  );
}
