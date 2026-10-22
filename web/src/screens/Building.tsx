import { Drawer, Spinner } from "@heroui/react";
import { useNavigate, useParams } from "@tanstack/react-router";
import type { ReactNode } from "react";
import type { components } from "../api/schema";
import { copy } from "../copy/messages";
import { empties, failures } from "../copy/voices";
import { at, building, duration, mass, progress, rate, ship, stock } from "../format";
import { grown } from "../game/live";
import { useMe, usePlanet, useRules } from "../game/queries";
import { Failure, Notice } from "../shell/Failure";
import { useLayout } from "../shell/useLayout";
import { Actions, Confirm, OrderButton, Unavailable } from "./Orders";
import {
  type BuildingView,
  flowWords,
  type PlanetView,
  planetName,
  StatusWord,
  statusOf,
  useLive,
} from "./parts";

type S = components["schemas"];

/** A drawer over the planet: docked right on wide screens, a bottom sheet on a phone. */
function PlanetDrawer({
  what,
  children,
}: {
  what: string;
  children: (view: PlanetView, receivedAt: number) => ReactNode;
}) {
  const { planetId } = useParams({ from: "/game/planets/$planetId" });
  const navigate = useNavigate();
  const layout = useLayout();
  const planet = usePlanet(planetId);
  const name = planetName(planetId, useMe().data?.homeworld_id);
  const close = () => void navigate({ to: "/planets/$planetId", params: { planetId } });
  return (
    <Drawer.Backdrop
      isOpen
      onOpenChange={(open) => {
        if (!open) close();
      }}
    >
      <Drawer.Content placement={layout === "phone" ? "bottom" : "right"}>
        <Drawer.Dialog aria-label={`${name} · ${what}`}>
          <Drawer.Header>
            <Drawer.Heading className="display text-lg">
              {name} · {what}
            </Drawer.Heading>
            <Drawer.CloseTrigger />
          </Drawer.Header>
          <Drawer.Body className="flex flex-col gap-3">
            {planet.isPending ? (
              <Spinner aria-label="Loading" />
            ) : planet.isError ? (
              <Failure
                error={planet.error}
                fill={{ planet: planetId }}
                onRetry={() => void planet.refetch()}
              />
            ) : (
              children(planet.data, planet.dataUpdatedAt)
            )}
          </Drawer.Body>
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
  );
}

const Uncharted = () => <Notice heading={failures.uncharted} copy={failures.unchartedCopy} />;

/** One building's drawer, by id, which stays the building's for its whole life. */
export function BuildingDrawer() {
  const { buildingId } = useParams({ from: "/game/planets/$planetId/buildings/$buildingId" });
  const planet = usePlanet(useParams({ from: "/game/planets/$planetId" }).planetId);
  const type = planet.data?.buildings.find((b) => b.id === buildingId)?.type;
  return (
    <PlanetDrawer what={type ? building(type) : "Building"}>
      {(view, receivedAt) => {
        const b = view.buildings.find((x) => x.id === buildingId);
        return b ? <BuildingBody view={view} receivedAt={receivedAt} b={b} /> : <Uncharted />;
      }}
    </PlanetDrawer>
  );
}

/** A free slot's drawer: what each building costs and takes. Construction draws ingots as it goes. */
export function BuildDrawer() {
  return (
    <PlanetDrawer what="Free slot">
      {(view, receivedAt) =>
        view.buildings.length < view.slots ? (
          <BuildPicker view={view} receivedAt={receivedAt} />
        ) : (
          <Uncharted />
        )
      }
    </PlanetDrawer>
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
      {mine && <BuildingOrders planet={view.planet_id} b={b} hasBuilds={builds.length > 0} />}
    </>
  );
}

/** Cancel construction or Demolish, each behind its confirm. */
function BuildingOrders({ planet, b, hasBuilds }: { planet: number; b: BuildingView; hasBuilds: boolean }) {
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
          consequence={`Planet #${planet.toString()}. The ${mass(b.delivered_g)} of ingots delivered so far are lost, and the slot frees now.`}
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
        consequence={`Planet #${planet.toString()}. The ${mass(s.delivered_g)} of ingots delivered so far are lost.`}
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
