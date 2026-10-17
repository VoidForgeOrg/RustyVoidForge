//! Fleets: derived speed, capacity, distance and travel time; the engine side of send and
//! recall; and what happens on arrival and return (Architectural Design, Fleets; decision log
//! D-027).
//!
//! [`World::send_fleet`] and [`World::recall_fleet`] are the entry points P1.10's commands
//! call. They do no command validation (ownership, mission requirements, another planet): that
//! is the commands' job. Both are atomic. Arrivals and returns run inside the event loop as
//! [`crate::solver::Solver`]'s fleet steps: an arrival brings its target to now, runs the
//! mission, stations the fleet and re-solves the target.

use crate::quantity::{MS_PER_HOUR, Quantity};
use crate::rules::{Rules, ShipSpec};
use crate::state::{
    Cargo, DomainEvent, EngineError, FleetId, FleetState, Mission, PlanetId, Position, ShipKind,
    StateError, World,
};

/// The integer square root of the summed squared coordinate differences, in distance units
/// (D-021), rounded down; `None` if it leaves 64 bits.
pub fn distance(a: Position, b: Position) -> Option<i64> {
    let square = |from: i64, to: i64| {
        let d = i128::from(to) - i128::from(from);
        d.checked_mul(d)
    };
    let sum = square(a.x, b.x)?
        .checked_add(square(a.y, b.y)?)?
        .checked_add(square(a.z, b.z)?)?;
    i64::try_from(sum.isqrt()).ok()
}

/// ceil(distance × 3,600,000 / speed) milliseconds, with speed in distance units per hour;
/// `None` if it leaves 64 bits. `distance` is not negative and `speed` is positive.
pub fn travel_time_ms(distance: i64, speed_du_per_h: i64) -> Option<i64> {
    let scaled = u128::try_from(distance).ok()? * MS_PER_HOUR as u128;
    let speed = u128::try_from(speed_du_per_h).ok().filter(|&s| s > 0)?;
    i64::try_from(scaled.div_ceil(speed)).ok()
}

/// The rules' spec for a kind of ship.
pub fn ship_spec(rules: &Rules, kind: ShipKind) -> &ShipSpec {
    match kind {
        ShipKind::ColonyShip => &rules.ships.colony_ship,
        ShipKind::CargoVessel => &rules.ships.cargo_vessel,
    }
}

impl World {
    /// The fleet's speed: its slowest ship's, in distance units per hour.
    pub fn fleet_speed(&self, fleet: FleetId, rules: &Rules) -> Result<i64, EngineError> {
        let f = self.fleet(fleet)?;
        let slowest = f.ships.iter().try_fold(i64::MAX, |slowest, id| {
            let speed = ship_spec(rules, self.ship_kind(*id)?).speed_du_per_h;
            Ok::<_, StateError>(slowest.min(speed))
        })?;
        Ok(slowest)
    }

    /// The fleet's cargo capacity: the sum of its ships' capacities, in grams (a Colony Ship's
    /// is 0 by the rules, D-027).
    pub fn fleet_capacity(&self, fleet: FleetId, rules: &Rules) -> Result<i64, EngineError> {
        let f = self.fleet(fleet)?;
        f.ships.iter().try_fold(0i64, |sum, id| {
            let capacity = ship_spec(rules, self.ship_kind(*id)?).cargo_capacity_g;
            sum.checked_add(capacity)
                .ok_or(EngineError::FleetOutOfRange {
                    fleet,
                    what: "capacity",
                })
        })
    }

    /// Sends a stationed fleet from its planet to `target` on `mission`, departing now. The
    /// arrival time is fixed here: now plus the travel time at the fleet's speed. Changes nothing
    /// on error. A target at zero distance gives a zero travel time, which the scheduler refuses
    /// ([`EngineError::NotAfterNow`]).
    pub fn send_fleet(
        &mut self,
        fleet: FleetId,
        target: PlanetId,
        mission: Mission,
        rules: &Rules,
    ) -> Result<(), EngineError> {
        self.not_poisoned()?;
        let FleetState::Stationed { at: origin } = self.fleet(fleet)?.state else {
            return Err(EngineError::FleetNotStationed(fleet));
        };
        let out_of_range = |what| EngineError::FleetOutOfRange { fleet, what };
        let from = self.planet_at(origin)?;
        let to = self.planet_at(target)?;
        let distance = distance(from, to).ok_or(out_of_range("distance"))?;
        let speed = self.fleet_speed(fleet, rules)?;
        let travel = travel_time_ms(distance, speed).ok_or(out_of_range("travel time"))?;
        let arrives_at_ms = self
            .time_ms
            .checked_add(travel)
            .ok_or(out_of_range("arrival time"))?;
        let state = FleetState::InTransit {
            origin,
            target,
            mission,
            departed_at_ms: self.time_ms,
            arrives_at_ms,
        };
        self.reschedule(fleet, state)
    }

    /// Recalls a fleet in transit: it turns around now and returns to its origin in the time it
    /// has traveled, arriving stationed with its cargo and ships. Its version goes up, so the old
    /// arrival entry goes stale. A recall at the departure instant has traveled 0 ms, so the
    /// fleet is stationed at its origin at once, with no entry (D-027). Changes nothing on error.
    pub fn recall_fleet(&mut self, fleet: FleetId) -> Result<(), EngineError> {
        self.not_poisoned()?;
        let FleetState::InTransit {
            origin,
            departed_at_ms,
            ..
        } = self.fleet(fleet)?.state
        else {
            return Err(EngineError::FleetNotInTransit(fleet));
        };
        let now = self.time_ms;
        if departed_at_ms == now {
            let f = self.fleets.get_mut(&fleet).expect("checked above");
            f.version = f
                .version
                .checked_add(1)
                .ok_or(EngineError::VersionExhausted(fleet.0))?;
            f.state = FleetState::Stationed { at: origin };
            return Ok(());
        }
        // Departure is never after now, so the time traveled is not negative.
        let arrives_at_ms =
            (now - departed_at_ms)
                .checked_add(now)
                .ok_or(EngineError::FleetOutOfRange {
                    fleet,
                    what: "return time",
                })?;
        self.reschedule(
            fleet,
            FleetState::Returning {
                origin,
                arrives_at_ms,
            },
        )
    }

    /// Puts a fleet into a moving state and queues its entry, or changes nothing.
    fn reschedule(&mut self, fleet: FleetId, state: FleetState) -> Result<(), EngineError> {
        let f = self.fleets.get_mut(&fleet).expect("checked by the caller");
        let before = f.state;
        f.state = state;
        let result = self.schedule_fleet(fleet);
        if result.is_err() {
            self.fleets.get_mut(&fleet).expect("still there").state = before;
        }
        result
    }

    /// A fleet's arrival at now (the [`crate::solver::Solver`] step): brings the target to now,
    /// runs the mission, stations the fleet there (removing it if colonizing used its last
    /// ship), then re-solves the target. Returns the arrival's events, then the re-solve's.
    pub(crate) fn arrive(
        &mut self,
        fleet: FleetId,
        rules: &Rules,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        let f = self.fleet(fleet)?;
        let FleetState::InTransit {
            target, mission, ..
        } = f.state
        else {
            return Err(link(format!("fleet {} arrived but is not in transit", fleet.0)).into());
        };
        let owner = f.owner;
        self.bring_planet(target)?;

        let mut events = vec![DomainEvent::FleetArrived {
            fleet,
            planet: target,
            mission,
        }];
        let unloads = match mission {
            Mission::Colonize if self.planets[&target].owner.is_none() => {
                let ship = self.fleets[&fleet]
                    .ships
                    .iter()
                    .copied()
                    .find(|id| self.ships[id].kind == ShipKind::ColonyShip)
                    .ok_or(EngineError::NoColonyShip(fleet))?;
                self.ships.remove(&ship);
                self.fleets
                    .get_mut(&fleet)
                    .expect("checked above")
                    .ships
                    .remove(&ship);
                self.planets.get_mut(&target).expect("brought above").owner = Some(owner);
                events.push(DomainEvent::PlanetColonized {
                    planet: target,
                    owner,
                    fleet,
                    ship,
                });
                true
            }
            Mission::Colonize => {
                events.push(DomainEvent::ColonizationFailed {
                    planet: target,
                    fleet,
                });
                false
            }
            Mission::Transport => true,
            Mission::Move => false,
        };
        if unloads {
            let cargo = self.unload(fleet, target)?;
            if cargo != Cargo::default() {
                events.push(DomainEvent::CargoUnloaded {
                    planet: target,
                    fleet,
                    cargo,
                });
            }
        }

        let f = self.fleets.get_mut(&fleet).expect("checked above");
        f.state = FleetState::Stationed { at: target };
        if f.ships.is_empty() {
            if f.cargo != Cargo::default() {
                return Err(EngineError::CargoWithoutShips(fleet));
            }
            self.fleets.remove(&fleet);
            events.push(DomainEvent::FleetRemoved {
                fleet,
                planet: target,
            });
        }
        events.extend(self.solve_planet(target, rules)?);
        Ok(events)
    }

    /// A recalled fleet's return at now: stationed at its origin, cargo and ships intact. The
    /// origin does not change, so it is not re-solved.
    pub(crate) fn return_home(&mut self, fleet: FleetId) -> Result<Vec<DomainEvent>, EngineError> {
        let f = self
            .fleets
            .get_mut(&fleet)
            .ok_or_else(|| link(format!("fleet {} does not exist", fleet.0)))?;
        let FleetState::Returning { origin, .. } = f.state else {
            return Err(link(format!("fleet {} returned but is not returning", fleet.0)).into());
        };
        f.state = FleetState::Stationed { at: origin };
        Ok(vec![DomainEvent::FleetReturned {
            fleet,
            planet: origin,
        }])
    }

    /// Unloads each resource up to the planet's free storage, now. The planet has been brought
    /// to now, so its stocks are checkpointed here; a stock that gains mass keeps its carried
    /// remainder and is left at rate 0 for the re-solve to set (D-027). Returns what unloaded.
    fn unload(&mut self, fleet: FleetId, planet: PlanetId) -> Result<Cargo, EngineError> {
        let aboard = self.fleets[&fleet].cargo;
        let p = &self.planets[&planet];
        let (ore, ore_g) = filled(p.ore, p.ore_cap_g, aboard.ore_g, planet)?;
        let (ingots, ingots_g) = filled(p.ingots, p.ingot_cap_g, aboard.ingots_g, planet)?;
        let p = self.planets.get_mut(&planet).expect("read above");
        p.ore = ore;
        p.ingots = ingots;
        let f = self.fleets.get_mut(&fleet).expect("read above");
        // Each amount is at most what is aboard, so neither difference goes negative.
        f.cargo.ore_g -= ore_g;
        f.cargo.ingots_g -= ingots_g;
        Ok(Cargo { ore_g, ingots_g })
    }

    fn fleet(&self, fleet: FleetId) -> Result<&crate::state::Fleet, StateError> {
        self.fleets
            .get(&fleet)
            .ok_or_else(|| link(format!("fleet {} does not exist", fleet.0)))
    }

    fn ship_kind(&self, ship: crate::state::ShipId) -> Result<ShipKind, StateError> {
        self.ships
            .get(&ship)
            .map(|s| s.kind)
            .ok_or_else(|| link(format!("ship {} does not exist", ship.0)))
    }

    fn planet_at(&self, planet: PlanetId) -> Result<Position, StateError> {
        self.planets
            .get(&planet)
            .map(|p| p.position)
            .ok_or_else(|| link(format!("planet {} does not exist", planet.0)))
    }

    fn not_poisoned(&self) -> Result<(), EngineError> {
        if self.is_poisoned() {
            Err(EngineError::Poisoned)
        } else {
            Ok(())
        }
    }
}

/// A stock checkpointed now, after taking up to `aboard` grams into its free storage: the new
/// stock and the grams taken. An unchanged stock is returned as it is.
fn filled(
    stock: Quantity,
    cap_g: i64,
    aboard: i64,
    planet: PlanetId,
) -> Result<(Quantity, i64), EngineError> {
    let free = cap_g.saturating_sub(stock.value_g()).max(0);
    let taken = aboard.min(free);
    if taken <= 0 {
        return Ok((stock, 0));
    }
    // value + taken is at most the cap, so it fits.
    let stock = Quantity::from_parts(
        stock.value_g() + taken,
        0,
        stock.remainder(),
        stock.checkpoint_ms(),
    )
    .map_err(|error| EngineError::Quantity {
        planet,
        entity: planet.0,
        error,
    })?;
    Ok((stock, taken))
}

fn link(message: String) -> StateError {
    StateError::Link(message)
}

#[cfg(test)]
mod tests;
