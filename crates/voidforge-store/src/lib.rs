//! Postgres access, migrations and the snapshot codec.
//!
//! P2.1 adds the schema: `log_records`, `snapshots` and `accounts`, created by the sqlx migrations
//! in `migrations/` (decision log D-034). Migrations only ever add. P2.2 adds the snapshot codec
//! ([`snapshot`]) and the canonical encoding it hashes ([`canonical`]; D-024, D-035).
//! P2.3 adds the log ([`log`]): typed records, the `LogStore` interface, an in-memory store and the
//! Postgres store (D-036).

pub mod canonical;
pub mod log;
pub mod snapshot;

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
