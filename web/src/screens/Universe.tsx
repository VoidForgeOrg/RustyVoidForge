import { Label, ListBox, NumberField, Select, Spinner, Table } from "@heroui/react";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { empties } from "../copy/voices";
import { distance, owner, position } from "../format";
import { homeworldFirst } from "../game/empire";
import { useMe, useNear, usePlanet } from "../game/queries";
import { Failure } from "../shell/Failure";
import { planetName } from "./parts";

/** Distance units in a world unit (D-021). */
const DU_PER_U = 1_000;
/** The default search radius, in world units. */
const DEFAULT_RADIUS_U = 50;

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
        <NumberField
          className="w-40"
          value={radius}
          minValue={0}
          maxValue={1_000_000}
          step={10}
          formatOptions={{ maximumFractionDigits: 0 }}
          onChange={(value) => {
            if (Number.isSafeInteger(value)) setRadius(value);
          }}
        >
          <Label>Radius (u)</Label>
          <NumberField.Group>
            <NumberField.DecrementButton />
            <NumberField.Input />
            <NumberField.IncrementButton />
          </NumberField.Group>
        </NumberField>
      </div>
      <Near center={from} radiusDu={radius * DU_PER_U} me={me.player_id} />
    </>
  );
}

function Near({ center, radiusDu, me }: { center: number; radiusDu: number; me: string }) {
  const planet = usePlanet(center);
  const p = planet.data?.position;
  const near = useNear(
    p ? `${p.x.toString()},${p.y.toString()},${p.z.toString()}` : "",
    radiusDu,
    p !== undefined,
  );
  if (planet.isError)
    return <Failure error={planet.error} fill={{ planet: center }} onRetry={() => void planet.refetch()} />;
  if (near.isError) return <Failure error={near.error} onRetry={() => void near.refetch()} />;
  if (!near.data) return <Spinner aria-label="Charting" />;
  const rows = [...near.data.planets].sort((a, b) => a.distance - b.distance || a.planet_id - b.planet_id);
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
