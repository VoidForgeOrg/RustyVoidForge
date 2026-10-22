import { toast } from "@heroui/react";
import { type QueryClient, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { api } from "../api/client";
import type { components } from "../api/schema";
import { failure, type Fill } from "../copy/messages";
import { doneLine, eventLine } from "../copy/outcomes";
import { orders } from "../copy/voices";

type S = components["schemas"];
type Command = S["CommandRequest"];
type Of<T extends Command["type"]> = Extract<Command, { type: T }>;

/**
 * A command with what the screen knew when it sent it: the words of its outcome and the views it
 * changes come from here, since a 200 carries only the command's own events (D-045).
 */
export type Order =
  | Of<"build_building">
  | (Of<"cancel_construction"> & { planet: number; building: S["BuildingType"] })
  | (Of<"demolish_building"> & { planet: number; building: S["BuildingType"] })
  | (Of<"queue_ship"> & { planet: number })
  | (Of<"cancel_ship_build"> & { planet: number })
  | Of<"assemble_fleet">
  | Of<"send_fleet">
  | (Of<"recall_fleet"> & { origin: number })
  | (Of<"disband_fleet"> & { planet: number })
  | (Of<"trigger_unload"> & { planet: number; shown: S["Cargo"] });

/** The planet whose view the order changes, if any. */
function planetOf(order: Order): number | null {
  switch (order.type) {
    case "build_building":
    case "assemble_fleet":
      return order.payload.planet_id;
    case "send_fleet":
    case "recall_fleet":
      return null;
    default:
      return order.planet;
  }
}

/** The numbers a refusal's copy names. */
export function fillOf(order: Order): Fill {
  if (order.type === "send_fleet") return { target: order.payload.target_planet_id };
  const planet = planetOf(order);
  return planet === null ? {} : { planet };
}

/** The fleets query, as `useFleets` keys it. */
export const fleetsKey = ["get", "/players/me/fleets", {}];
const movesFleets = new Set<Order["type"]>([
  "assemble_fleet",
  "send_fleet",
  "recall_fleet",
  "disband_fleet",
  "trigger_unload",
]);

/** Refetches every view the order and its events changed, and waits for them. */
async function refresh(client: QueryClient, order: Order, accepted: S["CommandAccepted"]) {
  const planets = new Set<number>();
  const own = planetOf(order);
  if (own !== null) planets.add(own);
  let me = false;
  for (const e of accepted.events) {
    if ("planet_id" in e) planets.add(e.planet_id);
    if (e.type === "planet_colonized") me = true;
  }
  await Promise.all([
    ...[...planets].map((id) =>
      client.invalidateQueries({ queryKey: ["get", "/planets/{id}", { params: { path: { id } } }] }),
    ),
    movesFleets.has(order.type) || accepted.events.length > 0
      ? client.invalidateQueries({ queryKey: fleetsKey })
      : null,
    me ? client.invalidateQueries({ queryKey: ["get", "/players/me"] }) : null,
  ]);
}

/** What an unloaded fleet has aboard after the refetch, when the read has it. */
function aboard(client: QueryClient, order: Order): S["Cargo"] | null {
  if (order.type !== "trigger_unload") return null;
  const view = client.getQueryData<S["FleetsView"]>(fleetsKey);
  return view?.fleets.find((f) => f.fleet_id === order.payload.fleet_id)?.cargo ?? null;
}

const newKey = () => crypto.randomUUID();

/**
 * One form's sender. The form keeps its idempotency key until an answer settles the order (a 200,
 * or a refusal that says nothing was done), so a retry after an unknown outcome can't run twice.
 */
export function useOrder() {
  const client = useQueryClient();
  const [key, setKey] = useState(newKey);
  const mutation = api.useMutation("post", "/commands");

  async function send(order: Order): Promise<S["CommandAccepted"] | null> {
    let accepted: S["CommandAccepted"];
    try {
      accepted = await mutation.mutateAsync({
        params: { header: { "Idempotency-Key": key } },
        body: { type: order.type, payload: order.payload } as Command,
      });
    } catch (error) {
      if (failure(error, "command").kind !== "retry") setKey(newKey());
      return null;
    }
    setKey(newKey());
    await refresh(client, order, accepted);
    const lines = [doneLine(order, aboard(client, order)), ...accepted.events.map(eventLine)];
    toast.success(orders.done, {
      description: (
        <ul>
          {lines.map((line, i) => (
            <li key={i}>{line}</li>
          ))}
        </ul>
      ),
    });
    return accepted;
  }

  return { send, pending: mutation.isPending, error: mutation.error, reset: mutation.reset };
}
