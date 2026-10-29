use std::borrow::Cow;
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hookrelay_store::error::StoreError;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    Conflict(String),
    #[error("rate limited")]
    RateLimited(Duration),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<StoreError> for AppError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::NotFound => Self::NotFound,
            StoreError::Conflict(message) => Self::Conflict(message.to_owned()),
            StoreError::Db(error) => Self::Internal(error.into()),
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: Cow<'static, str>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let mut retry_after = None;
        let (status, code, message) = match self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                Cow::Borrowed("invalid api key"),
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                Cow::Borrowed("resource not found"),
            ),
            Self::Validation(message) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request",
                message.into(),
            ),
            Self::Conflict(message) => (StatusCode::CONFLICT, "conflict", message.into()),
            Self::RateLimited(delay) => {
                retry_after = Some(delay);

                (
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    Cow::Borrowed("rate limit exceeded"),
                )
            }
            Self::Internal(error) => {
                tracing::error!(error = %error, "internal error");

                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    Cow::Borrowed("internal error"),
                )
            }
        };

        let mut response = (
            status,
            Json(ErrorBody {
                error: ErrorDetail { code, message },
            }),
        )
            .into_response();

        if let Some(delay) = retry_after {
            let seconds = delay.as_secs().max(1).to_string();

            if let Ok(value) = axum::http::HeaderValue::from_str(&seconds) {
                response
                    .headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, value);
            }
        }

        response
    }
}
