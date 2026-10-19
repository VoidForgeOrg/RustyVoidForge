//! Postgres access, migrations and the snapshot codec.
//!
//! P2.1 adds the schema: `log_records`, `snapshots` and `accounts`, created by the sqlx migrations
//! in `migrations/` (decision log D-034). Migrations only ever add. P2.2 adds the snapshot codec
//! ([`snapshot`]) and the canonical encoding it hashes ([`canonical`]; D-024, D-035).
//! P2.3 adds the log ([`log`]): typed records, the `LogStore` interface, an in-memory store and the
//! Postgres store (D-036). P2.5 adds snapshot storage and [`replay`], the one record→core function
//! the host and the replay tool share (D-038). P2.6 and P2.7 add the golden format ([`golden`]) and
//! offline verification ([`verify`]), which the host's tests and the replay tool share (D-040,
//! D-041).

pub mod accounts;
pub mod canonical;
pub mod golden;
pub mod log;
pub mod replay;
pub mod snapshot;
pub mod verify;

use sqlx::PgPool;
use sqlx::migrate::{MigrateError, Migrator};

/// The schema migrations, embedded at compile time from `migrations/`.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// A store failure.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Applying the migrations failed, including a migration that changed after it was applied.
    #[error("migrations failed: {0}")]
    Migrate(#[from] MigrateError),
}

/// Applies every migration not yet applied to the database. Re-running is a no-op.
///
/// # Errors
/// [`StoreError::Migrate`] if a migration fails, or if an applied migration's checksum no longer
/// matches the embedded file.
pub async fn migrate(pool: &PgPool) -> Result<(), StoreError> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

/// Why a universe could not be bootstrapped.
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    /// The database URL is not a Postgres URL.
    #[error("the database URL is not a Postgres URL")]
    Url,
    /// The database could not be reached or written.
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    /// Applying the migrations failed.
    #[error("migrations failed: {0}")]
    Migrate(#[from] MigrateError),
    /// The snapshot row's rules version exceeds the column.
    #[error("rules version {0} exceeds i32")]
    RulesVersion(u32),
    /// The database already holds a universe (any snapshot or log record): never overwritten.
    #[error("the database already holds a universe")]
    NotEmpty {
        /// The existing seq-0 snapshot's state hash, if there is one.
        seq0_state_hash: Option<Vec<u8>>,
    },
    /// The database has no universe but has accounts: the universe was lost, not never made.
    /// Seeding would give those players a fresh start under old identities, so the accounts must
    /// be cleared by hand first (D-047).
    #[error(
        "the database has no universe but has {count} accounts; clear them by hand before seeding"
    )]
    HasAccounts {
        /// How many accounts exist.
        count: i64,
    },
}

/// Bootstraps a universe (Technical Plan P2.9; decision log D-043): connects to `url`, applies
/// the migrations, and writes `snapshot` at seq 0, in one transaction that holds the three
/// tables locked and refuses unless the database is empty. A universe is never overwritten, and
/// an accounts-only database is not empty (D-047).
///
/// # Errors
/// [`BootstrapError::NotEmpty`] if any snapshot or log record exists;
/// [`BootstrapError::HasAccounts`] if only accounts exist; otherwise the connection, migration
/// or write failure.
pub async fn bootstrap(url: &str, snapshot: &snapshot::Snapshot) -> Result<(), BootstrapError> {
    let row = log::SnapshotRow::new(0, snapshot);
    let options: sqlx::postgres::PgConnectOptions = url.parse().map_err(|_| BootstrapError::Url)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    migrate(&pool).await.map_err(|StoreError::Migrate(e)| e)?;
    let rules_version = i32::try_from(row.rules_version)
        .map_err(|_| BootstrapError::RulesVersion(row.rules_version))?;
    let mut tx = pool.begin().await?;
    // Two seeds at once cannot both find the database empty, and no account can appear between
    // the check and the commit.
    sqlx::query!("LOCK TABLE snapshots, log_records, accounts IN EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await?;
    let used = sqlx::query_scalar!(
        r#"SELECT (EXISTS (SELECT 1 FROM snapshots) OR EXISTS (SELECT 1 FROM log_records)) AS "used!""#
    )
    .fetch_one(&mut *tx)
    .await?;
    if used {
        let seq0_state_hash = sqlx::query_scalar!("SELECT state_hash FROM snapshots WHERE seq = 0")
            .fetch_optional(&mut *tx)
            .await?;
        return Err(BootstrapError::NotEmpty { seq0_state_hash });
    }
    let count = sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM accounts"#)
        .fetch_one(&mut *tx)
        .await?;
    if count > 0 {
        return Err(BootstrapError::HasAccounts { count });
    }
    sqlx::query!(
        r#"INSERT INTO snapshots (seq, game_time, rules_version, engine_build, state_hash, payload)
               VALUES ($1, $2, $3, $4, $5, $6)"#,
        row.seq,
        row.game_time,
        rules_version,
        row.engine_build,
        row.state_hash,
        row.payload,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
