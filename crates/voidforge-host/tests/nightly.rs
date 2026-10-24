//! The nightly simulation's invariants over its whole log (Technical Plan P5.3; decision log
//! D-066). After the bots stop and `serve` exits, this replays the simulation's Postgres log from
//! its earliest snapshot and checks the core's invariants on the World after every record: no
//! planet holds more buildings than its slots, every ship is in exactly one place, no Shipyard has
//! more active builds than the rules allow, fleet cargo is within its capacity and every fleet has
//! ships, and every stock lies within 0 and its cap. The replay itself must agree with every
//! snapshot (`voidforge-replay postgres` checks that, separately).
//!
//! It reads only a `voidforge_nightly_*` database, named by VOIDFORGE_NIGHTLY_DATABASE_URL, and
//! never writes.

use std::str::FromStr;

use sqlx::postgres::PgConnectOptions;
use voidforge_core::rules::Rules;
use voidforge_core::state::World;
use voidforge_store::log::{LogStore, PgLog};
use voidforge_store::replay;
use voidforge_store::snapshot::{self, EngineBuild, Expected};
use voidforge_testkit::determinism::{assert_fleet_invariants, assert_transition_invariants};
use voidforge_testkit::{busy_world, v0_fast_rules};

const PREFIX: &str = "voidforge_nightly_";

/// The database sqlx will connect to for this URL, if it is a nightly scratch database:
/// `voidforge_nightly_` and then digits and underscores only. It's read from the parsed options,
/// never from the URL's text: a query can name another database (`?dbname=`, P5.2 QA's F2) or
/// hide one behind a `/` (`.../voidforge_test?application_name=/voidforge_nightly_1`, R2-B4).
fn nightly_database(url: &str) -> Result<String, String> {
    let options =
        PgConnectOptions::from_str(url).map_err(|e| format!("not a Postgres URL: {e}"))?;
    let name = options
        .get_database()
        .ok_or("the URL names no database")?
        .to_owned();
    match name.strip_prefix(PREFIX) {
        Some(rest) if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'_') => {
            Ok(name)
        }
        _ => Err(format!(
            "refusing database {name}: only {PREFIX}<time>_<pid>"
        )),
    }
}

#[test]
fn only_a_nightly_scratch_database_is_read() {
    let base = "postgres://voidforge:pw@localhost:5432/";
    assert_eq!(
        nightly_database(&format!("{base}voidforge_nightly_20261023180000_123")),
        Ok("voidforge_nightly_20261023180000_123".to_owned())
    );
    for refused in [
        "voidforge",
        "voidforge_test",
        "voidforge_dev",
        "voidforge_nightly_",
        "voidforge_nightly_x",
        "voidforge_e2e_20261023_1",
        // A query that names another database, or hides one behind a `/` (R2-B4).
        "voidforge_nightly_1?dbname=voidforge",
        "voidforge_test?application_name=/voidforge_nightly_123",
    ] {
        assert!(
            nightly_database(&format!("{base}{refused}")).is_err(),
            "{refused}"
        );
    }
}

/// Every stock on every planet lies within 0 and its cap at world time.
fn stocks_within_caps(w: &World) {
    for p in w.planets.values() {
        let (ore, ingots) = (
            p.ore.value_at(w.time_ms).unwrap(),
            p.ingots.value_at(w.time_ms).unwrap(),
        );
        assert!(
            (0..=p.ore_cap_g).contains(&ore),
            "planet {} ore {ore} at {}",
            p.id.0,
            w.time_ms
        );
        assert!(
            (0..=p.ingot_cap_g).contains(&ingots),
            "planet {} ingots {ingots} at {}",
            p.id.0,
            w.time_ms
        );
    }
}

/// QA's B4 on e82382e: the stock check fails a World it should, not only passes good ones.
#[test]
fn a_stock_over_its_cap_breaks_the_stock_check() {
    stocks_within_caps(&busy_world());
    for ore in [true, false] {
        let mut w = busy_world();
        let t = w.time_ms;
        let p = w
            .planets
            .values_mut()
            .find(|p| if ore { &p.ore } else { &p.ingots }.value_at(t).unwrap() > 0)
            .expect("a planet with some of the stock");
        if ore {
            p.ore_cap_g = p.ore.value_at(t).unwrap() - 1;
        } else {
            p.ingot_cap_g = p.ingots.value_at(t).unwrap() - 1;
        }
        let broken =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| stocks_within_caps(&w)));
        assert!(broken.is_err(), "a stock over its cap passed (ore: {ore})");
    }
}

fn invariants(w: &World, rules: &Rules, seq: i64, checked_states: &mut u64) {
    *checked_states += 1;
    let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_transition_invariants(w, rules);
        assert_fleet_invariants(w, rules);
        stocks_within_caps(w);
    }));
    if let Err(panic) = checked {
        let why = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default();
        panic!("an invariant broke after record {seq}: {why}");
    }
}

#[tokio::test]
#[ignore = "nightly: VOIDFORGE_NIGHTLY_DATABASE_URL names the simulation's voidforge_nightly_* database"]
async fn nightly_invariants() {
    let url = std::env::var("VOIDFORGE_NIGHTLY_DATABASE_URL")
        .expect("VOIDFORGE_NIGHTLY_DATABASE_URL names the simulation's database");
    let name = nightly_database(&url).unwrap_or_else(|e| panic!("{e}"));
    let rules = v0_fast_rules();
    let mut store = PgLog::lazy_url(&url).expect("a Postgres URL");
    let index = store.snapshot_index().await.unwrap();
    let first = index
        .first()
        .expect("the simulation's database holds a snapshot");
    let row = store
        .snapshot_at(first.seq)
        .await
        .unwrap()
        .expect("the first snapshot");
    let build = EngineBuild::new(row.engine_build.clone()).unwrap();
    let mut world = snapshot::decode(
        &row.payload,
        &Expected {
            rules_version: rules.version,
            engine_build: &build,
        },
    )
    .expect("the first snapshot decodes under v0-fast")
    .world;
    let mut checked = 0;
    invariants(&world, &rules, row.seq, &mut checked);
    let records = store.records_after(row.seq).await.unwrap();
    let mut last = row.seq;
    for record in &records {
        replay::replay(
            &mut world,
            last,
            std::slice::from_ref(record),
            &rules,
            &record.engine_build,
        )
        .unwrap_or_else(|e| panic!("record {} doesn't replay: {e}", record.seq));
        last = record.seq;
        invariants(&world, &rules, last, &mut checked);
    }
    println!(
        "@nightly-invariants {}",
        serde_json::json!({
            "database": name, "from_snapshot": row.seq, "records": records.len(), "head": last,
            "checked": checked,
            "snapshots": index.len(), "game_time_ms": world.time_ms
        })
    );
}
