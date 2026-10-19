//! Every answer that isn't a 200 is a D-045 `ErrorBody`, axum's own rejections included: a wrong
//! content type, a bad body, path or query, an unknown route or method, and a panic (D-049).

use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::header::RETRY_AFTER;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use voidforge_api_types::error::{ErrorBody, ErrorCode};
use voidforge_host::HostError;

/// A non-200 answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// The stable code; the status is its [`ErrorCode::status`].
    pub code: ErrorCode,
    /// For people; not the contract.
    pub message: String,
    /// Seconds to wait, on the 503s that promise a retry.
    pub retry_after: Option<u64>,
    /// A status other than the code's own: only `GET /planets/{id}`'s 404 `unknown_planet`
    /// (D-045's table).
    pub status: Option<StatusCode>,
}

impl ApiError {
    /// An error without Retry-After.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after: None,
            status: None,
        }
    }

    /// The same, answered with `status` instead of the code's own.
    #[must_use]
    pub fn with_status(mut self, status: StatusCode) -> Self {
        self.status = Some(status);
        self
    }

    /// The same, with Retry-After.
    #[must_use]
    pub fn retry_after(mut self, secs: u64) -> Self {
        self.retry_after = Some(secs);
        self
    }

    /// 500 `internal`, with a generic message: the cause is logged, never shown.
    pub fn internal(cause: &dyn std::fmt::Display) -> Self {
        tracing::error!(error = %cause, "internal error");
        Self::new(
            ErrorCode::Internal,
            "something unexpected failed on the server",
        )
    }
}

/// Seconds a client waits after `busy`: the inbox drains in moments.
pub const BUSY_RETRY_AFTER_SECS: u64 = 1;
/// Seconds a client waits after `unavailable` because the host stopped.
pub const UNAVAILABLE_RETRY_AFTER_SECS: u64 = 5;

/// The one mapping of a host error to its answer, shared by the account path and
/// `POST /commands`, so the two can't drift (the lead's ruling).
pub fn host_error(e: HostError) -> ApiError {
    match e {
        HostError::Busy => ApiError::new(ErrorCode::Busy, "the engine is busy; retry shortly")
            .retry_after(BUSY_RETRY_AFTER_SECS),
        HostError::Stopped => ApiError::new(
            ErrorCode::Unavailable,
            "the engine is not running; retry later",
        )
        .retry_after(UNAVAILABLE_RETRY_AFTER_SECS),
        HostError::KeyConflict { .. } => ApiError::new(
            ErrorCode::IdempotencyKeyConflict,
            "this Idempotency-Key was used for a different command",
        ),
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status.unwrap_or_else(|| {
            StatusCode::from_u16(self.code.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
        });
        let body = ErrorBody {
            code: self.code,
            message: self.message,
        };
        let mut response = (status, axum::Json(body)).into_response();
        if let Some(secs) = self.retry_after {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(secs));
        }
        response
    }
}

/// A JSON body, refused as 400 `malformed_body`: a missing or wrong content type, invalid JSON,
/// an unknown command type (`join_game` included), a missing, unknown or mistyped field, an
/// out-of-range ID, an integral float, or a body over the limit.
pub struct ApiJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        axum::Json::<T>::from_request(req, state)
            .await
            .map(|axum::Json(v)| Self(v))
            .map_err(|e: JsonRejection| {
                ApiError::new(
                    ErrorCode::MalformedBody,
                    format!("the body is malformed: {e}"),
                )
            })
    }
}

/// Query parameters, refused as 400 `invalid_query`.
pub struct ApiQuery<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for ApiQuery<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Query(v)| Self(v))
            .map_err(|e: QueryRejection| {
                ApiError::new(
                    ErrorCode::InvalidQuery,
                    format!("the query is invalid: {e}"),
                )
            })
    }
}

/// Path parameters, refused as 400 `invalid_query`.
pub struct ApiPath<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for ApiPath<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        axum::extract::Path::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Path(v)| Self(v))
            .map_err(|e: PathRejection| {
                ApiError::new(ErrorCode::InvalidQuery, format!("the path is invalid: {e}"))
            })
    }
}

/// An unknown route, or a known route with another method: 404 `not_found`. The code list is
/// closed, so there is no 405 (D-049, T3).
pub async fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "no such route or method")
}

/// A panicking handler answers 500 `internal`, never a dropped connection. Each request runs as
/// its own task, so a panic surfaces as a `JoinError` here.
pub async fn catch_panic(req: Request, next: Next) -> Response {
    match tokio::spawn(next.run(req)).await {
        Ok(response) => response,
        Err(e) => ApiError::internal(&format_args!("a handler failed: {e}")).into_response(),
    }
}
