//! The event loop: scheduling into the private queue, `advance_to` and the step seam
//! (Architectural Design, Time model, Engine core and Next event; decision log D-023).
//!
//! The loop owns order, staleness, time and the guards. What happens at a planet change or a
//! fleet arrival belongs to [`Steps`]. [`World::advance_to`] runs the production steps,
//! [`crate::solver::Solver`] (D-025): planet changes re-solve the planet, and fleet entries run
//! their arrival or return (D-027).

use crate::quantity::QuantityError;
use crate::rules::Rules;
use crate::solver::Solver;

use serde::{Deserialize, Serialize};

use super::{
    BuildingId, BuildingKind, Cargo, EntryStatus, FleetId, FleetState, Mission, PlanetId, PlayerId,
    QueueEntry, QueueKind, ScheduledEvent, ShipId, ShipKind, StateError, World, expected_entry,
    link,
};

/// What happens when a live entry is popped. The loop has already set world time to the entry's
/// instant, and for a planet change has cleared the planet's `next_event`. A step brings its
/// entity to now, does its work, reschedules through [`World::schedule_planet`] or
/// [`World::schedule_fleet`], and returns the domain events it produced in order.
pub trait Steps {
    /// The domain events the steps produce.
    type Event;

    /// A planet's earliest crossing or timer is due.
    fn planet_change(
        &mut self,
        world: &mut World,
        planet: PlanetId,
    ) -> Result<Vec<Self::Event>, EngineError>;

    /// A fleet reaches its target.
    fn fleet_arrival(
        &mut self,
        world: &mut World,
        fleet: FleetId,
    ) -> Result<Vec<Self::Event>, EngineError>;

    /// A recalled fleet reaches its origin.
    fn fleet_return(
        &mut self,
        world: &mut World,
        fleet: FleetId,
    ) -> Result<Vec<Self::Event>, EngineError>;
}

/// The engine's domain events, returned by `advance_to` in the order they happened (D-026,
/// D-027).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DomainEvent {
    /// A construction reached its cost and is operational.
    BuildingCompleted {
        /// Its planet.
        planet: PlanetId,
        /// The building.
        building: BuildingId,
        /// What it is.
        kind: BuildingKind,
    },
    /// A ship build reached its cost; the ship joined the planet's roster.
    ShipCompleted {
        /// The Shipyard's planet.
        planet: PlanetId,
        /// The Shipyard.
        shipyard: BuildingId,
        /// The new ship.
        ship: ShipId,
        /// What it is.
        kind: ShipKind,
    },
    /// A demolition ended; the building is gone and its slot free.
    DemolitionCompleted {
        /// Its planet.
        planet: PlanetId,
        /// The building removed.
        building: BuildingId,
        /// What it was.
        kind: BuildingKind,
    },
    /// A fleet reached its target; first of an arrival's events. It is stationed there now.
    FleetArrived {
        /// The fleet.
        fleet: FleetId,
        /// Its target.
        planet: PlanetId,
        /// The mission it flew.
        mission: Mission,
    },
    /// A Colony Ship was consumed and the planet is the fleet owner's.
    PlanetColonized {
        /// The planet.
        planet: PlanetId,
        /// Its new owner.
        owner: PlayerId,
        /// The fleet.
        fleet: FleetId,
        /// The Colony Ship consumed.
        ship: ShipId,
    },
    /// A colony fleet found its target owned: nothing consumed, cargo still aboard.
    ColonizationFailed {
        /// The planet.
        planet: PlanetId,
        /// The fleet.
        fleet: FleetId,
    },
    /// Cargo went into the planet's storage; only when something did.
    CargoUnloaded {
        /// The planet.
        planet: PlanetId,
        /// The fleet.
        fleet: FleetId,
        /// The grams unloaded, per resource.
        cargo: Cargo,
    },
    /// Colonizing used the fleet's last ship, so the fleet is gone.
    FleetRemoved {
        /// The fleet.
        fleet: FleetId,
        /// Where it arrived.
        planet: PlanetId,
    },
    /// A recalled fleet is back, stationed at its origin.
    FleetReturned {
        /// The fleet.
        fleet: FleetId,
        /// Its origin.
        planet: PlanetId,
    },
}

/// Why the engine stopped. Except for [`EngineError::TimeBeforeNow`], an error from an advance
/// poisons the world (D-023).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// `advance_to` was asked to go back in time. Checked first; the world is unchanged.
    #[error("cannot advance to {t} ms: world time is already {now} ms")]
    TimeBeforeNow {
        /// The requested time.
        t: i64,
        /// World time.
        now: i64,
    },
    /// Work scheduled at or before now: a step scheduled at its own instant, which is a bug.
    #[error("cannot schedule at {at_ms} ms: it is not after now ({now} ms)")]
    NotAfterNow {
        /// The requested time.
        at_ms: i64,
        /// World time, the instant being processed during an advance.
        now: i64,
    },
    /// A live entry is due before the instant the loop has reached.
    #[error("live queue entry {entry:?} is behind the instant reached, {now} ms")]
    LiveEntryBehind {
        /// The entry.
        entry: QueueEntry,
        /// The instant reached.
        now: i64,
    },
    /// Only a fleet in transit or returning has an entry to schedule.
    #[error("fleet {} is stationed, so it has nothing to schedule", .0.0)]
    FleetNotMoving(FleetId),
    /// `advance_to` was given rules other than the world's version. Checked first; the world is
    /// unchanged and not poisoned.
    #[error("the world runs rules version {world}, but rules version {given} were given")]
    RulesMismatch {
        /// The world's rules version.
        world: u32,
        /// The given rules' version.
        given: u32,
    },
    /// Step 2 repeated more often than the planet has buildings and builds: a bug (Next event).
    #[error("planet {}: {rounds} rounds of transitions at one instant", planet.0)]
    TransitionLoop {
        /// The planet being solved.
        planet: PlanetId,
        /// The rounds reached.
        rounds: usize,
    },
    /// A ship completed at a Shipyard on a planet nobody owns, so the ship has no owner.
    #[error("planet {}: Shipyard {} completed a ship, but the planet has no owner", planet.0, shipyard.0)]
    UnownedShipyard {
        /// The planet.
        planet: PlanetId,
        /// The Shipyard.
        shipyard: BuildingId,
    },
    /// A Shipyard finished demolition while it still had builds.
    #[error("planet {}: Shipyard {} was demolished with builds", planet.0, shipyard.0)]
    DemolishedShipyardHasBuilds {
        /// The planet.
        planet: PlanetId,
        /// The Shipyard.
        shipyard: BuildingId,
    },
    /// A quantity's crossing lies before now: its event was missed.
    #[error("planet {}: entity {entity} crossed its boundary at {at_ms} ms, before now ({now} ms)", planet.0)]
    MissedCrossing {
        /// The planet being solved.
        planet: PlanetId,
        /// The planet itself for a stock, or the build.
        entity: u64,
        /// The missed crossing.
        at_ms: i64,
        /// Now.
        now: i64,
    },
    /// A planet's total rate leaves 64 bits: rules that validate, but at extreme rates.
    #[error("planet {}: the {what} total leaves the 64-bit range", planet.0)]
    RateOutOfRange {
        /// The planet.
        planet: PlanetId,
        /// Which total.
        what: &'static str,
    },
    /// A quantity could not be computed within 64 bits.
    #[error("planet {}: entity {entity}: {error}", planet.0)]
    Quantity {
        /// The planet being solved.
        planet: PlanetId,
        /// The planet itself for a stock, or the build.
        entity: u64,
        /// What went wrong.
        error: QuantityError,
    },
    /// A fleet's distance, travel time, arrival time or capacity leaves 64 bits: valid rules
    /// and coordinates, but extreme ones.
    #[error("fleet {}: the {what} leaves the 64-bit range", fleet.0)]
    FleetOutOfRange {
        /// The fleet.
        fleet: FleetId,
        /// Which value.
        what: &'static str,
    },
    /// Only a stationed fleet can be sent.
    #[error("fleet {} is not stationed", .0.0)]
    FleetNotStationed(FleetId),
    /// Only a fleet in transit can be recalled, and so only once.
    #[error("fleet {} is not in transit", .0.0)]
    FleetNotInTransit(FleetId),
    /// A Colonize arrival on an unowned planet with no Colony Ship aboard: a bug, since the
    /// send requires one, not a failed colonization.
    #[error("fleet {} arrived to colonize without a Colony Ship", .0.0)]
    NoColonyShip(FleetId),
    /// Colonizing used a fleet's last ship while cargo stayed aboard: unreachable while a
    /// Colony Ship carries nothing, and never resolved by destroying the cargo.
    #[error("fleet {} has cargo aboard but no ships left", .0.0)]
    CargoWithoutShips(FleetId),
    /// An entity's version counter is exhausted.
    #[error("entity {0} has no versions left")]
    VersionExhausted(u64),
    /// The state broke an invariant: an invalid queue entry, or a step left its entity's
    /// schedule inconsistent.
    #[error(transparent)]
    State(#[from] StateError),
    /// A step called `advance_to` or `advance_with` while an advance was running. Rejected before
    /// anything changes, and it stops the outer advance.
    #[error("cannot advance from inside a step: an advance is already running")]
    NestedAdvance,
    /// An earlier advance failed; the world may be mid-instant and must be discarded.
    #[error("the world is poisoned by a failed advance and must be discarded")]
    Poisoned,
}

impl World {
    /// Processes every queued entry with time ≤ `t` in queue order, then sets world time to `t`,
    /// returning the domain events produced. Runs [`World::advance_with`] with the production
    /// steps under `rules`, whose version must be the world's: otherwise
    /// [`EngineError::RulesMismatch`], with nothing changed.
    pub fn advance_to(&mut self, t: i64, rules: &Rules) -> Result<Vec<DomainEvent>, EngineError> {
        if rules.version != self.rules_version {
            return Err(EngineError::RulesMismatch {
                world: self.rules_version,
                given: rules.version,
            });
        }
        self.advance_with(t, &mut Solver { rules })
    }

    /// The event loop. Pops entries in queue order up to `t`: stale ones are dropped, live ones
    /// set world time to their instant and run their step, after which the step's entity must
    /// have a consistent schedule. Then sets world time to `t`.
    ///
    /// `t` before world time is [`EngineError::TimeBeforeNow`] and changes nothing. Any other
    /// error may leave the world mid-instant, so it poisons the world: advancing, scheduling and
    /// serializing it then fail with [`EngineError::Poisoned`], and the caller must discard it.
    /// A step that calls this gets [`EngineError::NestedAdvance`], which also stops the running
    /// advance, even if the step ignores it.
    pub fn advance_with<S: Steps>(
        &mut self,
        t: i64,
        steps: &mut S,
    ) -> Result<Vec<S::Event>, EngineError> {
        if self.advancing {
            return Err(self.latch(EngineError::NestedAdvance));
        }
        self.usable()?;
        if t < self.time_ms {
            return Err(EngineError::TimeBeforeNow {
                t,
                now: self.time_ms,
            });
        }
        self.advancing = true;
        let result = self.run(t, steps);
        self.advancing = false;
        self.fault = None;
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn run<S: Steps>(&mut self, t: i64, steps: &mut S) -> Result<Vec<S::Event>, EngineError> {
        let mut events = Vec::new();
        while let Some(&entry) = self.queue.first() {
            if entry.at_ms > t {
                break;
            }
            self.queue.remove(&entry);
            if self.entry_status(&entry)? == EntryStatus::Stale {
                continue;
            }
            if entry.at_ms < self.time_ms {
                return Err(EngineError::LiveEntryBehind {
                    entry,
                    now: self.time_ms,
                });
            }
            self.time_ms = entry.at_ms;
            match entry.kind {
                QueueKind::PlanetChange(id) => {
                    // The popped entry was the planet's next event; the step sets the next one.
                    if let Some(planet) = self.planets.get_mut(&id) {
                        planet.next_event = None;
                    }
                    events.extend(steps.planet_change(self, id)?);
                    self.stop_on_fault()?;
                    self.check_planet_schedule(self.planet(id, "processed entry")?)?;
                }
                QueueKind::FleetArrival(id) => {
                    // An arrival re-solves its target, so the target's schedule is checked
                    // too, even if the step removed the fleet (D-023, D-027).
                    let target = match self.fleets.get(&id).map(|f| f.state) {
                        Some(FleetState::InTransit { target, .. }) => Some(target),
                        _ => None,
                    };
                    events.extend(steps.fleet_arrival(self, id)?);
                    self.stop_on_fault()?;
                    self.check_fleet_after_step(id)?;
                    if let Some(target) = target {
                        self.check_planet_schedule(self.planet(target, "arrival target")?)?;
                    }
                }
                QueueKind::FleetReturn(id) => {
                    events.extend(steps.fleet_return(self, id)?);
                    self.stop_on_fault()?;
                    self.check_fleet_after_step(id)?;
                }
            }
        }
        self.time_ms = t;
        Ok(events)
    }

    /// A scheduling error inside the step stops the advance, even if the step went on and
    /// returned success.
    fn stop_on_fault(&mut self) -> Result<(), EngineError> {
        match self.fault.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Inside an advance, keeps the first scheduling error for [`World::stop_on_fault`].
    fn latch(&mut self, error: EngineError) -> EngineError {
        if self.advancing && self.fault.is_none() {
            self.fault = Some(error.clone());
        }
        error
    }

    /// A fleet the step removed has nothing to check.
    fn check_fleet_after_step(&self, id: FleetId) -> Result<(), StateError> {
        match self.fleets.get(&id) {
            Some(fleet) => self.check_fleet_schedule(fleet),
            None => Ok(()),
        }
    }

    /// Sets a planet's next event, or clears it with `None`. Bumps the planet's version first, so
    /// any earlier entry goes stale. `at_ms` must be after now. An error changes nothing; inside
    /// an advance it also stops the advance, even if the step ignores it.
    pub fn schedule_planet(
        &mut self,
        planet: PlanetId,
        at_ms: Option<i64>,
    ) -> Result<(), EngineError> {
        self.try_schedule_planet(planet, at_ms)
            .map_err(|error| self.latch(error))
    }

    fn try_schedule_planet(
        &mut self,
        planet: PlanetId,
        at_ms: Option<i64>,
    ) -> Result<(), EngineError> {
        self.usable()?;
        if let Some(at_ms) = at_ms {
            self.after_now(at_ms)?;
        }
        let Some(p) = self.planets.get_mut(&planet) else {
            return Err(link(format!("schedule: planet {} does not exist", planet.0)).into());
        };
        let version = p
            .version
            .checked_add(1)
            .ok_or(EngineError::VersionExhausted(planet.0))?;
        p.version = version;
        p.next_event = at_ms.map(|at_ms| ScheduledEvent { at_ms, version });
        if let Some(at_ms) = at_ms {
            self.queue.insert(QueueEntry {
                at_ms,
                kind: QueueKind::PlanetChange(planet),
                version,
            });
        }
        Ok(())
    }

    /// Queues a moving fleet's entry as its state says: an arrival in transit, a return when
    /// returning. Bumps the fleet's version first, so any earlier entry goes stale. The time must
    /// be after now. An error changes nothing; inside an advance it also stops the advance, even
    /// if the step ignores it.
    pub fn schedule_fleet(&mut self, fleet: FleetId) -> Result<(), EngineError> {
        self.try_schedule_fleet(fleet)
            .map_err(|error| self.latch(error))
    }

    fn try_schedule_fleet(&mut self, fleet: FleetId) -> Result<(), EngineError> {
        self.usable()?;
        let Some(f) = self.fleets.get_mut(&fleet) else {
            return Err(link(format!("schedule: fleet {} does not exist", fleet.0)).into());
        };
        let Some(mut entry) = expected_entry(f) else {
            return Err(EngineError::FleetNotMoving(fleet));
        };
        if entry.at_ms <= self.time_ms {
            return Err(EngineError::NotAfterNow {
                at_ms: entry.at_ms,
                now: self.time_ms,
            });
        }
        let version = f
            .version
            .checked_add(1)
            .ok_or(EngineError::VersionExhausted(fleet.0))?;
        f.version = version;
        entry.version = version;
        self.queue.insert(entry);
        Ok(())
    }

    fn after_now(&self, at_ms: i64) -> Result<(), EngineError> {
        if at_ms > self.time_ms {
            Ok(())
        } else {
            Err(EngineError::NotAfterNow {
                at_ms,
                now: self.time_ms,
            })
        }
    }

    fn usable(&self) -> Result<(), EngineError> {
        if self.poisoned {
            Err(EngineError::Poisoned)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
