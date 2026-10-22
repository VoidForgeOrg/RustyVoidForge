import {
  Button,
  Checkbox,
  Input,
  Label,
  ListBox,
  Modal,
  Radio,
  RadioGroup,
  Spinner,
  TextField,
} from "@heroui/react";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { components } from "../api/schema";
import { checks, copy } from "../copy/messages";
import { empties, orders } from "../copy/voices";
import {
  distance,
  duration,
  mass,
  mission as missionName,
  missionTo,
  owner,
  parseTonnes,
  shipCounts,
  ship,
  ships as shipsText,
  travelMs,
} from "../format";
import { fleetsKey, type Order, useOrder } from "../game/commands";
import { grown } from "../game/live";
import { useChart, useMe, usePlanet, useRules } from "../game/queries";
import { Refusal } from "./Orders";
import { DEFAULT_RADIUS_U, DU_PER_U, type PlanetView, RadiusField, useLive } from "./parts";

type S = components["schemas"];
type Stationed = Extract<S["FleetView"], { state: "stationed" }>;
type Ship = { ship_id: number; ship_type: S["ShipType"] };

const MISSIONS = ["colonize", "transport", "move"] as const;

/**
 * The New mission stepper: ships and cargo, then mission and target, then confirm. From a planet
 * it assembles and sends; from a stationed fleet it starts at the target.
 */
export function MissionModal({
  planet,
  fleet,
  onClose,
}: {
  planet: number;
  fleet?: Stationed;
  onClose: () => void;
}) {
  const view = usePlanet(planet);
  return (
    <Modal.Backdrop
      isOpen
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <Modal.Container size="lg" scroll="inside">
        <Modal.Dialog aria-label="New mission">
          <Modal.Header>
            <Modal.Heading className="display text-lg">
              New mission · Planet #{planet.toString()}
            </Modal.Heading>
            <Modal.CloseTrigger />
          </Modal.Header>
          {view.data ? (
            <Stepper view={view.data} receivedAt={view.dataUpdatedAt} fleet={fleet} onClose={onClose} />
          ) : (
            <Modal.Body>
              <Spinner aria-label="Loading" />
            </Modal.Body>
          )}
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

function Stepper({
  view,
  receivedAt,
  fleet,
  onClose,
}: {
  view: PlanetView;
  receivedAt: number;
  fleet: Stationed | undefined;
  onClose: () => void;
}) {
  const me = useMe().data;
  const rules = useRules().data;
  const client = useQueryClient();
  const assemble = useOrder();
  const sendOrder = useOrder();
  const { ran } = useLive(view, receivedAt);
  const [step, setStep] = useState(fleet ? 2 : 1);
  const [picked, setPicked] = useState<number[]>([]);
  const [ore, setOre] = useState("");
  const [ingots, setIngots] = useState("");
  const [chosenMission, setMission] = useState<S["Mission"] | null>(null);
  const [target, setTarget] = useState("");
  const [fleetId, setFleetId] = useState(fleet?.fleet_id ?? null);
  const [shown, setShown] = useState<string | null>(null);
  if (!me || !rules) return <Spinner aria-label="Loading" />;

  const roster = view.roster.filter((s) => s.owner === me.player_id);
  const chosen: Ship[] = fleet ? fleet.ships : roster.filter((s) => picked.includes(s.ship_id));
  const cargo = { iron_ore_g: parseTonnes(ore), iron_ingot_g: parseTonnes(ingots) };
  const capacity = fleet
    ? fleet.capacity_g
    : chosen.reduce((sum, s) => sum + rules.ships[s.ship_type].cargo_capacity_g, 0);
  const speed = fleet
    ? fleet.speed_du_per_h
    : Math.min(...chosen.map((s) => rules.ships[s.ship_type].speed_du_per_h));
  const has = (t: S["ShipType"]) => chosen.some((s) => s.ship_type === t);

  const shipsCheck = (() => {
    if (chosen.length === 0) return checks.ships;
    if (cargo.iron_ore_g === null || cargo.iron_ingot_g === null) return checks.cargo;
    const stock = (s: S["Stock"]) => grown(s.amount_g, s.rate_g_per_h, ran, s.cap_g);
    if (cargo.iron_ore_g > stock(view.stocks.iron_ore) || cargo.iron_ingot_g > stock(view.stocks.iron_ingot))
      return copy("insufficient_stock", "command");
    if (cargo.iron_ore_g + cargo.iron_ingot_g > capacity) return copy("insufficient_capacity", "command");
    return null;
  })();
  const missionCheck = (m: S["Mission"]) =>
    m === "colonize" && !has("colony_ship")
      ? copy("no_colony_ship", "command")
      : m === "transport" && !has("cargo_vessel")
        ? copy("no_cargo_vessel", "command")
        : null;
  const mission =
    chosenMission !== null && missionCheck(chosenMission) === null
      ? chosenMission
      : (MISSIONS.find((m) => missionCheck(m) === null) ?? "move");
  const targetId = /^\d+$/.test(target.trim()) ? Number(target.trim()) : null;

  const assembleOrder = (): Order => ({
    type: "assemble_fleet",
    payload: {
      planet_id: view.planet_id,
      ship_ids: chosen.map((s) => s.ship_id),
      cargo: { iron_ore_g: cargo.iron_ore_g ?? 0, iron_ingot_g: cargo.iron_ingot_g ?? 0 },
    },
  });

  /** The fleet just assembled from the chosen ships, as the refetched fleets show it. */
  const assembled = () => {
    const ids = new Set(chosen.map((s) => s.ship_id));
    return client
      .getQueryData<S["FleetsView"]>(fleetsKey)
      ?.fleets.find(
        (f) =>
          f.state === "stationed" &&
          f.at_planet_id === view.planet_id &&
          f.ships.length === ids.size &&
          f.ships.every((s) => ids.has(s.ship_id)),
      )?.fleet_id;
  };

  async function assembleOnly() {
    if (await assemble.send(assembleOrder())) onClose();
  }

  async function launch() {
    if (targetId === null) return;
    let id = fleetId;
    if (id === null) {
      if (!(await assemble.send(assembleOrder()))) return;
      id = assembled() ?? null;
      setFleetId(id);
      if (id === null) {
        setShown(orders.assembledUnsent);
        return;
      }
    }
    const order: Order = {
      type: "send_fleet",
      payload: { fleet_id: id, target_planet_id: targetId, mission },
    };
    if (await sendOrder.send(order)) onClose();
  }

  const sendFailed = sendOrder.error !== null && fleetId !== null && targetId !== null;

  return (
    <>
      <Modal.Body className="flex flex-col gap-4">
        <ol className="display flex gap-4 text-sm" aria-label="Steps">
          {["Ships and cargo", "Mission and target", "Confirm"].map((name, i) => (
            <li
              key={name}
              aria-current={step === i + 1 ? "step" : undefined}
              className={step === i + 1 ? "text-accent" : "text-muted"}
            >
              {(i + 1).toString()}. {name}
            </li>
          ))}
        </ol>

        {step === 1 && (
          <>
            <fieldset className="flex flex-col gap-2">
              <legend className="display mb-1 text-muted">Ships here</legend>
              {roster.length === 0 && <p className="text-muted">{empties.roster}</p>}
              {roster.map((s) => (
                <Checkbox
                  key={s.ship_id}
                  isSelected={picked.includes(s.ship_id)}
                  onChange={(on) => {
                    setPicked(on ? [...picked, s.ship_id] : picked.filter((x) => x !== s.ship_id));
                  }}
                >
                  <Checkbox.Control>
                    <Checkbox.Indicator />
                  </Checkbox.Control>
                  <Checkbox.Content>
                    <Label>
                      {ship(s.ship_type)} #{s.ship_id.toString()}
                    </Label>
                  </Checkbox.Content>
                </Checkbox>
              ))}
            </fieldset>
            <div className="flex flex-wrap gap-4">
              <TextField className="w-40" value={ore} onChange={setOre}>
                <Label>Ore (t)</Label>
                <Input inputMode="decimal" placeholder="0" />
              </TextField>
              <TextField className="w-40" value={ingots} onChange={setIngots}>
                <Label>Ingots (t)</Label>
                <Input inputMode="decimal" placeholder="0" />
              </TextField>
            </div>
            <p className="text-sm text-muted">Capacity {mass(capacity)}</p>
            {shipsCheck && <p className="text-sm text-warning">{shipsCheck}</p>}
            {assemble.error !== null && (
              <Refusal error={assemble.error} order={assembleOrder()} retry={() => void assembleOnly()} />
            )}
          </>
        )}

        {step === 2 && (
          <>
            <RadioGroup
              value={mission}
              onChange={(m) => {
                setMission(m as S["Mission"]);
              }}
            >
              <Label>Mission</Label>
              {MISSIONS.map((m) => (
                <Radio key={m} value={m} isDisabled={missionCheck(m) !== null}>
                  <Radio.Control>
                    <Radio.Indicator />
                  </Radio.Control>
                  <Radio.Content>
                    <Label>{missionName(m)}</Label>
                    {missionCheck(m) && <span className="block text-xs text-muted">{missionCheck(m)}</span>}
                  </Radio.Content>
                </Radio>
              ))}
            </RadioGroup>
            <Targets
              center={view.planet_id}
              mission={mission}
              me={me.player_id}
              speed={speed}
              selected={targetId}
              onSelect={(id) => {
                setTarget(id.toString());
              }}
            />
            <TextField className="w-40" value={target} onChange={setTarget}>
              <Label>Target planet #</Label>
              <Input inputMode="numeric" />
            </TextField>
            {targetId === null && target.trim() !== "" && (
              <p className="text-sm text-warning">{checks.target}</p>
            )}
          </>
        )}

        {step === 3 && targetId !== null && (
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
            <dt className="display text-muted">Ships</dt>
            <dd>
              {fleet || fleetId !== null
                ? `Fleet #${(fleetId ?? 0).toString()}`
                : shipCounts(chosen)
                    .map(([t, n]) => shipsText(n, t))
                    .join(", ")}
            </dd>
            <dt className="display text-muted">Cargo</dt>
            <dd>
              {fleet
                ? `${mass(fleet.cargo.iron_ore_g)} ore, ${mass(fleet.cargo.iron_ingot_g)} ingots`
                : `${mass(cargo.iron_ore_g ?? 0)} ore, ${mass(cargo.iron_ingot_g ?? 0)} ingots`}
            </dd>
            <dt className="display text-muted">Mission</dt>
            <dd>{missionTo(mission, targetId)}</dd>
          </dl>
        )}
        {step === 3 && fleetId !== null && !fleet && (
          <p className="text-sm text-muted">{orders.assembled(fleetId, view.planet_id)}</p>
        )}
        {shown && <p className="text-sm text-warning">{shown}</p>}
        {step === 3 && sendFailed && (
          <Refusal
            error={sendOrder.error}
            order={{
              type: "send_fleet",
              payload: { fleet_id: fleetId, target_planet_id: targetId, mission },
            }}
            retry={() => void launch()}
          />
        )}
        {step === 3 && assemble.error !== null && fleetId === null && (
          <Refusal error={assemble.error} order={assembleOrder()} retry={() => void launch()} />
        )}
      </Modal.Body>
      <Modal.Footer>
        {step > (fleet ? 2 : 1) && fleetId === (fleet?.fleet_id ?? null) && (
          <Button
            variant="tertiary"
            onPress={() => {
              setStep(step - 1);
            }}
          >
            Back
          </Button>
        )}
        {step === 1 && (
          <>
            <Button
              variant="secondary"
              isDisabled={shipsCheck !== null}
              isPending={assemble.pending}
              onPress={() => void assembleOnly()}
            >
              Assemble only
            </Button>
            <Button
              variant="primary"
              isDisabled={shipsCheck !== null}
              onPress={() => {
                setStep(2);
              }}
            >
              Next
            </Button>
          </>
        )}
        {step === 2 && (
          <Button
            variant="primary"
            isDisabled={targetId === null || missionCheck(mission) !== null}
            onPress={() => {
              setStep(3);
            }}
          >
            Next
          </Button>
        )}
        {step === 3 && (
          <Button
            variant="primary"
            isPending={assemble.pending || sendOrder.pending}
            onPress={() => void launch()}
          >
            Send
          </Button>
        )}
      </Modal.Footer>
    </>
  );
}

/** Planets in range, nearest first, with the trip's time; those the mission can't target are disabled. */
function Targets({
  center,
  mission,
  me,
  speed,
  selected,
  onSelect,
}: {
  center: number;
  mission: S["Mission"];
  me: string;
  speed: number;
  selected: number | null;
  onSelect: (id: number) => void;
}) {
  const [radius, setRadius] = useState(DEFAULT_RADIUS_U);
  const { rows } = useChart(center, radius * DU_PER_U);
  const field = <RadiusField value={radius} onChange={setRadius} />;
  if (!rows)
    return (
      <>
        {field}
        <Spinner aria-label="Charting" />
      </>
    );
  const refused = (r: S["PlanetSummary"]) =>
    r.planet_id === center ||
    (mission === "colonize" && r.owner !== null) ||
    (mission === "transport" && r.owner !== me);
  const shown =
    mission === "colonize"
      ? [...rows].sort((a, b) => Number(a.owner !== null) - Number(b.owner !== null))
      : rows;
  return (
    <>
      {field}
      <ListBox
        aria-label="Targets in range, nearest first"
        className="max-h-60 overflow-y-auto"
        selectionMode="single"
        selectedKeys={selected === null ? [] : [selected]}
        disabledKeys={shown.filter(refused).map((r) => r.planet_id)}
        onSelectionChange={(keys) => {
          const [key] = keys === "all" ? [] : [...keys];
          if (key !== undefined) onSelect(Number(key));
        }}
      >
        {shown.map((r) => {
          const trip = travelMs(r.distance, speed);
          return (
            <ListBox.Item key={r.planet_id} id={r.planet_id} textValue={`Planet #${r.planet_id.toString()}`}>
              #{r.planet_id} · {owner(r.owner, me)} · {distance(r.distance)} ·{" "}
              {trip === null ? "—" : duration(trip)}
            </ListBox.Item>
          );
        })}
      </ListBox>
    </>
  );
}
