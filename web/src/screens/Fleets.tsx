import { buttonVariants, Spinner } from "@heroui/react";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import type { components } from "../api/schema";
import { copy } from "../copy/messages";
import { empties } from "../copy/voices";
import { at, duration, mass, mission, missionTo, shipCounts, ships, speed } from "../format";
import { useFleets, useMe } from "../game/queries";
import { Failure } from "../shell/Failure";
import { FleetsIcon } from "../theme/icons";
import { MissionModal } from "./Mission";
import { Actions, Confirm, OrderButton, Unavailable } from "./Orders";
import { Panel, useLive } from "./parts";

type Fleet = components["schemas"]["FleetView"];

const stateWords = { stationed: "stationed", in_transit: "in flight", returning: "returning" } as const;

export function Fleets() {
  const fleets = useFleets();
  const { send } = useSearch({ from: "/game/fleets" });
  const navigate = useNavigate();
  if (fleets.isPending) return <Spinner aria-label="Loading fleets" />;
  if (fleets.isError) return <Failure error={fleets.error} onRetry={() => void fleets.refetch()} />;
  if (fleets.data.fleets.length === 0) return <p className="text-muted">{empties.fleets}</p>;
  const sending = fleets.data.fleets.find((f) => f.fleet_id === send);
  return (
    <>
      <div className="grid grid-cols-[repeat(auto-fill,minmax(18rem,1fr))] gap-4">
        {fleets.data.fleets.map((f) => (
          <FleetCard key={f.fleet_id} fleet={f} view={fleets.data} receivedAt={fleets.dataUpdatedAt} />
        ))}
      </div>
      {sending?.state === "stationed" && (
        <MissionModal
          planet={sending.at_planet_id}
          fleet={sending}
          onClose={() => void navigate({ to: "/fleets" })}
        />
      )}
    </>
  );
}

function FleetCard({
  fleet,
  view,
  receivedAt,
}: {
  fleet: Fleet;
  view: components["schemas"]["FleetsView"];
  receivedAt: number;
}) {
  const { gameNow } = useLive(view, receivedAt);
  const planet = (id: number) => (
    <Link to="/planets/$planetId" params={{ planetId: id }} className="text-accent">
      #{id}
    </Link>
  );
  const cargo = fleet.cargo.iron_ore_g + fleet.cargo.iron_ingot_g;
  return (
    <Panel>
      <h2 className="flex items-center gap-2 text-lg" id={`fleet-${fleet.fleet_id.toString()}`}>
        <FleetsIcon />
        Fleet {fleet.fleet_id}
        <span className="ml-auto text-sm text-muted">{stateWords[fleet.state]}</span>
      </h2>
      <p>
        {shipCounts(fleet.ships)
          .map(([type, n]) => ships(n, type))
          .join(", ")}
      </p>
      <p className="text-sm">
        <span className="text-muted">Cargo</span> {mass(fleet.cargo.iron_ore_g)} ore,{" "}
        {mass(fleet.cargo.iron_ingot_g)} ingots{" "}
        <span className="text-muted">
          · {mass(cargo)} of {mass(fleet.capacity_g)}
        </span>
      </p>
      <p className="text-sm">
        {fleet.state === "stationed" && <>At {planet(fleet.at_planet_id)}</>}
        {fleet.state === "in_transit" && (
          <>
            {mission(fleet.mission)}: {planet(fleet.origin_planet_id)} → {planet(fleet.target_planet_id)},
            arrives {at(fleet.arrives_at, gameNow)}
          </>
        )}
        {fleet.state === "returning" && (
          <>
            Back to {planet(fleet.origin_planet_id)}, arrives {at(fleet.arrives_at, gameNow)}
          </>
        )}
      </p>
      <p className="text-xs text-muted">Speed {speed(fleet.speed_du_per_h)}</p>
      <FleetOrders fleet={fleet} gameNow={gameNow} />
    </Panel>
  );
}

/** What the fleet can do in its state: Send, Unload and Disband at a planet; Turn back in flight. */
function FleetOrders({ fleet, gameNow }: { fleet: Fleet; gameNow: number }) {
  const owned = useMe().data?.planet_ids ?? [];
  const f = fleet.fleet_id;
  const name = `Fleet #${f.toString()}`;
  if (fleet.state === "returning") return null;
  if (fleet.state === "in_transit") {
    const flown = (Math.floor(gameNow / 1_000) - Math.floor(Date.parse(fleet.departed_at) / 1_000)) * 1_000;
    return (
      <Actions>
        <Confirm
          order={{ type: "recall_fleet", payload: { fleet_id: f }, origin: fleet.origin_planet_id }}
          label={`Turn ${name} back`}
          question={`Turn ${name} back?`}
          consequence={`It has flown ${duration(Math.max(0, flown))}, so it returns to Planet #${fleet.origin_planet_id.toString()} in about that long, and can't be turned again. Its mission (${missionTo(fleet.mission, fleet.target_planet_id)}) is abandoned.`}
          keep="Keep flying"
        />
      </Actions>
    );
  }
  const planet = fleet.at_planet_id;
  const loaded = fleet.cargo.iron_ore_g + fleet.cargo.iron_ingot_g > 0;
  return (
    <Actions>
      <Link to="/fleets" search={{ send: f }} className={buttonVariants({ variant: "primary", size: "sm" })}>
        Send
      </Link>
      {loaded &&
        (owned.includes(planet) ? (
          <OrderButton
            order={{ type: "trigger_unload", payload: { fleet_id: f }, planet, shown: fleet.cargo }}
          >
            Unload
          </OrderButton>
        ) : (
          <Unavailable reason={copy("planet_not_owned", "command") ?? ""}>Unload</Unavailable>
        ))}
      {loaded ? (
        <Unavailable reason={copy("cargo_aboard", "command") ?? ""}>Disband</Unavailable>
      ) : (
        <OrderButton order={{ type: "disband_fleet", payload: { fleet_id: f }, planet }}>Disband</OrderButton>
      )}
    </Actions>
  );
}
