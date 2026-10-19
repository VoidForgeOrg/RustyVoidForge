//! Accounts: a verified [`Principal`] becomes a [`Player`], and a new one joins the game exactly
//! once (Technical Plan P3.3, Decision 9; decision log D-047).
//!
//! Every call, in this order:
//! 1. finds the principal's account, creating it with a fresh UUIDv7 on first sight;
//! 2. returns the player if the host's read model shows them in the World (published only once
//!    durable, so a join that could still be lost is never reported);
//! 3. answers `no_free_homeworld` from the read model when no candidate is free, appending
//!    nothing;
//! 4. otherwise submits Join game, with no idempotency key: D-037 would keep a rejection under a
//!    key for good, so a refused account could never join later. The core refuses a second join
//!    with `already_joined` and the host applies commands one at a time, so that receipt means
//!    another call joined this player first.
//!
//! The row comes before the join, so a crash between them leaves an account without a homeworld,
//! which that user's next call repairs; the reverse order could strand a homeworld under an ID no
//! account names. The subject is personal data: nothing here logs it or puts it in an error.

use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use voidforge_api_types::error::ErrorCode;
use voidforge_host::{Action, Command, HostError, HostHandle, PlayerId, Rejection};
use voidforge_store::accounts::{AccountError, AccountStore, PgAccounts};

use crate::api::error::{ApiError, host_error};
pub use crate::api::error::{BUSY_RETRY_AFTER_SECS, UNAVAILABLE_RETRY_AFTER_SECS};
use crate::auth::{Principal, Refusal, TokenVerifier};
use crate::codes::game_code;

/// The player a request acts as. Its ID is the public identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Player {
    /// The player UUID.
    pub id: PlayerId,
}

/// Why a principal has no player for this request. None of these is ever a join.
#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    /// The core refused Join game (`no_free_homeworld` in the MVP): 422 with its code. The
    /// account stays, and each later call checks again.
    #[error("the game refused to join this player: {}", .0.code())]
    Refused(Rejection),
    /// The engine's inbox is full: 503 `busy`.
    #[error("the engine host is busy")]
    Busy,
    /// The engine host has stopped: 503 `unavailable`.
    #[error("the engine host has stopped")]
    Stopped,
    /// The accounts table could not be read or written: 503 `unavailable`, or 500 `internal`
    /// for a principal that can never be stored ([`AccountError::Unstorable`]).
    #[error(transparent)]
    Store(#[from] AccountError),
    /// Something that cannot happen did: 500 `internal`.
    #[error("internal: {0}")]
    Internal(&'static str),
}

impl PlayerError {
    /// The API's code for this error; its status is [`ErrorCode::status`].
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Refused(r) => game_code(*r),
            Self::Busy => ErrorCode::Busy,
            Self::Store(AccountError::Unstorable) | Self::Internal(_) => ErrorCode::Internal,
            Self::Stopped | Self::Store(_) => ErrorCode::Unavailable,
        }
    }
}

/// Turns principals into players, over an account store and the engine host.
#[derive(Debug)]
pub struct Players<A> {
    accounts: A,
    host: HostHandle,
}

impl<A: AccountStore> Players<A> {
    /// Players over `accounts` and `host`.
    pub fn new(accounts: A, host: HostHandle) -> Self {
        Self { accounts, host }
    }

    /// The principal's player, creating the account and joining the game on first sight.
    ///
    /// # Errors
    /// A [`PlayerError`]; the player is never reported as joined unless the World holds them at
    /// a durable record.
    pub async fn player(&self, principal: &Principal) -> Result<Player, PlayerError> {
        let fresh = PlayerId(uuid::Uuid::now_v7().as_u128());
        let id = self
            .accounts
            .lookup_or_create(&principal.issuer, &principal.subject, fresh)
            .await
            .inspect_err(
                |e| tracing::warn!(issuer = %principal.issuer, error = %e, "account lookup failed"),
            )?;
        let player = Player { id };
        // One snapshot for both checks: a second read could see this player's own join land
        // between them, and answer a joined player with `no_free_homeworld` (dev-qa's I1).
        let read = self.host.read();
        if read.world.players.contains_key(&id) {
            return Ok(player);
        }
        if read.world.free_homeworld().is_none() {
            return Err(PlayerError::Refused(Rejection::NoFreeHomeworld));
        }
        let receipt = self
            .host
            .submit(Command {
                player: id,
                action: Action::JoinGame,
            })
            .await
            .map_err(|e| match e {
                HostError::Busy => PlayerError::Busy,
                HostError::Stopped => PlayerError::Stopped,
                HostError::KeyConflict { .. } => {
                    PlayerError::Internal("a key conflict on a join submitted without a key")
                }
            })?;
        match receipt.outcome {
            Ok(_) => {
                tracing::info!(player = %player_uuid(id), seq = receipt.seq, "player joined");
                Ok(player)
            }
            Err(Rejection::AlreadyJoined) => {
                tracing::debug!(player = %player_uuid(id), "another call joined this player first");
                Ok(player)
            }
            Err(r) => {
                tracing::info!(player = %player_uuid(id), code = r.code(), "join refused");
                Err(PlayerError::Refused(r))
            }
        }
    }
}

fn player_uuid(id: PlayerId) -> uuid::Uuid {
    uuid::Uuid::from_u128(id.0)
}

// --- The extractor ---

impl<S> FromRequestParts<S> for Player
where
    Arc<TokenVerifier>: FromRef<S>,
    Arc<Players<PgAccounts>>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Response> {
        let principal = Principal::from_request_parts(parts, state)
            .await
            .map_err(Refusal::into_response)?;
        Arc::<Players<PgAccounts>>::from_ref(state)
            .player(&principal)
            .await
            .map_err(IntoResponse::into_response)
    }
}

impl From<PlayerError> for ApiError {
    fn from(e: PlayerError) -> Self {
        let code = e.code();
        match e {
            // The one host-error mapping, shared with `POST /commands` (D-049).
            PlayerError::Busy => host_error(HostError::Busy),
            PlayerError::Stopped => host_error(HostError::Stopped),
            PlayerError::Refused(Rejection::NoFreeHomeworld) => ApiError::new(
                code,
                "no homeworld is free, so this account can't join the game",
            ),
            // The store's error can name database internals; the body says only what happened.
            PlayerError::Store(AccountError::Unstorable) => {
                ApiError::new(code, "this account's identity can't be stored")
            }
            PlayerError::Store(_) => {
                ApiError::new(code, "accounts can't be read right now; retry later")
                    .retry_after(UNAVAILABLE_RETRY_AFTER_SECS)
            }
            refused @ PlayerError::Refused(_) => ApiError::new(code, refused.to_string()),
            // Loud, as every other 500: the cause is logged and the body is generic (R1's INFO).
            internal @ PlayerError::Internal(_) => ApiError::internal(&internal),
        }
    }
}

impl IntoResponse for PlayerError {
    fn into_response(self) -> Response {
        ApiError::from(self).into_response()
    }
}
