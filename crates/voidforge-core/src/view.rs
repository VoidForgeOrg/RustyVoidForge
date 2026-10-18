//! Views and score: [`World::view`] projects state to a time without changing it, and says until
//! when that projection stays exact (Architectural Design, Engine core and Read side and API;
//! Game Design, Scoring; decision log D-030).
//!
//! A view takes `&World`, so it cannot write ("Pure reads"). It is defined only for t at or after
//! world time and before its `valid_until`, the earliest event that would change what it shows:
//! for a planet, its next event or the arrival of a fleet heading to it, whichever is first.
//! Past that it is [`ViewError::Stale`], never a silently wrong projection. Score is computed
//! from the projection at t and never stored.

use crate::fleet::distance;
use crate::quantity::Quantity;
use crate::rules::{MassPoints, Rules};
use crate::solver::{FlowLabel, planet_flows};
use crate::state::{
    BuildingId, BuildingKind, BuildingState, Cargo, EngineError, FleetId, FleetState, Mission,
    PlanetId, PlayerId, Position, ShipBuildId, ShipBuildStatus, ShipId, ShipKind, World,
};

/// What to view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
    /// One planet, in full. Public.
    Planet {
        /// The planet.
        planet: PlanetId,
    },
    /// Planets within `radius` distance units of `center`, summarized. Public.
    PlanetsNear {
        /// The point.
        center: Position,
        /// The radius, in distance units (D-021).
        radius: i64,
    },
    /// A player's profile and score. Public.
    Player {
        /// The player.
        player: PlayerId,
    },
    /// The viewer's own fleets: fleets are visible only to their owner.
    Fleets {
        /// The player asking, from the auth token.
        viewer: PlayerId,
    },
}

/// A view's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// For [`Query::Planet`].
    Planet(PlanetView),
    /// For [`Query::PlanetsNear`].
    PlanetsNear(NearView),
    /// For [`Query::Player`].
    Player(PlayerView),
    /// For [`Query::Fleets`].
    Fleets(FleetsView),
}

/// Why a view cannot be given.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ViewError {
    /// `t` is before world time.
    #[error("cannot view at {t} ms: world time is already {now} ms")]
    BeforeNow {
        /// The requested time.
        t: i64,
        /// World time.
        now: i64,
    },
    /// `t` is at or after the view's `valid_until`: advance first.
    #[error("the view changes at {valid_until} ms; advance before viewing at or after it")]
    Stale {
        /// When the view stops being exact.
        valid_until: i64,
    },
    /// The rules given are not the world's.
    #[error("the world runs rules version {world}, but rules version {given} were given")]
    RulesMismatch {
        /// The world's rules version.
        world: u32,
        /// The given rules' version.
        given: u32,
    },
    /// No such planet or player.
    #[error("no such planet or player")]
    Unknown,
    /// A score leaves 64 bits: valid but extreme rules.
    #[error("the player's score leaves the 64-bit range")]
    ScoreOutOfRange,
    /// A value could not be computed: an engine error, as from a solve.
    #[error(transparent)]
    Engine(#[from] EngineError),
}

/// A stock at the view's time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockView {
    /// Grams in storage at `as_of`.
    pub amount_g: i64,
    /// Its rate until `valid_until`.
    pub rate_g_per_h: i64,
    /// The storage cap.
    pub cap_g: i64,
}

/// A building's state, without its quantities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildingStatus {
    /// Being built.
    UnderConstruction,
    /// Working.
    Operational,
    /// Being taken down; the slot frees at `ends_at_ms`.
    Demolishing {
        /// When demolition ends.
        ends_at_ms: i64,
    },
}

/// Progress of a build (a construction or a ship build) at the view's time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Ingots delivered at `as_of`.
    pub delivered_g: i64,
    /// The total cost.
    pub cost_g: i64,
    /// The delivery rate until `valid_until`.
    pub rate_g_per_h: i64,
    /// When it completes at the current rate, exactly; `None` while nothing flows.
    pub eta_ms: Option<i64>,
}

/// A building on a planet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildingView {
    /// Its ID.
    pub id: BuildingId,
    /// What it is.
    pub kind: BuildingKind,
    /// Construction, operation or demolition.
    pub status: BuildingStatus,
    /// Running, throttled or halted, for buildings that work or are being built (D-025).
    pub label: Option<FlowLabel>,
    /// Construction progress, while under construction.
    pub progress: Option<Progress>,
}

/// A ship build at one of the planet's Shipyards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipBuildView {
    /// Its ID.
    pub id: ShipBuildId,
    /// The Shipyard.
    pub shipyard: BuildingId,
    /// The ship it will be.
    pub kind: ShipKind,
    /// Active or waiting.
    pub status: ShipBuildStatus,
    /// Running, throttled or halted, while active (D-025).
    pub label: Option<FlowLabel>,
    /// Its progress.
    pub progress: Progress,
}

/// One planet at the view's time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanetView {
    /// Its ID.
    pub id: PlanetId,
    /// Where it is.
    pub position: Position,
    /// Its owner.
    pub owner: Option<PlayerId>,
    /// Building slots.
    pub slots: u32,
    /// The view's time.
    pub as_of: i64,
    /// The view is exact before this: the planet's next event or the first fleet arriving.
    pub valid_until: Option<i64>,
    /// Iron Ore.
    pub ore: StockView,
    /// Iron Ingots.
    pub ingots: StockView,
    /// Its buildings, by ID.
    pub buildings: Vec<BuildingView>,
    /// Its Shipyards' builds, by ID.
    pub ship_builds: Vec<ShipBuildView>,
    /// Ships on its roster, any owner's, by ID.
    pub roster: Vec<(ShipId, ShipKind, PlayerId)>,
}

/// A planet in a [`Query::PlanetsNear`] result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanetSummary {
    /// Its ID.
    pub id: PlanetId,
    /// Where it is.
    pub position: Position,
    /// Its owner.
    pub owner: Option<PlayerId>,
    /// Its distance from the center, in distance units.
    pub distance: i64,
}

/// Planets near a point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearView {
    /// The view's time.
    pub as_of: i64,
    /// Exact before this: the first colonization arrival at a listed planet.
    pub valid_until: Option<i64>,
    /// The planets within the radius, by ID.
    pub planets: Vec<PlanetSummary>,
}

/// A player's profile and score.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerView {
    /// The player.
    pub id: PlayerId,
    /// Their homeworld.
    pub homeworld: PlanetId,
    /// The planets they own, by ID.
    pub planets: Vec<PlanetId>,
    /// Their score at `as_of`.
    pub score: i64,
    /// The view's time.
    pub as_of: i64,
    /// Exact before this: the earliest change on their planets or to their fleets.
    pub valid_until: Option<i64>,
}

/// One fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetView {
    /// Its ID.
    pub id: FleetId,
    /// Its ships, by ID.
    pub ships: Vec<(ShipId, ShipKind)>,
    /// Cargo aboard.
    pub cargo: Cargo,
    /// Cargo capacity, in grams.
    pub capacity_g: i64,
    /// Speed, in distance units per hour.
    pub speed_du_per_h: i64,
    /// Stationed, in transit or returning.
    pub state: FleetState,
}

/// The viewer's fleets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetsView {
    /// The view's time.
    pub as_of: i64,
    /// Exact before this: the first arrival or return among them.
    pub valid_until: Option<i64>,
    /// The fleets, by ID.
    pub fleets: Vec<FleetView>,
}

impl World {
    /// Projects `query` to `t` without changing the world. `t` must be at or after world time
    /// and before the view's `valid_until`; otherwise [`ViewError::BeforeNow`] or
    /// [`ViewError::Stale`]. `rules` must be the world's.
    pub fn view(&self, query: &Query, t: i64, rules: &Rules) -> Result<View, ViewError> {
        if rules.version != self.rules_version {
            return Err(ViewError::RulesMismatch {
                world: self.rules_version,
                given: rules.version,
            });
        }
        if t < self.time_ms {
            return Err(ViewError::BeforeNow {
                t,
                now: self.time_ms,
            });
        }
        let valid = |valid_until: Option<i64>| match valid_until {
            Some(v) if t >= v => Err(ViewError::Stale { valid_until: v }),
            _ => Ok(valid_until),
        };
        match *query {
            Query::Planet { planet } => {
                if !self.planets.contains_key(&planet) {
                    return Err(ViewError::Unknown);
                }
                let valid_until = valid(self.planet_valid_until(planet))?;
                Ok(View::Planet(self.planet_view(
                    planet,
                    t,
                    valid_until,
                    rules,
                )?))
            }
            Query::PlanetsNear { center, radius } => {
                let planets: Vec<PlanetSummary> = self
                    .planets
                    .values()
                    .filter_map(|p| {
                        let d = distance(center, p.position).filter(|&d| d <= radius)?;
                        Some(PlanetSummary {
                            id: p.id,
                            position: p.position,
                            owner: p.owner,
                            distance: d,
                        })
                    })
                    .collect();
                let colonizing = self.fleets.values().filter_map(|f| match f.state {
                    FleetState::InTransit {
                        target,
                        mission: Mission::Colonize,
                        arrives_at_ms,
                        ..
                    } if planets.iter().any(|p| p.id == target) => Some(arrives_at_ms),
                    _ => None,
                });
                let valid_until = valid(colonizing.min())?;
                Ok(View::PlanetsNear(NearView {
                    as_of: t,
                    valid_until,
                    planets,
                }))
            }
            Query::Player { player } => {
                let p = self.players.get(&player).ok_or(ViewError::Unknown)?;
                let planets: Vec<PlanetId> = self
                    .planets
                    .values()
                    .filter(|x| x.owner == Some(player))
                    .map(|x| x.id)
                    .collect();
                let fleets = self.fleets.values().filter(|f| f.owner == player);
                let valid_until = valid(earliest(
                    planets.iter().map(|&x| self.planet_valid_until(x)),
                    fleets.map(|f| moving_until(f.state)),
                ))?;
                Ok(View::Player(PlayerView {
                    id: player,
                    homeworld: p.homeworld,
                    score: self.score(player, t, rules)?,
                    planets,
                    as_of: t,
                    valid_until,
                }))
            }
            Query::Fleets { viewer } => {
                let mine: Vec<_> = self.fleets.values().filter(|f| f.owner == viewer).collect();
                let valid_until = valid(earliest(
                    std::iter::empty(),
                    mine.iter().map(|f| moving_until(f.state)),
                ))?;
                let fleets = mine
                    .iter()
                    .map(|f| {
                        Ok(FleetView {
                            id: f.id,
                            ships: f.ships.iter().map(|s| (*s, self.ships[s].kind)).collect(),
                            cargo: f.cargo,
                            capacity_g: self.fleet_capacity(f.id, rules)?,
                            speed_du_per_h: self.fleet_speed(f.id, rules)?,
                            state: f.state,
                        })
                    })
                    .collect::<Result<_, EngineError>>()?;
                Ok(View::Fleets(FleetsView {
                    as_of: t,
                    valid_until,
                    fleets,
                }))
            }
        }
    }

    /// When a planet's view stops being exact: its next event, or the first arrival of a fleet
    /// heading to it (an arrival re-solves its target, D-027), whichever is earlier.
    pub fn planet_valid_until(&self, planet: PlanetId) -> Option<i64> {
        let incoming = self.fleets.values().filter_map(|f| match f.state {
            FleetState::InTransit {
                target,
                arrives_at_ms,
                ..
            } if target == planet => Some(arrives_at_ms),
            _ => None,
        });
        let next = self.planets[&planet].next_event.map(|e| e.at_ms);
        earliest(std::iter::once(next), incoming.map(Some))
    }

    fn planet_view(
        &self,
        planet: PlanetId,
        t: i64,
        valid_until: Option<i64>,
        rules: &Rules,
    ) -> Result<PlanetView, ViewError> {
        let p = &self.planets[&planet];
        let flows = planet_flows(self, planet, rules)?;
        let quantity = |q: Quantity, entity: u64| {
            q.value_at(t).map_err(|error| EngineError::Quantity {
                planet,
                entity,
                error,
            })
        };
        let stock = |q: Quantity, cap_g| {
            Ok::<_, EngineError>(StockView {
                amount_g: quantity(q, planet.0)?,
                rate_g_per_h: q.rate_g_per_h(),
                cap_g,
            })
        };
        let progress = |q: Quantity, cost_g: i64, entity: u64| {
            let eta_ms = q
                .rising_crossing(cost_g)
                .map_err(|error| EngineError::Quantity {
                    planet,
                    entity,
                    error,
                })?;
            Ok::<_, EngineError>(Progress {
                delivered_g: quantity(q, entity)?,
                cost_g,
                rate_g_per_h: q.rate_g_per_h(),
                eta_ms: eta_ms.filter(|_| q.rate_g_per_h() > 0),
            })
        };
        let mut buildings = Vec::new();
        for id in &p.buildings {
            let b = &self.buildings[id];
            let (status, progress) = match b.state {
                BuildingState::UnderConstruction {
                    delivered, cost_g, ..
                } => (
                    BuildingStatus::UnderConstruction,
                    Some(progress(delivered, cost_g, id.0)?),
                ),
                BuildingState::Operational => (BuildingStatus::Operational, None),
                BuildingState::Demolishing { ends_at_ms } => {
                    (BuildingStatus::Demolishing { ends_at_ms }, None)
                }
            };
            buildings.push(BuildingView {
                id: *id,
                kind: b.kind,
                status,
                label: flows.buildings.get(id).map(|f| f.label),
                progress,
            });
        }
        let mut ship_builds = Vec::new();
        for s in self.ship_builds.values() {
            if p.buildings.contains(&s.shipyard) {
                ship_builds.push(ShipBuildView {
                    id: s.id,
                    shipyard: s.shipyard,
                    kind: s.kind,
                    status: s.status,
                    label: flows.ship_builds.get(&s.id).map(|f| f.label),
                    progress: progress(s.delivered, s.cost_g, s.id.0)?,
                });
            }
        }
        Ok(PlanetView {
            id: planet,
            position: p.position,
            owner: p.owner,
            slots: p.slots,
            as_of: t,
            valid_until,
            ore: stock(p.ore, p.ore_cap_g)?,
            ingots: stock(p.ingots, p.ingot_cap_g)?,
            buildings,
            ship_builds,
            roster: p
                .roster
                .iter()
                .map(|s| (*s, self.ships[s].kind, self.ships[s].owner))
                .collect(),
        })
    }

    /// A player's score at `t` from `ScoringSpecs` (Game Design, Scoring; D-030): planets owned;
    /// finished buildings by kind; buildings under construction and ship builds at points ×
    /// delivered ÷ cost, rounded down each; each ship the player owns once, wherever it is;
    /// and ore and ingots in their storage and aboard their fleets, each resource totalled and
    /// then rounded down once. A building being demolished counts in full until it is gone:
    /// PROVISIONAL, parked for Tom as game intent (D-030).
    /// Never stored. `t` must be one at which the player's view is valid.
    pub fn score(&self, player: PlayerId, t: i64, rules: &Rules) -> Result<i64, ViewError> {
        let specs = &rules.scoring;
        let points = |kind: BuildingKind| match kind {
            BuildingKind::Drill => specs.drill,
            BuildingKind::Refinery => specs.refinery,
            BuildingKind::Shipyard => specs.shipyard,
        };
        let ship_points = |kind: ShipKind| match kind {
            ShipKind::ColonyShip => specs.colony_ship,
            ShipKind::CargoVessel => specs.cargo_vessel,
        };
        let read = |q: Quantity, planet: PlanetId, entity: u64| {
            q.value_at(t)
                .map(i128::from)
                .map_err(|error| EngineError::Quantity {
                    planet,
                    entity,
                    error,
                })
        };
        // Every step is checked: rules that validate can still give a score beyond i128, which
        // is ScoreOutOfRange in every build profile, never a panic or a wrapped value (D-030).
        let over = || ViewError::ScoreOutOfRange;
        let add = |sum: i128, x: i128| sum.checked_add(x).ok_or_else(over);
        // Partial credit: points × delivered ÷ cost, rounded down (all values are not negative;
        // validated costs are positive).
        let partial = |points: i64, delivered: i128, cost: i64| {
            i128::from(points)
                .checked_mul(delivered.max(0))
                .and_then(|x| x.checked_div(i128::from(cost.max(1))))
                .ok_or_else(over)
        };
        let mut total: i128 = 0;
        let (mut ore, mut ingots): (i128, i128) = (0, 0);
        for p in self.planets.values().filter(|p| p.owner == Some(player)) {
            total = add(total, i128::from(specs.planet))?;
            ore = add(ore, read(p.ore, p.id, p.id.0)?)?;
            ingots = add(ingots, read(p.ingots, p.id, p.id.0)?)?;
            for id in &p.buildings {
                let b = &self.buildings[id];
                let points = match b.state {
                    BuildingState::UnderConstruction {
                        delivered, cost_g, ..
                    } => partial(points(b.kind), read(delivered, p.id, id.0)?, cost_g)?,
                    BuildingState::Operational | BuildingState::Demolishing { .. } => {
                        i128::from(points(b.kind))
                    }
                };
                total = add(total, points)?;
            }
            for s in self.ship_builds.values() {
                if p.buildings.contains(&s.shipyard) {
                    let delivered = read(s.delivered, p.id, s.id.0)?;
                    total = add(total, partial(ship_points(s.kind), delivered, s.cost_g)?)?;
                }
            }
        }
        for ship in self.ships.values().filter(|s| s.owner == player) {
            total = add(total, i128::from(ship_points(ship.kind)))?;
        }
        for f in self.fleets.values().filter(|f| f.owner == player) {
            ore = add(ore, i128::from(f.cargo.ore_g))?;
            ingots = add(ingots, i128::from(f.cargo.ingots_g))?;
        }
        // Once per resource: grams × points ÷ grams per point, rounded down (per_g is positive).
        let mass = |grams: i128, m: MassPoints| {
            grams
                .checked_mul(i128::from(m.points))
                .and_then(|x| x.checked_div(i128::from(m.per_g)))
                .ok_or_else(over)
        };
        total = add(total, mass(ore, specs.ore)?)?;
        total = add(total, mass(ingots, specs.ingots)?)?;
        i64::try_from(total).map_err(|_| ViewError::ScoreOutOfRange)
    }
}

/// When a fleet's state changes by itself: its arrival or return.
fn moving_until(state: FleetState) -> Option<i64> {
    match state {
        FleetState::Stationed { .. } => None,
        FleetState::InTransit { arrives_at_ms, .. }
        | FleetState::Returning { arrives_at_ms, .. } => Some(arrives_at_ms),
    }
}

/// The earliest of all the given times; `None` if there are none.
fn earliest(
    a: impl Iterator<Item = Option<i64>>,
    b: impl Iterator<Item = Option<i64>>,
) -> Option<i64> {
    a.chain(b).flatten().min()
}
