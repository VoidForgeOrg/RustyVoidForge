//! Postgres access, migrations and the snapshot codec.
//!
//! P2.1 adds the schema: `log_records`, `snapshots` and `accounts`, created by the sqlx migrations
//! in `migrations/` (decision log D-034). Migrations only ever add. P2.2 adds the snapshot codec
//! ([`snapshot`]) and the canonical encoding it hashes ([`canonical`]; D-024, D-035).
//! P2.3 adds the log ([`log`]): typed records, the `LogStore` interface, an in-memory store and the
//! Postgres store (D-036). P2.5 adds snapshot storage and [`replay`], the one record→core function
//! the host and the replay tool share (D-038). P2.6 and P2.7 add the golden format ([`golden`]) and
//! offline verification ([`verify`]), which the host's tests and the replay tool share (D-040,
//! D-041). P3.4s adds the universe's durable epoch: [`bootstrap`] records it and
//! [`open_universe`] reads it for `serve` (D-050).

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

/// The first epoch a universe may have: 1970-01-01T00:00:00Z, in Unix milliseconds (D-050).
pub const EPOCH_MIN_UNIX_MS: i64 = 0;

/// The first epoch past the window: 2100-01-01T00:00:00Z. The window keeps every stamped time
/// formattable: a stamp is max(wall − epoch, last stamp), so epoch + stamp is at most the later of
/// the wall clock and the epoch, both far below year 9999 (D-050, closing the P3.4 QA's I2).
pub const EPOCH_END_UNIX_MS: i64 = 4_102_444_800_000;

/// Whether `epoch_unix_ms` is in the window [`EPOCH_MIN_UNIX_MS`, [`EPOCH_END_UNIX_MS`]).
#[must_use]
pub fn epoch_in_window(epoch_unix_ms: i64) -> bool {
    (EPOCH_MIN_UNIX_MS..EPOCH_END_UNIX_MS).contains(&epoch_unix_ms)
}

/// Why a universe could not be bootstrapped.
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    /// The database URL is not a Postgres URL.
    #[error("the database URL is not a Postgres URL")]
    Url,
    /// The epoch is outside the window; nothing was written (D-050).
    #[error(
        "the epoch {0} ms is outside 1970-01-01 to 2100-01-01 ({EPOCH_MIN_UNIX_MS} to {EPOCH_END_UNIX_MS} ms, exclusive)"
    )]
    EpochOutOfRange(i64),
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
/// the migrations, and writes `snapshot` at seq 0 and the universe's epoch, in one transaction
/// that holds the four tables locked and refuses unless the database is empty. A universe is
/// never overwritten, an accounts-only database is not empty (D-047), and a universe never
/// exists without its epoch (D-050).
///
/// # Errors
/// [`BootstrapError::EpochOutOfRange`] before connecting, if the epoch is outside the window;
/// [`BootstrapError::NotEmpty`] if any snapshot, log record or epoch exists;
/// [`BootstrapError::HasAccounts`] if only accounts exist; otherwise the connection, migration
/// or write failure.
pub async fn bootstrap(
    url: &str,
    snapshot: &snapshot::Snapshot,
    epoch_unix_ms: i64,
) -> Result<(), BootstrapError> {
    if !epoch_in_window(epoch_unix_ms) {
        return Err(BootstrapError::EpochOutOfRange(epoch_unix_ms));
    }
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
    sqlx::query!("LOCK TABLE snapshots, log_records, accounts, universe IN EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await?;
    // An epoch can't exist without a snapshot, but one written by hand still counts as used:
    // seeding refuses rather than half-seeds.
    let used = sqlx::query_scalar!(
        r#"SELECT (EXISTS (SELECT 1 FROM snapshots) OR EXISTS (SELECT 1 FROM log_records)
                   OR EXISTS (SELECT 1 FROM universe)) AS "used!""#
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
    sqlx::query!(
        "INSERT INTO universe (epoch_unix_ms) VALUES ($1)",
        epoch_unix_ms
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Why `serve` can't open a universe (D-050).
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The database URL is not a Postgres URL.
    #[error("the database URL is not a Postgres URL")]
    Url,
    /// The database could not be reached or read.
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    /// Applying the migrations failed.
    #[error("migrations failed: {0}")]
    Migrate(#[from] MigrateError),
    /// The database holds no universe (no `snapshots` table, or an empty one). Nothing was
    /// migrated or written: `serve` never creates a schema; `seed` does.
    #[error("the database holds no universe; seed it first")]
    NoUniverse,
    /// The universe was seeded before its epoch was recorded, so its game time can't be placed.
    /// An epoch is never invented (D-044, D-048).
    #[error(
        "this universe was seeded before its epoch was recorded (D-048), so its game time can't \
         be placed, and an epoch is never invented; re-seed it (for voidforge_dev: just seed-reset)"
    )]
    NoEpoch,
    /// The recorded epoch is outside the window (only possible past the table's CHECK).
    #[error(
        "the recorded epoch {0} ms is outside 1970-01-01 to 2100-01-01 ({EPOCH_MIN_UNIX_MS} to {EPOCH_END_UNIX_MS} ms, exclusive)"
    )]
    EpochOutOfRange(i64),
}

/// Opens the universe at `url` for `serve` (D-050): refuses a database without a universe
/// **before** migrating it, so a mistyped URL leaves an empty database untouched; then applies
/// any pending migrations (they only add, D-034) and reads the recorded epoch. Returns the epoch.
///
/// # Errors
/// [`OpenError::NoUniverse`], [`OpenError::NoEpoch`] or [`OpenError::EpochOutOfRange`], or the
/// connection, read or migration failure.
pub async fn open_universe(url: &str) -> Result<i64, OpenError> {
    let options: sqlx::postgres::PgConnectOptions = url.parse().map_err(|_| OpenError::Url)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let has_snapshots =
        sqlx::query_scalar!(r#"SELECT to_regclass('snapshots') IS NOT NULL AS "exists!""#)
            .fetch_one(&pool)
            .await?;
    if !has_snapshots {
        return Err(OpenError::NoUniverse);
    }
    let seeded = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM snapshots) AS "seeded!""#)
        .fetch_one(&pool)
        .await?;
    if !seeded {
        return Err(OpenError::NoUniverse);
    }
    migrate(&pool).await.map_err(|StoreError::Migrate(e)| e)?;
    let epoch = universe_epoch(&pool).await?.ok_or(OpenError::NoEpoch)?;
    if !epoch_in_window(epoch) {
        return Err(OpenError::EpochOutOfRange(epoch));
    }
    pool.close().await;
    Ok(epoch)
}

/// The recorded epoch, if any. The database has the migrations applied.
///
/// # Errors
/// The read failure.
pub async fn universe_epoch(pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!("SELECT epoch_unix_ms FROM universe")
        .fetch_optional(pool)
        .await
}

/// The advisory lock key a serving process holds: the bytes of "VoidForg". An advisory lock is
/// scoped to its database, and a database holds one universe, so one key per database is one lock
/// per universe (D-050, the P3.4s QA's I4).
pub const SERVE_LOCK_KEY: i64 = 6_228_312_689_076_040_295;

/// Why `serve` can't hold its universe.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// The database URL is not a Postgres URL.
    #[error("the database URL is not a Postgres URL")]
    Url,
    /// The database could not be reached or read.
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    /// Another process holds this universe: two hosts on one log would race for its sequence.
    #[error("another serve already holds this universe; only one may run against it")]
    Held,
}

/// A held universe: a session advisory lock on a connection of its own. Postgres releases it when
/// the connection closes, whether this value is dropped or the process dies.
#[derive(Debug)]
pub struct UniverseLock {
    _connection: sqlx::PgConnection,
}

/// Takes the universe at `url` for one serving process, without waiting: a second `serve` on the
/// same universe refuses rather than queue behind the first (D-050).
///
/// # Errors
/// [`LockError::Held`] if another session holds it; otherwise the connection failure.
pub async fn lock_universe(url: &str) -> Result<UniverseLock, LockError> {
    use sqlx::Connection as _;
    let options: sqlx::postgres::PgConnectOptions = url.parse().map_err(|_| LockError::Url)?;
    let mut connection = sqlx::PgConnection::connect_with(&options).await?;
    let taken = sqlx::query_scalar!(
        r#"SELECT pg_try_advisory_lock($1) AS "taken!""#,
        SERVE_LOCK_KEY
    )
    .fetch_one(&mut connection)
    .await?;
    if !taken {
        return Err(LockError::Held);
    }
    Ok(UniverseLock {
        _connection: connection,
    })
}
