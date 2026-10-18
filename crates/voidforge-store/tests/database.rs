//! P2.1: the schema against a real Postgres (decision log D-034).
//!
//! Every test here needs Postgres, so each is `#[ignore]`: the `test` and `lint` jobs compile and
//! lint them with the committed offline query metadata, and the `database` job runs them with
//! `--run-ignored only` (`just db-test` locally). `#[sqlx::test]` gives each test its own fresh
//! database, created from `DATABASE_URL` and dropped afterwards.
//!
//! The expected columns are the Architectural Design's Persistence tables and Decision 9, written
//! out by hand; they are not generated from the migrations.

use std::path::Path;

use sqlx::PgPool;
use sqlx::migrate::{MigrateError, Migrator};
use voidforge_store::{MIGRATOR, StoreError, migrate};

// SQLSTATE codes (Postgres, Appendix A).
const NOT_NULL_VIOLATION: &str = "23502";
const UNIQUE_VIOLATION: &str = "23505";
const CHECK_VIOLATION: &str = "23514";

const PLAYER_A: &str = "01890a5d-ac96-774b-bcce-b302099a8057";
const PLAYER_B: &str = "01890a5d-ac96-7c4b-8cce-b302099a8058";
const PLAYER_V4: &str = "9b2f1c4e-8d3a-4f6b-9e1c-2a7d5b8c0f13";

/// One column as the catalog reports it: name, type, NOT NULL, and its default if any.
type Column = (String, String, bool, Option<String>);

fn col(name: &str, ty: &str, not_null: bool, default: Option<&str>) -> Column {
    (
        name.to_owned(),
        ty.to_owned(),
        not_null,
        default.map(str::to_owned),
    )
}

async fn columns(pool: &PgPool, table: &str) -> Vec<Column> {
    sqlx::query!(
        r#"SELECT a.attname::text AS "name!",
                  format_type(a.atttypid, a.atttypmod) AS "ty!",
                  a.attnotnull AS "not_null!",
                  pg_get_expr(d.adbin, d.adrelid) AS "default?"
           FROM pg_attribute a
           LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
           WHERE a.attrelid = $1::text::regclass AND a.attnum > 0 AND NOT a.attisdropped
           ORDER BY a.attnum"#,
        table
    )
    .fetch_all(pool)
    .await
    .expect("read columns")
    .into_iter()
    .map(|r| (r.name, r.ty, r.not_null, r.default))
    .collect()
}

/// Every constraint except NOT NULL (Postgres 18 lists those too; `columns` covers them), as
/// (name, definition), ordered by name.
async fn constraints(pool: &PgPool, table: &str) -> Vec<(String, String)> {
    sqlx::query!(
        r#"SELECT conname::text AS "name!", pg_get_constraintdef(oid) AS "def!"
           FROM pg_constraint
           WHERE conrelid = $1::text::regclass AND contype <> 'n'
           ORDER BY conname"#,
        table
    )
    .fetch_all(pool)
    .await
    .expect("read constraints")
    .into_iter()
    .map(|r| (r.name, r.def))
    .collect()
}

/// The ordinary tables in the public schema, ordered by name.
async fn tables(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar!(
        r#"SELECT tablename::text AS "name!" FROM pg_tables
           WHERE schemaname = 'public' ORDER BY tablename"#
    )
    .fetch_all(pool)
    .await
    .expect("read tables")
}

/// The applied migrations: (version, success, checksum, installed_on as text).
async fn applied(pool: &PgPool) -> Vec<(i64, bool, Vec<u8>, String)> {
    sqlx::query!(
        r#"SELECT version, success, checksum, installed_on::text AS "installed_on!"
           FROM _sqlx_migrations ORDER BY version"#
    )
    .fetch_all(pool)
    .await
    .expect("read _sqlx_migrations")
    .into_iter()
    .map(|r| (r.version, r.success, r.checksum, r.installed_on))
    .collect()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_owned()).collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

/// The SQLSTATE and constraint name of a database error; panics on any other result.
fn violation<T: std::fmt::Debug>(r: Result<T, sqlx::Error>) -> (String, Option<String>) {
    let err = r.expect_err("the statement must be refused");
    let db = err
        .as_database_error()
        .unwrap_or_else(|| panic!("not a database error: {err}"));
    (
        db.code().expect("a SQLSTATE").into_owned(),
        db.constraint().map(str::to_owned),
    )
}

/// A `log_records` row; `Default` is a valid keyless advance record.
#[derive(Clone)]
struct Log {
    seq: i64,
    kind: &'static str,
    player: Option<&'static str>,
    key: Option<&'static str>,
    events: &'static str,
    rules_version: i32,
}

impl Default for Log {
    fn default() -> Self {
        Self {
            seq: 1,
            kind: "advance",
            player: None,
            key: None,
            events: "[]",
            rules_version: 0,
        }
    }
}

async fn insert_log(pool: &PgPool, r: &Log) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO log_records
               (seq, kind, game_time, player_id, idempotency_key, payload, outcome, events,
                rules_version, engine_build)
           VALUES ($1, $2, 0, $3::text::uuid, $4, 'null', 'null', $5::text::jsonb, $6, 'test')"#,
        r.seq,
        r.kind,
        r.player,
        r.key,
        r.events,
        r.rules_version
    )
    .execute(pool)
    .await
    .map(|_| ())
}

async fn insert_snapshot(
    pool: &PgPool,
    seq: i64,
    hash: &[u8],
    rules_version: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO snapshots (seq, game_time, rules_version, engine_build, state_hash, payload)
           VALUES ($1, 0, $2, 'test', $3, '\x00')"#,
        seq,
        rules_version,
        hash
    )
    .execute(pool)
    .await
    .map(|_| ())
}

async fn insert_account(
    pool: &PgPool,
    player: &str,
    issuer: &str,
    subject: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO accounts (player_id, issuer, subject) VALUES ($1::text::uuid, $2, $3)",
        player,
        issuer,
        subject
    )
    .execute(pool)
    .await
    .map(|_| ())
}

// --- Migrations ---

#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn migrations_apply_cleanly_to_an_empty_database(pool: PgPool) {
    assert_eq!(
        tables(&pool).await,
        Vec::<String>::new(),
        "the database starts empty"
    );
    migrate(&pool).await.expect("migrations apply");
    assert_eq!(
        tables(&pool).await,
        s(&["_sqlx_migrations", "accounts", "log_records", "snapshots"])
    );
    let applied = applied(&pool).await;
    let versions: Vec<(i64, bool)> = applied.iter().map(|(v, ok, _, _)| (*v, *ok)).collect();
    assert_eq!(versions, vec![(1, true), (2, true), (3, true)]);
    assert_eq!(
        MIGRATOR.iter().count(),
        3,
        "every embedded migration was applied"
    );
}

#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn rerunning_migrations_is_a_no_op(pool: PgPool) {
    migrate(&pool).await.expect("first run");
    let first = applied(&pool).await;
    let schema: Vec<_> = [
        columns(&pool, "log_records").await,
        columns(&pool, "snapshots").await,
        columns(&pool, "accounts").await,
    ]
    .concat();
    migrate(&pool).await.expect("second run");
    assert_eq!(
        applied(&pool).await,
        first,
        "no migration re-applied (same rows, same times)"
    );
    let again: Vec<_> = [
        columns(&pool, "log_records").await,
        columns(&pool, "snapshots").await,
        columns(&pool, "accounts").await,
    ]
    .concat();
    assert_eq!(again, schema);
}

/// The runtime half of "migrations only add": an applied migration whose file later changed is
/// refused, so a database never silently diverges from the files (the CI guardrail is the other).
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn a_changed_applied_migration_is_refused(pool: PgPool) {
    migrate(&pool).await.expect("apply the real migrations");
    let dir = std::env::temp_dir().join(format!("voidforge-p21-changed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(&src).expect("read migrations") {
        let path = entry.expect("entry").path();
        std::fs::copy(&path, dir.join(path.file_name().expect("name"))).expect("copy");
    }
    let edited = dir.join("0002_snapshots.sql");
    let mut sql = std::fs::read_to_string(&edited).expect("read");
    sql.push_str("-- an edit after the fact\n");
    std::fs::write(&edited, sql).expect("write");
    let changed = Migrator::new(dir.as_path())
        .await
        .expect("load the edited copy");
    let result = changed.run(&pool).await;
    std::fs::remove_dir_all(&dir).expect("remove temp dir");
    assert!(
        matches!(result, Err(MigrateError::VersionMismatch(2))),
        "expected VersionMismatch(2), got {result:?}"
    );
    // The same refusal through the store's own error type.
    let err = StoreError::from(MigrateError::VersionMismatch(2));
    assert!(err.to_string().contains("migrations failed"));
}

// --- Columns, keys and constraints, introspected ---

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn log_records_columns_match_the_architecture(pool: PgPool) {
    assert_eq!(
        columns(&pool, "log_records").await,
        vec![
            col("seq", "bigint", true, None),
            col("kind", "text", true, None),
            col("game_time", "bigint", true, None),
            col("wall_time", "timestamp with time zone", true, Some("now()")),
            col("player_id", "uuid", false, None),
            col("idempotency_key", "text", false, None),
            col("payload", "jsonb", true, None),
            col("outcome", "jsonb", true, None),
            col("events", "jsonb", true, None),
            col("rules_version", "integer", true, None),
            col("engine_build", "text", true, None),
        ]
    );
    assert_eq!(
        constraints(&pool, "log_records").await,
        pairs(&[
            (
                "log_records_events_check",
                "CHECK ((jsonb_typeof(events) = 'array'::text))"
            ),
            (
                "log_records_key_needs_player",
                "CHECK (((idempotency_key IS NULL) OR (player_id IS NOT NULL)))"
            ),
            (
                "log_records_kind_check",
                "CHECK ((kind = ANY (ARRAY['command'::text, 'advance'::text, \
                 'rules_change'::text, 'system'::text])))"
            ),
            ("log_records_pkey", "PRIMARY KEY (seq)"),
            (
                "log_records_player_key_unique",
                "UNIQUE (player_id, idempotency_key)"
            ),
            (
                "log_records_rules_version_check",
                "CHECK ((rules_version >= 0))"
            ),
            ("log_records_seq_check", "CHECK ((seq >= 1))"),
        ])
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn snapshots_columns_match_the_architecture(pool: PgPool) {
    assert_eq!(
        columns(&pool, "snapshots").await,
        vec![
            col("seq", "bigint", true, None),
            col("game_time", "bigint", true, None),
            col("rules_version", "integer", true, None),
            col("engine_build", "text", true, None),
            col("state_hash", "bytea", true, None),
            col("payload", "bytea", true, None),
            col(
                "created_at",
                "timestamp with time zone",
                true,
                Some("now()")
            ),
        ]
    );
    assert_eq!(
        constraints(&pool, "snapshots").await,
        pairs(&[
            ("snapshots_pkey", "PRIMARY KEY (seq)"),
            (
                "snapshots_rules_version_check",
                "CHECK ((rules_version >= 0))"
            ),
            ("snapshots_seq_check", "CHECK ((seq >= 0))"),
            (
                "snapshots_state_hash_check",
                "CHECK ((octet_length(state_hash) = 32))"
            ),
        ])
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn accounts_columns_match_decision_9(pool: PgPool) {
    assert_eq!(
        columns(&pool, "accounts").await,
        vec![
            col("player_id", "uuid", true, None),
            col("issuer", "text", true, None),
            col("subject", "text", true, None),
            col(
                "created_at",
                "timestamp with time zone",
                true,
                Some("now()")
            ),
        ]
    );
    assert_eq!(
        constraints(&pool, "accounts").await,
        pairs(&[
            ("accounts_issuer_check", "CHECK ((issuer <> ''::text))"),
            ("accounts_issuer_subject_unique", "UNIQUE (issuer, subject)"),
            ("accounts_pkey", "PRIMARY KEY (player_id)"),
            (
                "accounts_player_id_check",
                "CHECK ((COALESCE((uuid_extract_version(player_id))::integer, 0) = 7))"
            ),
            ("accounts_subject_check", "CHECK ((subject <> ''::text))"),
        ])
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn no_foreign_keys_tie_the_log_to_accounts(pool: PgPool) {
    let fks = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM pg_constraint c
           JOIN pg_namespace n ON n.oid = c.connamespace
           WHERE c.contype = 'f' AND n.nspname = 'public'"#
    )
    .fetch_one(&pool)
    .await
    .expect("count foreign keys");
    assert_eq!(
        fks, 0,
        "the log replays into a database without accounts (D-034)"
    );
    // By effect: a command from a player with no account is stored.
    let r = Log {
        kind: "command",
        player: Some(PLAYER_A),
        key: Some("k"),
        ..Log::default()
    };
    insert_log(&pool, &r).await.expect("no account needed");
}

// --- Constraints, by effect ---

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn kind_accepts_exactly_the_four_documented_values(pool: PgPool) {
    for (seq, kind) in (1..).zip(["command", "advance", "rules_change", "system"]) {
        insert_log(
            &pool,
            &Log {
                seq,
                kind,
                ..Log::default()
            },
        )
        .await
        .expect(kind);
    }
    for (seq, kind) in (10..).zip(["", "Command", "advance ", "rules-change", "snapshot"]) {
        assert_eq!(
            violation(
                insert_log(
                    &pool,
                    &Log {
                        seq,
                        kind,
                        ..Log::default()
                    }
                )
                .await
            ),
            (
                CHECK_VIOLATION.to_owned(),
                Some("log_records_kind_check".to_owned())
            ),
            "kind {kind:?}"
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn idempotency_key_is_unique_per_player(pool: PgPool) {
    let first = Log {
        seq: 1,
        kind: "command",
        player: Some(PLAYER_A),
        key: Some("k1"),
        ..Log::default()
    };
    insert_log(&pool, &first)
        .await
        .expect("first use of the key");
    assert_eq!(
        violation(
            insert_log(
                &pool,
                &Log {
                    seq: 2,
                    ..first.clone()
                }
            )
            .await
        ),
        (
            UNIQUE_VIOLATION.to_owned(),
            Some("log_records_player_key_unique".to_owned())
        ),
        "the same player reusing a key"
    );
    insert_log(
        &pool,
        &Log {
            seq: 3,
            player: Some(PLAYER_B),
            ..first.clone()
        },
    )
    .await
    .expect("another player may use the same key");
    insert_log(
        &pool,
        &Log {
            seq: 4,
            key: Some("k2"),
            ..first.clone()
        },
    )
    .await
    .expect("the same player with another key");
    insert_log(
        &pool,
        &Log {
            seq: 5,
            key: None,
            ..first.clone()
        },
    )
    .await
    .expect("keyless 1");
    insert_log(
        &pool,
        &Log {
            seq: 6,
            key: None,
            ..first.clone()
        },
    )
    .await
    .expect("keyless 2");
    insert_log(
        &pool,
        &Log {
            seq: 7,
            ..Log::default()
        },
    )
    .await
    .expect("no player, no key 1");
    insert_log(
        &pool,
        &Log {
            seq: 8,
            ..Log::default()
        },
    )
    .await
    .expect("no player, no key 2");
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_key_needs_a_player(pool: PgPool) {
    assert_eq!(
        violation(
            insert_log(
                &pool,
                &Log {
                    key: Some("k"),
                    ..Log::default()
                }
            )
            .await
        ),
        (
            CHECK_VIOLATION.to_owned(),
            Some("log_records_key_needs_player".to_owned())
        )
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn log_seq_starts_at_one_and_is_unique(pool: PgPool) {
    for seq in [0, -1, i64::MIN] {
        assert_eq!(
            violation(
                insert_log(
                    &pool,
                    &Log {
                        seq,
                        ..Log::default()
                    }
                )
                .await
            ),
            (
                CHECK_VIOLATION.to_owned(),
                Some("log_records_seq_check".to_owned())
            ),
            "seq {seq}"
        );
    }
    insert_log(
        &pool,
        &Log {
            seq: 1,
            ..Log::default()
        },
    )
    .await
    .expect("seq 1");
    insert_log(
        &pool,
        &Log {
            seq: i64::MAX,
            ..Log::default()
        },
    )
    .await
    .expect("seq max");
    assert_eq!(
        violation(
            insert_log(
                &pool,
                &Log {
                    seq: 1,
                    ..Log::default()
                }
            )
            .await
        ),
        (
            UNIQUE_VIOLATION.to_owned(),
            Some("log_records_pkey".to_owned())
        )
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn events_must_be_a_json_array(pool: PgPool) {
    for (seq, events) in (1..).zip(["[]", r#"[{"BuildingCompleted":{}}]"#]) {
        insert_log(
            &pool,
            &Log {
                seq,
                events,
                ..Log::default()
            },
        )
        .await
        .expect(events);
    }
    for (seq, events) in (10..).zip(["{}", "null", "0", r#""[]""#]) {
        assert_eq!(
            violation(
                insert_log(
                    &pool,
                    &Log {
                        seq,
                        events,
                        ..Log::default()
                    }
                )
                .await
            ),
            (
                CHECK_VIOLATION.to_owned(),
                Some("log_records_events_check".to_owned())
            ),
            "events {events}"
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn rules_version_is_never_negative(pool: PgPool) {
    insert_log(
        &pool,
        &Log {
            seq: 1,
            rules_version: 0,
            ..Log::default()
        },
    )
    .await
    .expect("0");
    insert_log(
        &pool,
        &Log {
            seq: 2,
            rules_version: i32::MAX,
            ..Log::default()
        },
    )
    .await
    .expect("max");
    assert_eq!(
        violation(
            insert_log(
                &pool,
                &Log {
                    seq: 3,
                    rules_version: -1,
                    ..Log::default()
                }
            )
            .await
        ),
        (
            CHECK_VIOLATION.to_owned(),
            Some("log_records_rules_version_check".to_owned())
        )
    );
    assert_eq!(
        violation(insert_snapshot(&pool, 0, &[0; 32], -1).await),
        (
            CHECK_VIOLATION.to_owned(),
            Some("snapshots_rules_version_check".to_owned())
        )
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn log_not_null_columns_refuse_sql_null(pool: PgPool) {
    // An advance record stores JSON values, never SQL NULL (D-034); the same for every NOT NULL
    // column of the log.
    for column in [
        "kind",
        "game_time",
        "wall_time",
        "payload",
        "outcome",
        "events",
        "rules_version",
        "engine_build",
    ] {
        // The column name comes from the fixed list above, never from input.
        let sql = format!(
            "INSERT INTO log_records (seq, kind, game_time, payload, outcome, events, \
             rules_version, engine_build) VALUES (1, 'advance', 0, 'null', 'null', '[]', 0, 'b'); \
             UPDATE log_records SET {column} = NULL WHERE seq = 1"
        );
        let mut tx = pool.begin().await.expect("begin");
        let result = sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut *tx)
            .await;
        let (code, _) = violation(result);
        assert_eq!(code, NOT_NULL_VIOLATION, "column {column}");
        tx.rollback().await.expect("rollback");
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn defaults_stamp_the_write_time(pool: PgPool) {
    insert_log(&pool, &Log::default()).await.expect("log");
    insert_snapshot(&pool, 0, &[0; 32], 0)
        .await
        .expect("snapshot");
    insert_account(&pool, PLAYER_A, "http://localhost:5556/dex", "s")
        .await
        .expect("account");
    let stamped = sqlx::query_scalar!(
        r#"SELECT (SELECT wall_time FROM log_records) IS NOT NULL
              AND (SELECT created_at FROM snapshots) IS NOT NULL
              AND (SELECT created_at FROM accounts) IS NOT NULL AS "stamped!""#
    )
    .fetch_one(&pool)
    .await
    .expect("read");
    assert!(stamped);
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn snapshot_seq_may_be_zero_but_not_negative(pool: PgPool) {
    insert_snapshot(&pool, 0, &[0; 32], 0)
        .await
        .expect("the bootstrap snapshot, before any record");
    for seq in [-1, i64::MIN] {
        assert_eq!(
            violation(insert_snapshot(&pool, seq, &[0; 32], 0).await),
            (
                CHECK_VIOLATION.to_owned(),
                Some("snapshots_seq_check".to_owned())
            ),
            "seq {seq}"
        );
    }
    assert_eq!(
        violation(insert_snapshot(&pool, 0, &[1; 32], 0).await),
        (
            UNIQUE_VIOLATION.to_owned(),
            Some("snapshots_pkey".to_owned())
        )
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn state_hash_is_exactly_32_bytes(pool: PgPool) {
    insert_snapshot(&pool, 1, &[7; 32], 0)
        .await
        .expect("32 bytes, BLAKE3-256");
    for (seq, len) in (10..).zip([0_usize, 1, 31, 33, 64]) {
        assert_eq!(
            violation(insert_snapshot(&pool, seq, &vec![7; len], 0).await),
            (
                CHECK_VIOLATION.to_owned(),
                Some("snapshots_state_hash_check".to_owned())
            ),
            "length {len}"
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn accounts_map_issuer_and_subject_to_one_v7_player(pool: PgPool) {
    let dex = "http://localhost:5556/dex";
    insert_account(&pool, PLAYER_A, dex, "alice")
        .await
        .expect("first account");
    assert_eq!(
        violation(insert_account(&pool, PLAYER_B, dex, "alice").await),
        (
            UNIQUE_VIOLATION.to_owned(),
            Some("accounts_issuer_subject_unique".to_owned())
        ),
        "one player per issuer and subject"
    );
    insert_account(&pool, PLAYER_B, "https://github.com", "alice")
        .await
        .expect("the same subject at another issuer is another account");
    assert_eq!(
        violation(insert_account(&pool, PLAYER_A, dex, "bob").await),
        (
            UNIQUE_VIOLATION.to_owned(),
            Some("accounts_pkey".to_owned())
        ),
        "one account per player"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn accounts_refuse_non_v7_ids_and_empty_names(pool: PgPool) {
    let dex = "http://localhost:5556/dex";
    // A v4 ID; the nil UUID; and a version-7 nibble outside the RFC 9562 variant, for which
    // uuid_extract_version is NULL (a NULL CHECK would pass, so the constraint treats it as not 7).
    for id in [
        PLAYER_V4,
        "00000000-0000-0000-0000-000000000000",
        "01890a5d-ac96-774b-4cce-b302099a8057",
        "01890a5d-ac96-774b-ecce-b302099a8057",
    ] {
        assert_eq!(
            violation(insert_account(&pool, id, dex, "carol").await),
            (
                CHECK_VIOLATION.to_owned(),
                Some("accounts_player_id_check".to_owned())
            ),
            "id {id}"
        );
    }
    assert_eq!(
        violation(insert_account(&pool, PLAYER_A, "", "carol").await),
        (
            CHECK_VIOLATION.to_owned(),
            Some("accounts_issuer_check".to_owned())
        )
    );
    assert_eq!(
        violation(insert_account(&pool, PLAYER_A, dex, "").await),
        (
            CHECK_VIOLATION.to_owned(),
            Some("accounts_subject_check".to_owned())
        )
    );
}
