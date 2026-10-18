//! The host in the determinism suite (decision log D-024, D-036): the same command script and
//! clock through the host give the same log records and receipts, in one process and in two.
//! `.github/scripts/determinism.sh` runs this binary and [`two_process_double_run`] twice.

mod common;

use common::{clock_at, config, drive, economy, start};
use voidforge_core::quantity::MS_PER_HOUR;
use voidforge_store::log::{LogRecord, MemoryLog};
use voidforge_testkit::v0_rules;

/// A stable text form of a run: every record column (the JSON ones as the Postgres store writes
/// them) and every receipt.
fn run(seed: u64) -> String {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let e = economy(seed);
        let rules = v0_rules();
        let log = MemoryLog::new();
        let clock = clock_at(0);
        let (mut handle, task) = voidforge_host::spawn(
            start(&e, &rules),
            rules,
            {
                let log = log.clone();
                move || log
            },
            clock.clone(),
            config(),
        )
        .unwrap();
        let receipts = drive(&mut handle, &clock, &e, 110 * MS_PER_HOUR).await;
        drop(handle);
        task.join().unwrap();
        let records: Vec<String> = log.snapshot().iter().map(line).collect();
        format!("{}\n{receipts:?}\n", records.join("\n"))
    })
}

fn line(r: &LogRecord) -> String {
    let json = r.json().unwrap();
    format!(
        "{} {:?} {} {} {:?} {:?} {} {} {} {} {}",
        r.seq,
        r.kind,
        r.game_time,
        r.wall_time_ms,
        r.player,
        r.idempotency_key,
        json.payload,
        json.outcome,
        json.events,
        r.rules_version,
        r.engine_build.as_str()
    )
}

fn recorded_runs() -> String {
    [0x2_3101, 0x2_3102, 0x2_3103].map(run).concat()
}

#[test]
fn host_double_run_in_process() {
    let a = recorded_runs();
    assert!(a.lines().count() > 200, "the runs log real work");
    assert_eq!(a, recorded_runs(), "two runs gave different records");
}

/// The two-process double run: `VOIDFORGE_DOUBLE_RUN=write` saves the runs to
/// `VOIDFORGE_DOUBLE_RUN_FILE`, then `compare` requires identical bytes. A missing file fails.
#[test]
#[ignore = "run by .github/scripts/determinism.sh in two processes"]
fn two_process_double_run() {
    let mode =
        std::env::var("VOIDFORGE_DOUBLE_RUN").expect("VOIDFORGE_DOUBLE_RUN is write or compare");
    let file =
        std::env::var("VOIDFORGE_DOUBLE_RUN_FILE").expect("VOIDFORGE_DOUBLE_RUN_FILE is set");
    let bytes = recorded_runs();
    match mode.as_str() {
        "write" => std::fs::write(&file, &bytes).expect("the bytes are written"),
        "compare" => {
            let first = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("the first process's runs are missing at {file}: {e}"));
            assert_eq!(first.len(), bytes.len(), "the runs differ in length");
            assert!(
                first == bytes,
                "the two processes produced different records"
            );
        }
        other => panic!("VOIDFORGE_DOUBLE_RUN must be write or compare, not {other}"),
    }
}
