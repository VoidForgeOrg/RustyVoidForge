//! Views and score: named tests and the done-when property (Technical Plan P1.11: "A view at
//! any t before `valid_until` matches an advance to t"; Architectural Design, Engine core and
//! Read side and API; Game Design, Scoring; decision log D-030), on Rules v0 worlds. Names start
//! with what they cover: `view_*` (the done-when), `planet_*`, `validity_*`, `near_*`,
//! `score_*`, `fleets_*`.

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use voidforge_core::command::{Action, Command};
use voidforge_core::quantity::{MS_PER_HOUR, Quantity};
use voidforge_core::rules::{GRAMS_PER_TON, Rules};
use voidforge_core::solver::FlowLabel;
use voidforge_core::state::{
    BuildingKind, BuildingState, Cargo, DomainEvent, FleetState, Mission, PlanetId, PlayerId,
    Position, ShipBuildStatus, ShipKind, World,
};
use voidforge_core::view::{BuildingStatus, Query, View, ViewError};
use voidforge_testkit::determinism::{CAROL, Economy, canonical_bytes, recall_due, stops};
use voidforge_testkit::{ALICE, BOB, WorldBuilder, v0_rules};

const T: i64 = GRAMS_PER_TON;
const H: i64 = MS_PER_HOUR;

/// Every query a world supports: each planet, planets near each planet within 100 units, each
/// player (and an unknown one), and each player's fleets.
fn queries(w: &World) -> Vec<Query> {
    let mut q: Vec<Query> = w
        .planets
        .values()
        .flat_map(|p| {
            [
                Query::Planet { planet: p.id },
                Query::PlanetsNear {
                    center: p.position,
                    radius: 100_000,
                },
            ]
        })
        .collect();
    for player in w.players.keys().copied().chain([CAROL]) {
        q.push(Query::Player { player });
        q.push(Query::Fleets { viewer: player });
    }
    q
}

fn valid_until(v: &View) -> Option<i64> {
    match v {
        View::Planet(p) => p.valid_until,
        View::PlanetsNear(n) => n.valid_until,
        View::Player(p) => p.valid_until,
        View::Fleets(f) => f.valid_until,
    }
}

/// The done-when, at one world: for every query and several t from now up to its
/// `valid_until` − 1, the view equals the same view taken on a copy advanced to t, field for
/// field; at `valid_until` it is `Stale`. Returns how many (query, t) pairs were compared.
fn check_views(w: &World, rules: &Rules) -> usize {
    let now = w.time_ms;
    let before = canonical_bytes(w);
    let mut compared = 0;
    for q in queries(w) {
        let at_now = match w.view(&q, now, rules) {
            Ok(v) => v,
            Err(ViewError::Unknown) => continue,
            Err(e) => panic!("{q:?} at {now}: {e}"),
        };
        let until = valid_until(&at_now);
        let last = until.map_or(now + 10 * H, |v| v - 1);
        let mut times = vec![now, now + (last - now) / 3, last];
        times.dedup();
        for t in times {
            let projected = w.view(&q, t, rules).unwrap();
            let mut advanced = w.clone();
            advanced.advance_to(t, rules).unwrap();
            assert_eq!(
                projected,
                advanced.view(&q, t, rules).unwrap(),
                "{q:?} at {t} (world at {now})"
            );
            compared += 1;
        }
        if let Some(v) = until {
            assert_eq!(
                w.view(&q, v, rules),
                Err(ViewError::Stale { valid_until: v }),
                "{q:?}"
            );
        }
    }
    assert!(canonical_bytes(w) == before, "a view changed the world");
    compared
}

/// An economy with fleets and commands, checked at each of `points` stops through 100 hours.
fn economy_views(seed: u64, planets: u64, points: i64) -> usize {
    let rules = v0_rules();
    let e = Economy {
        seed,
        planets,
        fleets: true,
        commands: 60,
    };
    let (mut w, recalls) = e.start(&rules);
    let script = e.script();
    let mut compared = check_views(&w, &rules);
    for t in stops(100 * H, points, &e.instants(&recalls)) {
        w.advance_to(t, &rules).unwrap();
        recall_due(&mut w, &recalls).unwrap();
        for &(_, k) in script.iter().filter(|(at, _)| *at == t) {
            let command = e.command(k, &w);
            w.apply(&command, t, &rules).unwrap();
        }
        compared += check_views(&w, &rules);
    }
    compared
}

#[test]
fn view_at_any_t_before_valid_until_matches_an_advance_to_t() {
    let compared = economy_views(0x7e11_0001, 3, 12);
    assert!(compared > 500, "only {compared} comparisons");
}

fn config(cases: u32, seed: u64) -> Config {
    Config {
        cases,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(seed),
        ..Config::default()
    }
}

proptest! {
    #![proptest_config(config(12, 0x7e11_0000_0000_0002))]

    /// The done-when over generated economies with fleets and commands.
    #[test]
    fn view_matches_an_advance_over_generated_economies(seed in any::<u64>(), planets in 2u64..=5) {
        prop_assert!(economy_views(seed, planets, 6) > 100);
    }
}

// --- Planet ---

/// Alice's homeworld with a Drill under construction and a Shipyard with one active build,
/// solved at 0.
fn busy_home() -> (World, PlanetId) {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    b.construction(home, BuildingKind::Drill, 0);
    let yard = b.building(home, BuildingKind::Shipyard, BuildingState::Operational);
    b.ship_build(yard, ShipKind::CargoVessel, ShipBuildStatus::Active);
    b.ship_build(yard, ShipKind::ColonyShip, ShipBuildStatus::Waiting);
    b.ship(BOB, ShipKind::CargoVessel, home);
    b.player(BOB);
    let mut w = b.build();
    w.solve_planet(home, &v0_rules()).unwrap();
    (w, home)
}

fn planet(w: &World, p: PlanetId, t: i64) -> voidforge_core::view::PlanetView {
    match w
        .view(&Query::Planet { planet: p }, t, &v0_rules())
        .unwrap()
    {
        View::Planet(v) => v,
        other => panic!("{other:?}"),
    }
}

#[test]
fn planet_view_projects_stocks_builds_labels_and_roster_to_t() {
    let (w, home) = busy_home();
    let v = planet(&w, home, H);
    let p = &w.planets[&home];
    assert_eq!((v.as_of, v.owner, v.slots), (H, Some(ALICE), p.slots));
    assert_eq!(v.ore.amount_g, p.ore.value_at(H).unwrap());
    assert_eq!(v.ore.rate_g_per_h, p.ore.rate_g_per_h());
    assert_eq!((v.ore.cap_g, v.ingots.cap_g), (10_000 * T, 5_000 * T));
    assert_eq!(v.ingots.amount_g, p.ingots.value_at(H).unwrap());
    // The Drill's construction: 1 hour at 50 t/h, completing at exactly 3 h.
    let site = v
        .buildings
        .iter()
        .find(|b| b.status == BuildingStatus::UnderConstruction)
        .unwrap();
    let progress = site.progress.unwrap();
    assert_eq!(
        (
            progress.delivered_g,
            progress.cost_g,
            progress.rate_g_per_h,
            progress.eta_ms
        ),
        (50 * T, 150 * T, 50 * T, Some(3 * H))
    );
    assert_eq!(site.label, Some(FlowLabel::Running));
    // Operational buildings are labeled; the Shipyard has its active build.
    assert!(v.buildings.iter().all(|b| b.label.is_some()));
    let (active, waiting) = (&v.ship_builds[0], &v.ship_builds[1]);
    assert_eq!(
        (active.status, active.progress.delivered_g, active.label),
        (ShipBuildStatus::Active, 75 * T, Some(FlowLabel::Running))
    );
    assert_eq!(
        (
            waiting.status,
            waiting.progress.delivered_g,
            waiting.progress.eta_ms,
            waiting.label
        ),
        (ShipBuildStatus::Waiting, 0, None, None)
    );
    // Bob's ship on Alice's roster is public.
    assert_eq!(v.roster.len(), 1);
    assert_eq!(v.roster[0].2, BOB);
}

#[test]
fn planet_eta_is_the_exact_completion_instant() {
    let (mut w, home) = busy_home();
    let eta = planet(&w, home, 0).ship_builds[0].progress.eta_ms.unwrap();
    let events = w.advance_to(eta, &v0_rules()).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DomainEvent::ShipCompleted { .. })),
        "{events:?}"
    );
    // One millisecond earlier, nothing had completed.
    let (mut w, _) = busy_home();
    let before = w.advance_to(eta - 1, &v0_rules()).unwrap();
    assert!(
        !before
            .iter()
            .any(|e| matches!(e, DomainEvent::ShipCompleted { .. }))
    );
}

// --- Validity ---

#[test]
fn validity_is_the_next_event_and_the_view_is_stale_from_it() {
    let (mut w, home) = busy_home();
    let next = w.planets[&home].next_event.unwrap().at_ms;
    let v = planet(&w, home, 0);
    assert_eq!(v.valid_until, Some(next));
    // At valid_until − 1 it still equals the advanced world's view; at valid_until it is stale.
    let mut advanced = w.clone();
    advanced.advance_to(next - 1, &v0_rules()).unwrap();
    assert_eq!(
        planet(&w, home, next - 1),
        planet(&advanced, home, next - 1)
    );
    let q = Query::Planet { planet: home };
    assert_eq!(
        w.view(&q, next, &v0_rules()),
        Err(ViewError::Stale { valid_until: next })
    );
    // Before world time, or under other rules: typed errors.
    w.advance_to(H, &v0_rules()).unwrap();
    assert_eq!(
        w.view(&q, H - 1, &v0_rules()),
        Err(ViewError::BeforeNow { t: H - 1, now: H })
    );
    let mut other = v0_rules();
    other.version += 1;
    assert!(matches!(
        w.view(&q, H, &other),
        Err(ViewError::RulesMismatch { .. })
    ));
    let unknown = Query::Planet {
        planet: PlanetId(w.next_id),
    };
    assert_eq!(w.view(&unknown, H, &v0_rules()), Err(ViewError::Unknown));
}

#[test]
fn validity_ends_at_an_incoming_fleets_arrival_before_the_next_event() {
    // A Transport landing on Alice's colony changes its stocks: the colony's view ends there,
    // though the colony has no event of its own.
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let colony = b.planet(Some(ALICE));
    let hauler = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(
        &[hauler],
        Cargo {
            ore_g: 0,
            ingots_g: 100 * T,
        },
    );
    let mut w = b.build();
    w.send_fleet(fleet, colony, Mission::Transport, &v0_rules())
        .unwrap();
    let FleetState::InTransit { arrives_at_ms, .. } = w.fleets[&fleet].state else {
        unreachable!()
    };
    assert_eq!(w.planets[&colony].next_event, None);
    assert_eq!(planet(&w, colony, 0).valid_until, Some(arrives_at_ms));
    assert_eq!(w.planet_valid_until(colony), Some(arrives_at_ms));
}

// --- Planets near ---

#[test]
fn near_lists_planets_within_the_radius_inclusive_with_their_distance() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let edge = b.planet(None);
    let beyond = b.planet(None);
    for (p, x) in [(home, 0), (edge, 5_000), (beyond, 5_001)] {
        b.position(p, Position { x, y: 0, z: 0 });
    }
    let w = b.build();
    let q = Query::PlanetsNear {
        center: Position { x: 0, y: 0, z: 0 },
        radius: 5_000,
    };
    let View::PlanetsNear(near) = w.view(&q, 0, &v0_rules()).unwrap() else {
        unreachable!()
    };
    let got: Vec<_> = near
        .planets
        .iter()
        .map(|p| (p.id, p.distance, p.owner))
        .collect();
    assert_eq!(got, [(home, 0, Some(ALICE)), (edge, 5_000, None)]);
    assert_eq!(near.valid_until, None);
}

#[test]
fn near_is_valid_until_a_colony_fleet_lands_on_a_listed_planet() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let free = b.planet(None);
    let ship = b.ship(ALICE, ShipKind::ColonyShip, home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let mut w = b.build();
    w.send_fleet(fleet, free, Mission::Colonize, &v0_rules())
        .unwrap();
    let FleetState::InTransit { arrives_at_ms, .. } = w.fleets[&fleet].state else {
        unreachable!()
    };
    let q = Query::PlanetsNear {
        center: w.planets[&free].position,
        radius: 0,
    };
    let View::PlanetsNear(near) = w.view(&q, 0, &v0_rules()).unwrap() else {
        unreachable!()
    };
    assert_eq!(near.valid_until, Some(arrives_at_ms));
}

// --- What a viewer may see (D-049): planets are public, fleets and roster ships owner-only ---

fn planet_for(
    w: &World,
    p: PlanetId,
    viewer: PlayerId,
    t: i64,
) -> voidforge_core::view::PlanetView {
    match w
        .view_for(&Query::Planet { planet: p }, viewer, t, &v0_rules())
        .unwrap()
    {
        View::Planet(v) => v,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_planets_roster_shows_each_viewer_only_their_own_ships() {
    let (w, home) = busy_home();
    let all = planet(&w, home, 0).roster;
    assert!(
        all.iter().any(|s| s.2 == BOB),
        "the fixture has Bob's ship on Alice's home"
    );
    for viewer in [ALICE, BOB, CAROL] {
        let seen = planet_for(&w, home, viewer, 0).roster;
        let own: Vec<_> = all.iter().copied().filter(|s| s.2 == viewer).collect();
        assert_eq!(seen, own, "{viewer:?}");
    }
    // Everything else about the planet is the same for every viewer.
    let mut alice = planet_for(&w, home, ALICE, 0);
    let mut bob = planet_for(&w, home, BOB, 0);
    alice.roster.clear();
    bob.roster.clear();
    assert_eq!(alice, bob);
}

/// Bob's colony fleet is heading for a free planet. Bob's views end at its arrival; everyone
/// else's don't show it, though the true validity still makes every view stale there.
fn bob_colonizing() -> (World, PlanetId, i64) {
    let mut b = WorldBuilder::new();
    b.player(ALICE);
    let bob_home = b.player(BOB);
    let free = b.planet(None);
    let ship = b.ship(BOB, ShipKind::ColonyShip, bob_home);
    let fleet = b.fleet(&[ship], Cargo::default());
    let mut w = b.build();
    w.send_fleet(fleet, free, Mission::Colonize, &v0_rules())
        .unwrap();
    let FleetState::InTransit { arrives_at_ms, .. } = w.fleets[&fleet].state else {
        unreachable!()
    };
    (w, free, arrives_at_ms)
}

#[test]
fn another_players_inbound_fleet_is_hidden_from_a_planets_validity() {
    let (w, free, arrives) = bob_colonizing();
    assert_eq!(w.planets[&free].next_event, None);
    assert_eq!(
        planet(&w, free, 0).valid_until,
        Some(arrives),
        "the true validity"
    );
    assert_eq!(
        planet_for(&w, free, BOB, 0).valid_until,
        Some(arrives),
        "Bob's own fleet"
    );
    assert_eq!(
        planet_for(&w, free, ALICE, 0).valid_until,
        None,
        "hidden from Alice"
    );
    assert_eq!(w.planet_public_valid_until(free, ALICE), None);
    // Staleness is the true validity's, for every viewer.
    for viewer in [ALICE, BOB] {
        assert_eq!(
            w.view_for(
                &Query::Planet { planet: free },
                viewer,
                arrives,
                &v0_rules()
            ),
            Err(ViewError::Stale {
                valid_until: arrives
            })
        );
    }
}

#[test]
fn another_players_inbound_fleet_is_hidden_from_planets_near() {
    let (w, free, arrives) = bob_colonizing();
    let q = Query::PlanetsNear {
        center: w.planets[&free].position,
        radius: 0,
    };
    let near = |viewer| match w.view_for(&q, viewer, 0, &v0_rules()).unwrap() {
        View::PlanetsNear(n) => n.valid_until,
        other => panic!("{other:?}"),
    };
    assert_eq!(near(BOB), Some(arrives));
    assert_eq!(near(ALICE), None);
    assert_eq!(
        w.view_for(&q, ALICE, arrives, &v0_rules()),
        Err(ViewError::Stale {
            valid_until: arrives
        })
    );
}

#[test]
fn another_players_inbound_fleet_is_hidden_from_a_players_validity() {
    // Alice got there first: Bob's colony fleet is still heading for what is now her planet.
    let (mut w, free, arrives) = bob_colonizing();
    w.planets.get_mut(&free).unwrap().owner = Some(ALICE);
    let player = |viewer, player| match w
        .view_for(&Query::Player { player }, viewer, 0, &v0_rules())
        .unwrap()
    {
        View::Player(p) => p.valid_until,
        other => panic!("{other:?}"),
    };
    let View::Player(truth) = w
        .view(&Query::Player { player: ALICE }, 0, &v0_rules())
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(truth.valid_until, Some(arrives), "the true validity");
    assert_eq!(
        player(ALICE, ALICE),
        None,
        "Bob's fleet is hidden from Alice"
    );
    assert_eq!(player(BOB, BOB), Some(arrives), "Bob's own fleet moving");
    assert_eq!(
        player(ALICE, BOB),
        None,
        "Bob's fleets are hidden from Alice's view of Bob"
    );
}

#[test]
fn a_fleets_query_for_another_viewer_is_unknown() {
    let (w, _, _) = bob_colonizing();
    assert_eq!(
        w.view_for(&Query::Fleets { viewer: BOB }, ALICE, 0, &v0_rules()),
        Err(ViewError::Unknown)
    );
    assert!(
        w.view_for(&Query::Fleets { viewer: BOB }, BOB, 0, &v0_rules())
            .is_ok()
    );
}

// --- Score (Rules v0: planet 1,000; Drill 100, Refinery 150, Shipyard 500; Colony Ship 400,
// Cargo Vessel 150; ore 1 per 10 t, ingots 2 per 10 t) ---

fn score(w: &World, player: PlayerId, t: i64) -> i64 {
    w.score(player, t, &v0_rules()).unwrap()
}

/// The Phase 1 gate's finding (review-r2): rules that validate can give a true score beyond
/// i128. Three planets full of ore at i64::MAX points per gram total about 1.5 × 2^127: the score
/// is ScoreOutOfRange in every build profile, never a panic or a wrapped 0. Beyond i64 but
/// within i128 is ScoreOutOfRange too; ordinary scores are exact (the tests below).
#[test]
fn score_beyond_the_range_is_score_out_of_range_never_a_panic_or_a_wrap() {
    let mut rules = v0_rules();
    rules.planet.ore_cap_g = i64::MAX;
    rules.scoring.ore = voidforge_core::rules::MassPoints {
        points: i64::MAX,
        per_g: 1,
    };
    rules.validate().unwrap();
    let world = |planets: usize| {
        let mut b = WorldBuilder::with_rules(rules.clone());
        let home = b.player(ALICE);
        let mut owned = vec![home];
        for _ in 1..planets {
            owned.push(b.planet(Some(ALICE)));
        }
        for p in owned {
            b.stocks(p, Quantity::new(i64::MAX, 0, 0), Quantity::new(0, 0, 0));
        }
        b.build()
    };
    for planets in [1, 3] {
        assert_eq!(
            world(planets).score(ALICE, 0, &rules),
            Err(ViewError::ScoreOutOfRange),
            "{planets} planets"
        );
    }
    // The player view reports it as a typed error as well.
    let w = world(3);
    assert_eq!(
        w.view(&Query::Player { player: ALICE }, 0, &rules),
        Err(ViewError::ScoreOutOfRange)
    );
}

#[test]
fn score_counts_planets_buildings_ships_and_resources() {
    // A homeworld: 1 planet + Drill + Refinery = 1,250; 2,000 t ore = 200; 1,500 t ingots = 300.
    let mut b = WorldBuilder::new();
    b.player(ALICE);
    b.player(BOB);
    let w = b.build();
    assert_eq!(score(&w, ALICE, 0), 1_000 + 100 + 150 + 200 + 300);
    // Plus a second planet, a Shipyard and two ships: one on the roster, one in a fleet on
    // another player's planet, each counted once.
    let mut b = WorldBuilder::new();
    let home2 = b.player(ALICE);
    let bob = b.player(BOB);
    b.planet(Some(ALICE));
    b.building(home2, BuildingKind::Shipyard, BuildingState::Operational);
    b.ship(ALICE, ShipKind::ColonyShip, home2);
    let away = b.ship(ALICE, ShipKind::CargoVessel, bob);
    b.fleet(&[away], Cargo::default());
    let w2 = b.build();
    assert_eq!(score(&w2, ALICE, 0), 1_750 + 1_000 + 500 + 400 + 150);
}

#[test]
fn score_gives_partial_credit_rounded_down_for_builds_in_progress() {
    // A Shipyard construction (cost 1,200 t, 500 points) at 0, at cost − 1 g and at cost; a
    // Colony Ship build (800 t, 400 points) at 1 t.
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    b.stocks(home, Quantity::new(0, 0, 0), Quantity::new(0, 0, 0));
    let site = b.construction(home, BuildingKind::Shipyard, 0);
    let yard = b.building(home, BuildingKind::Shipyard, BuildingState::Operational);
    let build = b.ship_build(yard, ShipKind::ColonyShip, ShipBuildStatus::Active);
    let mut w = b.build();
    let base = 1_000 + 100 + 150 + 500; // planet, Drill, Refinery, Shipyard
    let set = |w: &mut World, site_g: i64, build_g: i64| {
        if let BuildingState::UnderConstruction { delivered, .. } =
            &mut w.buildings.get_mut(&site).unwrap().state
        {
            *delivered = Quantity::new(site_g, 0, 0);
        }
        w.ship_builds.get_mut(&build).unwrap().delivered = Quantity::new(build_g, 0, 0);
    };
    set(&mut w, 0, 0);
    assert_eq!(score(&w, ALICE, 0), base);
    // 500 × (1,200 t − 1 g) ÷ 1,200 t = 499.99…, rounded down to 499; 400 × 1 t ÷ 800 t = 0.5 → 0.
    set(&mut w, 1_200 * T - 1, T);
    assert_eq!(score(&w, ALICE, 0), base + 499);
    // 400 × 2 t ÷ 800 t = 1.
    set(&mut w, 1_200 * T, 2 * T);
    assert_eq!(score(&w, ALICE, 0), base + 500 + 1);
}

#[test]
fn score_rounds_each_resource_once_over_storage_and_cargo() {
    // 5 t of ore on each of two planets and 5 t aboard a fleet: 15 t = 1 point, where rounding
    // per place would give 0. Moving cargo between storage and a fleet never changes score.
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let colony = b.planet(Some(ALICE));
    b.stocks(home, Quantity::new(5 * T, 0, 0), Quantity::new(0, 0, 0));
    b.stocks(colony, Quantity::new(5 * T, 0, 0), Quantity::new(0, 0, 0));
    let ship = b.ship(ALICE, ShipKind::CargoVessel, home);
    b.fleet(
        &[ship],
        Cargo {
            ore_g: 5 * T,
            ingots_g: 0,
        },
    );
    let w = b.build();
    let fixed = 2 * 1_000 + 100 + 150 + 150;
    assert_eq!(score(&w, ALICE, 0), fixed + 1);
}

/// P1.11 QA's carry: ore aboard a fleet counts on its own. With empty storage, 10 t of ore
/// aboard is exactly 1 point, so leaving fleet ore out of the score loses it.
#[test]
fn score_counts_ore_aboard_a_fleet_with_empty_storage() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    b.stocks(home, Quantity::new(0, 0, 0), Quantity::new(0, 0, 0));
    let ship = b.ship(ALICE, ShipKind::CargoVessel, home);
    b.fleet(
        &[ship],
        Cargo {
            ore_g: 10 * T,
            ingots_g: 0,
        },
    );
    let w = b.build();
    assert_eq!(score(&w, ALICE, 0), 1_250 + 150 + 1);
}

#[test]
fn score_counts_ships_in_transit_and_cargo_aboard_moving_fleets() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let free = b.planet(None);
    b.stocks(home, Quantity::new(0, 0, 0), Quantity::new(0, 0, 0));
    let ship = b.ship(ALICE, ShipKind::CargoVessel, home);
    let fleet = b.fleet(
        &[ship],
        Cargo {
            ore_g: 0,
            ingots_g: 50 * T,
        },
    );
    let mut w = b.build();
    let at_rest = score(&w, ALICE, 0);
    w.send_fleet(fleet, free, Mission::Move, &v0_rules())
        .unwrap();
    assert_eq!(score(&w, ALICE, 1), at_rest);
    assert_eq!(at_rest, 1_250 + 150 + 10);
}

#[test]
fn score_a_building_being_demolished_counts_nothing() {
    // D-039 (Tom, 2026-10-18): a building counts from the moment it is operational until its
    // demolition begins. The same homeworld with and without a Shipyard being demolished (ending
    // at 2 h); its Drill and Refinery produce the same in both, so the scores are equal
    // throughout the demolition and after it.
    let world = |demolishing: bool| {
        let mut b = WorldBuilder::new();
        let home = b.player(ALICE);
        b.stocks(home, Quantity::new(0, 0, 0), Quantity::new(0, 0, 0));
        if demolishing {
            b.demolition(home, BuildingKind::Shipyard);
        }
        let mut w = b.build();
        w.solve_planet(home, &v0_rules()).unwrap();
        w
    };
    let (mut with, mut without) = (world(true), world(false));
    assert_eq!(score(&with, ALICE, 0), score(&without, ALICE, 0));
    assert_eq!(
        score(&with, ALICE, 2 * H - 1),
        score(&without, ALICE, 2 * H - 1)
    );
    for w in [&mut with, &mut without] {
        w.advance_to(2 * H, &v0_rules()).unwrap();
    }
    assert_eq!(score(&with, ALICE, 2 * H), score(&without, ALICE, 2 * H));
}

#[test]
fn score_drops_at_the_instant_demolition_begins() {
    // D-039: an operational Shipyard (500 points) counts at every t before the Demolish command,
    // and its points leave the score at the very t the command is applied, through `apply`.
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    b.stocks(home, Quantity::new(0, 0, 0), Quantity::new(0, 0, 0));
    let yard = b.building(home, BuildingKind::Shipyard, BuildingState::Operational);
    let mut w = b.build();
    let rules = v0_rules();
    w.solve_planet(home, &rules).unwrap();
    let before = w.clone();
    let t = H;
    let applied = w
        .apply(
            &Command {
                player: ALICE,
                action: Action::DemolishBuilding { building: yard },
            },
            t,
            &rules,
        )
        .unwrap();
    assert!(applied.outcome.is_ok(), "Demolish was rejected");
    assert!(matches!(
        w.buildings[&yard].state,
        BuildingState::Demolishing { .. }
    ));
    // One millisecond before, it still counts; at t it counts nothing, and nothing else moved.
    assert_eq!(score(&w, ALICE, t), score(&before, ALICE, t) - 500);
    assert_eq!(score(&before, ALICE, t - 1), score(&before, ALICE, t));
}

// --- Fleets ---

#[test]
fn fleets_are_visible_only_to_their_owner() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let bob_home = b.player(BOB);
    let mine = b.ship(ALICE, ShipKind::CargoVessel, home);
    let theirs = b.ship(BOB, ShipKind::ColonyShip, bob_home);
    let alice_fleet = b.fleet(
        &[mine],
        Cargo {
            ore_g: T,
            ingots_g: 0,
        },
    );
    b.fleet(&[theirs], Cargo::default());
    let w = b.build();
    let View::Fleets(seen) = w
        .view(&Query::Fleets { viewer: ALICE }, 0, &v0_rules())
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(seen.fleets.len(), 1);
    let f = &seen.fleets[0];
    assert_eq!(
        (f.id, f.cargo.ore_g, f.capacity_g, f.speed_du_per_h),
        (alice_fleet, T, 500 * T, 15_000)
    );
    let View::Fleets(none) = w
        .view(&Query::Fleets { viewer: CAROL }, 0, &v0_rules())
        .unwrap()
    else {
        unreachable!()
    };
    assert!(none.fleets.is_empty());
}

#[test]
fn fleets_are_valid_until_the_first_arrival_or_return_among_them() {
    let mut b = WorldBuilder::new();
    let home = b.player(ALICE);
    let near = b.planet(None);
    let ships = [
        b.ship(ALICE, ShipKind::CargoVessel, home),
        b.ship(ALICE, ShipKind::ColonyShip, home),
    ];
    let fast = b.fleet(&ships[..1], Cargo::default());
    let slow = b.fleet(&ships[1..], Cargo::default());
    let mut w = b.build();
    for f in [fast, slow] {
        w.send_fleet(f, near, Mission::Move, &v0_rules()).unwrap();
    }
    let arrives = |f| match w.fleets[&f].state {
        FleetState::InTransit { arrives_at_ms, .. } => arrives_at_ms,
        _ => unreachable!(),
    };
    let first = arrives(fast).min(arrives(slow));
    let View::Fleets(v) = w
        .view(&Query::Fleets { viewer: ALICE }, 0, &v0_rules())
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(v.valid_until, Some(first));
}
