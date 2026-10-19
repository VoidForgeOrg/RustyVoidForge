//! The core ↔ wire mapping (decision log D-045, D-049): requests in, views and events out. Every
//! match is exhaustive, with no wildcard arm, so a new core variant fails to compile until it's
//! mapped here, as `codes::game_code` does for reason codes.

use std::collections::BTreeSet;

use voidforge_api_types::command::CommandRequest;
use voidforge_api_types::event::Event;
use voidforge_api_types::view as api;
use voidforge_api_types::wire as w;
use voidforge_host::view::{self as core, BuildingStatus};
use voidforge_host::{
    Action, BuildingKind, Cargo, DomainEvent, FleetState, FlowLabel, Mission, PlayerId,
    ShipBuildStatus, ShipKind,
};

use super::time::{OutOfRange, timestamp};

/// A request's action. The duplicate-ship check comes first (400 `duplicate_ship_ids`).
///
/// # Errors
/// [`voidforge_api_types::error::ErrorCode::DuplicateShipIds`].
pub fn action(request: CommandRequest) -> Result<Action, voidforge_api_types::error::ErrorCode> {
    Ok(match request {
        CommandRequest::BuildBuilding(b) => Action::BuildBuilding {
            planet: voidforge_host::PlanetId(b.planet_id.0),
            kind: building_kind(b.building_type),
        },
        CommandRequest::CancelConstruction(b) => Action::CancelConstruction {
            building: voidforge_host::BuildingId(b.building_id.0),
        },
        CommandRequest::DemolishBuilding(b) => Action::DemolishBuilding {
            building: voidforge_host::BuildingId(b.building_id.0),
        },
        CommandRequest::QueueShip(q) => Action::QueueShip {
            shipyard: voidforge_host::BuildingId(q.shipyard_id.0),
            kind: ship_kind(q.ship_type),
        },
        CommandRequest::CancelShipBuild(c) => Action::CancelShipBuild {
            build: voidforge_host::ShipBuildId(c.ship_build_id.0),
        },
        CommandRequest::AssembleFleet(a) => {
            a.check_unique_ships()?;
            Action::AssembleFleet {
                planet: voidforge_host::PlanetId(a.planet_id.0),
                ships: a
                    .ship_ids
                    .iter()
                    .map(|s| voidforge_host::ShipId(s.0))
                    .collect::<BTreeSet<_>>(),
                cargo: Cargo {
                    ore_g: a.cargo.iron_ore_g,
                    ingots_g: a.cargo.iron_ingot_g,
                },
            }
        }
        CommandRequest::SendFleet(s) => Action::SendFleet {
            fleet: voidforge_host::FleetId(s.fleet_id.0),
            target: voidforge_host::PlanetId(s.target_planet_id.0),
            mission: mission(s.mission),
        },
        CommandRequest::RecallFleet(f) => Action::RecallFleet {
            fleet: voidforge_host::FleetId(f.fleet_id.0),
        },
        CommandRequest::DisbandFleet(f) => Action::DisbandFleet {
            fleet: voidforge_host::FleetId(f.fleet_id.0),
        },
        CommandRequest::TriggerUnload(f) => Action::TriggerUnload {
            fleet: voidforge_host::FleetId(f.fleet_id.0),
        },
    })
}

fn building_kind(t: w::BuildingType) -> BuildingKind {
    match t {
        w::BuildingType::Drill => BuildingKind::Drill,
        w::BuildingType::Refinery => BuildingKind::Refinery,
        w::BuildingType::Shipyard => BuildingKind::Shipyard,
    }
}

fn building_type(k: BuildingKind) -> w::BuildingType {
    match k {
        BuildingKind::Drill => w::BuildingType::Drill,
        BuildingKind::Refinery => w::BuildingType::Refinery,
        BuildingKind::Shipyard => w::BuildingType::Shipyard,
    }
}

fn ship_kind(t: w::ShipType) -> ShipKind {
    match t {
        w::ShipType::ColonyShip => ShipKind::ColonyShip,
        w::ShipType::CargoVessel => ShipKind::CargoVessel,
    }
}

fn ship_type(k: ShipKind) -> w::ShipType {
    match k {
        ShipKind::ColonyShip => w::ShipType::ColonyShip,
        ShipKind::CargoVessel => w::ShipType::CargoVessel,
    }
}

fn mission(m: w::Mission) -> Mission {
    match m {
        w::Mission::Colonize => Mission::Colonize,
        w::Mission::Transport => Mission::Transport,
        w::Mission::Move => Mission::Move,
    }
}

fn wire_mission(m: Mission) -> w::Mission {
    match m {
        Mission::Colonize => w::Mission::Colonize,
        Mission::Transport => w::Mission::Transport,
        Mission::Move => w::Mission::Move,
    }
}

fn cargo(c: Cargo) -> w::Cargo {
    w::Cargo {
        iron_ore_g: c.ore_g,
        iron_ingot_g: c.ingots_g,
    }
}

fn status(l: Option<FlowLabel>) -> Option<api::FlowStatus> {
    l.map(|l| match l {
        FlowLabel::Running => api::FlowStatus::Running,
        FlowLabel::Throttled => api::FlowStatus::Throttled,
        FlowLabel::Halted => api::FlowStatus::Halted,
    })
}

/// A core player ID as the wire's UUID text.
pub fn player_id(p: PlayerId) -> w::PlayerId {
    w::PlayerId::try_from(uuid::Uuid::from_u128(p.0).hyphenated().to_string())
        .expect("a UUID's hyphenated form is the wire's player ID")
}

fn position(p: voidforge_host::Position) -> w::Position {
    w::Position {
        x: p.x,
        y: p.y,
        z: p.z,
    }
}

/// Why a view can't be put on the wire: a 500, never a made-up value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MapError {
    /// A time the wire can't carry.
    #[error(transparent)]
    Time(#[from] OutOfRange),
    /// A building under construction without its progress: the core's views always give one.
    #[error("building {0} is under construction but has no progress")]
    MissingProgress(u64),
}

/// Times on the wire for one universe.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    /// The universe epoch, Unix milliseconds.
    pub epoch_unix_ms: i64,
}

impl Clock {
    fn at(self, t: i64) -> Result<w::Timestamp, OutOfRange> {
        timestamp(self.epoch_unix_ms, t)
    }

    fn maybe(self, t: Option<i64>) -> Result<Option<w::Timestamp>, OutOfRange> {
        t.map(|t| self.at(t)).transpose()
    }

    /// A command's own events. Events carry no times, so this can't fail.
    pub fn events(self, events: &[DomainEvent]) -> Vec<Event> {
        events.iter().map(|e| event(*e)).collect()
    }

    /// A planet view.
    ///
    /// # Errors
    /// [`MapError`] for a time the wire can't carry, or an inconsistent view.
    pub fn planet(self, v: core::PlanetView) -> Result<api::PlanetView, MapError> {
        let stock = |s: core::StockView| api::Stock {
            amount_g: s.amount_g,
            rate_g_per_h: s.rate_g_per_h,
            cap_g: s.cap_g,
        };
        let buildings = v
            .buildings
            .into_iter()
            .map(|b| self.building(b))
            .collect::<Result<_, _>>()?;
        let ship_builds = v
            .ship_builds
            .into_iter()
            .map(|s| {
                Ok(api::ShipBuildView {
                    id: w::ShipBuildId(s.id.0),
                    shipyard_id: w::BuildingId(s.shipyard.0),
                    ship_type: ship_type(s.kind),
                    state: match s.status {
                        ShipBuildStatus::Active => api::ShipBuildState::Active,
                        ShipBuildStatus::Waiting => api::ShipBuildState::Waiting,
                    },
                    status: status(s.label),
                    delivered_g: s.progress.delivered_g,
                    cost_g: s.progress.cost_g,
                    rate_g_per_h: s.progress.rate_g_per_h,
                    eta: self.maybe(s.progress.eta_ms)?,
                })
            })
            .collect::<Result<_, MapError>>()?;
        Ok(api::PlanetView {
            planet_id: w::PlanetId(v.id.0),
            owner: v.owner.map(player_id),
            as_of: self.at(v.as_of)?,
            valid_until: self.maybe(v.valid_until)?,
            stocks: api::Stocks {
                iron_ore: stock(v.ore),
                iron_ingot: stock(v.ingots),
            },
            buildings,
            position: position(v.position),
            slots: v.slots,
            ship_builds,
            roster: v
                .roster
                .into_iter()
                .map(|(ship, kind, owner)| api::RosterShip {
                    ship_id: w::ShipId(ship.0),
                    ship_type: ship_type(kind),
                    owner: player_id(owner),
                })
                .collect(),
        })
    }

    fn building(self, b: core::BuildingView) -> Result<api::BuildingView, MapError> {
        let id = w::BuildingId(b.id.0);
        let building_type = building_type(b.kind);
        let status = status(b.label);
        Ok(match b.status {
            BuildingStatus::UnderConstruction => {
                let p = b.progress.ok_or(MapError::MissingProgress(b.id.0))?;
                api::BuildingView::UnderConstruction {
                    id,
                    building_type,
                    status,
                    delivered_g: p.delivered_g,
                    cost_g: p.cost_g,
                    rate_g_per_h: p.rate_g_per_h,
                    eta: self.maybe(p.eta_ms)?,
                }
            }
            BuildingStatus::Operational => api::BuildingView::Operational {
                id,
                building_type,
                status,
            },
            BuildingStatus::Demolishing { ends_at_ms } => api::BuildingView::Demolishing {
                id,
                building_type,
                status,
                demolition_ends_at: self.at(ends_at_ms)?,
            },
        })
    }

    /// A planets-near view.
    ///
    /// # Errors
    /// [`MapError`] for a time the wire can't carry, or an inconsistent view.
    pub fn near(self, v: core::NearView) -> Result<api::PlanetsNearView, MapError> {
        Ok(api::PlanetsNearView {
            as_of: self.at(v.as_of)?,
            valid_until: self.maybe(v.valid_until)?,
            planets: v
                .planets
                .into_iter()
                .map(|p| api::PlanetSummary {
                    planet_id: w::PlanetId(p.id.0),
                    position: position(p.position),
                    owner: p.owner.map(player_id),
                    distance: p.distance,
                })
                .collect(),
        })
    }

    /// A player view.
    ///
    /// # Errors
    /// [`MapError`] for a time the wire can't carry, or an inconsistent view.
    pub fn player(self, v: core::PlayerView) -> Result<api::PlayerView, MapError> {
        Ok(api::PlayerView {
            player_id: player_id(v.id),
            homeworld_id: w::PlanetId(v.homeworld.0),
            planet_ids: v.planets.into_iter().map(|p| w::PlanetId(p.0)).collect(),
            score: v.score,
            as_of: self.at(v.as_of)?,
            valid_until: self.maybe(v.valid_until)?,
        })
    }

    /// The viewer's fleets.
    ///
    /// # Errors
    /// [`MapError`] for a time the wire can't carry, or an inconsistent view.
    pub fn fleets(self, v: core::FleetsView) -> Result<api::FleetsView, MapError> {
        let fleets = v
            .fleets
            .into_iter()
            .map(|f| {
                let fleet_id = w::FleetId(f.id.0);
                let ships = f
                    .ships
                    .iter()
                    .map(|&(ship, kind)| api::FleetShip {
                        ship_id: w::ShipId(ship.0),
                        ship_type: ship_type(kind),
                    })
                    .collect();
                let cargo = cargo(f.cargo);
                let (capacity_g, speed_du_per_h) = (f.capacity_g, f.speed_du_per_h);
                Ok(match f.state {
                    FleetState::Stationed { at } => api::FleetView::Stationed {
                        fleet_id,
                        ships,
                        cargo,
                        capacity_g,
                        speed_du_per_h,
                        at_planet_id: w::PlanetId(at.0),
                    },
                    FleetState::InTransit {
                        origin,
                        target,
                        mission,
                        departed_at_ms,
                        arrives_at_ms,
                    } => api::FleetView::InTransit {
                        fleet_id,
                        ships,
                        cargo,
                        capacity_g,
                        speed_du_per_h,
                        origin_planet_id: w::PlanetId(origin.0),
                        target_planet_id: w::PlanetId(target.0),
                        mission: wire_mission(mission),
                        departed_at: self.at(departed_at_ms)?,
                        arrives_at: self.at(arrives_at_ms)?,
                    },
                    FleetState::Returning {
                        origin,
                        arrives_at_ms,
                    } => api::FleetView::Returning {
                        fleet_id,
                        ships,
                        cargo,
                        capacity_g,
                        speed_du_per_h,
                        origin_planet_id: w::PlanetId(origin.0),
                        arrives_at: self.at(arrives_at_ms)?,
                    },
                })
            })
            .collect::<Result<_, MapError>>()?;
        Ok(api::FleetsView {
            as_of: self.at(v.as_of)?,
            valid_until: self.maybe(v.valid_until)?,
            fleets,
        })
    }
}

fn event(e: DomainEvent) -> Event {
    match e {
        DomainEvent::BuildingCompleted {
            planet,
            building,
            kind,
        } => Event::BuildingCompleted {
            planet_id: w::PlanetId(planet.0),
            building_id: w::BuildingId(building.0),
            building_type: building_type(kind),
        },
        DomainEvent::ShipCompleted {
            planet,
            shipyard,
            ship,
            kind,
        } => Event::ShipCompleted {
            planet_id: w::PlanetId(planet.0),
            shipyard_id: w::BuildingId(shipyard.0),
            ship_id: w::ShipId(ship.0),
            ship_type: ship_type(kind),
        },
        DomainEvent::DemolitionCompleted {
            planet,
            building,
            kind,
        } => Event::DemolitionCompleted {
            planet_id: w::PlanetId(planet.0),
            building_id: w::BuildingId(building.0),
            building_type: building_type(kind),
        },
        DomainEvent::FleetArrived {
            fleet,
            planet,
            mission,
        } => Event::FleetArrived {
            fleet_id: w::FleetId(fleet.0),
            planet_id: w::PlanetId(planet.0),
            mission: wire_mission(mission),
        },
        DomainEvent::PlanetColonized {
            planet,
            owner,
            fleet,
            ship,
        } => Event::PlanetColonized {
            planet_id: w::PlanetId(planet.0),
            owner: player_id(owner),
            fleet_id: w::FleetId(fleet.0),
            ship_id: w::ShipId(ship.0),
        },
        DomainEvent::ColonizationFailed { planet, fleet } => Event::ColonizationFailed {
            planet_id: w::PlanetId(planet.0),
            fleet_id: w::FleetId(fleet.0),
        },
        DomainEvent::CargoUnloaded {
            planet,
            fleet,
            cargo: c,
        } => Event::CargoUnloaded {
            planet_id: w::PlanetId(planet.0),
            fleet_id: w::FleetId(fleet.0),
            cargo: cargo(c),
        },
        DomainEvent::FleetRemoved { fleet, planet } => Event::FleetRemoved {
            fleet_id: w::FleetId(fleet.0),
            planet_id: w::PlanetId(planet.0),
        },
        DomainEvent::FleetReturned { fleet, planet } => Event::FleetReturned {
            fleet_id: w::FleetId(fleet.0),
            planet_id: w::PlanetId(planet.0),
        },
    }
}
