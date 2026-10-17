//! State tests: ID counter, ordering, serde round trips and the deserialization boundary. The
//! world here is built by hand, without testkit, so it shows the raw contract.

use super::*;

const ALICE: PlayerId = PlayerId(0x0192_8c5e_7a3b_7c2d_9e4f_5a6b_7c8d_9e0f);

fn stock(value_g: i64, rate: i64, remainder: i64, at: i64) -> Quantity {
    Quantity::from_parts(value_g, rate, remainder, at).expect("valid stock")
}

fn planet(id: u64, owner: Option<PlayerId>) -> Planet {
    Planet {
        id: PlanetId(id),
        position: Position {
            x: 1_000 * id as i64,
            y: -7,
            z: 3,
        },
        owner,
        slots: 8,
        ore_cap_g: 10_000_000_000,
        ingot_cap_g: 5_000_000_000,
        ore: stock(2_000_000_000, 2_000_000, 0, 0),
        ingots: stock(1_500_000_000, -50_000_000, 1_234_567, 5),
        buildings: BTreeSet::new(),
        roster: BTreeSet::new(),
        version: 0,
        next_event: None,
    }
}

/// Every entity, every state and status, both ship locations, all three fleet states and
/// missions, every queue kind, a stale queue entry and quantities with remainders.
fn sample_world() -> World {
    let mut w = World::new(10, 0, 0xdead_beef);
    w.players.insert(
        ALICE,
        Player {
            id: ALICE,
            homeworld: PlanetId(1),
        },
    );
    let mut home = planet(1, Some(ALICE));
    home.version = 3;
    home.next_event = Some(ScheduledEvent {
        at_ms: 100,
        version: 3,
    });
    home.buildings = [4, 5, 6].into_iter().map(BuildingId).collect();
    home.roster.insert(ShipId(10));
    w.planets.insert(home.id, home);
    w.planets.insert(PlanetId(2), planet(2, None));
    let mut colony = planet(3, Some(ALICE));
    colony.buildings.insert(BuildingId(7));
    w.planets.insert(colony.id, colony);

    let buildings = [
        (4, 1, BuildingKind::Drill, BuildingState::Operational),
        (
            5,
            1,
            BuildingKind::Refinery,
            BuildingState::UnderConstruction {
                delivered: stock(30_000_000, 62_500_000, 17, 2),
                cost_g: 250_000_000,
                rated_g_per_h: 62_500_000,
            },
        ),
        (6, 1, BuildingKind::Shipyard, BuildingState::Operational),
        (
            7,
            3,
            BuildingKind::Drill,
            BuildingState::Demolishing {
                ends_at_ms: 3_600_010,
            },
        ),
    ];
    for (id, planet, kind, state) in buildings {
        w.buildings.insert(
            BuildingId(id),
            Building {
                id: BuildingId(id),
                planet: PlanetId(planet),
                kind,
                state,
            },
        );
    }
    for (id, kind, status, rate) in [
        (8, ShipKind::ColonyShip, ShipBuildStatus::Active, 80_000_000),
        (9, ShipKind::CargoVessel, ShipBuildStatus::Waiting, 0),
    ] {
        w.ship_builds.insert(
            ShipBuildId(id),
            ShipBuild {
                id: ShipBuildId(id),
                shipyard: BuildingId(6),
                kind,
                delivered: stock(0, rate, 0, 10),
                cost_g: if kind == ShipKind::ColonyShip {
                    800_000_000
                } else {
                    300_000_000
                },
                rated_g_per_h: if kind == ShipKind::ColonyShip {
                    80_000_000
                } else {
                    75_000_000
                },
                status,
            },
        );
    }
    let ships = [
        (10, ShipKind::ColonyShip, ShipLocation::Roster(PlanetId(1))),
        (11, ShipKind::CargoVessel, ShipLocation::Fleet(FleetId(13))),
        (12, ShipKind::CargoVessel, ShipLocation::Fleet(FleetId(14))),
        (16, ShipKind::ColonyShip, ShipLocation::Fleet(FleetId(15))),
    ];
    for (id, kind, location) in ships {
        w.ships.insert(
            ShipId(id),
            Ship {
                id: ShipId(id),
                kind,
                owner: ALICE,
                location,
            },
        );
    }
    let fleets = [
        (
            13,
            11,
            Cargo {
                ore_g: 0,
                ingots_g: 300_000_000,
            },
            FleetState::InTransit {
                origin: PlanetId(1),
                target: PlanetId(3),
                mission: Mission::Transport,
                departed_at_ms: 0,
                arrives_at_ms: 200,
            },
        ),
        (
            14,
            12,
            Cargo::default(),
            FleetState::Returning {
                origin: PlanetId(1),
                arrives_at_ms: 150,
            },
        ),
        (
            16 - 1,
            16,
            Cargo::default(),
            FleetState::Stationed { at: PlanetId(2) },
        ),
    ];
    for (id, ship, cargo, state) in fleets {
        w.fleets.insert(
            FleetId(id),
            Fleet {
                id: FleetId(id),
                owner: ALICE,
                ships: [ShipId(ship)].into(),
                cargo,
                version: 1,
                state,
            },
        );
    }
    // A Colonize mission in flight too, so every mission variant round-trips.
    if let Some(Fleet {
        state: FleetState::Stationed { .. },
        ..
    }) = w.fleets.get(&FleetId(15))
    {
        w.fleets.get_mut(&FleetId(15)).unwrap().state = FleetState::InTransit {
            origin: PlanetId(2),
            target: PlanetId(2),
            mission: Mission::Colonize,
            departed_at_ms: 5,
            arrives_at_ms: 300,
        };
    }
    w.homeworld_candidates = vec![PlanetId(2), PlanetId(1)];
    w.queue = [
        QueueEntry {
            at_ms: 100,
            kind: QueueKind::PlanetChange(PlanetId(1)),
            version: 3,
        },
        QueueEntry {
            at_ms: 50,
            kind: QueueKind::PlanetChange(PlanetId(1)),
            version: 2,
        }, // stale
        QueueEntry {
            at_ms: 200,
            kind: QueueKind::FleetArrival(FleetId(13)),
            version: 1,
        },
        QueueEntry {
            at_ms: 150,
            kind: QueueKind::FleetReturn(FleetId(14)),
            version: 1,
        },
        QueueEntry {
            at_ms: 300,
            kind: QueueKind::FleetArrival(FleetId(15)),
            version: 1,
        },
    ]
    .into();
    w.next_id = 17;
    w
}

fn json(w: &World) -> String {
    serde_json::to_string(w).expect("serialize")
}

fn reload(text: &str) -> Result<World, serde_json::Error> {
    serde_json::from_str(text)
}

#[test]
fn the_sample_world_is_linked() {
    assert_eq!(sample_world().check_links(), Ok(()));
}

#[test]
fn ids_come_from_one_counter_and_never_wrap() {
    let mut w = World::new(0, 0, 0);
    assert_eq!(
        (w.allocate_id(), w.allocate_id(), w.next_id),
        (Ok(1), Ok(2), 3)
    );
    w.next_id = u64::MAX - 1;
    assert_eq!(w.allocate_id(), Ok(u64::MAX - 1));
    assert_eq!(w.allocate_id(), Err(StateError::IdsExhausted(u64::MAX)));
    assert_eq!(
        w.next_id,
        u64::MAX,
        "a failed allocation leaves the counter alone"
    );
}

#[test]
fn every_variant_round_trips_exactly() {
    let w = sample_world();
    let text = json(&w);
    let back = reload(&text).expect("deserialize");
    assert_eq!(back, w);
    assert_eq!(json(&back), text, "and serializes to the same bytes again");
    // Quantities keep their remainders and checkpoints.
    assert_eq!(back.planets[&PlanetId(1)].ingots.remainder(), 1_234_567);
    assert_eq!(back.planets[&PlanetId(1)].ingots.checkpoint_ms(), 5);
}

#[test]
fn serialization_ignores_insertion_order() {
    let w = sample_world();
    let mut reversed = World::new(w.time_ms, w.rules_version, w.seed);
    reversed.next_id = w.next_id;
    reversed.homeworld_candidates = w.homeworld_candidates.clone();
    for (k, v) in w.players.iter().rev() {
        reversed.players.insert(*k, v.clone());
    }
    for (k, v) in w.planets.iter().rev() {
        reversed.planets.insert(*k, v.clone());
    }
    for (k, v) in w.buildings.iter().rev() {
        reversed.buildings.insert(*k, *v);
    }
    for (k, v) in w.ship_builds.iter().rev() {
        reversed.ship_builds.insert(*k, *v);
    }
    for (k, v) in w.ships.iter().rev() {
        reversed.ships.insert(*k, *v);
    }
    for (k, v) in w.fleets.iter().rev() {
        reversed.fleets.insert(*k, v.clone());
    }
    for e in w.queue.iter().rev() {
        reversed.queue.insert(*e);
    }
    assert_eq!(json(&reversed), json(&w));
}

#[test]
fn collections_iterate_in_id_order() {
    let w = sample_world();
    let buildings: Vec<u64> = w.buildings.keys().map(|k| k.0).collect();
    assert_eq!(buildings, [4, 5, 6, 7]);
    let ships: Vec<u64> = w.ships.keys().map(|k| k.0).collect();
    assert_eq!(ships, [10, 11, 12, 16]);
}

#[test]
fn the_queue_orders_by_time_then_kind_rank_then_entity() {
    let at = |t, kind| QueueEntry {
        at_ms: t,
        kind,
        version: 0,
    };
    let queue: BTreeSet<QueueEntry> = [
        at(5, QueueKind::FleetArrival(FleetId(1))),
        at(5, QueueKind::PlanetChange(PlanetId(9))),
        at(4, QueueKind::FleetReturn(FleetId(2))),
        at(5, QueueKind::PlanetChange(PlanetId(3))),
        at(5, QueueKind::FleetReturn(FleetId(0))),
        at(5, QueueKind::FleetArrival(FleetId(0))),
        at(5, QueueKind::FleetReturn(FleetId(7))),
    ]
    .into();
    let order: Vec<QueueKind> = queue.iter().map(|e| e.kind).collect();
    assert_eq!(
        order,
        [
            QueueKind::FleetReturn(FleetId(2)),
            QueueKind::PlanetChange(PlanetId(3)),
            QueueKind::PlanetChange(PlanetId(9)),
            // Fleet arrivals and returns share rank 1 and go by fleet ID, whatever their kind.
            QueueKind::FleetArrival(FleetId(0)),
            QueueKind::FleetReturn(FleetId(0)),
            QueueKind::FleetArrival(FleetId(1)),
            QueueKind::FleetReturn(FleetId(7)),
        ]
    );
}

#[test]
fn a_quantity_with_an_out_of_range_remainder_does_not_deserialize() {
    let text = json(&sample_world()).replacen("\"remainder\":1234567", "\"remainder\":3600000", 1);
    let error = reload(&text).expect_err("rejected");
    assert!(
        error.to_string().contains("remainder 3600000 is outside"),
        "{error}"
    );
}

/// Applies `edit` to the sample world and expects deserialization to reject it with `expected`
/// in the message, after confirming the edited world fails check_links the same way.
fn rejected(edit: impl FnOnce(&mut World), expected: &str) {
    let mut w = sample_world();
    edit(&mut w);
    let error = w.check_links().expect_err("the edit breaks a link");
    assert!(error.to_string().contains(expected), "{error}");
    let message = reload(&json(&w))
        .expect_err("deserialization rejects it")
        .to_string();
    assert!(message.contains(expected), "{message}");
}

#[test]
fn deserialization_rejects_broken_worlds() {
    rejected(|w| w.next_id = 16, "entity ID 16 is not below next_id 16");
    rejected(
        |w| {
            let mut ghost = w.ships[&ShipId(10)];
            ghost.id = ShipId(4); // same ID as a building
            w.ships.insert(ShipId(4), ghost);
        },
        "entity ID 4 is used more than once",
    );
    rejected(
        |w| {
            let b = w.buildings.remove(&BuildingId(4)).unwrap();
            w.buildings.insert(BuildingId(3), b);
        },
        "building key 3 holds building 4",
    );
    rejected(
        |w| {
            let _ = (w
                .planets
                .get_mut(&PlanetId(1))
                .unwrap()
                .buildings
                .remove(&BuildingId(5)),);
        },
        "building 5 stands on planet 1, which does not list it",
    );
    rejected(
        |w| {
            let _ = (w
                .planets
                .get_mut(&PlanetId(3))
                .unwrap()
                .roster
                .insert(ShipId(10)),);
        },
        "planet 3 lists ship 10 on its roster, but the ship is at Roster(PlanetId(1))",
    );
    rejected(
        |w| w.planets.get_mut(&PlanetId(1)).unwrap().owner = None,
        "homeworld 1 is not owned by them",
    );
    rejected(
        |w| w.buildings.get_mut(&BuildingId(6)).unwrap().kind = BuildingKind::Drill,
        "building 6 is not an operational Shipyard",
    );
    rejected(
        |w| {
            // Remove the ship as well, so the empty fleet is the only fault.
            w.ships.remove(&ShipId(12));
            w.fleets.get_mut(&FleetId(14)).unwrap().ships.clear();
        },
        "fleet 14 has no ships",
    );
    rejected(
        |w| w.fleets.get_mut(&FleetId(13)).unwrap().cargo.ore_g = -1,
        "negative cargo",
    );
    rejected(
        |w| w.planets.get_mut(&PlanetId(2)).unwrap().ore = stock(0, 0, 0, 11),
        "checkpoint 11 is after world time 10",
    );
    rejected(
        |w| {
            w.planets.get_mut(&PlanetId(1)).unwrap().next_event = Some(ScheduledEvent {
                at_ms: 99,
                version: 3,
            })
        },
        "next event at 99 ms is not in the queue",
    );
    rejected(
        |w| w.planets.get_mut(&PlanetId(1)).unwrap().version = 4,
        "next event has version 3, but the planet is at version 4",
    );
    rejected(
        |w| {
            let _ = w.queue.insert(QueueEntry {
                at_ms: 120,
                kind: QueueKind::PlanetChange(PlanetId(1)),
                version: 3,
            });
        },
        "is live for planet 1",
    );
    rejected(
        |w| w.homeworld_candidates.push(PlanetId(2)),
        "homeworld candidate 2 is listed twice",
    );
    rejected(
        |w| {
            let _ = w.queue.insert(QueueEntry {
                at_ms: 9,
                kind: QueueKind::FleetArrival(FleetId(99)),
                version: 0,
            });
        },
        "fleet 99 does not exist",
    );
    rejected(
        |w| {
            let _ = w.queue.remove(&QueueEntry {
                at_ms: 200,
                kind: QueueKind::FleetArrival(FleetId(13)),
                version: 1,
            });
        },
        "fleet 13 in state InTransit",
    );
    rejected(
        |w| {
            let fleet = w.fleets.get_mut(&FleetId(15)).unwrap();
            fleet.state = FleetState::Stationed { at: PlanetId(2) };
        },
        "fleet 15 in state Stationed",
    );
    rejected(
        |w| {
            let _ = w.queue.remove(&QueueEntry {
                at_ms: 150,
                kind: QueueKind::FleetReturn(FleetId(14)),
                version: 1,
            });
            let _ = w.queue.insert(QueueEntry {
                at_ms: 150,
                kind: QueueKind::FleetArrival(FleetId(14)),
                version: 1,
            });
        },
        "fleet 14 in state Returning",
    );
    rejected(
        |w| {
            if let FleetState::InTransit { arrives_at_ms, .. } =
                &mut w.fleets.get_mut(&FleetId(13)).unwrap().state
            {
                *arrives_at_ms = 201;
            }
        },
        "fleet 13 in state InTransit",
    );
    rejected(
        |w| {
            let _ = w.queue.insert(QueueEntry {
                at_ms: 250,
                kind: QueueKind::FleetArrival(FleetId(13)),
                version: 1,
            });
        },
        "fleet 13 has two live queue entries",
    );
    rejected(
        |w| {
            let _ = w.queue.insert(QueueEntry {
                at_ms: 400,
                kind: QueueKind::FleetArrival(FleetId(13)),
                version: 2,
            });
        },
        "is newer than fleet 13, at version 1",
    );
    rejected(
        |w| {
            let _ = w.queue.insert(QueueEntry {
                at_ms: 400,
                kind: QueueKind::PlanetChange(PlanetId(2)),
                version: 5,
            });
        },
        "is newer than planet 2, at version 0",
    );
}

#[test]
fn unknown_fields_do_not_deserialize() {
    let text = json(&sample_world()).replacen("\"seed\":", "\"sneaky\":0,\"seed\":", 1);
    let error = reload(&text).expect_err("rejected").to_string();
    assert!(error.contains("unknown field `sneaky`"), "{error}");
}
