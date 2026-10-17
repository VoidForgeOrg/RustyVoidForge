//! The event loop: scheduling into the private queue, `advance_to` and the step seam
//! (Architectural Design, Time model, Engine core and Next event; decision log D-023).
//!
//! The loop owns order, staleness, time and the guards. What happens at a planet change or a
//! fleet arrival belongs to [`Steps`]; until the solver (P1.6/P1.7) and fleets (P1.8) supply
//! real steps, [`World::advance_to`] runs [`Pending`], which stops on any live entry.

use super::{
    EntryStatus, FleetId, PlanetId, QueueEntry, QueueKind, ScheduledEvent, StateError, World,
    expected_entry, link,
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

/// The engine's domain events. Empty until the packages that produce them add their variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainEvent {}

/// The production steps until their packages land: every live entry is [`EngineError::StepNotBuilt`],
/// never a silent skip.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pending;

impl Steps for Pending {
    type Event = DomainEvent;

    fn planet_change(
        &mut self,
        _world: &mut World,
        planet: PlanetId,
    ) -> Result<Vec<DomainEvent>, EngineError> {
        Err(EngineError::StepNotBuilt(QueueKind::PlanetChange(planet)))
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
    /// A live entry whose step has not been built yet.
    #[error("no step is built yet for {0:?}")]
    StepNotBuilt(QueueKind),
    /// An entity's version counter is exhausted.
    #[error("entity {0} has no versions left")]
    VersionExhausted(u64),
    /// The state broke an invariant: an invalid queue entry, or a step left its entity's
    /// schedule inconsistent.
    #[error(transparent)]
    State(#[from] StateError),
    /// An earlier advance failed; the world may be mid-instant and must be discarded.
    #[error("the world is poisoned by a failed advance and must be discarded")]
    Poisoned,
}

impl World {
    /// Processes every queued entry with time ≤ `t` in queue order, then sets world time to `t`,
    /// returning the domain events produced. Runs [`World::advance_with`] with the production
    /// steps.
    pub fn advance_to(&mut self, t: i64) -> Result<Vec<DomainEvent>, EngineError> {
        self.advance_with(t, &mut Pending)
    }

    /// The event loop. Pops entries in queue order up to `t`: stale ones are dropped, live ones
    /// set world time to their instant and run their step, after which the step's entity must
    /// have a consistent schedule. Then sets world time to `t`.
    ///
    /// `t` before world time is [`EngineError::TimeBeforeNow`] and changes nothing. Any other
    /// error may leave the world mid-instant, so it poisons the world: advancing, scheduling and
    /// serializing it then fail with [`EngineError::Poisoned`], and the caller must discard it.
    pub fn advance_with<S: Steps>(
        &mut self,
        t: i64,
        steps: &mut S,
    ) -> Result<Vec<S::Event>, EngineError> {
        self.usable()?;
        if t < self.time_ms {
            return Err(EngineError::TimeBeforeNow {
                t,
                now: self.time_ms,
            });
        }
        let result = self.run(t, steps);
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
                    self.check_planet_schedule(self.planet(id, "processed entry")?)?;
                }
                QueueKind::FleetArrival(id) => {
                    events.extend(steps.fleet_arrival(self, id)?);
                    self.check_fleet_after_step(id)?;
                }
                QueueKind::FleetReturn(id) => {
                    events.extend(steps.fleet_return(self, id)?);
                    self.check_fleet_after_step(id)?;
                }
            }
        }
        self.time_ms = t;
        Ok(events)
    }

    /// A fleet the step removed has nothing to check.
    fn check_fleet_after_step(&self, id: FleetId) -> Result<(), StateError> {
        match self.fleets.get(&id) {
            Some(fleet) => self.check_fleet_schedule(fleet),
            None => Ok(()),
        }
    }

    /// Sets a planet's next event, or clears it with `None`. Bumps the planet's version first, so
    /// any earlier entry goes stale. `at_ms` must be after now.
    pub fn schedule_planet(
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
    /// be after now.
    pub fn schedule_fleet(&mut self, fleet: FleetId) -> Result<(), EngineError> {
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
