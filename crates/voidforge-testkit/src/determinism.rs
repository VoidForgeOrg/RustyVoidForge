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
//! - [`Economy`] and [`slow_build_rules`]: solved planets (P1.6) whose stocks cross 0 and their
//!   caps through the production solver, with builds slowed so no transition (P1.7) comes due.
//!   With `fleets`, real fleets fly every mission, race to one planet and are recalled (P1.8).
//!   With `commands`, a generated script of player commands, accepted and rejected, is applied
//!   at whole hours, often the instant an event fires (P1.9).

use serde::Serialize;
use voidforge_core::quantity::{Quantity, QuantityError};
use voidforge_core::rules::{GRAMS_PER_TON, Rules};
use voidforge_core::state::{
    Cargo, EngineError, FleetId, FleetState, Mission, PlanetId, Position, QueueKind, ShipKind,
    Steps, World,
};

use std::collections::{BTreeMap, BTreeSet};

use voidforge_core::state::{BuildingKind, BuildingState, DomainEvent, ShipBuildStatus};

use voidforge_core::command::{Action, Command, Rejection};

use crate::{ALICE, BOB, WorldBuilder, v0_rules};

/// A third test player, who has not joined in any generated world.
pub const CAROL: voidforge_core::state::PlayerId =
    voidforge_core::state::PlayerId(0x0192_8c5e_7a3b_7c2d_9e4f_5a6b_7c8d_9e11);

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

/// A stock settled event by event up to each stop in turn. Each `(at_ms, rate)` in `changes`
/// (in time order, none before the start) sets a new rate at its instant, the way a re-solve
/// does, with the remainder carried. When the stock reaches 0 (falling) or `cap_g` (rising) it
/// snaps there and stops until the next change. At each stop it is checkpointed and read. One
/// stop or a thousand, the result is the same if the rounding rule holds. Returns the final
/// quantity and the read at every stop.
pub fn settle_stock(
    start: Quantity,
    cap_g: i64,
    changes: &[(i64, i64)],
    stops: &[i64],
) -> Result<(Quantity, Vec<i64>), QuantityError> {
    let mut q = start;
    let mut next = 0;
    let mut reads = Vec::with_capacity(stops.len());
    for &t in stops {
        loop {
            let change = changes.get(next).copied().filter(|&(at, _)| at <= t);
            q = settle_crossing(q, cap_g, change.map_or(t, |(at, _)| at))?;
            let Some((at, rate)) = change else { break };
            q = q.with_rate(at, rate)?;
            next += 1;
        }
        q = q.advanced_to(t)?;
        reads.push(q.value_at(t)?);
    }
    Ok((q, reads))
}

/// Snaps a stock that reaches its boundary by `until`, and stops it there.
fn settle_crossing(q: Quantity, cap_g: i64, until: i64) -> Result<Quantity, QuantityError> {
    let rate = q.rate_g_per_h();
    let crossing = if rate > 0 {
        q.rising_crossing(cap_g)?.map(|c| (c, cap_g))
    } else if rate < 0 {
        q.falling_crossing(0)?.map(|c| (c, 0))
    } else {
        None
    };
    match crossing {
        Some((at, boundary)) if at <= until => q.snapped(boundary, at)?.with_rate(at, 0),
        _ => Ok(q),
    }
}

/// `n` stops evenly spread over `(from, to]`, ending exactly at `to`.
pub fn even_stops(from: i64, to: i64, n: i64) -> Vec<i64> {
    (1..=n).map(|k| from + (to - from) * k / n).collect()
}

/// Rules v0 with every build's cost and build time multiplied by 1,000: the same rated rates,
/// so flows and splits are Rules v0's, but no build completes within the suite's 100 hours.
/// Solved scenarios never reach a transition, which P1.7 builds (D-025).
pub fn slow_build_rules() -> Rules {
    let mut rules = v0_rules();
    let b = &mut rules.buildings;
    let s = &mut rules.ships;
    for build in [
        &mut b.drill.build,
        &mut b.refinery.build,
        &mut b.shipyard.build,
        &mut s.colony_ship.build,
        &mut s.cargo_vessel.build,
    ] {
        build.cost_g *= 1_000;
        build.build_time_ms *= 1_000;
    }
    rules
        .validate()
        .expect("slowing builds keeps the rules valid");
    rules
}

/// A generated economy: Alice's planets with operational Drills and Refineries, constructions,
/// demolitions and a Shipyard with ship builds (never more buildings than Rules v0's 8 slots), and stocks anywhere from 0 to their caps (often exactly at one),
/// every planet solved at time 0. Planet 0 is always busy: her homeworld with both stores
/// nearly full, a construction, a demolition and a Shipyard with three active and one waiting
/// build, so every run crosses boundaries and, under Rules v0, completes builds (P1.7).
///
/// With `fleets` (P1.8), Bob joins with a homeworld, three unowned planets are added, and
/// fleets are sent at time 0 through the engine's `send_fleet`, every one arriving within 100
/// hours:
///
/// - a race: Alice's and Bob's colony fleets (a Colony Ship and a Cargo Vessel with cargo) reach
///   one unowned planet at the same millisecond, so the lower fleet ID colonizes and unloads and
///   the other fails; which player assembled first is the seed's choice;
/// - a Transport of ore and ingots from Alice's homeworld to her first colony, unloading
///   partially when its storage is nearly full;
/// - a Colonize by a lone Colony Ship, which empties and removes its fleet;
/// - Bob's Colonize of Alice's first colony, which fails;
/// - Bob's Move with cargo aboard, which stays aboard;
/// - Alice's Colonize that is recalled halfway ([`Economy::start`] returns the recall).
///
/// Alice's planets then sit 50 world units apart along x, Bob and the race target west of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Economy {
    /// Drives every choice.
    pub seed: u64,
    /// Alice's planets, at least 1; with fleets, at least 2.
    pub planets: u64,
    /// Whether fleets fly (P1.8).
    pub fleets: bool,
    /// How many generated commands the run applies (P1.9); 0 for none. With commands, two
    /// unowned homeworld candidates are added.
    pub commands: u64,
}

/// A recall planned for a run: the fleet is recalled once world time reaches `at_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recall {
    /// When.
    pub at_ms: i64,
    /// Which fleet.
    pub fleet: FleetId,
}

impl Economy {
    /// The starting world, solved, with any fleets sent. Equal economies give equal worlds.
    pub fn world(&self, rules: &Rules) -> World {
        self.start(rules).0
    }

    /// The starting world and the recalls planned for its run.
    pub fn start(&self, rules: &Rules) -> (World, Vec<Recall>) {
        let mut b = WorldBuilder::with_rules(rules.clone()).seed(self.seed);
        let mut planets = Vec::new();
        let count = if self.fleets {
            self.planets.max(2)
        } else {
            self.planets.max(1)
        };
        for i in 0..count {
            // Planet 0 is Alice's homeworld (1 Drill, 1 Refinery); the others are her colonies.
            let p = if i == 0 {
                b.player(ALICE)
            } else {
                b.planet(Some(ALICE))
            };
            let r = |k: u64| mix(self.seed ^ i.rotate_left(20) ^ k);
            // Spread out with fleets or commands, so trips take hours (and commands at whole
            // hours can catch a fleet in transit).
            if self.fleets || self.commands > 0 {
                b.position(p, self.spot(i, r(50)));
            }
            // At most 2 + 2 + 1 + 1 + 1 = 7 of the 8 slots.
            let (drills, refineries) = if i == 0 { (0, 0) } else { (r(1) % 3, r(2) % 3) };
            let constructions = if i == 0 { 1 } else { r(3) % 2 };
            let shipyard = i == 0 || r(4) % 3 == 0;
            if i == 0 || r(6) % 4 == 0 {
                let kind = if r(7) % 2 == 0 {
                    BuildingKind::Drill
                } else {
                    BuildingKind::Refinery
                };
                b.demolition(p, kind);
            }
            for _ in 0..drills {
                b.building(p, BuildingKind::Drill, BuildingState::Operational);
            }
            for _ in 0..refineries {
                b.building(p, BuildingKind::Refinery, BuildingState::Operational);
            }
            let kinds = [
                BuildingKind::Drill,
                BuildingKind::Refinery,
                BuildingKind::Shipyard,
            ];
            for k in 0..constructions {
                b.construction(p, kinds[(r(5 + k) % 3) as usize], 0);
            }
            if shipyard {
                let yard = b.building(p, BuildingKind::Shipyard, BuildingState::Operational);
                let builds = if i == 0 { 4 } else { r(8) % 5 };
                for k in 0..builds {
                    let kind = if r(9 + k) % 2 == 0 {
                        ShipKind::CargoVessel
                    } else {
                        ShipKind::ColonyShip
                    };
                    let status = if k < 3 {
                        ShipBuildStatus::Active
                    } else {
                        ShipBuildStatus::Waiting
                    };
                    b.ship_build(yard, kind, status);
                }
            }
            let (ore_cap, ingot_cap) = (rules.planet.ore_cap_g, rules.planet.ingot_cap_g);
            let level = |k: u64, cap: i64| match r(k) % 4 {
                0 => 0,
                1 => cap,
                _ => (r(k + 1) % (cap as u64 / GRAMS_PER_TON as u64)) as i64 * GRAMS_PER_TON,
            };
            let (ore, ingots) = if i == 0 {
                (ore_cap - 10 * GRAMS_PER_TON, ingot_cap - 20 * GRAMS_PER_TON)
            } else {
                (level(20, ore_cap), level(30, ingot_cap))
            };
            let remainder = |k: u64| (r(k) % 3_600_000) as i64;
            b.stocks(
                p,
                Quantity::from_parts(
                    ore,
                    0,
                    if ore == 0 || ore == ore_cap {
                        0
                    } else {
                        remainder(40)
                    },
                    0,
                )
                .expect("remainder in range"),
                Quantity::from_parts(
                    ingots,
                    0,
                    if ingots == 0 || ingots == ingot_cap {
                        0
                    } else {
                        remainder(41)
                    },
                    0,
                )
                .expect("remainder in range"),
            );
            planets.push(p);
        }
        let sends = if self.fleets {
            self.fleets(&mut b, &mut planets, count)
        } else {
            Vec::new()
        };
        if self.commands > 0 {
            let candidates = [b.planet(None), b.planet(None)];
            for (k, &p) in candidates.iter().enumerate() {
                let i = count + 2 + k as u64;
                b.position(p, self.spot(i, mix(self.seed ^ 0xca4d ^ i)));
            }
            b.candidates(&candidates);
        }
        let mut w = b.build();
        for &p in &planets {
            w.solve_planet(p, rules).expect("a generated planet solves");
        }
        let mut recalls = Vec::new();
        for (fleet, target, mission, recall) in sends {
            w.send_fleet(fleet, target, mission, rules)
                .expect("a generated fleet is sent");
            if recall {
                let FleetState::InTransit { arrives_at_ms, .. } = w.fleets[&fleet].state else {
                    unreachable!("sent above")
                };
                recalls.push(Recall {
                    at_ms: arrives_at_ms / 2,
                    fleet,
                });
            }
        }
        (w, recalls)
    }

    /// The command script: (instant, index) pairs in the order they are applied. Instants are
    /// whole hours, where builds started at time 0 complete, so many commands meet an event at
    /// their instant; several can share one.
    pub fn script(&self) -> Vec<(i64, u64)> {
        let mut script: Vec<(i64, u64)> = (0..self.commands)
            .map(|k| {
                let hour = 1 + (mix(self.seed ^ 0xc0de_0000 ^ k) % 99) as i64;
                (hour * voidforge_core::quantity::MS_PER_HOUR, k)
            })
            .collect();
        script.sort_unstable();
        script
    }

    /// Every instant a run must stop at besides its even advances: recalls and commands.
    pub fn instants(&self, recalls: &[Recall]) -> Vec<i64> {
        recalls
            .iter()
            .map(|r| r.at_ms)
            .chain(self.script().into_iter().map(|(t, _)| t))
            .collect()
    }

    /// Command `k` of the script, chosen against the world at its instant (equal in every run
    /// of the economy). The eleven action types cycle in order. Half the time the target is one
    /// the command can act on (any player's), issued, three times in four, by that target's
    /// owner; otherwise the player is mostly Alice, sometimes Bob (joined only with fleets) or
    /// Carol (never joined at the start), and the target is any of its kind or, one time in
    /// eight, an ID nothing holds. Rejections follow from the mix.
    pub fn command(&self, k: u64, w: &World) -> Command {
        use voidforge_core::state::{BuildingId, FleetId, PlayerId, ShipBuildId, ShipId};
        let r = |i: u64| mix(self.seed ^ 0xc0de_0001 ^ (k << 8) ^ i);
        let mut player = [ALICE, ALICE, ALICE, BOB, CAROL][(r(1) % 5) as usize];
        // Picks (ID, owner) from `fitting` half the time, else from `any`, else an unheld ID.
        type Pick = (u64, Option<PlayerId>);
        let pick = |fitting: Vec<Pick>, any: Vec<Pick>, i: u64| -> (u64, Option<PlayerId>) {
            let fit = r(i) % 2 == 0 && !fitting.is_empty();
            let from = if fit { fitting } else { any };
            if from.is_empty() || (!fit && r(i + 1) % 8 == 0) {
                (w.next_id + r(i + 2) % 3, None)
            } else {
                let (id, owner) = from[(r(i + 3) % from.len() as u64) as usize];
                (id, if fit { owner } else { None })
            }
        };
        let owner_of = |planet: PlanetId| w.planets[&planet].owner;
        let buildings = |f: &dyn Fn(&voidforge_core::state::Building) -> bool| {
            w.buildings
                .values()
                .filter(|b| f(b))
                .map(|b| (b.id.0, owner_of(b.planet)))
                .collect::<Vec<_>>()
        };
        let fleets = |f: &dyn Fn(&voidforge_core::state::Fleet) -> bool| {
            w.fleets
                .values()
                .filter(|x| f(x))
                .map(|x| (x.id.0, Some(x.owner)))
                .collect::<Vec<_>>()
        };
        let all_fleets = fleets(&|_| true);
        let building_kind = [
            BuildingKind::Drill,
            BuildingKind::Refinery,
            BuildingKind::Shipyard,
        ][(r(4) % 3) as usize];
        let ship_kind = [ShipKind::CargoVessel, ShipKind::ColonyShip][(r(5) % 2) as usize];
        let planets: Vec<PlanetId> = w.planets.keys().copied().collect();
        let any_planet = |i: u64| planets[(r(i) % planets.len() as u64) as usize];
        let (action, fitted) = match k % 11 {
            0 => (Action::JoinGame, None),
            1 => {
                let owned = |only: bool| {
                    w.planets
                        .values()
                        .filter(|p| !only || p.owner.is_some())
                        .map(|p| (p.id.0, p.owner))
                        .collect::<Vec<_>>()
                };
                let (id, fit) = pick(owned(true), owned(false), 10);
                let action = Action::BuildBuilding {
                    planet: PlanetId(id),
                    kind: building_kind,
                };
                (action, fit)
            }
            2 => {
                let fitting =
                    buildings(&|b| matches!(b.state, BuildingState::UnderConstruction { .. }));
                let (id, fit) = pick(fitting, buildings(&|_| true), 20);
                let building = BuildingId(id);
                (Action::CancelConstruction { building }, fit)
            }
            3 => {
                // Shipyards are demolished only through `any`, so most runs keep one to queue at.
                let fitting = buildings(&|b| {
                    b.state == BuildingState::Operational && b.kind != BuildingKind::Shipyard
                });
                let (id, fit) = pick(fitting, buildings(&|_| true), 30);
                let building = BuildingId(id);
                (Action::DemolishBuilding { building }, fit)
            }
            4 => {
                let fitting = buildings(&|b| {
                    b.kind == BuildingKind::Shipyard && b.state == BuildingState::Operational
                });
                let (id, fit) = pick(fitting, buildings(&|_| true), 40);
                let action = Action::QueueShip {
                    shipyard: BuildingId(id),
                    kind: ship_kind,
                };
                (action, fit)
            }
            5 => {
                let builds: Vec<_> = w
                    .ship_builds
                    .values()
                    .map(|s| (s.id.0, owner_of(w.buildings[&s.shipyard].planet)))
                    .collect();
                let (id, fit) = pick(builds.clone(), builds, 50);
                let build = ShipBuildId(id);
                (Action::CancelShipBuild { build }, fit)
            }
            6 => {
                // Fitting: up to three of one owner's ships on one roster, with cargo that fits
                // the stock and the capacity when the planet is theirs. Otherwise any ships
                // (perhaps in fleets or elsewhere) and any cargo, perhaps too much or negative.
                let rosters: Vec<_> = w
                    .planets
                    .values()
                    .filter_map(|p| {
                        let first = w.ships.get(p.roster.iter().next()?)?;
                        Some((p.id, first.owner))
                    })
                    .collect();
                let fit = r(60) % 2 == 0 && !rosters.is_empty();
                if fit {
                    let (planet, owner) = rosters[(r(61) % rosters.len() as u64) as usize];
                    let p = &w.planets[&planet];
                    let ships: BTreeSet<ShipId> = p
                        .roster
                        .iter()
                        .copied()
                        .filter(|s| w.ships[s].owner == owner)
                        .take(1 + (r(62) % 3) as usize)
                        .collect();
                    // Rules v0's capacities; under other rules a load may be rejected, which is fine.
                    let capacity: i64 = ships
                        .iter()
                        .map(|s| match w.ships[s].kind {
                            ShipKind::CargoVessel => 500 * GRAMS_PER_TON,
                            ShipKind::ColonyShip => 0,
                        })
                        .sum();
                    let now = w.time_ms;
                    let cargo = if p.owner == Some(owner) && capacity > 0 && r(63) % 2 == 0 {
                        let ore = p.ore.value_at(now).unwrap_or(0).min(capacity / 2);
                        let ingots = p.ingots.value_at(now).unwrap_or(0).min(capacity - ore);
                        Cargo {
                            ore_g: ore * (1 + (r(64) % 4) as i64) / 4,
                            ingots_g: ingots * (1 + (r(65) % 4) as i64) / 4,
                        }
                    } else {
                        Cargo::default()
                    };
                    let action = Action::AssembleFleet {
                        planet,
                        ships,
                        cargo,
                    };
                    (action, Some(owner))
                } else {
                    let ids: Vec<ShipId> = w.ships.keys().copied().collect();
                    let ships = (0..(r(66) % 3))
                        .map(|i| match ids.len() {
                            0 => ShipId(w.next_id),
                            n => ids[(r(67 + i) % n as u64) as usize],
                        })
                        .collect();
                    let tons = (r(70) % 2_000) as i64 * GRAMS_PER_TON - 100 * GRAMS_PER_TON;
                    let cargo = Cargo {
                        ore_g: tons,
                        ingots_g: (r(71) % 600) as i64 * GRAMS_PER_TON,
                    };
                    let action = Action::AssembleFleet {
                        planet: any_planet(72),
                        ships,
                        cargo,
                    };
                    (action, None)
                }
            }
            7 => {
                // Fitting: a stationed fleet, on a mission its ships and the target allow.
                let stationed = fleets(&|f| matches!(f.state, FleetState::Stationed { .. }));
                let (id, fit) = pick(stationed, all_fleets.clone(), 80);
                let fleet = FleetId(id);
                let (target, mission) = match (fit, w.fleets.get(&fleet)) {
                    (Some(owner), Some(f)) => {
                        let FleetState::Stationed { at } = f.state else {
                            unreachable!("picked stationed")
                        };
                        let has = |kind| f.ships.iter().any(|s| w.ships[s].kind == kind);
                        let others = |want: &dyn Fn(Option<PlayerId>) -> bool| {
                            planets
                                .iter()
                                .copied()
                                .filter(|&p| p != at && want(owner_of(p)))
                                .collect::<Vec<_>>()
                        };
                        let free = others(&|o| o.is_none());
                        let own = others(&|o| o == Some(owner));
                        let one = |v: &[PlanetId], i| v[(r(i) % v.len() as u64) as usize];
                        if has(ShipKind::ColonyShip) && !free.is_empty() && r(81) % 2 == 0 {
                            (one(&free, 82), Mission::Colonize)
                        } else if has(ShipKind::CargoVessel) && !own.is_empty() && r(83) % 2 == 0 {
                            (one(&own, 84), Mission::Transport)
                        } else {
                            let any = others(&|_| true);
                            match any.is_empty() {
                                true => (at, Mission::Move),
                                false => (one(&any, 85), Mission::Move),
                            }
                        }
                    }
                    _ => {
                        let mission = [Mission::Colonize, Mission::Transport, Mission::Move]
                            [(r(86) % 3) as usize];
                        (any_planet(87), mission)
                    }
                };
                let action = Action::SendFleet {
                    fleet,
                    target,
                    mission,
                };
                (action, fit)
            }
            8 => {
                let moving = fleets(&|f| matches!(f.state, FleetState::InTransit { .. }));
                let (id, fit) = pick(moving, all_fleets, 90);
                let fleet = FleetId(id);
                (Action::RecallFleet { fleet }, fit)
            }
            9 => {
                let empty = fleets(&|f| {
                    matches!(f.state, FleetState::Stationed { .. }) && f.cargo == Cargo::default()
                });
                let (id, fit) = pick(empty, all_fleets, 100);
                let fleet = FleetId(id);
                (Action::DisbandFleet { fleet }, fit)
            }
            _ => {
                let at_home = fleets(&|f| match f.state {
                    FleetState::Stationed { at } => owner_of(at) == Some(f.owner),
                    _ => false,
                });
                let (id, fit) = pick(at_home, all_fleets, 110);
                let fleet = FleetId(id);
                (Action::TriggerUnload { fleet }, fit)
            }
        };
        if let Some(owner) = fitted
            && r(6) % 4 != 0
        {
            player = owner;
        }
        Command { player, action }
    }

    /// Where Alice's planet `i` sits with fleets: 50 world units apart along x, scattered over
    /// 200 in y and z, so trips take hours but end within 100.
    fn spot(&self, i: u64, r: u64) -> Position {
        let u = voidforge_core::rules::DISTANCE_UNITS_PER_WORLD_UNIT;
        Position {
            x: (i as i64 * 50 + (r % 40) as i64) * u,
            y: ((r >> 16) % 200) as i64 * u,
            z: ((r >> 32) % 200) as i64 * u,
        }
    }

    /// Adds Bob, the unowned planets, the ships and the fleets; returns the sends, each with
    /// whether it is recalled. `planets` gains the new planets so they are solved.
    fn fleets(
        &self,
        b: &mut WorldBuilder,
        planets: &mut Vec<PlanetId>,
        count: u64,
    ) -> Vec<(FleetId, PlanetId, Mission, bool)> {
        let r = |k: u64| mix(self.seed ^ 0xf1ee_7000 ^ k);
        let tons = |k: u64, most: u64| (1 + r(k) % most) as i64 * GRAMS_PER_TON;
        let u = voidforge_core::rules::DISTANCE_UNITS_PER_WORLD_UNIT;
        let (alice, colony) = (planets[0], planets[1]);
        let home = b.world().planets[&alice].position;

        // The race: Bob's homeworld mirrors Alice's across the target, west of every other planet.
        let d = (100 + r(1) % 100) as i64 * u;
        let bob = b.player(BOB);
        let race = b.planet(None);
        b.position(
            race,
            Position {
                x: home.x - d,
                ..home
            },
        );
        b.position(
            bob,
            Position {
                x: home.x - 2 * d,
                ..home
            },
        );
        let spare = [b.planet(None), b.planet(None)];
        for (k, &p) in spare.iter().enumerate() {
            b.position(p, self.spot(count + k as u64, r(2 + k as u64)));
        }
        planets.extend([bob, race, spare[0], spare[1]]);

        use ShipKind::{CargoVessel, ColonyShip};
        let racers = |b: &mut WorldBuilder, owner, at, k| {
            let cargo = Cargo {
                ore_g: tons(k, 200),
                ingots_g: tons(k + 1, 300),
            };
            fleet(b, owner, at, &[ColonyShip, CargoVessel], cargo)
        };
        let (alice_racer, bob_racer) = if r(10) % 2 == 0 {
            let a = racers(b, ALICE, alice, 11);
            (a, racers(b, BOB, bob, 13))
        } else {
            let bob_racer = racers(b, BOB, bob, 13);
            (racers(b, ALICE, alice, 11), bob_racer)
        };
        let hauler = Cargo {
            ore_g: tons(20, 250),
            ingots_g: tons(21, 250),
        };
        let transport = fleet(b, ALICE, alice, &[CargoVessel], hauler);
        let lone = fleet(b, ALICE, alice, &[ColonyShip], Cargo::default());
        let refused = fleet(b, BOB, bob, &[ColonyShip], Cargo::default());
        let mover = Cargo {
            ore_g: tons(30, 100),
            ingots_g: 0,
        };
        let moving = fleet(b, BOB, bob, &[CargoVessel], mover);
        let recalled = fleet(b, ALICE, alice, &[ColonyShip, CargoVessel], hauler);
        vec![
            (alice_racer, race, Mission::Colonize, false),
            (bob_racer, race, Mission::Colonize, false),
            (transport, colony, Mission::Transport, false),
            (lone, spare[0], Mission::Colonize, false),
            (refused, colony, Mission::Colonize, false),
            (moving, spare[1], Mission::Move, false),
            (recalled, spare[1], Mission::Colonize, true),
        ]
    }
}

/// New ships of `kinds` on a planet's roster, assembled into a fleet carrying `cargo`.
fn fleet(
    b: &mut WorldBuilder,
    owner: voidforge_core::state::PlayerId,
    at: PlanetId,
    kinds: &[ShipKind],
    cargo: Cargo,
) -> FleetId {
    let ships: Vec<_> = kinds.iter().map(|&k| b.ship(owner, k, at)).collect();
    b.fleet(&ships, cargo)
}

/// A finished economy run: the world, every domain event in order, and each command with its
/// outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EconomyRun {
    /// The world at the end.
    pub world: World,
    /// Every domain event: of advances, then of each accepted command, in order.
    pub events: Vec<DomainEvent>,
    /// Each command applied, accepted or rejected with its reason.
    pub outcomes: Vec<(Command, Result<(), Rejection>)>,
}

/// An economy run to `to_ms` through production `advance_to`, in `advances` equal advances (the
/// last takes the rest), also stopping at each planned recall and command (applied in that
/// order at their instant). `observe` reads the world after each stop and after each command.
/// A rejected command must leave the world's bytes as they were.
pub fn run_economy(
    e: &Economy,
    rules: &Rules,
    to_ms: i64,
    advances: i64,
    mut observe: impl FnMut(&World),
) -> Result<EconomyRun, EngineError> {
    let (mut world, recalls) = e.start(rules);
    let script = e.script();
    let mut events = Vec::new();
    let mut outcomes = Vec::new();
    for t in stops(to_ms, advances, &e.instants(&recalls)) {
        events.extend(world.advance_to(t, rules)?);
        recall_due(&mut world, &recalls)?;
        for &(_, k) in script.iter().filter(|(at, _)| *at == t) {
            let command = e.command(k, &world);
            let before = canonical_bytes(&world);
            let applied = world.apply(&command, t, rules)?;
            events.extend(applied.advanced);
            match applied.outcome {
                Ok(produced) => {
                    events.extend(produced);
                    outcomes.push((command, Ok(())));
                }
                Err(rejection) => {
                    assert!(
                        canonical_bytes(&world) == before,
                        "rejected {command:?} changed the world"
                    );
                    outcomes.push((command, Err(rejection)));
                }
            }
            observe(&world);
        }
        observe(&world);
    }
    Ok(EconomyRun {
        world,
        events,
        outcomes,
    })
}

/// The instants a run stops at: `advances` equal advances to `to_ms` (the last takes the rest)
/// and every extra instant up to `to_ms` (recalls, commands), so those happen at the same
/// instant however a run is split.
pub fn stops(to_ms: i64, advances: i64, instants: &[i64]) -> Vec<i64> {
    let mut stops: Vec<i64> = (1..=advances)
        .map(|k| to_ms * k / advances)
        .chain(instants.iter().copied().filter(|&t| t <= to_ms))
        .collect();
    stops.sort_unstable();
    stops.dedup();
    stops
}

/// Recalls the fleets planned for the current world time, as a command at that instant would.
/// A fleet that generated commands (P1.10) have already recalled or otherwise taken out of
/// transit is left alone, as the command would be rejected.
pub fn recall_due(w: &mut World, recalls: &[Recall]) -> Result<(), EngineError> {
    let now = w.time_ms;
    for r in recalls.iter().filter(|r| r.at_ms == now) {
        let moving = w
            .fleets
            .get(&r.fleet)
            .is_some_and(|f| matches!(f.state, FleetState::InTransit { .. }));
        if moving {
            w.recall_fleet(r.fleet)?;
        }
    }
    Ok(())
}

/// The invariants transitions must keep (P1.7): no planet holds more buildings than its slots;
/// every ship is in exactly one place, one roster or one fleet; and no Shipyard has more active
/// builds than the rules allow.
pub fn assert_transition_invariants(w: &World, rules: &Rules) {
    for p in w.planets.values() {
        assert!(
            p.buildings.len() <= p.slots as usize,
            "planet {} over its slots",
            p.id.0
        );
    }
    let mut places: BTreeMap<u64, usize> = BTreeMap::new();
    for p in w.planets.values() {
        for s in &p.roster {
            *places.entry(s.0).or_default() += 1;
        }
    }
    for f in w.fleets.values() {
        for s in &f.ships {
            *places.entry(s.0).or_default() += 1;
        }
    }
    for s in w.ships.keys() {
        assert_eq!(
            places.get(&s.0),
            Some(&1),
            "ship {} is not in exactly one place",
            s.0
        );
    }
    assert_eq!(
        places.len(),
        w.ships.len(),
        "a roster or fleet lists a ship that does not exist"
    );
    let mut active: BTreeMap<u64, u32> = BTreeMap::new();
    for b in w.ship_builds.values() {
        if b.status == ShipBuildStatus::Active {
            *active.entry(b.shipyard.0).or_default() += 1;
        }
    }
    for (yard, n) in active {
        assert!(
            n <= rules.buildings.shipyard.parallel_builds,
            "Shipyard {yard} has {n} active builds"
        );
    }
}

/// The invariants fleets must keep (P1.8): cargo is never negative and never above the fleet's
/// capacity, and every fleet has ships.
pub fn assert_fleet_invariants(w: &World, rules: &Rules) {
    for f in w.fleets.values() {
        assert!(!f.ships.is_empty(), "fleet {} has no ships", f.id.0);
        let capacity = w.fleet_capacity(f.id, rules).unwrap();
        let aboard = f.cargo.ore_g + f.cargo.ingots_g;
        assert!(
            f.cargo.ore_g >= 0 && f.cargo.ingots_g >= 0 && aboard <= capacity,
            "fleet {}: cargo {:?} against capacity {}",
            f.id.0,
            f.cargo,
            capacity
        );
    }
}

/// Ships are conserved: the ships at the end are those at the start, plus every ship completed,
/// less every Colony Ship consumed by a colonization. Commands move ships between rosters and
/// fleets but never create or destroy one.
pub fn assert_ships_conserved(start: &World, end: &World, events: &[DomainEvent]) {
    let completed = events
        .iter()
        .filter(|e| matches!(e, DomainEvent::ShipCompleted { .. }))
        .count();
    let consumed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            DomainEvent::PlanetColonized { ship, .. } => Some(*ship),
            _ => None,
        })
        .collect();
    // A consumed ship was a Colony Ship at the start or completed as one during the run (and
    // then, with commands, assembled and sent), and it is gone at the end.
    let colony_ships: BTreeSet<_> = start
        .ships
        .values()
        .filter(|s| s.kind == ShipKind::ColonyShip)
        .map(|s| s.id)
        .chain(events.iter().filter_map(|e| match e {
            DomainEvent::ShipCompleted {
                ship,
                kind: ShipKind::ColonyShip,
                ..
            } => Some(*ship),
            _ => None,
        }))
        .collect();
    for ship in &consumed {
        assert!(
            colony_ships.contains(ship) && !end.ships.contains_key(ship),
            "ship {} was consumed but is not a Colony Ship that left",
            ship.0
        );
    }
    assert_eq!(
        end.ships.len(),
        start.ships.len() + completed - consumed.len(),
        "ships are not conserved"
    );
}

/// How many times the economy's planets were solved past the first solve: one version per solve.
pub fn solves(start: &World, end: &World) -> u64 {
    end.planets
        .values()
        .map(|p| p.version - start.planets[&p.id].version)
        .sum()
}
