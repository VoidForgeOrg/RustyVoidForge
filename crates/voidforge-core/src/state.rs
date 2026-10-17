//! The engine's state: one [`World`] per universe, holding every entity in ID-ordered maps
//! (Architectural Design, State model and Engine core; decision log D-022).
//!
//! Entities refer to each other by integer ID, never by pointer, and every collection iterates
//! in ID order. New entities take the next value of one global counter, `World::next_id`; player
//! IDs are UUIDs chosen outside the core and arrive in commands. The child of a relationship
//! holds the truth (a building's planet, a ship's location) and the parent keeps an ordered set
//! of child IDs for lookup. [`World::check_links`] checks both directions and runs whenever a
//! world is deserialized.
//!
//! The event queue is private: [`World::schedule_planet`] and [`World::schedule_fleet`] are the
//! only way in, and [`World::advance_to`] the way out (the `engine` submodule, decision log
//! D-023). The solver, fleet missions and commands come with later packages.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize, Serializer};

use crate::quantity::Quantity;

/// An external player ID: the 128 bits of the player's UUIDv7, assigned at sign-up outside the
/// core and logged with the Join game command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub u128);

macro_rules! entity_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub u64);
    };
}

entity_id!(
    /// A planet, from the world's counter.
    PlanetId
);
entity_id!(
    /// A building, from the world's counter.
    BuildingId
);
entity_id!(
    /// A ship build in a Shipyard, from the world's counter.
    ShipBuildId
);
entity_id!(
    /// A ship, from the world's counter.
    ShipId
);
entity_id!(
    /// A fleet, from the world's counter.
    FleetId
);

/// A position in distance units (1 world unit = 1,000; decision log D-021).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    /// x, in distance units.
    pub x: i64,
    /// y, in distance units.
    pub y: i64,
    /// z, in distance units.
    pub z: i64,
}

/// A player: created by Join game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Player {
    /// The external player ID.
    pub id: PlayerId,
    /// The planet the player started on; owned by them for good.
    pub homeworld: PlanetId,
}

/// A planet and what is on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Planet {
    /// Its ID.
    pub id: PlanetId,
    /// Where it is.
    pub position: Position,
    /// Its owner; `None` until colonized, and ownership is permanent in the MVP.
    pub owner: Option<PlayerId>,
    /// Identical building slots.
    pub slots: u32,
    /// Iron Ore storage capacity in grams.
    pub ore_cap_g: i64,
    /// Iron Ingot storage capacity in grams.
    pub ingot_cap_g: i64,
    /// Iron Ore in storage.
    pub ore: Quantity,
    /// Iron Ingots in storage.
    pub ingots: Quantity,
    /// Buildings on the planet, counting those under construction or being demolished.
    pub buildings: BTreeSet<BuildingId>,
    /// Ships on the planet's roster, any owner's.
    pub roster: BTreeSet<ShipId>,
    /// Increases on every re-solve; queue entries carrying an older version are stale.
    pub version: u64,
    /// The planet's next event, if anything on it moves: stored with the planet so a restored
    /// snapshot needs no re-solve and a view can give `valid_until` without searching the queue.
    /// The queue holds the matching entry as its index.
    pub next_event: Option<ScheduledEvent>,
}

/// An entity's scheduled event: when, and at which entity version it was scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledEvent {
    /// When it is due.
    pub at_ms: i64,
    /// The entity's version when it was scheduled.
    pub version: u64,
}

/// Drill, Refinery or Shipyard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BuildingKind {
    /// Extracts Iron Ore.
    Drill,
    /// Turns Iron Ore into Iron Ingots.
    Refinery,
    /// Builds ships.
    Shipyard,
}

/// Where a building is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum BuildingState {
    /// Being built: ingots delivered so far, toward its cost. The build keeps the terms it
    /// started with, so a later rules change does not alter it.
    UnderConstruction {
        /// Ingots delivered, with the rate they arrive at.
        delivered: Quantity,
        /// The total cost, fixed when construction started.
        cost_g: i64,
        /// The rated delivery rate, cost over build time, fixed when construction started.
        rated_g_per_h: i64,
    },
    /// Working.
    Operational,
    /// Stopped and being taken down; the slot frees at `ends_at_ms`.
    Demolishing {
        /// When demolition ends.
        ends_at_ms: i64,
    },
}

/// A building on a planet. Slots are identical, so it has no slot index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Building {
    /// Its ID.
    pub id: BuildingId,
    /// The planet it stands on.
    pub planet: PlanetId,
    /// What it is.
    pub kind: BuildingKind,
    /// Construction, operation or demolition.
    pub state: BuildingState,
}

/// Colony Ship or Cargo Vessel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ShipKind {
    /// Colonizes an unowned planet; consumed on use.
    ColonyShip,
    /// Carries cargo.
    CargoVessel,
}

/// Whether a ship build is drawing ingots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShipBuildStatus {
    /// One of the Shipyard's parallel builds.
    Active,
    /// Queued; consumes nothing. Waiting builds start in ID order.
    Waiting,
}

/// A ship being built at a Shipyard. A Shipyard's queue is its ship builds in ID order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShipBuild {
    /// Its ID; queue order follows it.
    pub id: ShipBuildId,
    /// The Shipyard building it.
    pub shipyard: BuildingId,
    /// The ship it will be.
    pub kind: ShipKind,
    /// Ingots delivered, with the rate they arrive at.
    pub delivered: Quantity,
    /// The total cost, fixed when the build was queued.
    pub cost_g: i64,
    /// The rated delivery rate, cost over build time, fixed when the build was queued.
    pub rated_g_per_h: i64,
    /// Active or waiting.
    pub status: ShipBuildStatus,
}

/// Where a ship is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShipLocation {
    /// On a planet's roster.
    Roster(PlanetId),
    /// In a fleet.
    Fleet(FleetId),
}

/// A finished ship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ship {
    /// Its ID.
    pub id: ShipId,
    /// What it is.
    pub kind: ShipKind,
    /// Who owns it.
    pub owner: PlayerId,
    /// A roster or a fleet; exactly one place.
    pub location: ShipLocation,
}

/// A fleet's mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mission {
    /// Claim an unowned planet with a Colony Ship.
    Colonize,
    /// Deliver cargo to one of the player's planets.
    Transport,
    /// Relocate.
    Move,
}

/// Where a fleet is in its travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum FleetState {
    /// At a planet; the only state that accepts orders other than recall.
    Stationed {
        /// The planet.
        at: PlanetId,
    },
    /// On its way; can be recalled once.
    InTransit {
        /// Where it left from.
        origin: PlanetId,
        /// Where it is going.
        target: PlanetId,
        /// What it will do there.
        mission: Mission,
        /// When it left.
        departed_at_ms: i64,
        /// When it arrives, fixed at departure.
        arrives_at_ms: i64,
    },
    /// Recalled, heading back to its origin.
    Returning {
        /// The origin it returns to.
        origin: PlanetId,
        /// When it arrives back.
        arrives_at_ms: i64,
    },
}

/// Cargo carried as fleet totals per resource.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cargo {
    /// Iron Ore aboard, in grams.
    pub ore_g: i64,
    /// Iron Ingots aboard, in grams.
    pub ingots_g: i64,
}

/// Ships traveling together. Speed and capacity derive from its ships and the rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fleet {
    /// Its ID.
    pub id: FleetId,
    /// Who owns it, and every ship in it.
    pub owner: PlayerId,
    /// Its ships; never empty.
    pub ships: BTreeSet<ShipId>,
    /// Cargo totals.
    pub cargo: Cargo,
    /// Increases on recall and other reschedules; stale queue entries are skipped.
    pub version: u64,
    /// Stationed, in transit or returning.
    pub state: FleetState,
}

/// What a queue entry wakes. Ordered by the architecture's kind rank, then entity ID: planet
/// changes (rank 0) before fleet entries (rank 1), and fleet arrivals and returns together by
/// fleet ID. For one fleet, an arrival sorts before a return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueKind {
    /// A planet's earliest crossing or timer (rank 0).
    PlanetChange(PlanetId),
    /// A fleet arriving at its target (rank 1).
    FleetArrival(FleetId),
    /// A recalled fleet arriving back (rank 1).
    FleetReturn(FleetId),
}

impl QueueKind {
    /// The kind rank: 0 for planet changes, 1 for fleet entries.
    pub fn rank(self) -> u8 {
        match self {
            Self::PlanetChange(_) => 0,
            Self::FleetArrival(_) | Self::FleetReturn(_) => 1,
        }
    }

    /// The ID of the entity it wakes.
    pub fn entity(self) -> u64 {
        match self {
            Self::PlanetChange(id) => id.0,
            Self::FleetArrival(id) | Self::FleetReturn(id) => id.0,
        }
    }

    fn sort_key(self) -> (u8, u64, u8) {
        let variant = match self {
            Self::PlanetChange(_) | Self::FleetArrival(_) => 0,
            Self::FleetReturn(_) => 1,
        };
        (self.rank(), self.entity(), variant)
    }
}

impl Ord for QueueKind {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

impl PartialOrd for QueueKind {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// One scheduled entry. Ordering is time, then kind rank and entity ID, then version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueEntry {
    /// When it is due.
    pub at_ms: i64,
    /// What it wakes.
    pub kind: QueueKind,
    /// The entity's version when scheduled.
    pub version: u64,
}

/// The whole universe: the unit of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "WorldData")]
pub struct World {
    /// Game time in milliseconds since the universe epoch.
    pub time_ms: i64,
    /// The rules version in force (decision log D-020).
    pub rules_version: u32,
    /// The universe seed, recorded from world generation.
    pub seed: u64,
    /// The next entity ID to hand out; every existing entity ID is below it.
    pub next_id: u64,
    /// Players by ID.
    pub players: BTreeMap<PlayerId, Player>,
    /// Planets by ID.
    pub planets: BTreeMap<PlanetId, Planet>,
    /// Buildings by ID.
    pub buildings: BTreeMap<BuildingId, Building>,
    /// Ship builds by ID.
    pub ship_builds: BTreeMap<ShipBuildId, ShipBuild>,
    /// Ships by ID.
    pub ships: BTreeMap<ShipId, Ship>,
    /// Fleets by ID.
    pub fleets: BTreeMap<FleetId, Fleet>,
    /// Homeworld candidates in world generation order; Join game takes the next unowned one.
    pub homeworld_candidates: Vec<PlanetId>,
    /// Scheduled entries in queue order. Private, so scheduling always goes through the guard.
    queue: BTreeSet<QueueEntry>,
    /// Set by a failed advance: the world may be mid-instant and must be discarded. Never
    /// serialized.
    poisoned: bool,
}

/// Whether a queue entry still stands for its entity's current schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryStatus {
    /// At its entity's version: processed when popped.
    Live,
    /// Superseded, or its fleet was removed: dropped when popped.
    Stale,
}

/// Why a world or an ID request is not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    /// The ID counter has no IDs left.
    #[error("no entity IDs left: next_id is {0}")]
    IdsExhausted(u64),
    /// An entity's ID is not below `next_id`, so the counter could hand it out again.
    #[error("entity ID {id} is not below next_id {next_id}")]
    IdNotBelowCounter {
        /// The entity ID.
        id: u64,
        /// The counter.
        next_id: u64,
    },
    /// Two entities share an ID.
    #[error("entity ID {0} is used more than once")]
    DuplicateId(u64),
    /// A map key differs from the entity's own ID.
    #[error("{0}")]
    KeyMismatch(String),
    /// A reference that does not resolve, or that the other side does not mirror.
    #[error("{0}")]
    Link(String),
}

impl World {
    /// An empty world at `time_ms` under `rules_version`, with the counter at 1.
    pub fn new(time_ms: i64, rules_version: u32, seed: u64) -> Self {
        Self {
            time_ms,
            rules_version,
            seed,
            next_id: 1,
            players: BTreeMap::new(),
            planets: BTreeMap::new(),
            buildings: BTreeMap::new(),
            ship_builds: BTreeMap::new(),
            ships: BTreeMap::new(),
            fleets: BTreeMap::new(),
            homeworld_candidates: Vec::new(),
            queue: BTreeSet::new(),
            poisoned: false,
        }
    }

    /// The event queue, in queue order.
    pub fn queue(&self) -> &BTreeSet<QueueEntry> {
        &self.queue
    }

    /// Whether a failed advance has left this world unusable.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Classifies a queue entry against the current state; the one predicate the event loop and
    /// [`World::check_links`] share. An entry newer than its entity, or one naming an ID that
    /// was never allocated or that belongs to a different kind of entity, is an error.
    pub fn entry_status(&self, entry: &QueueEntry) -> Result<EntryStatus, StateError> {
        let (what, id, version) = match entry.kind {
            QueueKind::PlanetChange(planet) => (
                "planet",
                planet.0,
                self.planet(planet, "queue entry")?.version,
            ),
            QueueKind::FleetArrival(id) | QueueKind::FleetReturn(id) => {
                let Some(fleet) = self.fleets.get(&id) else {
                    // A fleet removed while an entry for it was queued (disbanded after an early
                    // recall, say) leaves that entry stale, and the queue is never searched to
                    // purge it. IDs are never reused, so an allocated ID that no entity holds
                    // can only be such a fleet.
                    if id.0 >= self.next_id || self.holds_id(id.0) {
                        return Err(link(format!(
                            "queue entry {entry:?}: fleet {} does not exist",
                            id.0
                        )));
                    }
                    return Ok(EntryStatus::Stale);
                };
                ("fleet", id.0, fleet.version)
            }
        };
        match entry.version.cmp(&version) {
            std::cmp::Ordering::Less => Ok(EntryStatus::Stale),
            std::cmp::Ordering::Equal => Ok(EntryStatus::Live),
            std::cmp::Ordering::Greater => Err(link(format!(
                "queue entry {entry:?} is newer than {what} {id}, at version {version}"
            ))),
        }
    }

    /// Whether a planet, building, ship build or ship holds `id`.
    fn holds_id(&self, id: u64) -> bool {
        self.planets.contains_key(&PlanetId(id))
            || self.buildings.contains_key(&BuildingId(id))
            || self.ship_builds.contains_key(&ShipBuildId(id))
            || self.ships.contains_key(&ShipId(id))
    }

    /// A planet's stored next event is at its version, after world time and in the queue.
    fn check_planet_schedule(&self, planet: &Planet) -> Result<(), StateError> {
        let Some(next) = planet.next_event else {
            return Ok(());
        };
        if next.at_ms <= self.time_ms {
            return Err(link(format!(
                "planet {}: next event at {} ms is not after world time {}",
                planet.id.0, next.at_ms, self.time_ms
            )));
        }
        if next.version != planet.version {
            return Err(link(format!(
                "planet {}: next event has version {}, but the planet is at version {}",
                planet.id.0, next.version, planet.version
            )));
        }
        let entry = QueueEntry {
            at_ms: next.at_ms,
            kind: QueueKind::PlanetChange(planet.id),
            version: next.version,
        };
        if !self.queue.contains(&entry) {
            return Err(link(format!(
                "planet {}: next event at {} ms is not in the queue",
                planet.id.0, next.at_ms
            )));
        }
        Ok(())
    }

    /// A moving fleet's expected live entry is after world time and in the queue. Whether a
    /// stationed fleet has none needs a full queue scan, so only [`World::check_links`] checks it.
    fn check_fleet_schedule(&self, fleet: &Fleet) -> Result<(), StateError> {
        let Some(entry) = expected_entry(fleet) else {
            return Ok(());
        };
        if entry.at_ms <= self.time_ms {
            return Err(link(format!(
                "fleet {}: arrival at {} ms is not after world time {}",
                fleet.id.0, entry.at_ms, self.time_ms
            )));
        }
        if !self.queue.contains(&entry) {
            return Err(link(format!(
                "fleet {} in state {:?} has no live queue entry {entry:?}",
                fleet.id.0, fleet.state
            )));
        }
        Ok(())
    }

    /// Takes the next entity ID. Fails, rather than wrapping, when the counter is exhausted.
    pub fn allocate_id(&mut self) -> Result<u64, StateError> {
        let id = self.next_id;
        if id == u64::MAX {
            return Err(StateError::IdsExhausted(id));
        }
        self.next_id = id + 1;
        Ok(id)
    }

    /// Checks that IDs are unique and below the counter, that map keys match the entities, that
    /// every reference resolves and is mirrored by the other side, and that the queue agrees with
    /// the planets and fleets it wakes. Gameplay rules (slot
    /// counts, Shipyard limits, cargo capacity) are not checked here.
    pub fn check_links(&self) -> Result<(), StateError> {
        let mut ids = BTreeSet::new();
        let mut claim = |id: u64| {
            if id >= self.next_id {
                return Err(StateError::IdNotBelowCounter {
                    id,
                    next_id: self.next_id,
                });
            }
            if !ids.insert(id) {
                Err(StateError::DuplicateId(id))
            } else {
                Ok(())
            }
        };
        for (key, planet) in &self.planets {
            keyed(key.0, planet.id.0, "planet")?;
            claim(planet.id.0)?;
        }
        for (key, building) in &self.buildings {
            keyed(key.0, building.id.0, "building")?;
            claim(building.id.0)?;
        }
        for (key, build) in &self.ship_builds {
            keyed(key.0, build.id.0, "ship build")?;
            claim(build.id.0)?;
        }
        for (key, ship) in &self.ships {
            keyed(key.0, ship.id.0, "ship")?;
            claim(ship.id.0)?;
        }
        for (key, fleet) in &self.fleets {
            keyed(key.0, fleet.id.0, "fleet")?;
            claim(fleet.id.0)?;
        }

        for (key, player) in &self.players {
            if *key != player.id {
                return Err(StateError::KeyMismatch(format!(
                    "player key {:?} holds {:?}",
                    key, player.id
                )));
            }
            let home = self.planet(player.homeworld, "homeworld of a player")?;
            if home.owner != Some(player.id) {
                return Err(link(format!(
                    "player {:?}: homeworld {} is not owned by them",
                    player.id, home.id.0
                )));
            }
        }

        for planet in self.planets.values() {
            if let Some(owner) = planet.owner
                && !self.players.contains_key(&owner)
            {
                return Err(link(format!(
                    "planet {}: owner {owner:?} is not a player",
                    planet.id.0
                )));
            }
            for (name, stock) in [("ore", planet.ore), ("ingots", planet.ingots)] {
                if stock.checkpoint_ms() > self.time_ms {
                    return Err(link(format!(
                        "planet {}: {name} checkpoint {} is after world time {}",
                        planet.id.0,
                        stock.checkpoint_ms(),
                        self.time_ms
                    )));
                }
            }
            self.check_planet_schedule(planet)?;
            for id in &planet.buildings {
                let building = self.buildings.get(id).ok_or_else(|| {
                    link(format!(
                        "planet {}: building {} does not exist",
                        planet.id.0, id.0
                    ))
                })?;
                if building.planet != planet.id {
                    return Err(link(format!(
                        "planet {} lists building {}, which stands on planet {}",
                        planet.id.0, id.0, building.planet.0
                    )));
                }
            }
            for id in &planet.roster {
                let ship = self.ships.get(id).ok_or_else(|| {
                    link(format!(
                        "planet {}: roster ship {} does not exist",
                        planet.id.0, id.0
                    ))
                })?;
                if ship.location != ShipLocation::Roster(planet.id) {
                    return Err(link(format!(
                        "planet {} lists ship {} on its roster, but the ship is at {:?}",
                        planet.id.0, id.0, ship.location
                    )));
                }
            }
        }

        for building in self.buildings.values() {
            let planet = self.planet(building.planet, "planet of a building")?;
            if !planet.buildings.contains(&building.id) {
                return Err(link(format!(
                    "building {} stands on planet {}, which does not list it",
                    building.id.0, planet.id.0
                )));
            }
        }

        for build in self.ship_builds.values() {
            let shipyard = self.buildings.get(&build.shipyard).ok_or_else(|| {
                link(format!(
                    "ship build {}: shipyard {} does not exist",
                    build.id.0, build.shipyard.0
                ))
            })?;
            if shipyard.kind != BuildingKind::Shipyard
                || shipyard.state != BuildingState::Operational
            {
                return Err(link(format!(
                    "ship build {}: building {} is not an operational Shipyard",
                    build.id.0, shipyard.id.0
                )));
            }
        }

        for ship in self.ships.values() {
            if !self.players.contains_key(&ship.owner) {
                return Err(link(format!(
                    "ship {}: owner {:?} is not a player",
                    ship.id.0, ship.owner
                )));
            }
            match ship.location {
                ShipLocation::Roster(at) => {
                    if !self
                        .planet(at, "roster of a ship")?
                        .roster
                        .contains(&ship.id)
                    {
                        return Err(link(format!(
                            "ship {} is on planet {}'s roster, which does not list it",
                            ship.id.0, at.0
                        )));
                    }
                }
                ShipLocation::Fleet(fleet) => {
                    let holder = self.fleets.get(&fleet).ok_or_else(|| {
                        link(format!(
                            "ship {}: fleet {} does not exist",
                            ship.id.0, fleet.0
                        ))
                    })?;
                    if !holder.ships.contains(&ship.id) {
                        return Err(link(format!(
                            "ship {} is in fleet {}, which does not list it",
                            ship.id.0, fleet.0
                        )));
                    }
                }
            }
        }

        for fleet in self.fleets.values() {
            if !self.players.contains_key(&fleet.owner) {
                return Err(link(format!(
                    "fleet {}: owner {:?} is not a player",
                    fleet.id.0, fleet.owner
                )));
            }
            if fleet.ships.is_empty() {
                return Err(link(format!("fleet {} has no ships", fleet.id.0)));
            }
            for id in &fleet.ships {
                let ship = self.ships.get(id).ok_or_else(|| {
                    link(format!(
                        "fleet {}: ship {} does not exist",
                        fleet.id.0, id.0
                    ))
                })?;
                if ship.location != ShipLocation::Fleet(fleet.id) || ship.owner != fleet.owner {
                    return Err(link(format!(
                        "fleet {} lists ship {}, which is at {:?} and owned by {:?}",
                        fleet.id.0, id.0, ship.location, ship.owner
                    )));
                }
            }
            if fleet.cargo.ore_g < 0 || fleet.cargo.ingots_g < 0 {
                return Err(link(format!(
                    "fleet {}: negative cargo {:?}",
                    fleet.id.0, fleet.cargo
                )));
            }
            let places: &[PlanetId] = match &fleet.state {
                FleetState::Stationed { at } => &[*at],
                FleetState::InTransit { origin, target, .. } => &[*origin, *target],
                FleetState::Returning { origin, .. } => &[*origin],
            };
            for place in places {
                self.planet(*place, "planet of a fleet")?;
            }
        }

        let mut seen = BTreeSet::new();
        for candidate in &self.homeworld_candidates {
            self.planet(*candidate, "homeworld candidate")?;
            if !seen.insert(*candidate) {
                return Err(link(format!(
                    "homeworld candidate {} is listed twice",
                    candidate.0
                )));
            }
        }

        let mut live_fleet_entries = BTreeMap::new();
        for entry in &self.queue {
            if self.entry_status(entry)? == EntryStatus::Stale {
                continue;
            }
            match entry.kind {
                QueueKind::PlanetChange(id) => {
                    // A live entry must be the planet's next event.
                    let planet = &self.planets[&id];
                    let next = planet.next_event.map(|n| (n.at_ms, n.version));
                    if next != Some((entry.at_ms, entry.version)) {
                        return Err(link(format!(
                            "queue entry {entry:?} is live for planet {}, whose next event is {:?}",
                            id.0, planet.next_event
                        )));
                    }
                }
                QueueKind::FleetArrival(id) | QueueKind::FleetReturn(id) => {
                    if live_fleet_entries.insert(id, *entry).is_some() {
                        return Err(link(format!("fleet {} has two live queue entries", id.0)));
                    }
                }
            }
        }

        // Each moving fleet has exactly one live entry, of its kind and at its arrival time; a
        // stationed fleet has none.
        for fleet in self.fleets.values() {
            self.check_fleet_schedule(fleet)?;
            let expected = expected_entry(fleet);
            let live = live_fleet_entries.get(&fleet.id).copied();
            if live != expected {
                return Err(link(format!(
                    "fleet {} in state {:?} has live queue entry {live:?}, expected {expected:?}",
                    fleet.id.0, fleet.state
                )));
            }
        }
        Ok(())
    }

    fn planet(&self, id: PlanetId, role: &str) -> Result<&Planet, StateError> {
        self.planets
            .get(&id)
            .ok_or_else(|| link(format!("{role}: planet {} does not exist", id.0)))
    }
}

/// The live entry a fleet's state calls for: an arrival in transit, a return when returning, and
/// none when stationed.
fn expected_entry(fleet: &Fleet) -> Option<QueueEntry> {
    let (at_ms, kind) = match fleet.state {
        FleetState::Stationed { .. } => return None,
        FleetState::InTransit { arrives_at_ms, .. } => {
            (arrives_at_ms, QueueKind::FleetArrival(fleet.id))
        }
        FleetState::Returning { arrives_at_ms, .. } => {
            (arrives_at_ms, QueueKind::FleetReturn(fleet.id))
        }
    };
    Some(QueueEntry {
        at_ms,
        kind,
        version: fleet.version,
    })
}

fn keyed(key: u64, id: u64, what: &str) -> Result<(), StateError> {
    if key == id {
        Ok(())
    } else {
        Err(StateError::KeyMismatch(format!(
            "{what} key {key} holds {what} {id}"
        )))
    }
}

fn link(message: String) -> StateError {
    StateError::Link(message)
}

/// The serialized form of a [`World`]. Deserializing a world checks its links. Field order must
/// match [`WorldDataRef`], which serializes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorldData {
    time_ms: i64,
    rules_version: u32,
    seed: u64,
    next_id: u64,
    players: BTreeMap<PlayerId, Player>,
    planets: BTreeMap<PlanetId, Planet>,
    buildings: BTreeMap<BuildingId, Building>,
    ship_builds: BTreeMap<ShipBuildId, ShipBuild>,
    ships: BTreeMap<ShipId, Ship>,
    fleets: BTreeMap<FleetId, Fleet>,
    homeworld_candidates: Vec<PlanetId>,
    queue: BTreeSet<QueueEntry>,
}

impl TryFrom<WorldData> for World {
    type Error = StateError;

    fn try_from(d: WorldData) -> Result<Self, StateError> {
        let world = Self {
            time_ms: d.time_ms,
            rules_version: d.rules_version,
            seed: d.seed,
            next_id: d.next_id,
            players: d.players,
            planets: d.planets,
            buildings: d.buildings,
            ship_builds: d.ship_builds,
            ships: d.ships,
            fleets: d.fleets,
            homeworld_candidates: d.homeworld_candidates,
            queue: d.queue,
            poisoned: false,
        };
        world.check_links()?;
        Ok(world)
    }
}

/// A borrowed [`WorldData`], so serializing a world does not copy it.
#[derive(Serialize)]
struct WorldDataRef<'a> {
    time_ms: i64,
    rules_version: u32,
    seed: u64,
    next_id: u64,
    players: &'a BTreeMap<PlayerId, Player>,
    planets: &'a BTreeMap<PlanetId, Planet>,
    buildings: &'a BTreeMap<BuildingId, Building>,
    ship_builds: &'a BTreeMap<ShipBuildId, ShipBuild>,
    ships: &'a BTreeMap<ShipId, Ship>,
    fleets: &'a BTreeMap<FleetId, Fleet>,
    homeworld_candidates: &'a Vec<PlanetId>,
    queue: &'a BTreeSet<QueueEntry>,
}

/// A poisoned world does not serialize: it may be mid-instant (D-023).
impl Serialize for World {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.poisoned {
            return Err(serde::ser::Error::custom(EngineError::Poisoned));
        }
        WorldDataRef {
            time_ms: self.time_ms,
            rules_version: self.rules_version,
            seed: self.seed,
            next_id: self.next_id,
            players: &self.players,
            planets: &self.planets,
            buildings: &self.buildings,
            ship_builds: &self.ship_builds,
            ships: &self.ships,
            fleets: &self.fleets,
            homeworld_candidates: &self.homeworld_candidates,
            queue: &self.queue,
        }
        .serialize(serializer)
    }
}

mod engine;
pub use engine::{DomainEvent, EngineError, Pending, Steps};

#[cfg(test)]
mod tests;
