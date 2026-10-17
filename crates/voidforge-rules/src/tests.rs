//! Rules file tests. Expected values are written in the game design's own units (tons, hours,
//! world units per hour) and converted here, so each assertion traces to a Rules v0 cell.

use voidforge_core::quantity::{MS_PER_HOUR, Quantity};
use voidforge_core::rules::{BuildCost, DISTANCE_UNITS_PER_WORLD_UNIT, GRAMS_PER_TON, Rules};

use super::*;

const V0: &str = include_str!("../../../rules/v0.toml");
const V0_FAST: &str = include_str!("../../../rules/v0-fast.toml");

/// The fast variant's factor: an hour of play takes a second.
const SPEEDUP: i64 = 3_600;

fn t(tons: i64) -> i64 {
    tons * GRAMS_PER_TON
}

fn h(hours: i64) -> i64 {
    hours * MS_PER_HOUR
}

fn per_h(tons_per_hour: i64) -> i64 {
    tons_per_hour * GRAMS_PER_TON
}

fn units(world_units: i64) -> i64 {
    world_units * DISTANCE_UNITS_PER_WORLD_UNIT
}

fn v0() -> Rules {
    parse(V0).expect("rules/v0.toml is valid")
}

#[test]
fn v0_matches_the_game_design_tables() {
    let r = v0();
    assert_eq!((r.version, r.name.as_str()), (0, "v0"));

    // Planet values.
    assert_eq!(r.planet.slots, 8);
    assert_eq!(r.planet.ore_cap_g, t(10_000));
    assert_eq!(r.planet.ingot_cap_g, t(5_000));

    // Starting conditions and Rules v0: 1 Drill, 1 Refinery, 2,000 t ore, 1,500 t ingots.
    assert_eq!((r.homeworld.drills, r.homeworld.refineries), (1, 1));
    assert_eq!(r.homeworld.ore_g, t(2_000));
    assert_eq!(r.homeworld.ingots_g, t(1_500));

    // Buildings: cost, build time, demolition time, output.
    let b = r.buildings;
    assert_eq!(
        b.drill.build,
        BuildCost {
            cost_g: t(150),
            build_time_ms: h(3)
        }
    );
    assert_eq!(b.drill.demolition_time_ms, h(1));
    assert_eq!(b.drill.output_g_per_h, per_h(10));
    assert_eq!(
        b.refinery.build,
        BuildCost {
            cost_g: t(250),
            build_time_ms: h(4)
        }
    );
    assert_eq!(b.refinery.demolition_time_ms, h(1));
    assert_eq!(b.refinery.intake_g_per_h, per_h(8));
    assert_eq!(b.refinery.output_g_per_h, per_h(16));
    assert_eq!(b.refinery.ingots_per_ore, 2);
    assert_eq!(
        b.shipyard.build,
        BuildCost {
            cost_g: t(1_200),
            build_time_ms: h(12)
        }
    );
    assert_eq!(b.shipyard.demolition_time_ms, h(2));
    assert_eq!(b.shipyard.parallel_builds, 3);

    // Ships: cost, build time, speed, cargo capacity.
    let s = r.ships;
    assert_eq!(
        s.colony_ship.build,
        BuildCost {
            cost_g: t(800),
            build_time_ms: h(10)
        }
    );
    assert_eq!(s.colony_ship.speed_du_per_h, units(10));
    assert_eq!(s.colony_ship.cargo_capacity_g, 0);
    assert_eq!(
        s.cargo_vessel.build,
        BuildCost {
            cost_g: t(300),
            build_time_ms: h(4)
        }
    );
    assert_eq!(s.cargo_vessel.speed_du_per_h, units(15));
    assert_eq!(s.cargo_vessel.cargo_capacity_g, t(500));

    // Score items.
    let p = r.scoring;
    assert_eq!(
        (
            p.planet,
            p.drill,
            p.refinery,
            p.shipyard,
            p.colony_ship,
            p.cargo_vessel
        ),
        (1_000, 100, 150, 500, 400, 150)
    );
    assert_eq!((p.ore.points, p.ore.per_g), (1, t(10)));
    assert_eq!((p.ingots.points, p.ingots.per_g), (2, t(10)));

    // Technical Plan, P1.12 world generation defaults.
    let w = r.worldgen;
    assert_eq!(w.planets_per_expected_player, 20);
    assert_eq!(w.cube_size_du, units(1_000));
    assert_eq!(w.min_planet_spacing_du, units(10));
    assert_eq!(w.homeworld_min_spacing_du, units(150));
    assert_eq!(w.homeworld_neighbor_radius_du, units(100));
    assert_eq!(w.homeworld_min_neighbors, 5);
}

#[test]
fn v0_matches_the_game_designs_worked_examples() {
    let r = v0();
    // "A Drill takes 50 t of ingots per hour for 3 hours."
    assert_eq!(r.buildings.drill.build.rate_g_per_h(), Some(per_h(50)));
    // "...while its Drill and Refinery bring in 16 t of ingots per hour."
    assert_eq!(r.buildings.refinery.output_g_per_h, per_h(16));
    // "Refined into Iron Ingots at 1 t of ore to 2 t of ingots."
    assert_eq!(
        r.buildings.refinery.output_g_per_h,
        2 * r.buildings.refinery.intake_g_per_h
    );
    // "...enough to start a Shipyard on day one": 1,500 t of ingots covers its 1,200 t.
    assert!(r.homeworld.ingots_g >= r.buildings.shipyard.build.cost_g);
}

/// Rules v0 as the fast variant should be: rates and speeds x3,600, durations /3,600 (which
/// must divide exactly), everything else unchanged.
fn accelerated(base: &Rules) -> Rules {
    let rate = |g_per_h: i64| g_per_h.checked_mul(SPEEDUP).expect("fast rate fits in i64");
    let time = |ms: i64| {
        assert_eq!(ms % SPEEDUP, 0, "{ms} ms does not divide by {SPEEDUP}");
        ms / SPEEDUP
    };
    let build = |b: BuildCost| BuildCost {
        cost_g: b.cost_g,
        build_time_ms: time(b.build_time_ms),
    };
    let mut fast = base.clone();
    fast.version = 1;
    fast.name = "v0-fast".to_owned();
    let bs = &mut fast.buildings;
    bs.drill.build = build(bs.drill.build);
    bs.drill.demolition_time_ms = time(bs.drill.demolition_time_ms);
    bs.drill.output_g_per_h = rate(bs.drill.output_g_per_h);
    bs.refinery.build = build(bs.refinery.build);
    bs.refinery.demolition_time_ms = time(bs.refinery.demolition_time_ms);
    bs.refinery.intake_g_per_h = rate(bs.refinery.intake_g_per_h);
    bs.refinery.output_g_per_h = rate(bs.refinery.output_g_per_h);
    bs.shipyard.build = build(bs.shipyard.build);
    bs.shipyard.demolition_time_ms = time(bs.shipyard.demolition_time_ms);
    for ship in [&mut fast.ships.colony_ship, &mut fast.ships.cargo_vessel] {
        ship.build = build(ship.build);
        ship.speed_du_per_h = rate(ship.speed_du_per_h);
    }
    fast
}

#[test]
fn v0_fast_is_exactly_v0_with_an_hour_per_second() {
    let fast = parse(V0_FAST).expect("rules/v0-fast.toml is valid");
    assert_eq!(fast, accelerated(&v0()));
}

#[test]
fn v0_fast_reads_like_v0_on_the_unchanged_clock() {
    let (slow, fast) = (v0(), parse(V0_FAST).unwrap());
    // A Drill's output: one fast second equals one v0 hour, read through P1.1 quantities.
    let slow_ore = Quantity::new(0, slow.buildings.drill.output_g_per_h, 0);
    let fast_ore = Quantity::new(0, fast.buildings.drill.output_g_per_h, 0);
    assert_eq!(
        fast_ore.value_at(1_000).unwrap(),
        slow_ore.value_at(MS_PER_HOUR).unwrap()
    );
    assert_eq!(fast_ore.value_at(1_000).unwrap(), t(10));
    // Each build, fed at its even rate, completes exactly at its build time in both variants.
    for rules in [&slow, &fast] {
        let b = &rules.buildings;
        let s = &rules.ships;
        for build in [
            b.drill.build,
            b.refinery.build,
            b.shipyard.build,
            s.colony_ship.build,
            s.cargo_vessel.build,
        ] {
            let delivered = Quantity::new(
                0,
                build
                    .rate_g_per_h()
                    .expect("validated rules have exact rates"),
                0,
            );
            assert_eq!(
                delivered.rising_crossing(build.cost_g).unwrap(),
                Some(build.build_time_ms)
            );
        }
    }
    // Fast builds take a 3,600th of the time; the clock itself is not scaled.
    assert_eq!(
        fast.buildings.shipyard.build.build_time_ms * SPEEDUP,
        slow.buildings.shipyard.build.build_time_ms
    );
    assert_eq!(MS_PER_HOUR, 3_600_000);
}

// Invalid files: each is the real v0 file with one edit, so a failure can only come from it.

fn edited(old: &str, new: &str) -> String {
    assert_eq!(
        V0.matches(old).count(),
        1,
        "the edit anchor must occur once: {old}"
    );
    V0.replacen(old, new, 1)
}

fn schema_error(text: &str) -> String {
    match parse(text) {
        Err(Error::Schema(message)) => message,
        other => panic!("expected a schema error, got {other:?}"),
    }
}

fn invalid(text: &str) -> RulesError {
    match parse(text) {
        Err(Error::Invalid(error)) => error,
        other => panic!("expected an invalid-rules error, got {other:?}"),
    }
}

#[test]
fn malformed_toml_is_rejected_with_its_line() {
    let message = schema_error(&edited("slots = 8 ", "slots = = 8 "));
    assert!(message.contains("line 12"), "{message}");
}

#[test]
fn a_missing_field_is_named() {
    let message = schema_error(&edited(
        "parallel_builds = 3                   # up to 3 ships at once\n",
        "",
    ));
    assert!(
        message.contains("missing field `parallel_builds`"),
        "{message}"
    );
}

#[test]
fn an_unknown_field_is_named() {
    let message = schema_error(&edited("slots = 8 ", "sluts = 8 "));
    assert!(message.contains("unknown field `sluts`"), "{message}");
}

#[test]
fn floats_are_rejected_not_rounded() {
    let message = schema_error(&edited("cost_g = 150_000_000 ", "cost_g = 150000000.0 "));
    assert!(
        message.contains("invalid type: floating point"),
        "{message}"
    );
}

#[test]
fn strings_negative_and_out_of_range_integers_are_rejected() {
    let message = schema_error(&edited("slots = 8 ", "slots = \"8\" "));
    assert!(message.contains("invalid type: string"), "{message}");
    let message = schema_error(&edited("slots = 8 ", "slots = -1 "));
    assert!(message.contains("invalid value: integer `-1`"), "{message}");
    let message = schema_error(&edited(
        "cost_g = 150_000_000 ",
        "cost_g = 9_223_372_036_854_775_808 ",
    ));
    assert!(message.contains("line 23"), "{message}");
}

#[test]
fn a_missing_version_is_rejected() {
    let message = schema_error(&edited("version = 0\n", ""));
    assert!(message.contains("missing field `version`"), "{message}");
}

#[test]
fn invariant_violations_name_the_field() {
    let cases: &[(&str, &str, &str, &str)] = &[
        ("name = \"v0\"", "name = \" \"", "name", "must not be empty"),
        (
            "slots = 8 ",
            "slots = 0 ",
            "planet.slots",
            "must be positive, got 0",
        ),
        (
            "drills = 1 ",
            "drills = 9 ",
            "homeworld",
            "10 starting buildings do not fit in 8 slots",
        ),
        (
            "ore_g = 2_000_000_000 ",
            "ore_g = 10_000_000_001 ",
            "homeworld.ore_g",
            "exceeds planet.ore_cap_g",
        ),
        (
            "ingots_g = 1_500_000_000 ",
            "ingots_g = -1 ",
            "homeworld.ingots_g",
            "must not be negative",
        ),
        (
            "build_time_ms = 10_800_000 ",
            "build_time_ms = 7 ",
            "buildings.drill.cost_g",
            "not a whole number of grams per hour",
        ),
        (
            "build_time_ms = 43_200_000 ",
            "build_time_ms = 0 ",
            "buildings.shipyard.build_time_ms",
            "must be positive",
        ),
        (
            "output_g_per_h = 16_000_000 ",
            "output_g_per_h = 15_000_000 ",
            "buildings.refinery.output_g_per_h",
            "8000000 x 2 = 16000000",
        ),
        (
            "parallel_builds = 3 ",
            "parallel_builds = 0 ",
            "buildings.shipyard.parallel_builds",
            "must be positive",
        ),
        (
            "speed_du_per_h = 10_000 ",
            "speed_du_per_h = 0 ",
            "ships.colony_ship.speed_du_per_h",
            "must be positive",
        ),
        (
            "cargo_capacity_g = 500_000_000 ",
            "cargo_capacity_g = 0 ",
            "ships.cargo_vessel.cargo_capacity_g",
            "must be positive",
        ),
        (
            "cargo_capacity_g = 0 ",
            "cargo_capacity_g = -5 ",
            "ships.colony_ship.cargo_capacity_g",
            "must not be negative",
        ),
        (
            "drill = 100\n",
            "drill = -100\n",
            "scoring.drill",
            "must not be negative",
        ),
        (
            "ore = { points = 1, per_g = 10_000_000 }",
            "ore = { points = 1, per_g = 0 }",
            "scoring.ore.per_g",
            "must be positive",
        ),
        (
            "min_planet_spacing_du = 10_000 ",
            "min_planet_spacing_du = 1_000_000 ",
            "worldgen.min_planet_spacing_du",
            "must be less than cube_size_du",
        ),
    ];
    for (old, new, field, problem) in cases {
        let error = invalid(&edited(old, new));
        assert_eq!(error.field, *field, "editing `{old}`");
        assert!(error.problem.contains(problem), "editing `{old}`: {error}");
    }
}

#[test]
fn a_build_rate_beyond_64_bits_is_rejected() {
    let text = edited(
        "cost_g = 150_000_000 ",
        "cost_g = 9_000_000_000_000_000_000 ",
    )
    .replacen("build_time_ms = 10_800_000 ", "build_time_ms = 1 ", 1);
    let error = invalid(&text);
    assert_eq!(error.field, "buildings.drill.cost_g");
    assert!(error.problem.contains("does not fit in 64 bits"), "{error}");
}

#[test]
fn a_colony_ship_may_carry_nothing() {
    // Zero capacity is valid for ships that carry no cargo; v0's Colony Ship relies on it.
    assert_eq!(v0().ships.colony_ship.cargo_capacity_g, 0);
}
