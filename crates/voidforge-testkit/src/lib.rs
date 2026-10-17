//! Test builders and golden logs, dev only (decision log D-022).
//!
//! [`WorldBuilder`] creates every entity with valid defaults taken from the rules (Rules v0
//! unless told otherwise) and lets a test override what it cares about. It allocates IDs from
//! the world's own counter, so they are predictable, keeps both sides of every link in step, and
//! [`WorldBuilder::build`] checks the links before handing the world over. Misuse panics with a
//! message, because these are test helpers. [`busy_world`] is a ready-made world with every
//! entity, state and queue kind.

use voidforge_core::quantity::{MS_PER_HOUR, Quantity};
use voidforge_core::rules::{BuildCost, Rules};
use voidforge_core::state::{
    Building, BuildingId, BuildingKind, BuildingState, Cargo, Fleet, FleetId, FleetState, Mission,
    Planet, PlanetId, Player, PlayerId, Position, Ship, ShipBuild, ShipBuildId, ShipBuildStatus,
    ShipId, ShipKind, ShipLocation, StateError, World,
};

/// The text of `rules/v0.toml`.
pub const V0_RULES: &str = include_str!("../../../rules/v0.toml");

/// Rules v0, parsed and validated.
pub fn v0_rules() -> Rules {
    voidforge_rules::parse(V0_RULES).expect("rules/v0.toml is valid")
}

/// Builds worlds for tests. See the crate documentation.
#[derive(Debug, Clone)]
pub struct WorldBuilder {
    world: World,
    rules: Rules,
}

impl Default for WorldBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldBuilder {
    /// An empty world at time 0 under Rules v0, seed 0.
    pub fn new() -> Self {
        Self::with_rules(v0_rules())
    }

    /// An empty world at time 0 under `rules`.
    pub fn with_rules(rules: Rules) -> Self {
        Self {
            world: World::new(0, rules.version, 0),
            rules,
        }
    }

    /// Sets the world time; entities created afterwards are checkpointed there.
    pub fn at(mut self, time_ms: i64) -> Self {
        self.world.time_ms = time_ms;
        self
    }

    /// Sets the universe seed.
    pub fn seed(mut self, seed: u64) -> Self {
        self.world.seed = seed;
        self
    }

    /// Starts the ID counter at `next_id`, for tests that need particular IDs.
    pub fn next_id(mut self, next_id: u64) -> Self {
        self.world.next_id = next_id;
        self
    }

    /// The rules this builder takes defaults from.
    pub fn rules(&self) -> &Rules {
        &self.rules
    }

    /// The world built so far.
    pub fn world(&self) -> &World {
        &self.world
    }

    fn id(&mut self) -> u64 {
        self.world
            .allocate_id()
            .expect("the test world has IDs left")
    }

    fn now(&self) -> i64 {
        self.world.time_ms
    }

    /// A new player with a homeworld as Join game would set it up: an owned planet with the
    /// rules' operational Drills and Refineries and starting stocks. Returns the homeworld.
    pub fn player(&mut self, id: PlayerId) -> PlanetId {
        assert!(
            !self.world.players.contains_key(&id),
            "player {id:?} already exists"
        );
        let home = self.rules.homeworld;
        let planet = self.planet(Some(id));
        self.world.players.insert(
            id,
            Player {
                id,
                homeworld: planet,
            },
        );
        for _ in 0..home.drills {
            self.building(planet, BuildingKind::Drill, BuildingState::Operational);
        }
        for _ in 0..home.refineries {
            self.building(planet, BuildingKind::Refinery, BuildingState::Operational);
        }
        let now = self.now();
        self.stocks(
            planet,
            Quantity::new(home.ore_g, 0, now),
            Quantity::new(home.ingots_g, 0, now),
        );
        planet
    }

    /// An empty planet with the rules' slots and caps and no stock, at a position derived from
    /// its ID (one world unit apart). `owner` must already be a player, or be added with
    /// [`player`](Self::player) before [`build`](Self::build).
    pub fn planet(&mut self, owner: Option<PlayerId>) -> PlanetId {
        let id = PlanetId(self.id());
        let spec = self.rules.planet;
        let now = self.now();
        let step = i64::try_from(id.0).expect("small test IDs")
            * voidforge_core::rules::DISTANCE_UNITS_PER_WORLD_UNIT;
        self.world.planets.insert(
            id,
            Planet {
                id,
                position: Position {
                    x: step,
                    y: 0,
                    z: 0,
                },
                owner,
                slots: spec.slots,
                ore_cap_g: spec.ore_cap_g,
                ingot_cap_g: spec.ingot_cap_g,
                ore: Quantity::new(0, 0, now),
                ingots: Quantity::new(0, 0, now),
                buildings: Default::default(),
                roster: Default::default(),
                version: 0,
                next_event: None,
            },
        );
        id
    }

    /// Moves a planet.
    pub fn position(&mut self, planet: PlanetId, position: Position) {
        self.planet_mut(planet).position = position;
    }

    /// Replaces a planet's stocks.
    pub fn stocks(&mut self, planet: PlanetId, ore: Quantity, ingots: Quantity) {
        let p = self.planet_mut(planet);
        p.ore = ore;
        p.ingots = ingots;
    }

    /// Adds a building in the given state.
    pub fn building(
        &mut self,
        planet: PlanetId,
        kind: BuildingKind,
        state: BuildingState,
    ) -> BuildingId {
        let id = BuildingId(self.id());
        self.planet_mut(planet).buildings.insert(id);
        self.world.buildings.insert(
            id,
            Building {
                id,
                planet,
                kind,
                state,
            },
        );
        id
    }

    /// Adds a building under construction with `delivered_g` delivered so far, on the rules'
    /// cost and rated rate, delivering at that rate from now.
    pub fn construction(
        &mut self,
        planet: PlanetId,
        kind: BuildingKind,
        delivered_g: i64,
    ) -> BuildingId {
        let build = self.building_cost(kind);
        let rate = rate(build);
        let state = BuildingState::UnderConstruction {
            delivered: Quantity::new(delivered_g, rate, self.now()),
            cost_g: build.cost_g,
            rated_g_per_h: rate,
        };
        self.building(planet, kind, state)
    }

    /// Adds a building being demolished, ending after the rules' demolition time from now.
    pub fn demolition(&mut self, planet: PlanetId, kind: BuildingKind) -> BuildingId {
        let b = self.rules.buildings;
        let duration = match kind {
            BuildingKind::Drill => b.drill.demolition_time_ms,
            BuildingKind::Refinery => b.refinery.demolition_time_ms,
            BuildingKind::Shipyard => b.shipyard.demolition_time_ms,
        };
        let ends_at_ms = self.now() + duration;
        self.building(planet, kind, BuildingState::Demolishing { ends_at_ms })
    }

    /// Queues a ship at an operational Shipyard on the rules' cost and rated rate. An active
    /// build delivers at that rate from now; a waiting one consumes nothing.
    pub fn ship_build(
        &mut self,
        shipyard: BuildingId,
        kind: ShipKind,
        status: ShipBuildStatus,
    ) -> ShipBuildId {
        let b = self
            .world
            .buildings
            .get(&shipyard)
            .expect("the shipyard exists");
        assert!(
            b.kind == BuildingKind::Shipyard && b.state == BuildingState::Operational,
            "ship builds need an operational Shipyard, not {b:?}"
        );
        let id = ShipBuildId(self.id());
        let build = self.ship_cost(kind);
        let rated = rate(build);
        let flowing = if status == ShipBuildStatus::Active {
            rated
        } else {
            0
        };
        self.world.ship_builds.insert(
            id,
            ShipBuild {
                id,
                shipyard,
                kind,
                delivered: Quantity::new(0, flowing, self.now()),
                cost_g: build.cost_g,
                rated_g_per_h: rated,
                status,
            },
        );
        id
    }

    /// A finished ship on a planet's roster.
    pub fn ship(&mut self, owner: PlayerId, kind: ShipKind, planet: PlanetId) -> ShipId {
        let id = ShipId(self.id());
        self.planet_mut(planet).roster.insert(id);
        self.world.ships.insert(
            id,
            Ship {
                id,
                kind,
                owner,
                location: ShipLocation::Roster(planet),
            },
        );
        id
    }

    /// Assembles ships from one planet's roster into a fleet stationed there, with `cargo`.
    pub fn fleet(&mut self, ships: &[ShipId], cargo: Cargo) -> FleetId {
        let first = *ships.first().expect("a fleet needs ships");
        let (owner, at) = match self.world.ships[&first] {
            Ship {
                owner,
                location: ShipLocation::Roster(at),
                ..
            } => (owner, at),
            ref other => panic!("ship {} is not on a roster: {other:?}", first.0),
        };
        let id = FleetId(self.id());
        for ship in ships {
            let s = self.world.ships.get_mut(ship).expect("the ship exists");
            assert_eq!(
                (s.owner, s.location),
                (owner, ShipLocation::Roster(at)),
                "ship {} is elsewhere",
                ship.0
            );
            s.location = ShipLocation::Fleet(id);
            self.planet_mut(at).roster.remove(ship);
        }
        self.world.fleets.insert(
            id,
            Fleet {
                id,
                owner,
                ships: ships.iter().copied().collect(),
                cargo,
                version: 0,
                state: FleetState::Stationed { at },
            },
        );
        id
    }

    /// Sends a stationed fleet: departing now, arriving at `arrives_at_ms`, with its arrival
    /// queued through [`World::schedule_fleet`] (which bumps its version).
    pub fn send(&mut self, fleet: FleetId, target: PlanetId, mission: Mission, arrives_at_ms: i64) {
        let now = self.now();
        let f = self.world.fleets.get_mut(&fleet).expect("the fleet exists");
        let FleetState::Stationed { at } = f.state else {
            panic!("fleet {} is not stationed", fleet.0)
        };
        f.state = FleetState::InTransit {
            origin: at,
            target,
            mission,
            departed_at_ms: now,
            arrives_at_ms,
        };
        self.world
            .schedule_fleet(fleet)
            .expect("the arrival is after now");
    }

    /// Recalls a fleet in transit: it returns in the time already traveled, with its version
    /// bumped so the old arrival entry goes stale.
    pub fn recall(&mut self, fleet: FleetId) {
        let now = self.now();
        let f = self.world.fleets.get_mut(&fleet).expect("the fleet exists");
        let FleetState::InTransit {
            origin,
            departed_at_ms,
            ..
        } = f.state
        else {
            panic!("fleet {} is not in transit", fleet.0)
        };
        let arrives_at_ms = now + (now - departed_at_ms);
        f.state = FleetState::Returning {
            origin,
            arrives_at_ms,
        };
        self.world
            .schedule_fleet(fleet)
            .expect("the return is after now");
    }

    /// Schedules a planet's next event through [`World::schedule_planet`]: its version goes up and
    /// an earlier entry for the planet stays in the queue as stale.
    pub fn schedule(&mut self, planet: PlanetId, at_ms: i64) {
        self.world
            .schedule_planet(planet, Some(at_ms))
            .expect("the event is after now");
    }

    /// Appends homeworld candidates in order.
    pub fn candidates(&mut self, planets: &[PlanetId]) {
        self.world.homeworld_candidates.extend_from_slice(planets);
    }

    /// The world, after checking its links.
    pub fn build(self) -> World {
        self.try_build().expect("the test world is consistent")
    }

    /// The world, or the first broken link.
    pub fn try_build(self) -> Result<World, StateError> {
        self.world.check_links()?;
        Ok(self.world)
    }

    fn planet_mut(&mut self, planet: PlanetId) -> &mut Planet {
        self.world
            .planets
            .get_mut(&planet)
            .unwrap_or_else(|| panic!("planet {} does not exist", planet.0))
    }

    fn building_cost(&self, kind: BuildingKind) -> BuildCost {
        let b = self.rules.buildings;
        match kind {
            BuildingKind::Drill => b.drill.build,
            BuildingKind::Refinery => b.refinery.build,
            BuildingKind::Shipyard => b.shipyard.build,
        }
    }

    fn ship_cost(&self, kind: ShipKind) -> BuildCost {
        match kind {
            ShipKind::ColonyShip => self.rules.ships.colony_ship.build,
            ShipKind::CargoVessel => self.rules.ships.cargo_vessel.build,
        }
    }
}

fn rate(build: BuildCost) -> i64 {
    build
        .rate_g_per_h()
        .expect("validated rules have exact build rates")
}

/// The first test player.
pub const ALICE: PlayerId = PlayerId(0x0192_8c5e_7a3b_7c2d_9e4f_5a6b_7c8d_9e0f);
/// The second test player.
pub const BOB: PlayerId = PlayerId(0x0192_8c5e_7a3b_7c2d_9e4f_5a6b_7c8d_9e10);

/// A world one hour in (built from half an hour in, so a fleet can be recalled mid-flight), with
/// two players and every entity, state and queue kind: homeworlds,
/// an unowned planet and a colony, buildings operational, under construction and being
/// demolished, active and waiting ship builds, ships on rosters and in stationed, in-transit (every
/// mission) and returning fleets, a planet event with a stale older entry, and homeworld candidates.
pub fn busy_world() -> World {
    let mut b = WorldBuilder::new().at(MS_PER_HOUR / 2).seed(42);
    let alice_home = b.player(ALICE);
    let bob_home = b.player(BOB);
    let free = b.planet(None);
    let colony = b.planet(Some(ALICE));
    b.position(
        free,
        Position {
            x: 250_500,
            y: -4_000,
            z: 990_000,
        },
    );

    let shipyard = b.building(
        alice_home,
        BuildingKind::Shipyard,
        BuildingState::Operational,
    );
    b.construction(alice_home, BuildingKind::Drill, 50_000_000);
    b.demolition(bob_home, BuildingKind::Refinery);
    b.ship_build(shipyard, ShipKind::ColonyShip, ShipBuildStatus::Active);
    b.ship_build(shipyard, ShipKind::CargoVessel, ShipBuildStatus::Waiting);

    let colony_ship = b.ship(ALICE, ShipKind::ColonyShip, alice_home);
    let cargo_a = b.ship(ALICE, ShipKind::CargoVessel, alice_home);
    let cargo_b = b.ship(ALICE, ShipKind::CargoVessel, alice_home);
    let cargo_c = b.ship(ALICE, ShipKind::CargoVessel, alice_home);
    b.ship(BOB, ShipKind::CargoVessel, free); // Bob's ship waits on an unowned planet's roster
    let bob_mover = b.ship(BOB, ShipKind::CargoVessel, free);
    b.ship(ALICE, ShipKind::ColonyShip, alice_home); // stays on the roster

    let colonizers = b.fleet(
        &[colony_ship, cargo_a],
        Cargo {
            ore_g: 0,
            ingots_g: 400_000_000,
        },
    );
    b.send(colonizers, free, Mission::Colonize, 5 * MS_PER_HOUR);
    let haulers = b.fleet(
        &[cargo_b],
        Cargo {
            ore_g: 100_000_000,
            ingots_g: 0,
        },
    );
    b.send(haulers, colony, Mission::Transport, 3 * MS_PER_HOUR);
    let mut b = b.at(MS_PER_HOUR);
    b.recall(haulers); // returns at 1.5 h; its arrival entry goes stale
    let supply = b.fleet(
        &[cargo_c],
        Cargo {
            ore_g: 0,
            ingots_g: 250_000_000,
        },
    );
    b.send(supply, colony, Mission::Transport, 6 * MS_PER_HOUR); // stays in transit
    let bob_ship = b.ship(BOB, ShipKind::ColonyShip, bob_home);
    b.fleet(&[bob_ship], Cargo::default()); // stationed
    let movers = b.fleet(&[bob_mover], Cargo::default());
    b.send(movers, bob_home, Mission::Move, 4 * MS_PER_HOUR);

    b.schedule(alice_home, 2 * MS_PER_HOUR);
    b.schedule(alice_home, 90 * 60_000); // replaces the first; that entry goes stale
    b.candidates(&[free, alice_home, bob_home]);
    b.build()
}

#[cfg(test)]
mod tests;
