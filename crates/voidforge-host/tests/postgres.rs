//! P2.3's done-when: commands round-trip through both the in-memory store and Postgres, with
//! identical records and receipts (decision log D-036). These need Postgres, so they are
//! `#[ignore]`d outside the database stage (`just db-test`, the CI `database` job).

mod common;

use common::{EPOCH, build, clock_at, config, drive, economy, start};
use sqlx::PgPool;
use voidforge_core::command::{Action, Command, Rejection};
use voidforge_core::state::{DomainEvent, PlanetId, PlayerId};
use voidforge_store::MIGRATOR;
use voidforge_store::log::{
    LogError, LogRecord, LogStore, MemoryLog, Outcome, Payload, PgLog, RecordKind,
};
use voidforge_testkit::v0_rules;

const END_MS: i64 = 110 * voidforge_core::quantity::MS_PER_HOUR;

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn commands_round_trip_through_memory_and_postgres_identically(pool: PgPool) {
    for seed in [0x2_3001, 0x2_3002] {
        let e = economy(seed);
        let rules = v0_rules();

        let memory = MemoryLog::new();
        let clock = clock_at(0);
        let (mut handle, task) = voidforge_host::spawn(
            start(&e, &rules),
            rules.clone(),
            {
                let s = memory.clone();
                move || s
            },
            clock.clone(),
            config(),
        )
        .unwrap();
        let memory_receipts = drive(&mut handle, &clock, &e, END_MS).await;
        let memory_world = handle.read().world.clone();
        drop(handle);
        task.join().unwrap();

        sqlx::query!("TRUNCATE log_records")
            .execute(&pool)
            .await
            .unwrap();
        let mut pg = PgLog::new(pool.clone());
        let clock = clock_at(0);
        let (mut handle, task) = voidforge_host::spawn(
            start(&e, &rules),
            rules.clone(),
            {
                let options = (*pool.connect_options()).clone();
                move || PgLog::lazy(options)
            },
            clock.clone(),
            config(),
        )
        .unwrap();
        let pg_receipts = drive(&mut handle, &clock, &e, END_MS).await;
        let pg_world = handle.read().world.clone();
        drop(handle);
        task.join().unwrap();

        let memory_records = memory.snapshot();
        let pg_records = pg.records().await.unwrap();
        assert!(
            memory_records.len() > 60,
            "{} records",
            memory_records.len()
        );
        assert!(memory_records.iter().any(|r| r.kind == RecordKind::Advance));
        assert!(
            memory_records
                .iter()
                .any(|r| r.outcome == Outcome::Accepted)
        );
        assert!(
            memory_records
                .iter()
                .any(|r| matches!(r.outcome, Outcome::Rejected(_)))
        );
        assert_eq!(pg_records, memory_records, "identical records");
        assert_eq!(pg_receipts, memory_receipts, "identical receipts");
        assert_eq!(pg_world, memory_world);
    }
}

fn record(seq: i64, player: PlayerId) -> LogRecord {
    LogRecord {
        seq,
        kind: RecordKind::Command,
        game_time: i64::MAX,
        wall_time_ms: EPOCH + 123_456_789,
        player: Some(player),
        idempotency_key: Some("k".to_owned()),
        payload: Payload::Command(Command {
            player,
            action: Action::RecallFleet {
                fleet: voidforge_core::state::FleetId(u64::MAX),
            },
        }),
        outcome: Outcome::Rejected(Rejection::UnknownFleet),
        events: vec![DomainEvent::BuildingCompleted {
            planet: PlanetId(u64::MAX),
            building: voidforge_core::state::BuildingId(1),
            kind: voidforge_core::state::BuildingKind::Refinery,
        }],
        rules_version: i32::MAX as u32,
        engine_build: build(),
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn extreme_values_round_trip_through_postgres_exactly(pool: PgPool) {
    // A player ID near u128::MAX (as JSON, a 39-digit number in payload, and the uuid column),
    // the largest u64 and i64 values, and the largest rules version that fits the column.
    let mut log = PgLog::new(pool);
    let records = vec![
        record(1, PlayerId(u128::MAX - 1)),
        record(2, PlayerId(u128::MAX)),
        record(3, PlayerId(1)),
    ];
    log.append(&records).await.unwrap();
    assert_eq!(log.records().await.unwrap(), records);
    assert_eq!(log.last().await.unwrap(), Some((3, i64::MAX)));
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn appends_are_all_or_none(pool: PgPool) {
    let mut log = PgLog::new(pool);
    log.append(&[record(1, PlayerId(7))]).await.unwrap();
    // A batch whose first record is valid and whose second repeats seq 1: the first must not
    // stay behind. (The first has its own key, so only the second can fail.)
    let mut valid = record(2, PlayerId(7));
    valid.idempotency_key = Some("k2".to_owned());
    let batch = [valid, record(1, PlayerId(8))];
    let err = log.append(&batch).await;
    assert!(matches!(err, Err(LogError::Database(_))), "{err:?}");
    assert_eq!(
        log.records().await.unwrap().len(),
        1,
        "nothing from the refused batch"
    );
    // The in-memory store refuses the same way.
    let mut memory = MemoryLog::new();
    memory.append(&[record(1, PlayerId(7))]).await.unwrap();
    assert!(memory.append(&batch).await.is_err());
    assert_eq!(memory.snapshot().len(), 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_rules_version_beyond_i32_is_refused_by_both_stores(pool: PgPool) {
    let mut big = record(1, PlayerId(7));
    big.rules_version = i32::MAX as u32 + 1;
    let mut log = PgLog::new(pool);
    assert!(matches!(
        log.append(&[big.clone()]).await,
        Err(LogError::Unrepresentable { seq: 1, .. })
    ));
    assert!(log.records().await.unwrap().is_empty());
    let mut memory = MemoryLog::new();
    assert!(matches!(
        memory.append(&[big]).await,
        Err(LogError::Unrepresentable { seq: 1, .. })
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn the_json_columns_hold_the_documented_forms(pool: PgPool) {
    let mut log = PgLog::new(pool.clone());
    let mut advance = record(2, PlayerId(7));
    advance.kind = RecordKind::Advance;
    advance.player = None;
    advance.idempotency_key = None;
    advance.payload = Payload::Empty;
    advance.outcome = Outcome::Empty;
    let mut accepted = record(3, PlayerId(7));
    accepted.idempotency_key = Some("k3".to_owned());
    accepted.outcome = Outcome::Accepted;
    log.append(&[record(1, PlayerId(7)), advance, accepted])
        .await
        .unwrap();
    let rows = sqlx::query!(
        r#"SELECT seq, kind, payload::text AS "payload!", outcome::text AS "outcome!",
                  jsonb_typeof(payload) AS "payload_type!", player_id::text AS player_id
           FROM log_records ORDER BY seq"#
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows[0].outcome,
        r#"{"reason": "unknown_fleet", "result": "rejected"}"#
    );
    assert_eq!(
        rows[0].player_id.as_deref(),
        Some("00000000-0000-0000-0000-000000000007")
    );
    assert_eq!(
        (
            rows[1].kind.as_str(),
            rows[1].payload.as_str(),
            rows[1].outcome.as_str()
        ),
        ("advance", "null", "null")
    );
    assert_eq!(rows[1].payload_type, "null", "a JSON null, never SQL NULL");
    assert_eq!(rows[2].outcome, r#"{"result": "accepted"}"#);
}
