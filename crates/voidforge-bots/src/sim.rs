//! The nightly bot simulation (Technical Plan P5.3; decision log D-066): bots play through the API
//! alone, on the fast rules, for a set wall time (one wall second plays one v0 hour, so 30 minutes
//! are about 10.7 v0 weeks). Each bot reads its views, checks them, and sends one command a tick:
//! a valid one chosen with a seeded generator from what its state allows, or about one in seven an
//! invalid one built to get exactly one error code. About one valid command in ten is sent again
//! with its key, and must get exactly the stored answer (D-037).
//!
//! Every check here holds in any interleaving of the bots: it reads one view at a time and never
//! asserts a value that depends on timing. A game refusal (422) of a valid command is counted, not
//! failed: the bots don't read the rules, so a build without the ingots is a fair try. The exact
//! checks (replay from every snapshot, the core's invariants after every record) run after the
//! simulation, on its log.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::Serialize;
use tokio::time::Instant;
use voidforge_api_types::command::{
    AssembleFleet, BuildBuilding, BuildingRef, CancelShipBuild, CommandAccepted, CommandRequest,
    FleetRef, QueueShip, SendFleet,
};
use voidforge_api_types::error::ErrorCode;
use voidforge_api_types::view::{BuildingView, FleetView, FleetsView, PlanetView, PlayerView};
use voidforge_api_types::wire::{
    BuildingId, BuildingType, Cargo, FleetId, Mission, PlanetId, ShipBuildId, ShipId, ShipType,
    Timestamp,
};

use crate::Failure;
use crate::client::{Player, Reply};
use crate::login::{self, TestUser};
use crate::wait::unix_ms;

/// One bot's tick: its reads, at most one command, and a wait for the rest. At most six reads
/// and about 1.1 writes a tick, under `serve`'s defaults of 10 reads and 2 writes a second (D-054).
pub const TICK: Duration = Duration::from_secs(1);

/// The planets a bot reads in one tick: its homeworld and up to two others.
const PLANETS_PER_TICK: usize = 3;

/// An ID no one has: entity IDs are a small engine counter (D-045).
const NO_SUCH_ID: u64 = 1 << 52;

/// Of every 1,000 commands, how many are invalid by construction; and of every 1,000 valid ones,
/// how many are sent again with their key.
const INVALID_PER_MILLE: u64 = 150;
const RETRY_PER_MILLE: u64 = 100;
/// Per tick, the chance (per 1,000) that an undo (a cancel or a recall) is offered at all. Offered
/// every tick, an undo wins whenever it's a bot's only option: the first runs recalled 70 of 71
/// fleets sent and cancelled 31 of 49 ship builds, so few missions ever arrived.
const UNDO_PER_MILLE: u64 = 5;
/// Ship builds a Shipyard may hold at once before the bots stop queueing.
const BUILDS_PER_SHIPYARD: usize = 2;

/// The bots' test users: the nightly's 8 players (deploy/dex/config.ci.yaml). TEST-ONLY.
pub const PLAYERS: [TestUser; 8] = [
    login::ALICE,
    login::BOB,
    TestUser {
        name: "bot1",
        email: "bot1@voidforge.test",
        password: "bot1-local-test-password",
    },
    TestUser {
        name: "bot2",
        email: "bot2@voidforge.test",
        password: "bot2-local-test-password",
    },
    TestUser {
        name: "bot3",
        email: "bot3@voidforge.test",
        password: "bot3-local-test-password",
    },
    TestUser {
        name: "bot4",
        email: "bot4@voidforge.test",
        password: "bot4-local-test-password",
    },
    TestUser {
        name: "bot5",
        email: "bot5@voidforge.test",
        password: "bot5-local-test-password",
    },
    TestUser {
        name: "bot6",
        email: "bot6@voidforge.test",
        password: "bot6-local-test-password",
    },
];

/// What the simulation did, for the job summary (`@nightly-sim <json>`).
#[derive(Debug, Default, Clone, Serialize)]
pub struct Tally {
    pub bots: usize,
    pub seed: u64,
    pub wall_seconds: f64,
    pub ticks: u64,
    pub reads: u64,
    pub views_checked: u64,
    pub valid_sent: u64,
    /// Valid commands by type.
    pub valid_by_type: BTreeMap<String, u64>,
    pub accepted: u64,
    /// Game refusals (422) of valid commands, by code.
    pub game_refused: BTreeMap<String, u64>,
    /// Invalid commands, by the code each got (always the one it was built for).
    pub invalid: BTreeMap<String, u64>,
    pub retried: u64,
    pub rate_limited: u64,
    /// The most planets any one bot held: colonies landed, or not.
    pub most_planets: usize,
}

impl Tally {
    fn add(&mut self, other: &Tally) {
        self.ticks += other.ticks;
        self.reads += other.reads;
        self.views_checked += other.views_checked;
        self.valid_sent += other.valid_sent;
        for (kind, n) in &other.valid_by_type {
            *self.valid_by_type.entry(kind.clone()).or_default() += n;
        }
        self.accepted += other.accepted;
        for (code, n) in &other.game_refused {
            *self.game_refused.entry(code.clone()).or_default() += n;
        }
        for (code, n) in &other.invalid {
            *self.invalid.entry(code.clone()).or_default() += n;
        }
        self.retried += other.retried;
        self.most_planets = self.most_planets.max(other.most_planets);
    }

    /// Commands a second, every kind: the load the D-037 measurement is recorded at.
    #[must_use]
    pub fn commands_per_second(&self) -> f64 {
        let sent = self.valid_sent + self.retried + self.invalid.values().sum::<u64>();
        if self.wall_seconds > 0.0 {
            sent as f64 / self.wall_seconds
        } else {
            0.0
        }
    }
}

/// A small seeded generator (SplitMix64): the bots' choices, reproducible from the printed seed.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// True `per_mille` times in 1,000.
    pub fn chance(&mut self, per_mille: u64) -> bool {
        self.next() % 1_000 < per_mille
    }

    /// An index below `n` (which is at least 1).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            items.get(self.below(items.len()))
        }
    }
}

// --- The checks on one view: each holds whatever the other bots do.

fn at_or_after(what: &str, t: &Timestamp, as_of: &Timestamp) -> Result<(), String> {
    if unix_ms(t) >= unix_ms(as_of) {
        Ok(())
    } else {
        Err(format!(
            "{what} {} is before the view's as_of {}",
            t.as_str(),
            as_of.as_str()
        ))
    }
}

/// A planet view: stocks within `[0, cap]`, buildings within the slots, every time it names at or
/// after its own `as_of`, and each ship on its roster once.
///
/// # Errors
/// The first rule the view breaks.
pub fn check_planet(v: &PlanetView) -> Result<(), String> {
    let p = v.planet_id.0;
    for (name, s) in [
        ("iron_ore", &v.stocks.iron_ore),
        ("iron_ingot", &v.stocks.iron_ingot),
    ] {
        if s.amount_g < 0 || s.amount_g > s.cap_g {
            return Err(format!(
                "planet {p}: {name} {} g outside [0, {}]",
                s.amount_g, s.cap_g
            ));
        }
    }
    if v.buildings.len() > v.slots as usize {
        return Err(format!(
            "planet {p}: {} buildings on {} slots",
            v.buildings.len(),
            v.slots
        ));
    }
    if let Some(t) = &v.valid_until {
        at_or_after(&format!("planet {p}: valid_until"), t, &v.as_of)?;
    }
    for b in &v.buildings {
        match b {
            BuildingView::UnderConstruction {
                id, eta: Some(t), ..
            } => {
                at_or_after(&format!("planet {p}: building {} eta", id.0), t, &v.as_of)?;
            }
            BuildingView::Demolishing {
                id,
                demolition_ends_at,
                ..
            } => {
                at_or_after(
                    &format!("planet {p}: building {} demolition", id.0),
                    demolition_ends_at,
                    &v.as_of,
                )?;
            }
            _ => {}
        }
    }
    for build in &v.ship_builds {
        if let Some(t) = &build.eta {
            at_or_after(
                &format!("planet {p}: ship build {} eta", build.id.0),
                t,
                &v.as_of,
            )?;
        }
    }
    let mut seen = BTreeSet::new();
    for s in &v.roster {
        if !seen.insert(s.ship_id) {
            return Err(format!(
                "planet {p}: ship {} twice on the roster",
                s.ship_id.0
            ));
        }
    }
    Ok(())
}

/// The player's fleets: each ship in one fleet, cargo non-negative and within the capacity, and
/// every time at or after the view's `as_of` (a departure at or before it).
///
/// # Errors
/// The first rule the view breaks.
pub fn check_fleets(v: &FleetsView) -> Result<(), String> {
    if let Some(t) = &v.valid_until {
        at_or_after("fleets: valid_until", t, &v.as_of)?;
    }
    let mut seen = BTreeSet::new();
    for f in &v.fleets {
        let (id, ships, cargo, capacity) = match f {
            FleetView::Stationed {
                fleet_id,
                ships,
                cargo,
                capacity_g,
                ..
            }
            | FleetView::InTransit {
                fleet_id,
                ships,
                cargo,
                capacity_g,
                ..
            }
            | FleetView::Returning {
                fleet_id,
                ships,
                cargo,
                capacity_g,
                ..
            } => (fleet_id.0, ships, cargo, *capacity_g),
        };
        match f {
            FleetView::InTransit {
                departed_at,
                arrives_at,
                ..
            } => {
                at_or_after(&format!("fleet {id}: arrives_at"), arrives_at, &v.as_of)?;
                at_or_after(
                    &format!("fleet {id}: the view's as_of"),
                    &v.as_of,
                    departed_at,
                )?;
            }
            FleetView::Returning { arrives_at, .. } => {
                at_or_after(&format!("fleet {id}: arrives_at"), arrives_at, &v.as_of)?;
            }
            FleetView::Stationed { .. } => {}
        }
        if cargo.iron_ore_g < 0 || cargo.iron_ingot_g < 0 {
            return Err(format!("fleet {id}: negative cargo {cargo:?}"));
        }
        let load = cargo.iron_ore_g.checked_add(cargo.iron_ingot_g);
        if load.is_none_or(|g| g > capacity) {
            return Err(format!(
                "fleet {id}: cargo {cargo:?} over its capacity {capacity} g"
            ));
        }
        if ships.is_empty() {
            return Err(format!("fleet {id} has no ships"));
        }
        for s in ships {
            if !seen.insert(s.ship_id) {
                return Err(format!(
                    "ship {} is in two fleets (the second: {id})",
                    s.ship_id.0
                ));
            }
        }
    }
    Ok(())
}

/// The player: its planets include its homeworld.
///
/// # Errors
/// The first rule the view breaks.
pub fn check_player(v: &PlayerView) -> Result<(), String> {
    if !v.planet_ids.contains(&v.homeworld_id) {
        return Err(format!(
            "the player's planets {:?} lack its homeworld {}",
            v.planet_ids, v.homeworld_id.0
        ));
    }
    if let Some(t) = &v.valid_until {
        at_or_after("player: valid_until", t, &v.as_of)?;
    }
    Ok(())
}

// --- The answers to commands.

/// What a valid command's answer means: accepted, or refused by a game rule (422, counted).
/// Anything else (a 400, 401, 404, 409 or 5xx) is a defect.
///
/// # Errors
/// The answer, when it is neither.
pub fn judge_valid(reply: &Reply<CommandAccepted>) -> Result<Option<String>, String> {
    match reply {
        Ok(_) => Ok(None),
        Err(r) if r.status == 422 => Ok(Some(code_name(r.body.code))),
        Err(r) => Err(format!(
            "a valid command got {} {}: {}",
            r.status,
            code_name(r.body.code),
            r.body.message
        )),
    }
}

/// An invalid command must get exactly the status and code it was built for.
///
/// # Errors
/// What it got instead.
pub fn judge_invalid(want: (u16, ErrorCode), reply: &Reply<CommandAccepted>) -> Result<(), String> {
    match reply {
        Err(r) if r.status == want.0 && r.body.code == want.1 => Ok(()),
        Err(r) => Err(format!(
            "built for {} {}, got {} {}: {}",
            want.0,
            code_name(want.1),
            r.status,
            code_name(r.body.code),
            r.body.message
        )),
        Ok(a) => Err(format!(
            "built for {} {}, accepted as seq {}",
            want.0,
            code_name(want.1),
            a.seq
        )),
    }
}

/// A command sent again with its key gets exactly the stored answer: the same record, or the same
/// refusal (D-037).
///
/// # Errors
/// The two answers, when they differ.
pub fn judge_retry(
    first: &Reply<CommandAccepted>,
    again: &Reply<CommandAccepted>,
) -> Result<(), String> {
    let same = match (first, again) {
        (Ok(a), Ok(b)) => a.seq == b.seq && a.game_time == b.game_time,
        (Err(a), Err(b)) => a.status == b.status && a.body.code == b.body.code,
        _ => false,
    };
    if same {
        Ok(())
    } else {
        Err(format!("first {}, again {}", brief(first), brief(again)))
    }
}

fn brief(reply: &Reply<CommandAccepted>) -> String {
    match reply {
        Ok(a) => format!("accepted as seq {} at {}", a.seq, a.game_time.as_str()),
        Err(r) => format!("{} {}", r.status, code_name(r.body.code)),
    }
}

/// An error code's wire name.
#[must_use]
pub fn code_name(code: ErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{code:?}"))
}

// --- One bot.

fn kind(b: &BuildingView) -> BuildingType {
    match b {
        BuildingView::UnderConstruction { building_type, .. }
        | BuildingView::Operational { building_type, .. }
        | BuildingView::Demolishing { building_type, .. } => *building_type,
    }
}

/// What a bot saw in one tick.
struct Seen {
    me: PlayerView,
    planets: Vec<PlanetView>,
    fleets: FleetsView,
}

fn fail(bot: &str, detail: impl std::fmt::Display) -> Failure {
    Failure::Assertion {
        step: format!("sim: {bot}"),
        detail: detail.to_string(),
    }
}

fn view<T>(bot: &str, what: &str, reply: Reply<T>) -> Result<T, Failure> {
    reply.map_err(|r| {
        fail(
            bot,
            format!(
                "{what} refused: {} {}: {}",
                r.status,
                code_name(r.body.code),
                r.body.message
            ),
        )
    })
}

struct Bot {
    player: Player,
    rng: Rng,
    run: String,
    sent: u64,
    tally: Tally,
    /// Planets other players own, and planets no one owns, from the last search.
    others: Vec<PlanetId>,
    free: Vec<PlanetId>,
    /// The homeworld as last seen: logged at the end, so a thin night shows what held it back.
    home: Option<PlanetView>,
}

impl Bot {
    fn key(&mut self) -> String {
        self.sent += 1;
        format!("{}-{}-{}", self.run, self.player.name, self.sent)
    }

    async fn observe(&mut self, tick: u64) -> Result<Seen, Failure> {
        let name = self.player.name;
        let me = view(name, "GET /players/me", self.player.me().await?)?;
        check_player(&me).map_err(|e| fail(name, e))?;
        self.tally.most_planets = self.tally.most_planets.max(me.planet_ids.len());
        let mut ids = vec![me.homeworld_id];
        let mut rest: Vec<PlanetId> = me
            .planet_ids
            .iter()
            .copied()
            .filter(|p| *p != me.homeworld_id)
            .collect();
        while ids.len() < PLANETS_PER_TICK && !rest.is_empty() {
            let i = self.rng.below(rest.len());
            ids.push(rest.swap_remove(i));
        }
        let mut planets = Vec::new();
        for id in ids {
            let p = view(name, "GET /planets/{id}", self.player.planet(id).await?)?;
            check_planet(&p).map_err(|e| fail(name, e))?;
            planets.push(p);
        }
        let fleets = view(name, "GET /players/me/fleets", self.player.fleets().await?)?;
        check_fleets(&fleets).map_err(|e| fail(name, e))?;
        self.tally.reads += 2 + planets.len() as u64;
        self.tally.views_checked += 2 + planets.len() as u64;
        // Every 20 ticks, the planets around the homeworld: targets, and other players' planets.
        if tick.is_multiple_of(20)
            && let Some(home) = planets.first()
        {
            let near = view(
                name,
                "GET /planets?near",
                self.player.near(home.position, 200_000).await?,
            )?;
            self.tally.reads += 1;
            self.others = near
                .planets
                .iter()
                .filter(|s| s.owner.is_some() && s.owner.as_ref() != Some(&me.player_id))
                .map(|s| s.planet_id)
                .collect();
            self.free = near
                .planets
                .iter()
                .filter(|s| s.owner.is_none())
                .map(|s| s.planet_id)
                .collect();
        }
        self.home = planets.first().cloned();
        Ok(Seen {
            me,
            planets,
            fleets,
        })
    }

    /// One valid command the state allows, if any, by weight: what moves the game forward (a
    /// Shipyard first, ships, fleets and their missions) far more often than what undoes it
    /// (cancels and demolitions, which never take a planet's last building of a type).
    fn valid(&mut self, seen: &Seen) -> Option<CommandRequest> {
        let mut options: Vec<(u64, CommandRequest)> = Vec::new();
        let rng = &mut self.rng;
        for p in &seen.planets {
            let shipyards = p
                .buildings
                .iter()
                .filter(|b| kind(b) == BuildingType::Shipyard)
                .count();
            // One construction at a time per planet: they share the planet's ingots, so more at
            // once starve each other (the first runs built and cancelled without end).
            let constructing = p
                .buildings
                .iter()
                .any(|b| matches!(b, BuildingView::UnderConstruction { .. }));
            if !constructing && p.buildings.len() < p.slots as usize {
                let building_type = if shipyards == 0 {
                    BuildingType::Shipyard
                } else {
                    [
                        BuildingType::Drill,
                        BuildingType::Refinery,
                        BuildingType::Shipyard,
                    ][rng.below(3)]
                };
                options.push((
                    30,
                    CommandRequest::BuildBuilding(BuildBuilding {
                        planet_id: p.planet_id,
                        building_type,
                    }),
                ));
            }
            for b in &p.buildings {
                match b {
                    // At most two builds per Shipyard: a ship costs nothing up front, so an open
                    // queue grows every tick and its builds starve each other of ingots (the
                    // first runs queued 1,320 ships and assembled 22 fleets).
                    BuildingView::Operational {
                        id,
                        building_type: BuildingType::Shipyard,
                        ..
                    } if p
                        .ship_builds
                        .iter()
                        .filter(|s| s.shipyard_id == *id)
                        .count()
                        < BUILDS_PER_SHIPYARD =>
                    {
                        let ship_type = [ShipType::ColonyShip, ShipType::CargoVessel][rng.below(2)];
                        options.push((
                            30,
                            CommandRequest::QueueShip(QueueShip {
                                shipyard_id: *id,
                                ship_type,
                            }),
                        ));
                    }
                    // Never a planet's last working one of a type: its only refinery gone, the
                    // ingots to build another never come (the first long runs deadlocked so).
                    BuildingView::Operational {
                        id, building_type, ..
                    } if p.buildings.len() >= 4
                        && p.buildings
                            .iter()
                            .filter(|o| {
                                matches!(o, BuildingView::Operational { .. })
                                    && kind(o) == *building_type
                            })
                            .count()
                            > 1 =>
                    {
                        options.push((
                            2,
                            CommandRequest::DemolishBuilding(BuildingRef { building_id: *id }),
                        ));
                    }
                    BuildingView::UnderConstruction { id, .. } if rng.chance(UNDO_PER_MILLE) => {
                        options.push((
                            1,
                            CommandRequest::CancelConstruction(BuildingRef { building_id: *id }),
                        ));
                    }
                    _ => {}
                }
            }
            if let Some(build) = p.ship_builds.first().filter(|_| rng.chance(UNDO_PER_MILLE)) {
                options.push((
                    1,
                    CommandRequest::CancelShipBuild(CancelShipBuild {
                        ship_build_id: build.id,
                    }),
                ));
            }
            let mine: Vec<ShipId> = p
                .roster
                .iter()
                .filter(|s| s.owner == seen.me.player_id)
                .map(|s| s.ship_id)
                .collect();
            if !mine.is_empty() {
                let n = 1 + rng.below(mine.len().min(4));
                let ingots = p.stocks.iron_ingot.amount_g.max(0);
                let cargo = if rng.chance(500) {
                    Cargo {
                        iron_ore_g: 0,
                        iron_ingot_g: (ingots / 10).min(100_000_000),
                    }
                } else {
                    Cargo::default()
                };
                options.push((
                    40,
                    CommandRequest::AssembleFleet(AssembleFleet {
                        planet_id: p.planet_id,
                        ship_ids: mine[..n].to_vec(),
                        cargo,
                    }),
                ));
            }
        }
        for f in &seen.fleets.fleets {
            match f {
                FleetView::Stationed {
                    fleet_id,
                    ships,
                    cargo,
                    at_planet_id,
                    ..
                } => {
                    let colony = ships.iter().any(|s| s.ship_type == ShipType::ColonyShip);
                    let cargo_vessel = ships.iter().any(|s| s.ship_type == ShipType::CargoVessel);
                    let owned: Vec<PlanetId> = seen
                        .me
                        .planet_ids
                        .iter()
                        .copied()
                        .filter(|p| p != at_planet_id)
                        .collect();
                    let target = if colony && !self.free.is_empty() {
                        rng.pick(&self.free).map(|t| (*t, Mission::Colonize))
                    } else if !owned.is_empty() {
                        // A Transport needs a Cargo Vessel; without one, the fleet moves (the first
                        // full runs sent 1,165 transports the game refused for want of one).
                        let mission = if cargo_vessel {
                            Mission::Transport
                        } else {
                            Mission::Move
                        };
                        rng.pick(&owned).map(|t| (*t, mission))
                    } else {
                        self.free
                            .iter()
                            .chain(&self.others)
                            .copied()
                            .find(|p| p != at_planet_id)
                            .map(|t| (t, Mission::Move))
                    };
                    if let Some((target_planet_id, mission)) = target {
                        options.push((
                            40,
                            CommandRequest::SendFleet(SendFleet {
                                fleet_id: *fleet_id,
                                target_planet_id,
                                mission,
                            }),
                        ));
                    }
                    options.push((
                        3,
                        CommandRequest::DisbandFleet(FleetRef {
                            fleet_id: *fleet_id,
                        }),
                    ));
                    if cargo.iron_ore_g > 0 || cargo.iron_ingot_g > 0 {
                        options.push((
                            20,
                            CommandRequest::TriggerUnload(FleetRef {
                                fleet_id: *fleet_id,
                            }),
                        ));
                    }
                }
                FleetView::InTransit { fleet_id, .. } if rng.chance(UNDO_PER_MILLE) => {
                    options.push((
                        2,
                        CommandRequest::RecallFleet(FleetRef {
                            fleet_id: *fleet_id,
                        }),
                    ));
                }
                FleetView::InTransit { .. } | FleetView::Returning { .. } => {}
            }
        }
        let total: u64 = options.iter().map(|(w, _)| w).sum();
        if total == 0 {
            return None;
        }
        let mut at = self.rng.next() % total;
        let i = options.iter().position(|(w, _)| {
            if at < *w {
                true
            } else {
                at -= w;
                false
            }
        })?;
        Some(options.swap_remove(i).1)
    }

    /// One invalid command and the exact answer it must get, chosen from those the state allows.
    fn invalid(&mut self, seen: &Seen) -> (CommandRequest, (u16, ErrorCode), bool) {
        let none = || FleetId(NO_SUCH_ID);
        let mut options: Vec<(CommandRequest, (u16, ErrorCode), bool)> = vec![
            (
                CommandRequest::BuildBuilding(BuildBuilding {
                    planet_id: PlanetId(NO_SUCH_ID),
                    building_type: BuildingType::Drill,
                }),
                (422, ErrorCode::UnknownPlanet),
                false,
            ),
            (
                CommandRequest::DemolishBuilding(BuildingRef {
                    building_id: BuildingId(NO_SUCH_ID),
                }),
                (422, ErrorCode::UnknownBuilding),
                false,
            ),
            (
                CommandRequest::CancelShipBuild(CancelShipBuild {
                    ship_build_id: ShipBuildId(NO_SUCH_ID),
                }),
                (422, ErrorCode::UnknownShipBuild),
                false,
            ),
            (
                CommandRequest::RecallFleet(FleetRef { fleet_id: none() }),
                (422, ErrorCode::UnknownFleet),
                false,
            ),
            // A well-formed command with a key the API refuses (a space): 400.
            (
                CommandRequest::RecallFleet(FleetRef { fleet_id: none() }),
                (400, ErrorCode::InvalidIdempotencyKey),
                true,
            ),
        ];
        if let Some(theirs) = self.rng.pick(&self.others) {
            options.push((
                CommandRequest::BuildBuilding(BuildBuilding {
                    planet_id: *theirs,
                    building_type: BuildingType::Drill,
                }),
                (422, ErrorCode::NotOwner),
                false,
            ));
        }
        for f in &seen.fleets.fleets {
            if let FleetView::Stationed {
                fleet_id,
                at_planet_id,
                ..
            } = f
            {
                options.push((
                    CommandRequest::SendFleet(SendFleet {
                        fleet_id: *fleet_id,
                        target_planet_id: *at_planet_id,
                        mission: Mission::Move,
                    }),
                    (422, ErrorCode::SamePlanet),
                    false,
                ));
                options.push((
                    CommandRequest::RecallFleet(FleetRef {
                        fleet_id: *fleet_id,
                    }),
                    (422, ErrorCode::FleetNotInTransit),
                    false,
                ));
            }
        }
        if let Some(ship) = seen
            .planets
            .iter()
            .flat_map(|p| &p.roster)
            .find(|s| s.owner == seen.me.player_id)
        {
            let planet = seen
                .planets
                .iter()
                .find(|p| p.roster.iter().any(|s| s.ship_id == ship.ship_id))
                .map_or(seen.me.homeworld_id, |p| p.planet_id);
            options.push((
                CommandRequest::AssembleFleet(AssembleFleet {
                    planet_id: planet,
                    ship_ids: vec![ship.ship_id, ship.ship_id],
                    cargo: Cargo::default(),
                }),
                (400, ErrorCode::DuplicateShipIds),
                false,
            ));
        }
        options.swap_remove(self.rng.below(options.len()))
    }

    async fn tick(&mut self, tick: u64) -> Result<(), Failure> {
        let name = self.player.name;
        let seen = self.observe(tick).await?;
        self.tally.ticks += 1;
        if self.rng.chance(INVALID_PER_MILLE) {
            let (command, want, bad_key) = self.invalid(&seen);
            let key = if bad_key {
                "bad key".to_owned()
            } else {
                self.key()
            };
            let reply = self.player.command(&command, &key).await?;
            judge_invalid(want, &reply).map_err(|e| fail(name, format!("{command:?}: {e}")))?;
            *self.tally.invalid.entry(code_name(want.1)).or_default() += 1;
            return Ok(());
        }
        let Some(command) = self.valid(&seen) else {
            return Ok(());
        };
        let key = self.key();
        let reply = self.player.command(&command, &key).await?;
        self.tally.valid_sent += 1;
        // A send by its mission too (send_fleet:colonize), so the summary shows which ran.
        let kind = serde_json::to_value(&command)
            .ok()
            .and_then(|v| {
                let kind = v["type"].as_str()?.to_owned();
                Some(match v["mission"].as_str() {
                    Some(m) => format!("{kind}:{m}"),
                    None => kind,
                })
            })
            .unwrap_or_default();
        *self.tally.valid_by_type.entry(kind).or_default() += 1;
        match judge_valid(&reply).map_err(|e| fail(name, format!("{command:?}: {e}")))? {
            None => self.tally.accepted += 1,
            Some(code) => *self.tally.game_refused.entry(code).or_default() += 1,
        }
        if self.rng.chance(RETRY_PER_MILLE) {
            let again = self.player.command(&command, &key).await?;
            judge_retry(&reply, &again)
                .map_err(|e| fail(name, format!("{command:?} sent again with key {key}: {e}")))?;
            self.tally.retried += 1;
        }
        Ok(())
    }
}

/// Runs the simulation for `wall`: every player logs in through dex's password grant at `issuer`,
/// joins, and plays until the time is up.
///
/// # Errors
/// The first failed check, or a login or transport error.
pub async fn run(
    http: &reqwest::Client,
    api: &str,
    issuer: &str,
    run: &str,
    seed: u64,
    wall: Duration,
) -> Result<Tally, Failure> {
    let mut bots = Vec::new();
    for (i, user) in PLAYERS.iter().enumerate() {
        let player = Player::new(
            user.name,
            http.clone(),
            api,
            login::password(http, issuer, *user).await?,
        );
        crate::scenario::join(&player).await?;
        bots.push(Bot {
            player,
            rng: Rng::new(seed ^ ((i as u64 + 1) << 32)),
            run: run.to_owned(),
            sent: 0,
            tally: Tally::default(),
            others: Vec::new(),
            free: Vec::new(),
            home: None,
        });
    }
    tracing::info!(
        "sim: {} bots joined; playing for {} s, seed {seed}",
        bots.len(),
        wall.as_secs()
    );
    let started = Instant::now();
    let end = started + wall;
    let mut tasks = tokio::task::JoinSet::new();
    for mut bot in bots {
        tasks.spawn(async move {
            let mut tick = 0;
            while Instant::now() < end {
                let next = Instant::now() + TICK;
                bot.tick(tick).await?;
                tick += 1;
                tokio::time::sleep_until(next.min(end)).await;
            }
            Ok::<Bot, Failure>(bot)
        });
    }
    let mut tally = Tally {
        bots: PLAYERS.len(),
        seed,
        ..Tally::default()
    };
    while let Some(done) = tasks.join_next().await {
        let bot = done.map_err(|e| fail("sim", format!("a bot's task failed: {e}")))??;
        tally.add(&bot.tally);
        if let Some(home) = &bot.home {
            tracing::info!(
                "sim: {}'s homeworld at the end: {}",
                bot.player.name,
                serde_json::to_string(home).unwrap_or_default()
            );
        }
        tally.rate_limited += bot.player.rate_limited();
    }
    tally.wall_seconds = started.elapsed().as_secs_f64();
    judge_rate(tally.rate_limited).map_err(|e| fail("sim", e))?;
    Ok(tally)
}

/// Any 429 fails the run: serve's defaults must leave eight bots their margin (D-054).
///
/// # Errors
/// How many requests met one.
pub fn judge_rate(rate_limited: u64) -> Result<(), String> {
    if rate_limited == 0 {
        Ok(())
    } else {
        Err(format!(
            "{rate_limited} requests met a 429: serve's defaults must leave the bots margin (D-054)"
        ))
    }
}

#[cfg(test)]
mod tests;
