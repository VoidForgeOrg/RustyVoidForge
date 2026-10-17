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
//! rates; schedule the earliest crossing or timer. [`Solver`] is the production step that runs it
//! for each `PlanetChange` (D-025).

use std::collections::BTreeMap;

use crate::quantity::Quantity;
use crate::rules::Rules;
use crate::state::{
    BuildingId, BuildingKind, BuildingState, DomainEvent, EngineError, FleetId, PlanetId,
    QueueKind, ShipBuildId, ShipBuildStatus, StateError, Steps, World,
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

/// The flows of `planet` at world time, from its stored state and `rules`. Fails only if the
/// planet does not exist or a stock cannot be read at world time.
pub fn planet_flows(
    world: &World,
    planet: PlanetId,
    rules: &Rules,
) -> Result<PlanetFlows, StateError> {
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
    Ok(flows_with(world, planet, rules, ore, ingots))
}

/// The flows of an existing planet whose stocks hold `ore` and `ingots` grams.
fn flows_with(
    world: &World,
    planet: PlanetId,
    rules: &Rules,
    ore: i64,
    ingots: i64,
) -> PlanetFlows {
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
    let e: i64 = drills.iter().map(|d| d.1).sum();
    let q: i64 = refineries.iter().map(|r| r.1).sum();
    let demand: i64 = consumers.iter().map(|c| c.1).sum();
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
    let o = ratio * a;
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
        let (rate, rated) = active
            .iter()
            .filter(|s| s.1 == yard)
            .fold((0, 0), |(r, d), s| (r + given[&s.0.0], d + s.2));
        buildings.insert(yard, Flow::new(rate, rated));
    }
    for id in &p.buildings {
        if let BuildingState::UnderConstruction { rated_g_per_h, .. } = world.buildings[id].state {
            buildings.insert(*id, Flow::new(given[&id.0], rated_g_per_h));
        }
    }

    let ingots_delivered: i64 = delivered.iter().sum();
    PlanetFlows {
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
    }
}

/// The production steps: a planet change re-solves its planet. Fleet arrivals and returns are
/// P1.8's and stop the engine until then.
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
        world.solve_planet(planet, self.rules)?;
        Ok(Vec::new())
    }

    fn fleet_arrival(
        &mut self,
        _world: &mut World,
        fleet: FleetId,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        Err(EngineError::StepNotBuilt(QueueKind::FleetArrival(fleet)))
    }

    fn fleet_return(
        &mut self,
        _world: &mut World,
        fleet: FleetId,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        Err(EngineError::StepNotBuilt(QueueKind::FleetReturn(fleet)))
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
    /// Re-solves `planet` at world time (Per-planet solver, steps 1 to 5): brings every quantity
    /// to now and snaps those crossing now, refuses a transition due now
    /// ([`EngineError::TransitionNotBuilt`] until P1.7), solves the flows, sets every net rate
    /// and schedules the earliest crossing or timer, bumping the planet's version. Everything is
    /// computed before anything is written, so an error leaves the planet as it was.
    pub fn solve_planet(&mut self, planet: PlanetId, rules: &Rules) -> Result<(), EngineError> {
        if self.is_poisoned() {
            return Err(EngineError::Poisoned);
        }
        let now = self.time_ms;
        let p = self.planets.get(&planet).ok_or_else(|| {
            StateError::Link(format!("solve: planet {} does not exist", planet.0))
        })?;

        // Step 1: every quantity to now.
        let ore = bring(p.ore, now, Some(0), Some(p.ore_cap_g), planet, planet.0)?;
        let ingots = bring(
            p.ingots,
            now,
            Some(0),
            Some(p.ingot_cap_g),
            planet,
            planet.0,
        )?;
        let mut constructions = Vec::new();
        let mut shipyards = Vec::new();
        for id in &p.buildings {
            match self.buildings[id].state {
                BuildingState::UnderConstruction {
                    delivered,
                    cost_g,
                    rated_g_per_h,
                } => {
                    let delivered = bring(delivered, now, None, Some(cost_g), planet, id.0)?;
                    constructions.push((*id, delivered, cost_g, rated_g_per_h));
                }
                // Step 2, demolitions: ending is a transition (P1.7).
                BuildingState::Demolishing { ends_at_ms } if ends_at_ms <= now => {
                    return Err(EngineError::TransitionNotBuilt {
                        planet,
                        entity: id.0,
                    });
                }
                BuildingState::Operational if self.buildings[id].kind == BuildingKind::Shipyard => {
                    shipyards.push(*id)
                }
                _ => {}
            }
        }
        let mut builds = Vec::new();
        for b in self.ship_builds.values() {
            if shipyards.contains(&b.shipyard) && b.status == ShipBuildStatus::Active {
                let delivered = bring(b.delivered, now, None, Some(b.cost_g), planet, b.id.0)?;
                builds.push((b.id, delivered, b.cost_g));
            }
        }
        // Step 2, builds: reaching the cost is a transition (P1.7).
        let complete = constructions
            .iter()
            .map(|c| (c.0.0, c.1, c.2))
            .chain(builds.iter().map(|b| (b.0.0, b.1, b.2)))
            .find(|&(_, delivered, cost)| delivered.value_g() >= cost);
        if let Some((entity, _, _)) = complete {
            return Err(EngineError::TransitionNotBuilt { planet, entity });
        }

        // Steps 3 and 4: flows, on the quantities brought to now, and the net rates.
        let flows = flows_with(self, planet, rules, ore.value_g(), ingots.value_g());
        let quantity = |entity| {
            move |error| EngineError::Quantity {
                planet,
                entity,
                error,
            }
        };
        let ore = ore
            .with_rate(now, flows.ore_net)
            .map_err(quantity(planet.0))?;
        let ingots = ingots
            .with_rate(now, flows.ingot_net)
            .map_err(quantity(planet.0))?;
        let mut next = [
            crossing(ore, Some(0), Some(p.ore_cap_g)),
            crossing(ingots, Some(0), Some(p.ingot_cap_g)),
        ]
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(quantity(planet.0))?
        .into_iter()
        .flatten()
        .min();
        let mut rated = Vec::new();
        for &(id, delivered, cost, _) in &constructions {
            let rate = flows.buildings[&id].rate_g_per_h;
            let delivered = delivered.with_rate(now, rate).map_err(quantity(id.0))?;
            next = earliest(
                next,
                crossing(delivered, None, Some(cost)).map_err(quantity(id.0))?,
            );
            rated.push((id.0, delivered));
        }
        for &(id, delivered, cost) in &builds {
            let rate = flows.ship_builds[&id].rate_g_per_h;
            let delivered = delivered.with_rate(now, rate).map_err(quantity(id.0))?;
            next = earliest(
                next,
                crossing(delivered, None, Some(cost)).map_err(quantity(id.0))?,
            );
            rated.push((id.0, delivered));
        }
        for id in &p.buildings {
            if let BuildingState::Demolishing { ends_at_ms } = self.buildings[id].state {
                next = earliest(next, Some(ends_at_ms));
            }
        }
        if let Some(at_ms) = next
            && at_ms <= now
        {
            return Err(EngineError::NotAfterNow { at_ms, now });
        }

        // Write, then step 5: schedule.
        let p = self.planets.get_mut(&planet).expect("checked above");
        p.ore = ore;
        p.ingots = ingots;
        for (entity, delivered) in rated {
            if let Some(b) = self.buildings.get_mut(&BuildingId(entity))
                && let BuildingState::UnderConstruction { delivered: d, .. } = &mut b.state
            {
                *d = delivered;
            } else if let Some(b) = self.ship_builds.get_mut(&ShipBuildId(entity)) {
                b.delivered = delivered;
            }
        }
        self.schedule_planet(planet, next)
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
