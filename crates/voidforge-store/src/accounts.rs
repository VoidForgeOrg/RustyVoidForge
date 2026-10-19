//! Player accounts: an issuer and subject map to a player UUIDv7 (Technical Plan, Decision 9;
//! decision log D-034, D-047).
//!
//! The server draws each new player ID; this module only finds or records it. Two first calls for
//! the same (issuer, subject) at once end with one row and one ID: the insert does nothing on the
//! unique constraint, and the loser reads the winner's row. The subject is personal data, so no
//! error here carries it.

use std::future::Future;

use sqlx::PgPool;
use voidforge_core::state::PlayerId;

use crate::log::{parse_player_uuid, player_uuid};

/// Why an account could not be found or created.
#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    /// The database could not be reached, or refused the write (a check constraint, or a player
    /// ID that collides with another account's). Redacted: see [`DbFailure`].
    #[error("accounts database: {0}")]
    Database(DbFailure),
    /// The insert yielded to an existing row, and then no row was there: it was deleted in
    /// between. Never answered with a fresh ID.
    #[error("the account row vanished between the insert and the read")]
    Vanished,
    /// The issuer or subject holds a NUL byte, which Postgres text cannot store. Permanent: a
    /// retry can't succeed.
    #[error("the issuer or subject holds a NUL byte and can't be stored")]
    Unstorable,
    /// A stored player ID is not a UUID: the table's check makes this unreachable.
    #[error("a stored player ID is not a UUID: {0:?}")]
    Corrupt(String),
}

impl From<sqlx::Error> for AccountError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(DbFailure::from(e))
    }
}

/// A database failure with nothing of the row in it. Postgres's message and DETAIL can quote the
/// values a statement wrote ("Failing row contains (…)"), the subject among them, so an error the
/// server refused keeps only its SQLSTATE and constraint name, in `Display` and `Debug` alike
/// (D-047, dev-qa's L1). Any other failure (a connection, a pool timeout) keeps sqlx's text, which
/// carries no row data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbFailure {
    /// The SQLSTATE of an error the server returned (`23514` for a check violation).
    pub sqlstate: Option<String>,
    /// The constraint it names, if any.
    pub constraint: Option<String>,
    /// What failed, when the server didn't return the error.
    pub other: Option<String>,
}

impl From<sqlx::Error> for DbFailure {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::Database(db) => Self {
                sqlstate: db.code().map(|c| c.into_owned()),
                constraint: db.constraint().map(str::to_owned),
                other: None,
            },
            other => Self {
                sqlstate: None,
                constraint: None,
                other: Some(other.to_string()),
            },
        }
    }
}

impl std::fmt::Display for DbFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(other) = &self.other {
            return f.write_str(other);
        }
        f.write_str("the database refused the statement")?;
        if let Some(code) = &self.sqlstate {
            write!(f, " (SQLSTATE {code}")?;
            if let Some(c) = &self.constraint {
                write!(f, ", constraint {c}")?;
            }
            f.write_str(")")?;
        }
        Ok(())
    }
}

/// Where accounts live. The server's account path takes any implementation, so the crash tests
/// can wrap the Postgres one without a hook in production code (D-042).
pub trait AccountStore: Send + Sync + 'static {
    /// The player ID for (issuer, subject). On first sight, records `fresh` and returns it; when
    /// the row exists, or another call records it first, returns the stored ID.
    fn lookup_or_create(
        &self,
        issuer: &str,
        subject: &str,
        fresh: PlayerId,
    ) -> impl Future<Output = Result<PlayerId, AccountError>> + Send;
}

/// Accounts in Postgres.
#[derive(Debug, Clone)]
pub struct PgAccounts {
    pool: PgPool,
}

impl PgAccounts {
    /// Accounts over `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Accounts over a pool that connects on first use, on the runtime that uses it (`serve`,
    /// P3.4s). The URL was checked by the caller; `None` if it is not a Postgres URL.
    #[must_use]
    pub fn lazy_url(url: &str) -> Option<Self> {
        let options: sqlx::postgres::PgConnectOptions = url.parse().ok()?;
        Some(Self::new(
            sqlx::postgres::PgPoolOptions::new().connect_lazy_with(options),
        ))
    }
}

impl AccountStore for PgAccounts {
    async fn lookup_or_create(
        &self,
        issuer: &str,
        subject: &str,
        fresh: PlayerId,
    ) -> Result<PlayerId, AccountError> {
        if issuer.contains('\0') || subject.contains('\0') {
            return Err(AccountError::Unstorable);
        }
        // Every authenticated request comes here, so the common case is one indexed read.
        if let Some(stored) = self.find(issuer, subject).await? {
            return parse(stored);
        }
        // At READ COMMITTED a concurrent insert of the same pair waits for the first one's
        // transaction: if it commits, this does nothing and the read below sees its row.
        let inserted = sqlx::query_scalar!(
            r#"INSERT INTO accounts (player_id, issuer, subject) VALUES ($1::text::uuid, $2, $3)
               ON CONFLICT (issuer, subject) DO NOTHING
               RETURNING player_id::text AS "player_id!""#,
            player_uuid(fresh),
            issuer,
            subject,
        )
        .fetch_optional(&self.pool)
        .await?;
        let stored = match inserted {
            Some(id) => id,
            None => self
                .find(issuer, subject)
                .await?
                .ok_or(AccountError::Vanished)?,
        };
        parse(stored)
    }
}

impl PgAccounts {
    async fn find(&self, issuer: &str, subject: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar!(
            r#"SELECT player_id::text AS "player_id!" FROM accounts
               WHERE issuer = $1 AND subject = $2"#,
            issuer,
            subject,
        )
        .fetch_optional(&self.pool)
        .await
    }
}

fn parse(stored: String) -> Result<PlayerId, AccountError> {
    parse_player_uuid(&stored).ok_or(AccountError::Corrupt(stored))
}
