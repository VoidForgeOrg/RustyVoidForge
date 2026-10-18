//! The golden scenarios (Technical Plan P2.6; decision log D-040): scripted worlds and commands
//! that the host's `golden` test binary runs through the real host and records into
//! `tests/golden/`. They live here because testkit is below the host (D-035): scenarios in
//! testkit, recording through the host in the host's tests.
//!
//! A scenario is a start World, cues and an end time, all on Rules v0, so a rules change makes
//! every golden fail until it is blessed again. A cue names the game time it runs at, the player,
//! an optional idempotency key, and the action, computed from the published World at that time
//! (so it can name a ship or fleet the run created). Each cue states whether it must be accepted
//! or rejected with a given reason, so a scenario cannot quietly stop doing what it says.

use std::collections::BTreeSet;

use voidforge_core::command::Action;
use voidforge_core::quantity::{MS_PER_HOUR, Quantity};
use voidforge_core::rules::Rules;
use voidforge_core::state::{
    BuildingId, BuildingKind, BuildingState, Cargo, FleetId, Mission, PlanetId, PlayerId, Position,
    ShipBuildId, ShipId, ShipKind, ShipLocation, World,
};

use crate::determinism::CAROL;
use crate::{ALICE, BOB, WorldBuilder, v0_rules};

const H: i64 = MS_PER_HOUR;
/// A ton in grams.
const T: i64 = 1_000_000;
/// A world unit in distance units (D-021).
const U: i64 = 1_000;

/// What a cue must get back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Accepted.
    Accepted,
    /// Rejected with this reason code (`Rejection::code`).
    Rejected(&'static str),
}

/// One scripted command.
pub struct Cue {
    /// The game time the clock moves to before the command (due events run first).
    pub at_ms: i64,
    /// Who issues it.
    pub player: PlayerId,
    /// The idempotency key, if any.
    pub key: Option<&'static str>,
    /// Submit it a second time at once with the same key: the retry must get the first receipt
    /// and append nothing (D-037).
    pub retry: bool,
    /// What it must get back.
    pub expect: Expect,
    /// The action, from the published World at `at_ms`.
    pub action: Box<dyn Fn(&World) -> Action + Send + Sync>,
}

impl Cue {
    /// An accepted command without a key.
    pub fn new(
        at_ms: i64,
        player: PlayerId,
        action: impl Fn(&World) -> Action + Send + Sync + 'static,
    ) -> Self {
        Self {
            at_ms,
            player,
            key: None,
            retry: false,
            expect: Expect::Accepted,
            action: Box::new(action),
        }
    }

    /// With this idempotency key.
    #[must_use]
    pub fn key(mut self, key: &'static str) -> Self {
        self.key = Some(key);
        self
    }

    /// Retried at once with the same key.
    #[must_use]
    pub fn retried(mut self) -> Self {
        self.retry = true;
        self
    }

    /// Rejected with this reason code.
    #[must_use]
    pub fn rejected(mut self, code: &'static str) -> Self {
        self.expect = Expect::Rejected(code);
        self
    }
}

/// A golden scenario.
pub struct Scenario {
    /// Its directory under `tests/golden/`.
    pub name: &'static str,
    /// What it covers, in a sentence (written into its manifest).
    pub about: &'static str,
    /// The rules: Rules v0.
    pub rules: Rules,
    /// The start World, solved and scheduled.
    pub world: World,
    /// The commands, in time order.
    pub cues: Vec<Cue>,
    /// The game time the run ends at; every event due by then runs.
    pub end_ms: i64,
}

/// Every golden scenario, in a fixed order.
pub fn scenarios() -> Vec<Scenario> {
    vec![
        start_and_build(),
        starve_and_resume(),
        ships_and_colonize(),
        transport_and_recall(),
        rejections_and_retries(),
        whole_loop(),
    ]
}

/// Builds the world and solves every owned planet, so its first events are scheduled.
fn finish(b: WorldBuilder) -> World {
    let rules = b.rules().clone();
    let mut w = b.build();
    let owned: Vec<PlanetId> = w
        .planets
        .values()
        .filter(|p| p.owner.is_some())
        .map(|p| p.id)
        .collect();
    for p in owned {
        w.solve_planet(p, &rules).expect("a scenario world solves");
    }
    w
}

fn at(b: &mut WorldBuilder, planet: PlanetId, x: i64, y: i64) {
    b.position(
        planet,
        Position {
            x: x * U,
            y: y * U,
            z: 0,
        },
    );
}

/// The lowest-ID building of `kind` on `planet` in `state` (any state if `None`).
fn building(w: &World, planet: PlanetId, kind: BuildingKind, operational: bool) -> BuildingId {
    *w.planets[&planet]
        .buildings
        .iter()
        .find(|id| {
            let b = &w.buildings[id];
            b.kind == kind && (!operational || b.state == BuildingState::Operational)
        })
        .expect("the scenario's building exists")
}

/// The highest-ID building of `kind` on `planet`: the one just ordered.
fn newest_building(w: &World, planet: PlanetId, kind: BuildingKind) -> BuildingId {
    *w.planets[&planet]
        .buildings
        .iter()
        .rev()
        .find(|id| w.buildings[id].kind == kind)
        .expect("the scenario's building exists")
}

/// The ships of `kind` on `planet`'s roster.
fn roster(w: &World, planet: PlanetId, kind: Option<ShipKind>) -> BTreeSet<ShipId> {
    w.ships
        .values()
        .filter(|s| s.location == ShipLocation::Roster(planet))
        .filter(|s| kind.is_none_or(|k| s.kind == k))
        .map(|s| s.id)
        .collect()
}

/// The fleet assembled last.
fn newest_fleet(w: &World) -> FleetId {
    *w.fleets.keys().next_back().expect("a fleet exists")
}

/// The ship build queued last at `shipyard`.
fn newest_ship_build(w: &World, shipyard: BuildingId) -> ShipBuildId {
    *w.ship_builds
        .values()
        .rev()
        .find(|b| b.shipyard == shipyard)
        .map(|b| &b.id)
        .expect("a ship build exists")
}

/// Two players join and build: constructions complete, one is cancelled, and a demolition
/// runs to its end.
fn start_and_build() -> Scenario {
    let mut b = WorldBuilder::with_rules(v0_rules());
    let home_a = b.planet(None);
    let home_b = b.planet(None);
    at(&mut b, home_a, 0, 0);
    at(&mut b, home_b, 200, 0);
    b.candidates(&[home_a, home_b]);
    let world = finish(b);
    Scenario {
        name: "start-and-build",
        about: "Two players join; a Drill and a Shipyard are built to completion, a Refinery's \
                construction is cancelled, and an original Drill is demolished to its end.",
        rules: v0_rules(),
        world,
        cues: vec![
            Cue::new(0, ALICE, |_| Action::JoinGame),
            Cue::new(0, BOB, |_| Action::JoinGame),
            Cue::new(0, ALICE, move |_| Action::BuildBuilding {
                planet: home_a,
                kind: BuildingKind::Drill,
            }),
            Cue::new(0, ALICE, move |_| Action::BuildBuilding {
                planet: home_a,
                kind: BuildingKind::Refinery,
            }),
            Cue::new(0, BOB, move |_| Action::BuildBuilding {
                planet: home_b,
                kind: BuildingKind::Shipyard,
            }),
            Cue::new(H, ALICE, move |w| Action::CancelConstruction {
                building: newest_building(w, home_a, BuildingKind::Refinery),
            }),
            Cue::new(5 * H, ALICE, move |w| Action::DemolishBuilding {
                building: building(w, home_a, BuildingKind::Drill, true),
            }),
        ],
        end_ms: 13 * H,
    }
}

/// Bob's constructions starve when his ingots run out and share the Refinery's output evenly;
/// Alice's full storage halts her Refinery and then her Drill, and a Shipyard construction that
/// draws ingots down resumes both (crossings that log no events).
fn starve_and_resume() -> Scenario {
    let mut b = WorldBuilder::with_rules(v0_rules());
    let home_a = b.player(ALICE);
    let home_b = b.player(BOB);
    at(&mut b, home_a, 0, 0);
    at(&mut b, home_b, 200, 0);
    b.stocks(
        home_a,
        Quantity::new(9_990 * T, 0, 0),
        Quantity::new(4_990 * T, 0, 0),
    );
    b.stocks(
        home_b,
        Quantity::new(2_000 * T, 0, 0),
        Quantity::new(100 * T, 0, 0),
    );
    let world = finish(b);
    Scenario {
        name: "starve-and-resume",
        about: "Constructions starve at zero ingots and split the Refinery's output evenly; full \
                storage halts a Refinery and then a Drill, and a Shipyard drawing ingots resumes \
                them.",
        rules: v0_rules(),
        world,
        cues: vec![
            Cue::new(0, BOB, move |_| Action::BuildBuilding {
                planet: home_b,
                kind: BuildingKind::Shipyard,
            }),
            Cue::new(0, BOB, move |_| Action::BuildBuilding {
                planet: home_b,
                kind: BuildingKind::Drill,
            }),
            Cue::new(5 * H, ALICE, move |_| Action::BuildBuilding {
                planet: home_a,
                kind: BuildingKind::Shipyard,
            }),
        ],
        end_ms: 20 * H,
    }
}

/// A Shipyard builds ships, three at once with a fourth waiting that is cancelled; two lone
/// Colony Ships race for one planet: the lower fleet ID colonizes it and is removed, the other
/// fails and stays aboard.
fn ships_and_colonize() -> Scenario {
    let mut b = WorldBuilder::with_rules(v0_rules());
    let home = b.player(ALICE);
    let target = b.planet(None);
    at(&mut b, home, 0, 0);
    at(&mut b, target, 30, 0);
    b.stocks(
        home,
        Quantity::new(2_000 * T, 0, 0),
        Quantity::new(3_000 * T, 0, 0),
    );
    let yard = b.building(home, BuildingKind::Shipyard, BuildingState::Operational);
    let world = finish(b);
    let queue = move |kind| {
        move |_: &World| Action::QueueShip {
            shipyard: yard,
            kind,
        }
    };
    let colony_fleet = move |w: &World| Action::AssembleFleet {
        planet: home,
        ships: roster(w, home, Some(ShipKind::ColonyShip))
            .into_iter()
            .take(1)
            .collect(),
        cargo: Cargo::default(),
    };
    let colonize = move |w: &World| Action::SendFleet {
        fleet: newest_fleet(w),
        target,
        mission: Mission::Colonize,
    };
    Scenario {
        name: "ships-and-colonize",
        about: "Ships are queued, built three at once and one waiting build is cancelled; two lone \
                Colony Ships reach one planet at the same millisecond: the lower fleet ID \
                colonizes it and its emptied fleet is removed, and the other fails.",
        rules: v0_rules(),
        world,
        cues: vec![
            Cue::new(0, ALICE, queue(ShipKind::CargoVessel)),
            Cue::new(0, ALICE, queue(ShipKind::ColonyShip)),
            Cue::new(0, ALICE, queue(ShipKind::ColonyShip)),
            Cue::new(0, ALICE, queue(ShipKind::CargoVessel)),
            Cue::new(H, ALICE, move |w| Action::CancelShipBuild {
                build: newest_ship_build(w, yard),
            }),
            Cue::new(11 * H, ALICE, colony_fleet),
            Cue::new(11 * H, ALICE, colonize),
            Cue::new(11 * H, ALICE, colony_fleet),
            Cue::new(11 * H, ALICE, colonize),
        ],
        end_ms: 16 * H,
    }
}

/// Cargo Vessels carry ingots on a Transport (unloaded on arrival) and ore on a Move (unloaded
/// by Trigger unload); a fleet is recalled in flight, returns to where it left and is disbanded.
fn transport_and_recall() -> Scenario {
    let mut b = WorldBuilder::with_rules(v0_rules());
    let home = b.player(ALICE);
    let colony = b.planet(Some(ALICE));
    at(&mut b, home, 0, 0);
    at(&mut b, colony, 30, 0);
    b.ship(ALICE, ShipKind::CargoVessel, home);
    b.ship(ALICE, ShipKind::CargoVessel, home);
    let world = finish(b);
    let first_vessel = move |w: &World| -> BTreeSet<ShipId> {
        roster(w, home, Some(ShipKind::CargoVessel))
            .into_iter()
            .take(1)
            .collect()
    };
    let lowest_fleet = |w: &World| *w.fleets.keys().next().expect("a fleet exists");
    Scenario {
        name: "transport-and-recall",
        about: "A Transport unloads ingots on arrival; a Move carries ore that a Trigger unload \
                delivers; a fleet recalled in flight returns to where it left and is disbanded.",
        rules: v0_rules(),
        world,
        cues: vec![
            Cue::new(0, ALICE, move |w| Action::AssembleFleet {
                planet: home,
                ships: first_vessel(w),
                cargo: Cargo {
                    ore_g: 0,
                    ingots_g: 400 * T,
                },
            }),
            Cue::new(0, ALICE, move |w| Action::SendFleet {
                fleet: newest_fleet(w),
                target: colony,
                mission: Mission::Transport,
            }),
            Cue::new(0, ALICE, move |w| Action::AssembleFleet {
                planet: home,
                ships: first_vessel(w),
                cargo: Cargo {
                    ore_g: 300 * T,
                    ingots_g: 0,
                },
            }),
            Cue::new(0, ALICE, move |w| Action::SendFleet {
                fleet: newest_fleet(w),
                target: colony,
                mission: Mission::Move,
            }),
            Cue::new(3 * H, ALICE, move |w| Action::TriggerUnload {
                fleet: newest_fleet(w),
            }),
            Cue::new(3 * H, ALICE, move |w| Action::SendFleet {
                fleet: lowest_fleet(w),
                target: home,
                mission: Mission::Move,
            }),
            Cue::new(4 * H, ALICE, move |w| Action::RecallFleet {
                fleet: lowest_fleet(w),
            }),
            Cue::new(6 * H, ALICE, move |w| Action::DisbandFleet {
                fleet: lowest_fleet(w),
            }),
        ],
        end_ms: 7 * H,
    }
}

/// Representative rejections, one per command family, and keyed commands retried with the same
/// key, which get their first receipt and append nothing.
fn rejections_and_retries() -> Scenario {
    let mut b = WorldBuilder::with_rules(v0_rules());
    let home_a = b.player(ALICE);
    let home_b = b.player(BOB);
    at(&mut b, home_a, 0, 0);
    at(&mut b, home_b, 50, 0);
    b.ship(ALICE, ShipKind::CargoVessel, home_a);
    let world = finish(b);
    let drill = move |w: &World| building(w, home_a, BuildingKind::Drill, true);
    Scenario {
        name: "rejections-and-retries",
        about: "A rejection per command family, each logged with its reason and changing nothing; \
                keyed commands retried with the same key get their first receipt, accepted or \
                rejected, and append nothing.",
        rules: v0_rules(),
        world,
        cues: vec![
            Cue::new(0, ALICE, |_| Action::JoinGame).rejected("already_joined"),
            Cue::new(0, CAROL, |_| Action::JoinGame).rejected("no_free_homeworld"),
            Cue::new(0, CAROL, move |_| Action::BuildBuilding {
                planet: home_a,
                kind: BuildingKind::Drill,
            })
            .rejected("not_joined"),
            Cue::new(0, ALICE, move |_| Action::BuildBuilding {
                planet: home_b,
                kind: BuildingKind::Drill,
            })
            .rejected("not_owner"),
            Cue::new(0, ALICE, move |w| Action::CancelConstruction {
                building: drill(w),
            })
            .rejected("not_under_construction"),
            Cue::new(0, ALICE, move |w| Action::QueueShip {
                shipyard: drill(w),
                kind: ShipKind::CargoVessel,
            })
            .rejected("not_a_shipyard"),
            Cue::new(0, ALICE, move |_| Action::AssembleFleet {
                planet: home_a,
                ships: BTreeSet::new(),
                cargo: Cargo::default(),
            })
            .rejected("no_ships"),
            Cue::new(0, ALICE, |_| Action::RecallFleet {
                fleet: FleetId(u64::MAX),
            })
            .rejected("unknown_fleet"),
            Cue::new(H, ALICE, move |_| Action::BuildBuilding {
                planet: home_a,
                kind: BuildingKind::Drill,
            })
            .key("build-drill-1")
            .retried(),
            Cue::new(2 * H, ALICE, move |_| Action::BuildBuilding {
                planet: home_b,
                kind: BuildingKind::Drill,
            })
            .key("build-elsewhere")
            .retried()
            .rejected("not_owner"),
            Cue::new(4 * H, BOB, move |_| Action::BuildBuilding {
                planet: home_b,
                kind: BuildingKind::Refinery,
            })
            .key("build-drill-1"),
        ],
        end_ms: 5 * H,
    }
}

/// The Phase 1 gate's whole loop (QA's hand-derived scenario, `tests/scenario.rs`): join, a
/// Shipyard, a Colony Ship and a Cargo Vessel, a colonization 45 units away carrying 150 t of
/// ingots, and a Drill on the colony. Its score at the end, 3,239, was derived by hand.
fn whole_loop() -> Scenario {
    let mut b = WorldBuilder::with_rules(v0_rules());
    let home = b.planet(None);
    let colony = b.planet(None);
    at(&mut b, home, 0, 0);
    at(&mut b, colony, 45, 0);
    b.candidates(&[home]);
    let world = finish(b);
    let yard = move |w: &World| building(w, home, BuildingKind::Shipyard, true);
    Scenario {
        name: "whole-loop",
        about: "The Phase 1 gate's whole Game Design loop, hand-derived by QA: join, a Shipyard, \
                two ships, a colonization carrying 150 t of ingots, and a Drill on the colony; \
                the score at the end is 3,239.",
        rules: v0_rules(),
        world,
        cues: vec![
            Cue::new(0, ALICE, |_| Action::JoinGame),
            Cue::new(0, ALICE, move |_| Action::BuildBuilding {
                planet: home,
                kind: BuildingKind::Shipyard,
            }),
            Cue::new(50 * H, ALICE, move |w| Action::QueueShip {
                shipyard: yard(w),
                kind: ShipKind::ColonyShip,
            }),
            Cue::new(50 * H, ALICE, move |w| Action::QueueShip {
                shipyard: yard(w),
                kind: ShipKind::CargoVessel,
            }),
            Cue::new(60 * H, ALICE, move |w| Action::AssembleFleet {
                planet: home,
                ships: roster(w, home, None),
                cargo: Cargo {
                    ore_g: 0,
                    ingots_g: 150 * T,
                },
            }),
            Cue::new(60 * H, ALICE, move |w| Action::SendFleet {
                fleet: newest_fleet(w),
                target: colony,
                mission: Mission::Colonize,
            }),
            Cue::new(645 * H / 10, ALICE, move |_| Action::BuildBuilding {
                planet: colony,
                kind: BuildingKind::Drill,
            }),
        ],
        end_ms: 675 * H / 10,
    }
}
