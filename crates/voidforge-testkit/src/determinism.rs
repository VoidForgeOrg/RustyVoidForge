//! Shared pieces of the determinism suite (Architectural Design, Testing strategy and
//! Determinism rules; Technical Plan P1.5; decision log D-024).
//!
//! The suite itself is the `determinism` test binary (`tests/determinism.rs`), run by
//! `just determinism` and the CI job of that name. This module holds what its harnesses share,
//! so later packages extend the same harnesses instead of writing parallel ones:
//!
//! - [`canonical_bytes`]: the comparison form, postcard bytes. P2.2's state hash is BLAKE3 over
//!   exactly the postcard bytes of the World (D-024).
//! - [`Scenario`] and [`Churn`]: a generated world and deterministic test [`Steps`] that keep
//!   the event loop busy (planet changes reschedule, fleets are sent, recalled, arrive and
//!   return) without inventing gameplay. Until the solver and fleets land, production
//!   `advance_to` refuses live events, so the harnesses drive `advance_with` with these steps.
//! - [`settle_stock`]: a stock settled event by event (crossing, snap, stop), for observation
//!   invariance over quantities.

use serde::Serialize;
use voidforge_core::quantity::{Quantity, QuantityError};
use voidforge_core::state::{
    Cargo, EngineError, FleetId, FleetState, Mission, PlanetId, QueueKind, ShipKind, Steps, World,
};

use crate::{ALICE, WorldBuilder};

/// The canonical bytes of a value: its postcard encoding. Two runs are the same run when these
/// are equal.
pub fn canonical_bytes<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    postcard::to_allocvec(value).expect("the value serializes")
}

/// A fixed integer mix (splitmix64), so scenarios and steps derive every choice from their
/// inputs.
pub fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A generated engine scenario: Alice's homeworld plus unowned planets, one-ship fleets, every
/// planet scheduled and some fleets already under way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scenario {
    /// Drives every choice in the world and the steps.
    pub seed: u64,
    /// Planets, at least 1.
    pub planets: u64,
    /// One-ship fleets.
    pub fleets: u64,
    /// Gaps between events and travel times are in `1..=max_gap_ms`.
    pub max_gap_ms: i64,
    /// Planets stop rescheduling at this time, so a run settles.
    pub horizon_ms: i64,
}

impl Scenario {
    /// The starting world, at time 0. Equal scenarios give equal worlds.
    pub fn world(&self) -> World {
        let mut b = WorldBuilder::new().seed(self.seed);
        let mut planets = vec![b.player(ALICE)];
        for _ in 1..self.planets.max(1) {
            planets.push(b.planet(None));
        }
        let mut fleets = Vec::new();
        for i in 0..self.fleets {
            let at = planets[(mix(self.seed ^ i) % planets.len() as u64) as usize];
            let ship = b.ship(ALICE, ShipKind::CargoVessel, at);
            fleets.push((b.fleet(&[ship], Cargo::default()), at));
        }
        for &p in &planets {
            b.schedule(p, self.gap(p.0));
        }
        for (f, at) in fleets {
            let r = mix(self.seed ^ f.0.rotate_left(17));
            if r.is_multiple_of(2) {
                let target = planets[(r / 2 % planets.len() as u64) as usize];
                let mission = if target == at {
                    Mission::Move
                } else {
                    Mission::Transport
                };
                b.send(f, target, mission, self.gap(r));
            }
        }
        b.build()
    }

    /// The steps for this scenario.
    pub fn steps(&self) -> Churn {
        Churn {
            seed: self.seed,
            max_gap_ms: self.max_gap_ms,
            horizon_ms: self.horizon_ms,
        }
    }

    fn gap(&self, x: u64) -> i64 {
        1 + (mix(self.seed ^ x) % self.max_gap_ms.max(1) as u64) as i64
    }
}

/// One processed entry, as the harnesses record it: the instant, what ran, and the entity's
/// version after its step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Stepped {
    /// The instant.
    pub at_ms: i64,
    /// What ran.
    pub kind: QueueKind,
    /// The entity's version after the step.
    pub version: u64,
}

/// Deterministic test steps, a stand-in for the solver and fleets that schedules real
/// follow-up work. Every choice is a function of the scenario seed, the entity, its version and
/// the instant, never of how the run was split into advances.
///
/// - A planet change may send one of the fleets stationed there (lowest ID first) and may recall
///   one in-transit fleet that left from there earlier, then reschedules the planet until the
///   horizon.
/// - An arrival stations the fleet at its target; a return stations it at its origin.
#[derive(Debug, Clone, Copy)]
pub struct Churn {
    seed: u64,
    max_gap_ms: i64,
    horizon_ms: i64,
}

impl Churn {
    fn roll(&self, entity: u64, version: u64, now: i64) -> u64 {
        mix(self.seed ^ mix(entity ^ mix(version ^ now as u64)))
    }

    fn gap(&self, r: u64) -> i64 {
        1 + (r % self.max_gap_ms.max(1) as u64) as i64
    }
}

impl Steps for Churn {
    type Event = Stepped;

    fn planet_change(&mut self, w: &mut World, p: PlanetId) -> Result<Vec<Stepped>, EngineError> {
        let now = w.time_ms;
        let r = self.roll(p.0, w.planets[&p].version, now);

        if r.is_multiple_of(3) {
            let stationed = w
                .fleets
                .values()
                .find(|f| f.state == FleetState::Stationed { at: p })
                .map(|f| f.id);
            if let Some(f) = stationed {
                let planets: Vec<PlanetId> = w.planets.keys().copied().collect();
                let target = planets[(r / 3 % planets.len() as u64) as usize];
                w.fleets.get_mut(&f).expect("found above").state = FleetState::InTransit {
                    origin: p,
                    target,
                    mission: Mission::Move,
                    departed_at_ms: now,
                    arrives_at_ms: now + self.gap(r >> 8),
                };
                w.schedule_fleet(f)?;
            }
        }

        if r.is_multiple_of(5) {
            let recalled = w.fleets.values().find_map(|f| match f.state {
                FleetState::InTransit {
                    origin,
                    departed_at_ms,
                    ..
                } if origin == p && departed_at_ms < now => Some((f.id, departed_at_ms)),
                _ => None,
            });
            if let Some((f, departed)) = recalled {
                w.fleets.get_mut(&f).expect("found above").state = FleetState::Returning {
                    origin: p,
                    arrives_at_ms: now + (now - departed),
                };
                w.schedule_fleet(f)?;
            }
        }

        if now < self.horizon_ms {
            w.schedule_planet(p, Some(now + self.gap(r >> 16)))?;
        }
        Ok(vec![Stepped {
            at_ms: now,
            kind: QueueKind::PlanetChange(p),
            version: w.planets[&p].version,
        }])
    }

    fn fleet_arrival(&mut self, w: &mut World, f: FleetId) -> Result<Vec<Stepped>, EngineError> {
        let fleet = w.fleets.get_mut(&f).expect("a live arrival has its fleet");
        let FleetState::InTransit { target, .. } = fleet.state else {
            panic!("fleet {} arrived but is not in transit", f.0)
        };
        fleet.state = FleetState::Stationed { at: target };
        Ok(vec![Stepped {
            at_ms: w.time_ms,
            kind: QueueKind::FleetArrival(f),
            version: fleet.version,
        }])
    }

    fn fleet_return(&mut self, w: &mut World, f: FleetId) -> Result<Vec<Stepped>, EngineError> {
        let fleet = w.fleets.get_mut(&f).expect("a live return has its fleet");
        let FleetState::Returning { origin, .. } = fleet.state else {
            panic!("fleet {} returned but is not returning", f.0)
        };
        fleet.state = FleetState::Stationed { at: origin };
        Ok(vec![Stepped {
            at_ms: w.time_ms,
            kind: QueueKind::FleetReturn(f),
            version: fleet.version,
        }])
    }
}

/// A scenario run to `to_ms` in one advance.
pub fn run_straight(s: &Scenario, to_ms: i64) -> Result<(World, Vec<Stepped>), EngineError> {
    let mut w = s.world();
    let events = w.advance_with(to_ms, &mut s.steps())?;
    Ok((w, events))
}

/// A scenario run to `to_ms` in `advances` equal advances (the last takes the rest), with
/// `observe` reading the world after each one.
pub fn run_observed(
    s: &Scenario,
    to_ms: i64,
    advances: i64,
    mut observe: impl FnMut(&World),
) -> Result<(World, Vec<Stepped>), EngineError> {
    let mut w = s.world();
    let mut steps = s.steps();
    let mut events = Vec::new();
    for k in 1..=advances {
        events.extend(w.advance_with(to_ms * k / advances, &mut steps)?);
        observe(&w);
    }
    Ok((w, events))
}

/// The canonical bytes of a finished run: the world and its events.
pub fn run_bytes(world: &World, events: &[Stepped]) -> Vec<u8> {
    canonical_bytes(&(world, events))
}

/// A stock settled event by event up to each stop in turn: when it reaches 0 (falling) or
/// `cap_g` (rising) it snaps there and stops, and at each stop it is checkpointed and read. One
/// stop or a thousand, the result is the same if the rounding rule holds. Returns the final
/// quantity and the read at every stop.
pub fn settle_stock(
    start: Quantity,
    cap_g: i64,
    stops: &[i64],
) -> Result<(Quantity, Vec<i64>), QuantityError> {
    let mut q = start;
    let mut reads = Vec::with_capacity(stops.len());
    for &t in stops {
        let rate = q.rate_g_per_h();
        let crossing = if rate > 0 {
            q.rising_crossing(cap_g)?.map(|c| (c, cap_g))
        } else if rate < 0 {
            q.falling_crossing(0)?.map(|c| (c, 0))
        } else {
            None
        };
        if let Some((at, boundary)) = crossing
            && at <= t
        {
            q = q.snapped(boundary, at)?.with_rate(at, 0)?;
        }
        q = q.advanced_to(t)?;
        reads.push(q.value_at(t)?);
    }
    Ok((q, reads))
}

/// `n` stops evenly spread over `(from, to]`, ending exactly at `to`.
pub fn even_stops(from: i64, to: i64, n: i64) -> Vec<i64> {
    (1..=n).map(|k| from + (to - from) * k / n).collect()
}
