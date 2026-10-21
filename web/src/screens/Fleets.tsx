import { Spinner } from "@heroui/react";
import { Link } from "@tanstack/react-router";
import type { components } from "../api/schema";
import { empties } from "../copy/voices";
import { at, mass, mission, ships, speed } from "../format";
import { useFleets } from "../game/queries";
import { Failure } from "../shell/Failure";
import { FleetsIcon } from "../theme/icons";
import { Panel, useLive } from "./parts";

type Fleet = components["schemas"]["FleetView"];

const stateWords = { stationed: "stationed", in_transit: "in flight", returning: "returning" } as const;

export function Fleets() {
  const fleets = useFleets();
  if (fleets.isPending) return <Spinner aria-label="Loading fleets" />;
  if (fleets.isError) return <Failure error={fleets.error} onRetry={() => void fleets.refetch()} />;
  if (fleets.data.fleets.length === 0) return <p className="text-muted">{empties.fleets}</p>;
  return (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(18rem,1fr))] gap-4">
      {fleets.data.fleets.map((f) => (
        <FleetCard key={f.fleet_id} fleet={f} view={fleets.data} receivedAt={fleets.dataUpdatedAt} />
      ))}
    </div>
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
  const counts = new Map<"colony_ship" | "cargo_vessel", number>();
  for (const s of fleet.ships) counts.set(s.ship_type, (counts.get(s.ship_type) ?? 0) + 1);
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
      <p>{[...counts].map(([type, n]) => ships(n, type)).join(", ")}</p>
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
    </Panel>
  );
}
