//! Even-split tests: the solver's pure arithmetic (Flows, "Even split"). The named solver
//! tests, on real Rules v0 worlds, are in testkit's `tests/solver.rs`.

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

use super::*;

const T: i64 = 1_000_000; // grams per ton

#[test]
fn even_split_equal_demands_get_equal_shares() {
    // The game design's example: 10 t/h shared by two Refineries wanting 8 t/h gives 5 t/h each.
    assert_eq!(
        even_split(10 * T, &[(4, 8 * T), (5, 8 * T)]),
        [5 * T, 5 * T]
    );
}

#[test]
fn even_split_a_small_demand_passes_the_rest_evenly_to_the_others() {
    // 12 among 2, 9 and 9: the first takes 2, the others split the remaining 10.
    assert_eq!(even_split(12, &[(1, 2), (2, 9), (3, 9)]), [2, 5, 5]);
    // A second round: 30 among 20, 4, 20 and 6 gives 4 and 6, then 10 each.
    assert_eq!(
        even_split(30, &[(7, 20), (3, 4), (9, 20), (5, 6)]),
        [10, 4, 10, 6]
    );
}

#[test]
fn even_split_leftover_grams_go_one_at_a_time_to_the_lowest_ids() {
    // 11 among three that each want more: 3 each, then 2 left over to IDs 2 and 5.
    assert_eq!(even_split(11, &[(9, 100), (2, 100), (5, 100)]), [3, 4, 4]);
    // Fewer grams than consumers: the lowest IDs get one each.
    assert_eq!(even_split(2, &[(9, 100), (2, 100), (5, 100)]), [0, 1, 1]);
}

#[test]
fn even_split_one_consumer_takes_up_to_its_demand() {
    assert_eq!(even_split(7, &[(3, 10)]), [7]);
    assert_eq!(even_split(70, &[(3, 10)]), [10]);
}

#[test]
fn even_split_supply_beyond_demand_meets_every_demand() {
    assert_eq!(even_split(100, &[(1, 3), (2, 0), (3, 40)]), [3, 0, 40]);
}

#[test]
fn even_split_of_nothing_gives_nothing() {
    assert_eq!(even_split(0, &[(1, 3), (2, 5)]), [0, 0]);
    assert_eq!(even_split(5, &[]), Vec::<i64>::new());
}

fn config() -> Config {
    Config {
        cases: 512,
        rng_seed: RngSeed::Fixed(0x0f10_5e7e_0000_0001),
        failure_persistence: None,
        ..Config::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// Max-min fairness, checked against its definition rather than the algorithm: nobody gets
    /// more than they want; the supply is used up unless every demand is met; and while anyone
    /// is short, nobody else has more than one gram per hour over them, that extra gram going
    /// only to a lower ID.
    #[test]
    fn even_split_is_max_min_fair(
        supply in 0i64..1_000,
        demands in prop::collection::vec((0u64..50, 0i64..200), 0..8),
    ) {
        let mut seen = std::collections::BTreeSet::new();
        let demands: Vec<(u64, i64)> = demands.into_iter().filter(|d| seen.insert(d.0)).collect();
        let given = even_split(supply, &demands);
        let total: i64 = given.iter().sum();
        let wanted: i64 = demands.iter().map(|d| d.1).sum();
        prop_assert_eq!(total, supply.min(wanted));
        for (g, d) in given.iter().zip(&demands) {
            prop_assert!(*g >= 0 && *g <= d.1);
        }
        for (i, (gi, di)) in given.iter().zip(&demands).enumerate() {
            if *gi < di.1 {
                for (j, (gj, dj)) in given.iter().zip(&demands).enumerate() {
                    if i != j {
                        prop_assert!(*gj <= gi + 1, "{:?} -> {:?}", demands, given);
                        if *gj == gi + 1 {
                            prop_assert!(dj.0 < di.0, "the extra gram goes to the lower ID: {:?} -> {:?}", demands, given);
                        }
                    }
                }
            }
        }
    }
}
