//! Commands, part 1: [`World::apply`] with Join game and the five planet and Shipyard commands
//! (Architectural Design, Engine core and Commands; Game Design, Player actions; decision log
//! D-028).
//!
//! `apply(command, t)` advances to t, so events at t run first, then validates the command
//! against the state at t. A rejection carries a stable reason code and changes nothing beyond
//! the advance. An accepted command applies its effect and re-solves the planet it touched.
//! Checks run in one fixed order: who, what, whose, then state. The host owns idempotency keys
//! and sequence numbers; the core never sees them.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::quantity::Quantity;
use crate::rules::{BuildCost, Rules};
use crate::state::{
    Building, BuildingId, BuildingKind, BuildingState, Cargo, DomainEvent, EngineError, FleetId,
    Mission, Planet, PlanetId, Player, PlayerId, ShipBuild, ShipBuildId, ShipBuildStatus, ShipId,
    ShipKind, World,
};

/// A player's command, as the core takes it. The shell adds the time (`t`) and the order of
/// calls (`seq`); the idempotency key stays with the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// The issuing player, from the auth token.
    pub player: PlayerId,
    /// What they ask for.
    pub action: Action,
}

/// What a command asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Take the next free homeworld candidate (a system command, issued at sign-up).
    JoinGame,
    /// Start a building on one of the player's planets.
    BuildBuilding {
        /// The planet.
        planet: PlanetId,
        /// What to build.
        kind: BuildingKind,
    },
    /// Remove a building under construction; nothing is refunded.
    CancelConstruction {
        /// The building.
        building: BuildingId,
    },
    /// Start demolishing an operational building.
    DemolishBuilding {
        /// The building.
        building: BuildingId,
    },
    /// Queue a ship at an operational Shipyard.
    QueueShip {
        /// The Shipyard.
        shipyard: BuildingId,
        /// What to build.
        kind: ShipKind,
    },
    /// Remove an active or waiting ship build; nothing is refunded.
    CancelShipBuild {
        /// The build.
        build: ShipBuildId,
    },
    /// Form a fleet from ships on one planet's roster, optionally loading cargo from its storage.
    AssembleFleet {
        /// The planet whose roster the ships are on.
        planet: PlanetId,
        /// The ships; never empty.
        ships: BTreeSet<ShipId>,
        /// Grams of each resource to load; zero for none.
        cargo: Cargo,
    },
    /// Send a stationed fleet on a mission.
    SendFleet {
        /// The fleet.
        fleet: FleetId,
        /// Where to.
        target: PlanetId,
        /// What to do there.
        mission: Mission,
    },
    /// Turn a fleet in transit around, once.
    RecallFleet {
        /// The fleet.
        fleet: FleetId,
    },
    /// Return a stationed, empty fleet's ships to the planet's roster.
    DisbandFleet {
        /// The fleet.
        fleet: FleetId,
    },
    /// Unload a stationed fleet's cargo into its planet's storage, up to free storage.
    TriggerUnload {
        /// The fleet.
        fleet: FleetId,
    },
}

/// Why a command was rejected. Each variant is one stable reason code, the contract the API
/// exposes (D-028); [`Rejection::code`] gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum Rejection {
    /// Join game: the player has already joined.
    #[error("the player has already joined")]
    AlreadyJoined,
    /// Join game: every homeworld candidate is owned.
    #[error("no homeworld candidate is free")]
    NoFreeHomeworld,
    /// Any other command: the player has not joined.
    #[error("the player has not joined")]
    NotJoined,
    /// No such planet.
    #[error("no such planet")]
    UnknownPlanet,
    /// No such building, or no longer.
    #[error("no such building")]
    UnknownBuilding,
    /// No such ship build, or no longer.
    #[error("no such ship build")]
    UnknownShipBuild,
    /// The planet is not the player's.
    #[error("the planet is not the player's")]
    NotOwner,
    /// Every slot is taken, counting construction and demolition.
    #[error("no free slot")]
    NoFreeSlot,
    /// The building is not under construction.
    #[error("the building is not under construction")]
    NotUnderConstruction,
    /// The building is not operational.
    #[error("the building is not operational")]
    NotOperational,
    /// A Shipyard with active or waiting builds cannot be demolished.
    #[error("the Shipyard has ships building or queued")]
    ShipyardHasBuilds,
    /// Ships are queued only at a Shipyard.
    #[error("the building is not a Shipyard")]
    NotAShipyard,
    /// The Shipyard is under construction or being demolished.
    #[error("the Shipyard is not operational")]
    ShipyardNotOperational,
    /// No such fleet, or no longer.
    #[error("no such fleet")]
    UnknownFleet,
    /// No such ship, or no longer.
    #[error("no such ship")]
    UnknownShip,
    /// A fleet needs at least one ship.
    #[error("no ships were given")]
    NoShips,
    /// Cargo amounts are not negative.
    #[error("a cargo amount is negative")]
    InvalidCargo,
    /// A ship is not on that planet's roster.
    #[error("a ship is not on the planet's roster")]
    ShipNotOnRoster,
    /// Not enough of a resource in storage to load.
    #[error("not enough in storage")]
    InsufficientStock,
    /// The cargo exceeds the fleet's capacity.
    #[error("the cargo exceeds the fleet's capacity")]
    InsufficientCapacity,
    /// The fleet is not stationed.
    #[error("the fleet is not stationed")]
    FleetNotStationed,
    /// The fleet is stationed, not in transit.
    #[error("the fleet is not in transit")]
    FleetNotInTransit,
    /// The fleet was already recalled.
    #[error("the fleet was already recalled")]
    AlreadyRecalled,
    /// The target is where the fleet is.
    #[error("the fleet is already at the target")]
    SamePlanet,
    /// Colonize needs a Colony Ship.
    #[error("the fleet has no Colony Ship")]
    NoColonyShip,
    /// Colonize needs an unowned target.
    #[error("the target is owned")]
    TargetOwned,
    /// Transport needs a Cargo Vessel.
    #[error("the fleet has no Cargo Vessel")]
    NoCargoVessel,
    /// Transport needs a target the player owns.
    #[error("the target is not the player's")]
    TargetNotOwned,
    /// The trip has no real duration: zero distance, a trip longer than the travel horizon
    /// (`MAX_TRAVEL_MS`, D-050), or an arrival past the end of time (Send); or a return longer
    /// than the horizon or past the end of time (Recall).
    #[error("the target cannot be reached")]
    UnreachableTarget,
    /// A fleet with cargo aboard cannot be disbanded.
    #[error("the fleet has cargo aboard")]
    CargoAboard,
    /// Unloading needs a planet the player owns.
    #[error("the fleet's planet is not the player's")]
    PlanetNotOwned,
}

impl Rejection {
    /// The reason code: stable, snake_case, as the API returns it.
    pub fn code(self) -> &'static str {
        match self {
            Self::AlreadyJoined => "already_joined",
            Self::NoFreeHomeworld => "no_free_homeworld",
            Self::NotJoined => "not_joined",
            Self::UnknownPlanet => "unknown_planet",
            Self::UnknownBuilding => "unknown_building",
            Self::UnknownShipBuild => "unknown_ship_build",
            Self::NotOwner => "not_owner",
            Self::NoFreeSlot => "no_free_slot",
            Self::NotUnderConstruction => "not_under_construction",
            Self::NotOperational => "not_operational",
            Self::ShipyardHasBuilds => "shipyard_has_builds",
            Self::NotAShipyard => "not_a_shipyard",
            Self::ShipyardNotOperational => "shipyard_not_operational",
            Self::UnknownFleet => "unknown_fleet",
            Self::UnknownShip => "unknown_ship",
            Self::NoShips => "no_ships",
            Self::InvalidCargo => "invalid_cargo",
            Self::ShipNotOnRoster => "ship_not_on_roster",
            Self::InsufficientStock => "insufficient_stock",
            Self::InsufficientCapacity => "insufficient_capacity",
            Self::FleetNotStationed => "fleet_not_stationed",
            Self::FleetNotInTransit => "fleet_not_in_transit",
            Self::AlreadyRecalled => "already_recalled",
            Self::SamePlanet => "same_planet",
            Self::NoColonyShip => "no_colony_ship",
            Self::TargetOwned => "target_owned",
            Self::NoCargoVessel => "no_cargo_vessel",
            Self::TargetNotOwned => "target_not_owned",
            Self::UnreachableTarget => "unreachable_target",
            Self::CargoAboard => "cargo_aboard",
            Self::PlanetNotOwned => "planet_not_owned",
        }
    }
}

/// What [`World::apply`] did: the events of the advance to t, then the command's outcome, with
/// the events its re-solve produced if it was accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Applied {
    /// Events at or before t that ran first.
    pub advanced: Vec<DomainEvent>,
    /// Accepted with the command's events, or rejected with its reason.
    pub outcome: Result<Vec<DomainEvent>, Rejection>,
}

impl World {
    /// Advances to `t`, then validates `command` against the state at `t` and applies it, or
    /// rejects it with a reason code and changes nothing more. An advance error is returned as
    /// `advance_to` returns it. After validation, the effect can fail only through an exhausted
    /// ID counter or an engine error in the re-solve, which poison the world (D-028); a
    /// rejection never does.
    pub fn apply(
        &mut self,
        command: &Command,
        t: i64,
        rules: &Rules,
    ) -> Result<Applied, EngineError> {
        let advanced = self.advance_to(t, rules)?;
        if let Err(rejection) = self.check(command, rules) {
            return Ok(Applied {
                advanced,
                outcome: Err(rejection),
            });
        }
        let result = self
            .execute(command, rules)
            .and_then(|planet| match planet {
                Some(planet) => self.solve_planet(planet, rules),
                None => Ok(Vec::new()),
            });
        match result {
            Ok(events) => Ok(Applied {
                advanced,
                outcome: Ok(events),
            }),
            Err(error) => {
                self.poison();
                Err(error)
            }
        }
    }

    /// The first rule `command` breaks at world time: who, what, whose, then state.
    fn check(&self, command: &Command, rules: &Rules) -> Result<(), Rejection> {
        let joined = self.players.contains_key(&command.player);
        let owned = |planet: PlanetId| {
            if self.planets[&planet].owner == Some(command.player) {
                Ok(())
            } else {
                Err(Rejection::NotOwner)
            }
        };
        let building = |id: BuildingId| {
            let b = self.buildings.get(&id).ok_or(Rejection::UnknownBuilding)?;
            owned(b.planet)?;
            Ok(b)
        };
        match command.action {
            Action::JoinGame if joined => Err(Rejection::AlreadyJoined),
            Action::JoinGame => self
                .free_homeworld()
                .map(|_| ())
                .ok_or(Rejection::NoFreeHomeworld),
            _ if !joined => Err(Rejection::NotJoined),
            Action::AssembleFleet { .. }
            | Action::SendFleet { .. }
            | Action::RecallFleet { .. }
            | Action::DisbandFleet { .. }
            | Action::TriggerUnload { .. } => self.check_fleet(command, rules),
            Action::BuildBuilding { planet, .. } => {
                let p = self.planets.get(&planet).ok_or(Rejection::UnknownPlanet)?;
                owned(planet)?;
                if p.buildings.len() >= p.slots as usize {
                    return Err(Rejection::NoFreeSlot);
                }
                Ok(())
            }
            Action::CancelConstruction { building: id } => match building(id)?.state {
                BuildingState::UnderConstruction { .. } => Ok(()),
                _ => Err(Rejection::NotUnderConstruction),
            },
            Action::DemolishBuilding { building: id } => {
                let b = building(id)?;
                if b.state != BuildingState::Operational {
                    return Err(Rejection::NotOperational);
                }
                if b.kind == BuildingKind::Shipyard
                    && self.ship_builds.values().any(|s| s.shipyard == id)
                {
                    return Err(Rejection::ShipyardHasBuilds);
                }
                Ok(())
            }
            Action::QueueShip { shipyard, .. } => {
                let b = building(shipyard)?;
                if b.kind != BuildingKind::Shipyard {
                    return Err(Rejection::NotAShipyard);
                }
                if b.state != BuildingState::Operational {
                    return Err(Rejection::ShipyardNotOperational);
                }
                Ok(())
            }
            Action::CancelShipBuild { build } => {
                let s = self
                    .ship_builds
                    .get(&build)
                    .ok_or(Rejection::UnknownShipBuild)?;
                owned(self.buildings[&s.shipyard].planet)
            }
        }
    }

    /// The first homeworld candidate, in list order, that nobody owns: the planet Join game
    /// would take. `None` means Join game is refused with `no_free_homeworld`.
    #[must_use]
    pub fn free_homeworld(&self) -> Option<PlanetId> {
        self.homeworld_candidates
            .iter()
            .copied()
            .find(|p| self.planets.get(p).is_some_and(|p| p.owner.is_none()))
    }

    /// Applies a validated command at world time; returns the planet to re-solve, if any.
    fn execute(
        &mut self,
        command: &Command,
        rules: &Rules,
    ) -> Result<Option<PlanetId>, EngineError> {
        let now = self.time_ms;
        match command.action {
            Action::AssembleFleet { .. }
            | Action::SendFleet { .. }
            | Action::RecallFleet { .. }
            | Action::DisbandFleet { .. }
            | Action::TriggerUnload { .. } => self.execute_fleet(command, rules),
            Action::JoinGame => {
                let home = self.free_homeworld().expect("checked");
                let spec = rules.homeworld;
                let kinds = std::iter::repeat_n(BuildingKind::Drill, spec.drills as usize).chain(
                    std::iter::repeat_n(BuildingKind::Refinery, spec.refineries as usize),
                );
                for kind in kinds {
                    self.add_building(home, kind, BuildingState::Operational)?;
                }
                let p = self.planets.get_mut(&home).expect("checked");
                p.owner = Some(command.player);
                p.ore = Quantity::new(spec.ore_g, 0, now);
                p.ingots = Quantity::new(spec.ingots_g, 0, now);
                self.players.insert(
                    command.player,
                    Player {
                        id: command.player,
                        homeworld: home,
                    },
                );
                Ok(Some(home))
            }
            Action::BuildBuilding { planet, kind } => {
                let build = building_cost(rules, kind);
                let state = BuildingState::UnderConstruction {
                    delivered: Quantity::new(0, 0, now),
                    cost_g: build.cost_g,
                    rated_g_per_h: rate(build, "building's build rate")?,
                };
                self.add_building(planet, kind, state)?;
                Ok(Some(planet))
            }
            Action::CancelConstruction { building } => {
                let b = self.buildings.remove(&building).expect("checked");
                self.planets
                    .get_mut(&b.planet)
                    .expect("checked")
                    .buildings
                    .remove(&building);
                Ok(Some(b.planet))
            }
            Action::DemolishBuilding { building } => {
                let b = self.buildings.get_mut(&building).expect("checked");
                let specs = &rules.buildings;
                let duration = match b.kind {
                    BuildingKind::Drill => specs.drill.demolition_time_ms,
                    BuildingKind::Refinery => specs.refinery.demolition_time_ms,
                    BuildingKind::Shipyard => specs.shipyard.demolition_time_ms,
                };
                let ends_at_ms =
                    now.checked_add(duration)
                        .ok_or(EngineError::CommandOutOfRange {
                            what: "demolition end",
                        })?;
                b.state = BuildingState::Demolishing { ends_at_ms };
                Ok(Some(b.planet))
            }
            Action::QueueShip { shipyard, kind } => {
                let build = match kind {
                    ShipKind::ColonyShip => rules.ships.colony_ship.build,
                    ShipKind::CargoVessel => rules.ships.cargo_vessel.build,
                };
                let rated_g_per_h = rate(build, "ship's build rate")?;
                let active = self
                    .ship_builds
                    .values()
                    .filter(|s| s.shipyard == shipyard && s.status == ShipBuildStatus::Active)
                    .count();
                let status = if active < rules.buildings.shipyard.parallel_builds as usize {
                    ShipBuildStatus::Active
                } else {
                    ShipBuildStatus::Waiting
                };
                let id = ShipBuildId(self.allocate_id()?);
                self.ship_builds.insert(
                    id,
                    ShipBuild {
                        id,
                        shipyard,
                        kind,
                        delivered: Quantity::new(0, 0, now),
                        cost_g: build.cost_g,
                        rated_g_per_h,
                        status,
                    },
                );
                Ok(Some(self.buildings[&shipyard].planet))
            }
            Action::CancelShipBuild { build } => {
                let s = self.ship_builds.remove(&build).expect("checked");
                if s.status == ShipBuildStatus::Active {
                    let limit = rules.buildings.shipyard.parallel_builds;
                    self.start_waiting_builds(s.shipyard, limit, now)?;
                }
                Ok(Some(self.buildings[&s.shipyard].planet))
            }
        }
    }

    fn add_building(
        &mut self,
        planet: PlanetId,
        kind: BuildingKind,
        state: BuildingState,
    ) -> Result<BuildingId, EngineError> {
        let id = BuildingId(self.allocate_id()?);
        self.buildings.insert(
            id,
            Building {
                id,
                planet,
                kind,
                state,
            },
        );
        let p: &mut Planet = self.planets.get_mut(&planet).expect("checked");
        p.buildings.insert(id);
        Ok(id)
    }
}

fn building_cost(rules: &Rules, kind: BuildingKind) -> BuildCost {
    let b = &rules.buildings;
    match kind {
        BuildingKind::Drill => b.drill.build,
        BuildingKind::Refinery => b.refinery.build,
        BuildingKind::Shipyard => b.shipyard.build,
    }
}

/// A build's rated rate; validated rules always have one.
fn rate(build: BuildCost, what: &'static str) -> Result<i64, EngineError> {
    build
        .rate_g_per_h()
        .ok_or(EngineError::CommandOutOfRange { what })
}

mod fleet;
