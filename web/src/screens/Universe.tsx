import { Label, ListBox, Select, Spinner, Table } from "@heroui/react";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { empties } from "../copy/voices";
import { distance, owner, position } from "../format";
import { homeworldFirst } from "../game/empire";
import { useChart, useMe } from "../game/queries";
import { Failure } from "../shell/Failure";
import { DEFAULT_RADIUS_U, DU_PER_U, planetName, RadiusField } from "./parts";

/** Star charts: planets near one of the player's own, nearest first. */
export function Universe() {
  const me = useMe().data;
  const [center, setCenter] = useState<number | null>(null);
  const [radius, setRadius] = useState(DEFAULT_RADIUS_U);
  if (!me) return null;
  const owned = homeworldFirst(me.planet_ids, me.homeworld_id);
  const from = center ?? me.homeworld_id;
  return (
    <>
      <div className="mb-4 flex flex-wrap items-end gap-4">
        <Select
          className="w-56"
          value={from}
          onChange={(key) => {
            if (key !== null) setCenter(Number(key));
          }}
        >
          <Label>Centre</Label>
          <Select.Trigger>
            <Select.Value />
            <Select.Indicator />
          </Select.Trigger>
          <Select.Popover>
            <ListBox>
              {owned.map((id) => (
                <ListBox.Item key={id} id={id} textValue={planetName(id, me.homeworld_id)}>
                  {planetName(id, me.homeworld_id)}
                </ListBox.Item>
              ))}
            </ListBox>
          </Select.Popover>
        </Select>
        <RadiusField value={radius} onChange={setRadius} />
      </div>
      <Near center={from} radiusDu={radius * DU_PER_U} me={me.player_id} />
    </>
  );
}

function Near({ center, radiusDu, me }: { center: number; radiusDu: number; me: string }) {
  const { planet, near, rows } = useChart(center, radiusDu);
  if (planet.isError)
    return <Failure error={planet.error} fill={{ planet: center }} onRetry={() => void planet.refetch()} />;
  if (near.isError) return <Failure error={near.error} onRetry={() => void near.refetch()} />;
  if (!rows) return <Spinner aria-label="Charting" />;
  if (rows.length === 0) return <p className="text-muted">{empties.near}</p>;
  return (
    <Table>
      <Table.ScrollContainer>
        <Table.Content aria-label="Planets in range, nearest first">
          <Table.Header>
            <Table.Column isRowHeader>Planet</Table.Column>
            <Table.Column>Owner</Table.Column>
            <Table.Column>Position</Table.Column>
            <Table.Column>Distance</Table.Column>
          </Table.Header>
          <Table.Body>
            {rows.map((r) => (
              <Table.Row key={r.planet_id}>
                <Table.Cell>
                  <Link to="/planets/$planetId" params={{ planetId: r.planet_id }} className="text-accent">
                    #{r.planet_id}
                  </Link>
                </Table.Cell>
                <Table.Cell>{owner(r.owner, me)}</Table.Cell>
                <Table.Cell>{position(r.position)}</Table.Cell>
                <Table.Cell>{distance(r.distance)}</Table.Cell>
              </Table.Row>
            ))}
          </Table.Body>
        </Table.Content>
      </Table.ScrollContainer>
    </Table>
  );
}
