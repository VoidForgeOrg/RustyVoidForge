//! P2.3's done-when: commands round-trip through both the in-memory store and Postgres, with
//! identical records and receipts (decision log D-036). These need Postgres, so they are
//! `#[ignore]`d outside the database stage (`just db-test`, the CI `database` job).

mod common;

use common::{EPOCH, build, clock_at, config, drive, economy, key_for, start};
use sqlx::PgPool;
use voidforge_core::command::{Action, Command, Rejection};
use voidforge_core::state::{DomainEvent, PlanetId, PlayerId};
use voidforge_store::MIGRATOR;
use voidforge_store::log::{
    IdempotencyKey, LogError, LogRecord, LogStore, MemoryLog, Outcome, Payload, PgLog, RecordKind,
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
                .any(|r| matches!(r.outcome, Outcome::Accepted { .. }))
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
        idempotency_key: Some(IdempotencyKey::new("k").unwrap()),
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
    valid.idempotency_key = Some(IdempotencyKey::new("k2").unwrap());
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
    accepted.idempotency_key = Some(IdempotencyKey::new("k3").unwrap());
    accepted.outcome = Outcome::Accepted { own_events: 1 };
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
    assert_eq!(
        rows[2].outcome,
        r#"{"result": "accepted", "own_events": 1}"#
    );
}

/// P2.4: restart and retry through Postgres, both from the rebuilt cache and through the log
/// lookup for a key older than the window (D-037).
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn keys_survive_a_restart_through_postgres(pool: PgPool) {
    use voidforge_core::state::BuildingKind;
    let h = voidforge_core::quantity::MS_PER_HOUR;
    let mut b = voidforge_testkit::WorldBuilder::new();
    let home = b.player(voidforge_testkit::ALICE);
    let world = b.build();
    let drill = Command {
        player: voidforge_testkit::ALICE,
        action: Action::BuildBuilding {
            planet: home,
            kind: BuildingKind::Drill,
        },
    };
    let refinery = Command {
        player: voidforge_testkit::ALICE,
        action: Action::BuildBuilding {
            planet: home,
            kind: BuildingKind::Refinery,
        },
    };
    let key = IdempotencyKey::new("restart-key").unwrap();
    let options = (*pool.connect_options()).clone();
    let mut cfg = config();
    cfg.key_window_ms = 2 * h;
    let host = |world, clock: voidforge_host::ManualClock, cfg: voidforge_host::HostConfig| {
        let options = options.clone();
        voidforge_host::spawn(world, v0_rules(), move || PgLog::lazy(options), clock, cfg).unwrap()
    };

    let (handle, task) = host(world, clock_at(h), cfg.clone());
    let first = handle
        .submit_with_key(drill.clone(), key.clone())
        .await
        .unwrap();
    let world = handle.read().world.as_ref().clone();
    drop(handle);
    task.join().unwrap();

    // Inside the window: the rebuilt cache answers.
    let (handle, task) = host(world, clock_at(h), cfg.clone());
    assert_eq!(
        handle
            .submit_with_key(drill.clone(), key.clone())
            .await
            .unwrap(),
        first
    );
    assert_eq!(handle.stats().key_lookups(), 0);
    handle
        .submit(Command {
            player: voidforge_testkit::ALICE,
            action: Action::JoinGame,
        })
        .await
        .unwrap();
    let world = handle.read().world.as_ref().clone();
    drop(handle);
    task.join().unwrap();

    // Far past the window: the World moves on, the cache is rebuilt without the key, and the
    // Postgres lookup answers it. Neither the retry nor the conflicting reuse reaches the append.
    let (handle, task) = host(world, clock_at(30 * h), cfg);
    handle
        .submit(Command {
            player: voidforge_testkit::ALICE,
            action: Action::JoinGame,
        })
        .await
        .unwrap();
    let records = PgLog::new(pool.clone()).records().await.unwrap().len();
    assert_eq!(
        handle.submit_with_key(drill, key.clone()).await.unwrap(),
        first
    );
    assert_eq!(
        handle.submit_with_key(refinery, key).await,
        Err(voidforge_host::HostError::KeyConflict { seq: first.seq })
    );
    assert!(
        handle.stats().key_lookups() >= 1,
        "answered by the Postgres lookup"
    );
    assert_eq!(
        PgLog::new(pool.clone()).records().await.unwrap().len(),
        records
    );
    drop(handle);
    task.join().unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn find_keys_and_keyed_since_match_the_memory_store(pool: PgPool) {
    let mut pg = PgLog::new(pool);
    let mut memory = MemoryLog::new();
    let mut records = vec![
        record(1, PlayerId(7)),
        record(2, PlayerId(8)),
        record(3, PlayerId(7)),
    ];
    records[1].game_time = 5;
    records[2].idempotency_key = Some(IdempotencyKey::new("other").unwrap());
    let mut keyless = record(4, PlayerId(9));
    keyless.idempotency_key = None;
    records.push(keyless);
    pg.append(&records).await.unwrap();
    memory.append(&records).await.unwrap();
    let k = IdempotencyKey::new("k").unwrap();
    for keys in [
        vec![(PlayerId(7), k.clone())],
        vec![
            (PlayerId(8), k.clone()),
            (PlayerId(7), IdempotencyKey::new("other").unwrap()),
        ],
        vec![(PlayerId(9), k.clone())],
        vec![],
    ] {
        assert_eq!(
            pg.find_keys(&keys).await.unwrap(),
            memory.find_keys(&keys).await.unwrap()
        );
    }
    assert_eq!(
        pg.find_keys(&[(PlayerId(7), k)]).await.unwrap(),
        vec![records[0].clone()]
    );
    for since in [i64::MIN, 0, 5, 6, i64::MAX] {
        assert_eq!(
            pg.keyed_since(since).await.unwrap(),
            memory.keyed_since(since).await.unwrap()
        );
    }
}

/// P2.5 through Postgres (D-038): the bootstrap snapshot, a host that runs half the script and
/// stops (cleanly, then by a crash after a durable append), recovery from the `snapshots` table and
/// the log, and the rest of the script: byte-identical to the memory run that never stopped, and
/// every retried key gets its logged outcome.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn recovery_through_postgres_is_byte_identical(pool: PgPool) {
    use voidforge_host::{Clock, ManualClock};
    use voidforge_store::log::SnapshotRow;
    use voidforge_store::snapshot;
    let e = economy(0x2_5101);
    let rules = v0_rules();
    let script = e.script();
    // The reference: memory, never stopped.
    let memory = MemoryLog::new();
    let clock = clock_at(0);
    let (mut handle, task) = voidforge_host::spawn(
        start(&e, &rules),
        rules.clone(),
        {
            let m = memory.clone();
            move || m
        },
        clock.clone(),
        config(),
    )
    .unwrap();
    drive(&mut handle, &clock, &e, END_MS).await;
    let reference = handle.read().world.as_ref().clone();
    drop(handle);
    task.join().unwrap();

    let options = (*pool.connect_options()).clone();
    let host = |world, clock: ManualClock, every: u64| {
        let options = options.clone();
        let mut cfg = config();
        cfg.snapshot_every_records = every;
        voidforge_host::spawn(world, v0_rules(), move || PgLog::lazy(options), clock, cfg).unwrap()
    };
    let mut pg = PgLog::new(pool.clone());
    pg.write_snapshot(&SnapshotRow::new(
        0,
        &snapshot::encode(&start(&e, &rules), &build()).unwrap(),
    ))
    .await
    .unwrap();
    let run =
        |handle: voidforge_host::HostHandle, clock: ManualClock, range: std::ops::Range<usize>| {
            let script = script.clone();
            async move {
                let mut handle = handle;
                for &(at, k) in &script[range] {
                    clock.set((EPOCH + at).max(clock.now_unix_ms()));
                    common::settle(&mut handle, &clock).await;
                    let command = e.command(k, &handle.read().world);
                    handle.submit_with_key(command, key_for(k)).await.unwrap();
                }
                handle
            }
        };
    // First third, then a clean stop.
    let clock = clock_at(0);
    let back = voidforge_host::recover(&mut pg, &rules, &build())
        .await
        .unwrap();
    let (handle, task) = host(back.world, clock.clone(), 7);
    let handle = run(handle, clock.clone(), 0..20).await;
    drop(handle);
    task.join().unwrap();
    // Second third, recovered from Postgres.
    let back = voidforge_host::recover(&mut pg, &rules, &build())
        .await
        .unwrap();
    assert_eq!(
        back.last_seq,
        pg.last().await.unwrap().unwrap().0,
        "the shutdown snapshot is at the head"
    );
    let (handle, task) = host(back.world, clock.clone(), 7);
    let handle = run(handle, clock.clone(), 20..40).await;
    // Then a stop whose shutdown snapshot is removed afterwards, as if it crashed before
    // writing it: recovery uses the last periodic snapshot and replays the tail from Postgres.
    // (The memory tests crash the host itself, at each crash-table row.)
    drop(handle);
    task.join().unwrap();
    let head = pg.last().await.unwrap().unwrap().0;
    sqlx::query!("DELETE FROM snapshots WHERE seq = $1", head)
        .execute(&pool)
        .await
        .unwrap();
    let back = voidforge_host::recover(&mut pg, &rules, &build())
        .await
        .unwrap();
    assert!(
        back.snapshot_seq < back.last_seq,
        "a tail was replayed from Postgres"
    );
    // Every key of the first two thirds, retried, gets its logged outcome.
    let (handle, task) = host(back.world, clock.clone(), 7);
    let before = pg.records().await.unwrap().len();
    for record in pg
        .records()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.idempotency_key.is_some())
    {
        let Payload::Command(c) = record.payload else {
            panic!()
        };
        let r = handle
            .submit_with_key(c, record.idempotency_key.unwrap())
            .await
            .unwrap();
        assert_eq!(r.seq, record.seq);
    }
    assert_eq!(
        pg.records().await.unwrap().len(),
        before,
        "no retry appended"
    );
    let mut handle = run(handle, clock.clone(), 40..script.len()).await;
    clock.set(EPOCH + END_MS);
    common::settle(&mut handle, &clock).await;
    let recovered = handle.read().world.as_ref().clone();
    drop(handle);
    task.join().unwrap();
    assert_eq!(
        snapshot::state_hash(&recovered).unwrap(),
        snapshot::state_hash(&reference).unwrap()
    );
    assert_eq!(recovered, reference);
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn upgrades_recover_through_postgres(pool: PgPool) {
    // Carry (f) through Postgres: the same cases as the memory store.
    let options = (*pool.connect_options()).clone();
    common::upgrade_cases(PgLog::new(pool.clone()), move || {
        PgLog::lazy(options.clone())
    })
    .await;
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn an_upgrade_over_another_builds_tail_is_refused_through_postgres(pool: PgPool) {
    let options = (*pool.connect_options()).clone();
    common::upgrade_over_another_builds_tail_is_refused(PgLog::new(pool.clone()), move || {
        PgLog::lazy(options.clone())
    })
    .await;
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_snapshot_past_the_log_head_is_refused_through_postgres(pool: PgPool) {
    // Review M1 through Postgres.
    let options = (*pool.connect_options()).clone();
    common::snapshot_past_head_is_refused(PgLog::new(pool.clone()), move || {
        PgLog::lazy(options.clone())
    })
    .await;
}

/// The Phase 2 gate's "golden replays pass against Postgres" (P2.6, D-040): every golden's start
/// and records go into Postgres, read back exactly through jsonb, and `recover` replays them to
/// the golden's last checkpoint; advanced to the end, the World has the golden's end hash.
#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn every_golden_recovers_through_postgres(pool: PgPool) {
    use common::golden::on_disk;
    use voidforge_store::golden::fixture;
    use voidforge_store::log::SnapshotRow;
    use voidforge_store::snapshot::{self, Expected};
    use voidforge_store::verify::hex;

    let rules = v0_rules();
    let goldens = on_disk();
    assert!(!goldens.is_empty(), "no goldens in tests/golden");
    for (name, files) in goldens {
        sqlx::query!("TRUNCATE log_records")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query!("DELETE FROM snapshots WHERE seq = $1", 0_i64)
            .execute(&pool)
            .await
            .unwrap();
        let mut pg = PgLog::new(pool.clone());
        let decoded = snapshot::decode(
            &files.start,
            &Expected {
                rules_version: rules.version,
                engine_build: &fixture(),
            },
        )
        .unwrap();
        let snap = snapshot::encode(&decoded.world, &fixture()).unwrap();
        assert_eq!(snap.bytes, files.start, "{name}: the start re-encodes");
        pg.write_snapshot(&SnapshotRow::new(0, &snap))
            .await
            .unwrap();
        let records = files.records().unwrap();
        pg.append(&records).await.unwrap();
        assert_eq!(
            pg.records().await.unwrap(),
            records,
            "{name}: through jsonb"
        );
        let back = voidforge_host::recover(&mut pg, &rules, &fixture())
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let m = files.manifest().unwrap();
        assert_eq!(
            back.last_seq,
            m.records.last().map_or(0, |c| c.seq),
            "{name}"
        );
        let mut world = back.world;
        let last = m.records.last().map_or(&m.start_hash, |c| &c.state_hash);
        assert_eq!(&hex(&snapshot::state_hash(&world).unwrap()), last, "{name}");
        assert!(
            world.advance_to(m.end_ms, &rules).unwrap().is_empty(),
            "{name}"
        );
        assert_eq!(
            hex(&snapshot::state_hash(&world).unwrap()),
            m.end_hash,
            "{name}"
        );
    }
}
