import { Drawer, Spinner } from "@heroui/react";
import { useNavigate, useParams } from "@tanstack/react-router";
import type { ReactNode } from "react";
import type { components } from "../api/schema";
import { empties, failures } from "../copy/voices";
import { at, building, duration, mass, progress, ship } from "../format";
import { grown } from "../game/live";
import { useMe, usePlanet, useRules } from "../game/queries";
import { Failure, Notice } from "../shell/Failure";
import { useLayout } from "../shell/useLayout";
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
      {(view) => (view.buildings.length < view.slots ? <BuildPicker /> : <Uncharted />)}
    </PlanetDrawer>
  );
}

function BuildingBody({ view, receivedAt, b }: { view: PlanetView; receivedAt: number; b: BuildingView }) {
  const live = useLive(view, receivedAt);
  const status = statusOf(b, view.ship_builds);
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
          builds={view.ship_builds.filter((s) => s.shipyard_id === b.id)}
          ran={live.ran}
          now={live.gameNow}
        />
      )}
    </>
  );
}

function ShipBuilds({ builds, ran, now }: { builds: S["ShipBuildView"][]; ran: number; now: number }) {
  const active = builds.filter((s) => s.state === "active");
  const waiting = builds.filter((s) => s.state === "waiting");
  return (
    <>
      <h3 className="display mt-2 text-muted">Build lanes</h3>
      {active.length === 0 && <p className="text-sm text-muted">No ships building.</p>}
      {active.map((s) => (
        <Row key={s.id} label={ship(s.ship_type)}>
          {progress(grown(s.delivered_g, s.rate_g_per_h, ran, s.cost_g), s.cost_g)}
          {s.eta && <span className="block text-xs text-muted">done {at(s.eta, now)}</span>}
        </Row>
      ))}
      {waiting.length > 0 && (
        <>
          <h3 className="display mt-2 text-muted">Queue</h3>
          <ol className="list-decimal pl-5 text-sm">
            {waiting.map((s) => (
              <li key={s.id}>{ship(s.ship_type)}</li>
            ))}
          </ol>
        </>
      )}
    </>
  );
}

function BuildPicker() {
  const rules = useRules().data;
  return (
    <>
      <p className="text-muted">{empties.slot}</p>
      {rules &&
        (["drill", "refinery", "shipyard"] as const).map((type) => (
          <Row key={type} label={building(type)}>
            {mass(rules.buildings[type].cost_g)} · {duration(rules.buildings[type].build_time_ms)}
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
