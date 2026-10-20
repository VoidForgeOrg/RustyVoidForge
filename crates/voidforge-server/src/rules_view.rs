//! `GET /rules` (P4.1r, D-058): the rules the running universe was loaded with, as
//! [`RulesView`]. `serve` maps the `Rules` it verified against the universe (D-050) once, and
//! the route answers the bytes built then: no file is read per request, and nothing can go
//! stale, since the rules are fixed for the process.
//!
//! The mapping destructures every spec **without `..`**, so a new building or ship kind, or a new
//! field, doesn't compile until it's mapped or explicitly dropped, as worldgen is (the advisor's
//! A1: a seed-time detail).

use axum::body::Bytes;
use voidforge_api_types::rules::{
    BuildingRules, DrillRules, HomeworldRules, MassPointsRules, PlanetRules, RefineryRules,
    RulesView, ScoringRules, ShipKindRules, ShipRules, ShipyardRules,
};
use voidforge_host::Rules;
use voidforge_host::rules::{
    BuildCost, BuildingSpecs, DrillSpec, HomeworldSpec, MassPoints, PlanetSpec, RefinerySpec,
    ScoringSpecs, ShipSpec, ShipSpecs, ShipyardSpec,
};

/// The view of `rules`, every field named.
#[must_use]
pub fn view(rules: &Rules) -> RulesView {
    let Rules {
        version,
        name,
        planet,
        homeworld,
        buildings,
        ships,
        scoring,
        worldgen: _,
    } = rules;
    let PlanetSpec {
        slots,
        ore_cap_g,
        ingot_cap_g,
    } = *planet;
    let HomeworldSpec {
        drills,
        refineries,
        ore_g,
        ingots_g,
    } = *homeworld;
    let BuildingSpecs {
        drill,
        refinery,
        shipyard,
    } = *buildings;
    let ShipSpecs {
        colony_ship,
        cargo_vessel,
    } = *ships;
    RulesView {
        version: *version,
        name: name.clone(),
        planet: PlanetRules {
            slots,
            ore_cap_g,
            ingot_cap_g,
        },
        homeworld: HomeworldRules {
            drills,
            refineries,
            ore_g,
            ingots_g,
        },
        buildings: BuildingRules {
            drill: drill_rules(drill),
            refinery: refinery_rules(refinery),
            shipyard: shipyard_rules(shipyard),
        },
        ships: ShipRules {
            colony_ship: ship_rules(colony_ship),
            cargo_vessel: ship_rules(cargo_vessel),
        },
        scoring: scoring_rules(*scoring),
    }
}

fn drill_rules(d: DrillSpec) -> DrillRules {
    let DrillSpec {
        build: BuildCost {
            cost_g,
            build_time_ms,
        },
        demolition_time_ms,
        output_g_per_h,
    } = d;
    DrillRules {
        cost_g,
        build_time_ms,
        demolition_time_ms,
        output_g_per_h,
    }
}

fn refinery_rules(r: RefinerySpec) -> RefineryRules {
    let RefinerySpec {
        build: BuildCost {
            cost_g,
            build_time_ms,
        },
        demolition_time_ms,
        intake_g_per_h,
        ingots_per_ore,
        output_g_per_h,
    } = r;
    RefineryRules {
        cost_g,
        build_time_ms,
        demolition_time_ms,
        intake_g_per_h,
        ingots_per_ore,
        output_g_per_h,
    }
}

fn shipyard_rules(s: ShipyardSpec) -> ShipyardRules {
    let ShipyardSpec {
        build: BuildCost {
            cost_g,
            build_time_ms,
        },
        demolition_time_ms,
        parallel_builds,
    } = s;
    ShipyardRules {
        cost_g,
        build_time_ms,
        demolition_time_ms,
        parallel_builds,
    }
}

fn ship_rules(s: ShipSpec) -> ShipKindRules {
    let ShipSpec {
        build: BuildCost {
            cost_g,
            build_time_ms,
        },
        speed_du_per_h,
        cargo_capacity_g,
    } = s;
    ShipKindRules {
        cost_g,
        build_time_ms,
        speed_du_per_h,
        cargo_capacity_g,
    }
}

fn mass_points(m: MassPoints) -> MassPointsRules {
    let MassPoints { points, per_g } = m;
    MassPointsRules { points, per_g }
}

fn scoring_rules(s: ScoringSpecs) -> ScoringRules {
    let ScoringSpecs {
        planet,
        drill,
        refinery,
        shipyard,
        colony_ship,
        cargo_vessel,
        ore,
        ingots,
    } = s;
    ScoringRules {
        planet,
        drill,
        refinery,
        shipyard,
        colony_ship,
        cargo_vessel,
        ore: mass_points(ore),
        ingots: mass_points(ingots),
    }
}

/// The view as the JSON `GET /rules` answers, built once.
///
/// # Panics
/// Never in practice: the view is plain integers and a string, which always serialize.
#[must_use]
pub fn body(rules: &Rules) -> Bytes {
    Bytes::from(serde_json::to_vec(&view(rules)).expect("the rules view serializes"))
}
