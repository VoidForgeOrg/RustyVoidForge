//! The per-planet solver, part 1: flows (Architectural Design, Per-planet solver, Flows, Halted
//! and throttled; decision log D-025).
//!
//! [`planet_flows`] computes a planet's flows from its state at world time, in the
//! architecture's order: Drill output e, Refinery demand q, Refinery intake A, ingot output O,
//! ingot consumers, Drill actual, then net rates. Every division rounds down. Operational
//! Drills and Refineries take their rates from the rules; builds use the start terms they
//! store (D-022). Labels are derived here and never stored.
//!
//! [`World::solve_planet`] is the solve itself (steps 1 to 5): bring every quantity to now,
//! snapping crossings due now; refuse transitions, which are P1.7's; solve the flows; set net
//! rates; schedule the earliest crossing or timer. [`Solver`] is the production steps: it runs the
//! solve for each `PlanetChange` (D-025) and the fleet arrivals and returns of [`crate::fleet`]
//! (D-027).

use std::collections::{BTreeMap, BTreeSet};

use crate::quantity::Quantity;
use crate::rules::Rules;
use crate::state::{
    Building, BuildingId, BuildingKind, BuildingState, DomainEvent, EngineError, FleetId, Planet,
    PlanetId, Ship, ShipBuild, ShipBuildId, ShipBuildStatus, ShipId, ShipLocation, StateError,
    Steps, World,
};

/// A building's or build's state of work, for the read side. Labels change nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowLabel {
    /// Working at its rated rate.
    Running,
    /// Working, but below its rated rate.
    Throttled,
    /// Not working: zero flow.
    Halted,
}

/// One building's or build's flow against its rated rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flow {
    /// Its flow, in grams per hour.
    pub rate_g_per_h: i64,
    /// Its rated rate, in grams per hour.
    pub rated_g_per_h: i64,
    /// Halted at zero, throttled below the rated rate, otherwise running.
    pub label: FlowLabel,
}

impl Flow {
    fn new(rate_g_per_h: i64, rated_g_per_h: i64) -> Self {
        let label = if rate_g_per_h <= 0 {
            FlowLabel::Halted
        } else if rate_g_per_h < rated_g_per_h {
            FlowLabel::Throttled
        } else {
            FlowLabel::Running
        };
        Self {
            rate_g_per_h,
            rated_g_per_h,
            label,
        }
    }
}

/// A planet's flows at one instant, all in grams per hour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanetFlows {
    /// e: rated extraction summed over operational Drills.
    pub drill_output: i64,
    /// q: rated intake summed over operational Refineries.
    pub refinery_demand: i64,
    /// A = min(q, ore limit, ingot limit).
    pub refinery_intake: i64,
    /// O = ingots per ore × A.
    pub ingot_output: i64,
    /// Rated demand summed over constructions and active ship builds.
    pub ingot_demand: i64,
    /// Ingots delivered to those consumers in total.
    pub ingots_delivered: i64,
    /// min(e, A) when the ore stock is full, otherwise e.
    pub drill_actual: i64,
    /// Ore stock net rate: Drill actual − A.
    pub ore_net: i64,
    /// Ingot stock net rate: O − ingots delivered.
    pub ingot_net: i64,
    /// Operational Drills, Refineries and Shipyards, and constructions.
    pub buildings: BTreeMap<BuildingId, Flow>,
    /// Active ship builds: what each is delivered.
    pub ship_builds: BTreeMap<ShipBuildId, Flow>,
}

/// Splits `supply` among consumers given as (entity ID, demand): equal shares capped at
/// demand, any unused share split evenly among the rest, and leftover grams per hour one at a
/// time to the lowest IDs (max-min fairness). The result is in input order and never exceeds a
/// demand; the supply is used up unless every demand is met.
pub fn even_split(supply: i64, demands: &[(u64, i64)]) -> Vec<i64> {
    let mut given = vec![0; demands.len()];
    let mut open: Vec<usize> = (0..demands.len()).filter(|&i| demands[i].1 > 0).collect();
    open.sort_by_key(|&i| demands[i].0);
    let mut left = supply.max(0);
    while !open.is_empty() && left > 0 {
        let n = open.len() as i64;
        let share = left / n;
        let (met, short): (Vec<usize>, Vec<usize>) =
            open.iter().partition(|&&i| demands[i].1 <= share);
        if met.is_empty() {
            // Everyone wants more than the share; each demand is at least share + 1.
            for &i in &open {
                given[i] = share;
            }
            left -= share * n;
            for &i in open.iter().take(left as usize) {
                given[i] += 1;
            }
            break;
        }
        for &i in &met {
            given[i] = demands[i].1;
            left -= demands[i].1;
        }
        open = short;
    }
    given
}

/// The flows of `planet` at world time, from its stored state and `rules`. Fails if the planet
/// does not exist, a stock cannot be read at world time, or a total rate leaves 64 bits
/// ([`EngineError::RateOutOfRange`], never a panic or a wrapped value).
pub fn planet_flows(
    world: &World,
    planet: PlanetId,
    rules: &Rules,
) -> Result<PlanetFlows, EngineError> {
    let p = world
        .planets
        .get(&planet)
        .ok_or_else(|| StateError::Link(format!("flows: planet {} does not exist", planet.0)))?;
    let now = world.time_ms;
    let read = |q: Quantity, what: &str| {
        q.value_at(now)
            .map_err(|e| StateError::Link(format!("flows: planet {} {what}: {e}", planet.0)))
    };
    let ore = read(p.ore, "ore")?;
    let ingots = read(p.ingots, "ingots")?;
    flows_with(world, planet, rules, ore, ingots)
}

/// A checked total of rates, so valid but extreme rules give an error rather than a panic in
/// debug builds and a wrapped value in release builds.
fn total(
    planet: PlanetId,
    what: &'static str,
    rates: impl IntoIterator<Item = i64>,
) -> Result<i64, EngineError> {
    rates
        .into_iter()
        .try_fold(0i64, |sum, rate| sum.checked_add(rate))
        .ok_or(EngineError::RateOutOfRange { planet, what })
}

/// The flows of an existing planet whose stocks hold `ore` and `ingots` grams.
fn flows_with(
    world: &World,
    planet: PlanetId,
    rules: &Rules,
    ore: i64,
    ingots: i64,
) -> Result<PlanetFlows, EngineError> {
    let p = &world.planets[&planet];
    let specs = &rules.buildings;

    let mut drills = Vec::new();
    let mut refineries = Vec::new();
    let mut shipyards = Vec::new();
    let mut consumers: Vec<(u64, i64)> = Vec::new();
    for id in &p.buildings {
        let b = &world.buildings[id];
        match (b.state, b.kind) {
            (BuildingState::Operational, BuildingKind::Drill) => {
                drills.push((id.0, specs.drill.output_g_per_h))
            }
            (BuildingState::Operational, BuildingKind::Refinery) => {
                refineries.push((id.0, specs.refinery.intake_g_per_h))
            }
            (BuildingState::Operational, BuildingKind::Shipyard) => shipyards.push(*id),
            (BuildingState::UnderConstruction { rated_g_per_h, .. }, _) => {
                consumers.push((id.0, rated_g_per_h))
            }
            (BuildingState::Demolishing { .. }, _) => {}
        }
    }
    let active: Vec<(ShipBuildId, BuildingId, i64)> = world
        .ship_builds
        .values()
        .filter(|s| s.status == ShipBuildStatus::Active && shipyards.contains(&s.shipyard))
        .map(|s| (s.id, s.shipyard, s.rated_g_per_h))
        .collect();
    consumers.extend(active.iter().map(|&(id, _, rated)| (id.0, rated)));
    consumers.sort_unstable();

    let ratio = specs.refinery.ingots_per_ore;
    let e = total(planet, "Drill output", drills.iter().map(|d| d.1))?;
    let q = total(planet, "Refinery demand", refineries.iter().map(|r| r.1))?;
    let demand = total(planet, "ingot demand", consumers.iter().map(|c| c.1))?;
    let ore_empty = ore <= 0;
    let ore_full = ore >= p.ore_cap_g;
    let ingots_empty = ingots <= 0;
    let ingots_full = ingots >= p.ingot_cap_g;

    let mut a = q;
    if ore_empty {
        a = a.min(e);
    }
    if ingots_full {
        a = a.min(demand / ratio);
    }
    let o = ratio.checked_mul(a).ok_or(EngineError::RateOutOfRange {
        planet,
        what: "ingot output",
    })?;
    let delivered = if ingots_empty {
        even_split(o, &consumers)
    } else {
        consumers.iter().map(|c| c.1).collect()
    };
    let drill_actual = if ore_full { e.min(a) } else { e };

    let mut buildings = BTreeMap::new();
    for (&(id, rated), share) in drills.iter().zip(even_split(drill_actual, &drills)) {
        buildings.insert(BuildingId(id), Flow::new(share, rated));
    }
    for (&(id, rated), share) in refineries.iter().zip(even_split(a, &refineries)) {
        buildings.insert(BuildingId(id), Flow::new(share, rated));
    }
    let given: BTreeMap<u64, i64> = consumers
        .iter()
        .zip(&delivered)
        .map(|(c, &d)| (c.0, d))
        .collect();
    let mut ship_builds = BTreeMap::new();
    for &(id, _, rated) in &active {
        ship_builds.insert(id, Flow::new(given[&id.0], rated));
    }
    for &yard in &shipyards {
        let builds = active.iter().filter(|s| s.1 == yard);
        let rate = total(
            planet,
            "Shipyard delivery",
            builds.clone().map(|s| given[&s.0.0]),
        )?;
        let rated = total(planet, "Shipyard rated rate", builds.map(|s| s.2))?;
        buildings.insert(yard, Flow::new(rate, rated));
    }
    for id in &p.buildings {
        if let BuildingState::UnderConstruction { rated_g_per_h, .. } = world.buildings[id].state {
            buildings.insert(*id, Flow::new(given[&id.0], rated_g_per_h));
        }
    }

    let ingots_delivered = total(planet, "ingots delivered", delivered.iter().copied())?;
    // Both differences are of two values in 0..=i64::MAX, so neither can overflow.
    Ok(PlanetFlows {
        drill_output: e,
        refinery_demand: q,
        refinery_intake: a,
        ingot_output: o,
        ingot_demand: demand,
        ingots_delivered,
        drill_actual,
        ore_net: drill_actual - a,
        ingot_net: o - ingots_delivered,
        buildings,
        ship_builds,
    })
}

/// The production steps: a planet change re-solves its planet; a fleet arrival runs its mission
/// and re-solves its target; a return stations the fleet at its origin (D-027).
#[derive(Debug, Clone, Copy)]
pub struct Solver<'a> {
    /// The rules in force; their version matches the world's.
    pub rules: &'a Rules,
}

impl Steps for Solver<'_> {
    type Event = DomainEvent;

    fn planet_change(
        &mut self,
        world: &mut World,
        planet: PlanetId,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        world.solve_planet(planet, self.rules)
    }

    fn fleet_arrival(
        &mut self,
        world: &mut World,
        fleet: FleetId,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        world.arrive(fleet, self.rules)
    }

    fn fleet_return(
        &mut self,
        world: &mut World,
        fleet: FleetId,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        world.return_home(fleet)
    }
}

/// A quantity brought to `now`: if it crosses `boundary` (0 or a cap for a stock, the cost for a
/// build) exactly now, it snaps there with remainder 0; a crossing before now was missed.
fn bring(
    q: Quantity,
    now: i64,
    floor: Option<i64>,
    ceiling: Option<i64>,
    planet: PlanetId,
    entity: u64,
) -> Result<Quantity, EngineError> {
    let quantity = |error| EngineError::Quantity {
        planet,
        entity,
        error,
    };
    let rate = q.rate_g_per_h();
    let crossing = match (rate.signum(), floor, ceiling) {
        (1, _, Some(top)) => q
            .rising_crossing(top)
            .map_err(quantity)?
            .map(|at| (at, top)),
        (-1, Some(bottom), _) => q
            .falling_crossing(bottom)
            .map_err(quantity)?
            .map(|at| (at, bottom)),
        _ => None,
    };
    match crossing {
        Some((at, _)) if at < now => Err(EngineError::MissedCrossing {
            planet,
            entity,
            at_ms: at,
            now,
        }),
        Some((at, boundary)) if at == now => q.snapped(boundary, now).map_err(quantity),
        _ => q.advanced_to(now).map_err(quantity),
    }
}

/// When a quantity moving at its rate first reaches `boundary`, if it moves toward it.
fn crossing(
    q: Quantity,
    floor: Option<i64>,
    ceiling: Option<i64>,
) -> Result<Option<i64>, crate::quantity::QuantityError> {
    match (q.rate_g_per_h().signum(), floor, ceiling) {
        (1, _, Some(top)) => q.rising_crossing(top),
        (-1, Some(bottom), _) => q.falling_crossing(bottom),
        _ => Ok(None),
    }
}

impl World {
    /// Re-solves `planet` at world time (Per-planet solver, steps 1 to 5; D-025, D-026): brings
    /// every quantity to now and snaps those crossing now; applies the transitions due now,
    /// round after round in ID order, until none remain; solves the flows; sets every net rate;
    /// and schedules the earliest crossing or timer, bumping the planet's version. Returns the
    /// domain events of the transitions, in the order applied.
    ///
    /// On error, the planet, its buildings and builds, the ships and the ID counter are
    /// restored, so a failed standalone solve changes nothing. Inside an advance the error
    /// poisons the world as usual (D-023).
    pub fn solve_planet(
        &mut self,
        planet: PlanetId,
        rules: &Rules,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        if self.is_poisoned() {
            return Err(EngineError::Poisoned);
        }
        let saved = Saved::take(self, planet)?;
        let bound = self.transition_bound(planet);
        let result = self.solve(planet, rules, bound);
        if result.is_err() {
            saved.restore(self);
        }
        result
    }

    /// The loop guard's bound (Next event): the planet's buildings plus its ship builds.
    pub(crate) fn transition_bound(&self, planet: PlanetId) -> usize {
        let p = &self.planets[&planet];
        let builds = self
            .ship_builds
            .values()
            .filter(|b| p.buildings.contains(&b.shipyard))
            .count();
        p.buildings.len() + builds
    }

    fn solve(
        &mut self,
        planet: PlanetId,
        rules: &Rules,
        bound: usize,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        self.bring_planet(planet)?;
        let events = self.settle_transitions(planet, rules, bound)?;
        self.set_rates_and_schedule(planet, rules)?;
        Ok(events)
    }

    /// Step 1: every quantity on the planet to now, snapping those crossing now. A demolition
    /// whose end lies before now was missed (D-026).
    pub(crate) fn bring_planet(&mut self, planet: PlanetId) -> Result<(), EngineError> {
        let now = self.time_ms;
        let p = &self.planets[&planet];
        let ore = bring(p.ore, now, Some(0), Some(p.ore_cap_g), planet, planet.0)?;
        let ingots = bring(
            p.ingots,
            now,
            Some(0),
            Some(p.ingot_cap_g),
            planet,
            planet.0,
        )?;
        let mut brought = Vec::new();
        for id in &p.buildings {
            match self.buildings[id].state {
                BuildingState::UnderConstruction {
                    delivered, cost_g, ..
                } => {
                    brought.push((
                        id.0,
                        bring(delivered, now, None, Some(cost_g), planet, id.0)?,
                    ));
                }
                BuildingState::Demolishing { ends_at_ms } if ends_at_ms < now => {
                    return Err(EngineError::MissedCrossing {
                        planet,
                        entity: id.0,
                        at_ms: ends_at_ms,
                        now,
                    });
                }
                _ => {}
            }
        }
        for b in self.ship_builds.values() {
            if p.buildings.contains(&b.shipyard) && b.status == ShipBuildStatus::Active {
                brought.push((
                    b.id.0,
                    bring(b.delivered, now, None, Some(b.cost_g), planet, b.id.0)?,
                ));
            }
        }
        let p = self
            .planets
            .get_mut(&planet)
            .expect("checked by the caller");
        p.ore = ore;
        p.ingots = ingots;
        for (entity, delivered) in brought {
            self.set_delivered(entity, delivered);
        }
        Ok(())
    }

    fn set_delivered(&mut self, entity: u64, quantity: Quantity) {
        if let Some(b) = self.buildings.get_mut(&BuildingId(entity))
            && let BuildingState::UnderConstruction { delivered, .. } = &mut b.state
        {
            *delivered = quantity;
        } else if let Some(b) = self.ship_builds.get_mut(&ShipBuildId(entity)) {
            b.delivered = quantity;
        }
    }

    /// Step 2 and its repeat: every transition due now, in ascending entity ID, round after
    /// round until a round finds none. More rounds than `bound` is [`EngineError::TransitionLoop`].
    pub(crate) fn settle_transitions(
        &mut self,
        planet: PlanetId,
        rules: &Rules,
        bound: usize,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        let now = self.time_ms;
        let mut events = Vec::new();
        let mut rounds = 0;
        loop {
            let due = self.due_transitions(planet, now);
            if due.is_empty() {
                return Ok(events);
            }
            rounds += 1;
            if rounds > bound {
                return Err(EngineError::TransitionLoop { planet, rounds });
            }
            let mut completed_at = BTreeSet::new();
            for transition in due {
                match transition {
                    Due::Construction(id) => {
                        let b = self.buildings.get_mut(&id).expect("due on this planet");
                        b.state = BuildingState::Operational;
                        events.push(DomainEvent::BuildingCompleted {
                            planet,
                            building: id,
                            kind: b.kind,
                        });
                    }
                    Due::Build(id) => {
                        let build = self.ship_builds.remove(&id).expect("due on this planet");
                        let owner =
                            self.planets[&planet]
                                .owner
                                .ok_or(EngineError::UnownedShipyard {
                                    planet,
                                    shipyard: build.shipyard,
                                })?;
                        let ship = ShipId(self.allocate_id()?);
                        self.ships.insert(
                            ship,
                            Ship {
                                id: ship,
                                kind: build.kind,
                                owner,
                                location: ShipLocation::Roster(planet),
                            },
                        );
                        let p = self
                            .planets
                            .get_mut(&planet)
                            .expect("checked by the caller");
                        p.roster.insert(ship);
                        completed_at.insert(build.shipyard);
                        events.push(DomainEvent::ShipCompleted {
                            planet,
                            shipyard: build.shipyard,
                            ship,
                            kind: build.kind,
                        });
                    }
                    Due::Demolition(id) => {
                        let kind = self.buildings[&id].kind;
                        if self.ship_builds.values().any(|b| b.shipyard == id) {
                            return Err(EngineError::DemolishedShipyardHasBuilds {
                                planet,
                                shipyard: id,
                            });
                        }
                        self.buildings.remove(&id);
                        let p = self
                            .planets
                            .get_mut(&planet)
                            .expect("checked by the caller");
                        p.buildings.remove(&id);
                        events.push(DomainEvent::DemolitionCompleted {
                            planet,
                            building: id,
                            kind,
                        });
                    }
                }
            }
            for yard in completed_at {
                self.start_waiting_builds(yard, rules.buildings.shipyard.parallel_builds, now)?;
            }
        }
    }

    /// The transitions due now on `planet`, in ascending entity ID.
    fn due_transitions(&self, planet: PlanetId, now: i64) -> Vec<Due> {
        let p = &self.planets[&planet];
        let mut due = Vec::new();
        for id in &p.buildings {
            match self.buildings[id].state {
                BuildingState::UnderConstruction {
                    delivered, cost_g, ..
                } if delivered.value_g() >= cost_g => {
                    due.push((id.0, Due::Construction(*id)));
                }
                BuildingState::Demolishing { ends_at_ms } if ends_at_ms <= now => {
                    due.push((id.0, Due::Demolition(*id)));
                }
                _ => {}
            }
        }
        for b in self.ship_builds.values() {
            if p.buildings.contains(&b.shipyard)
                && b.status == ShipBuildStatus::Active
                && b.delivered.value_g() >= b.cost_g
            {
                due.push((b.id.0, Due::Build(b.id)));
            }
        }
        due.sort_unstable_by_key(|d| d.0);
        due.into_iter().map(|d| d.1).collect()
    }

    /// Starts a Shipyard's waiting builds in ascending ID order until `limit` are active, each
    /// on the terms it was queued with (D-026), checkpointed at now.
    pub(crate) fn start_waiting_builds(
        &mut self,
        yard: BuildingId,
        limit: u32,
        now: i64,
    ) -> Result<(), EngineError> {
        let active = self
            .ship_builds
            .values()
            .filter(|b| b.shipyard == yard && b.status == ShipBuildStatus::Active)
            .count();
        let waiting: Vec<ShipBuildId> = self
            .ship_builds
            .values()
            .filter(|b| b.shipyard == yard && b.status == ShipBuildStatus::Waiting)
            .map(|b| b.id)
            .collect();
        let free = (limit as usize).saturating_sub(active);
        for id in waiting.into_iter().take(free) {
            let b = self.ship_builds.get_mut(&id).expect("listed above");
            b.status = ShipBuildStatus::Active;
            b.delivered = b
                .delivered
                .advanced_to(now)
                .map_err(|error| EngineError::Quantity {
                    planet: self.buildings[&yard].planet,
                    entity: id.0,
                    error,
                })?;
        }
        Ok(())
    }

    /// Steps 3 to 5 on the planet as transitions left it: flows, net rates with remainders
    /// carried, and the earliest crossing or timer scheduled. Everything is computed, and the
    /// next event and version checked, before anything is written.
    fn set_rates_and_schedule(
        &mut self,
        planet: PlanetId,
        rules: &Rules,
    ) -> Result<(), EngineError> {
        let now = self.time_ms;
        let p = &self.planets[&planet];
        let flows = flows_with(self, planet, rules, p.ore.value_g(), p.ingots.value_g())?;
        let quantity = |entity| {
            move |error| EngineError::Quantity {
                planet,
                entity,
                error,
            }
        };
        let ore = p
            .ore
            .with_rate(now, flows.ore_net)
            .map_err(quantity(planet.0))?;
        let ingots = p
            .ingots
            .with_rate(now, flows.ingot_net)
            .map_err(quantity(planet.0))?;
        let mut next = earliest(
            crossing(ore, Some(0), Some(p.ore_cap_g)).map_err(quantity(planet.0))?,
            crossing(ingots, Some(0), Some(p.ingot_cap_g)).map_err(quantity(planet.0))?,
        );
        let mut rated = Vec::new();
        for id in &p.buildings {
            match self.buildings[id].state {
                BuildingState::UnderConstruction {
                    delivered, cost_g, ..
                } => {
                    let rate = flows.buildings[id].rate_g_per_h;
                    let delivered = delivered.with_rate(now, rate).map_err(quantity(id.0))?;
                    next = earliest(
                        next,
                        crossing(delivered, None, Some(cost_g)).map_err(quantity(id.0))?,
                    );
                    rated.push((id.0, delivered));
                }
                BuildingState::Demolishing { ends_at_ms } => {
                    next = earliest(next, Some(ends_at_ms))
                }
                BuildingState::Operational => {}
            }
        }
        for (&id, flow) in &flows.ship_builds {
            let b = &self.ship_builds[&id];
            let delivered = b
                .delivered
                .with_rate(now, flow.rate_g_per_h)
                .map_err(quantity(id.0))?;
            next = earliest(
                next,
                crossing(delivered, None, Some(b.cost_g)).map_err(quantity(id.0))?,
            );
            rated.push((id.0, delivered));
        }
        after_now(next, now)?;
        // Scheduling bumps the version; check it here, so a failure changes nothing.
        if p.version == u64::MAX {
            return Err(EngineError::VersionExhausted(planet.0));
        }

        let p = self
            .planets
            .get_mut(&planet)
            .expect("checked by the caller");
        p.ore = ore;
        p.ingots = ingots;
        for (entity, delivered) in rated {
            self.set_delivered(entity, delivered);
        }
        self.schedule_planet(planet, next)
    }
}

/// A transition due now.
#[derive(Debug, Clone, Copy)]
enum Due {
    Construction(BuildingId),
    Build(ShipBuildId),
    Demolition(BuildingId),
}

/// The scheduler's guard on the solver's candidate (Next event): it must be strictly after now.
pub(crate) fn after_now(next: Option<i64>, now: i64) -> Result<(), EngineError> {
    match next {
        Some(at_ms) if at_ms <= now => Err(EngineError::NotAfterNow { at_ms, now }),
        _ => Ok(()),
    }
}

/// What a solve may change, kept so a failed standalone solve can be undone.
struct Saved {
    planet: Planet,
    buildings: Vec<Building>,
    builds: Vec<ShipBuild>,
    next_id: u64,
}

impl Saved {
    fn take(world: &World, planet: PlanetId) -> Result<Self, EngineError> {
        let p = world.planets.get(&planet).ok_or_else(|| {
            StateError::Link(format!("solve: planet {} does not exist", planet.0))
        })?;
        Ok(Self {
            planet: p.clone(),
            buildings: p.buildings.iter().map(|id| world.buildings[id]).collect(),
            builds: world
                .ship_builds
                .values()
                .filter(|b| p.buildings.contains(&b.shipyard))
                .cloned()
                .collect(),
            next_id: world.next_id,
        })
    }

    fn restore(self, world: &mut World) {
        let ids: BTreeSet<BuildingId> = self.buildings.iter().map(|b| b.id).collect();
        world.ship_builds.retain(|_, b| !ids.contains(&b.shipyard));
        for b in self.builds {
            world.ship_builds.insert(b.id, b);
        }
        for b in self.buildings {
            world.buildings.insert(b.id, b);
        }
        let next_id = self.next_id;
        world.ships.retain(|id, _| id.0 < next_id);
        world.next_id = next_id;
        world.planets.insert(self.planet.id, self.planet);
    }
}

fn earliest(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests;
