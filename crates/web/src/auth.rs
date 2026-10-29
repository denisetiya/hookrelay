use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

use crate::AppState;
use crate::error::AppError;
use crate::rate_limit::RateDecision;

#[derive(Debug, Clone, Copy)]
pub struct TenantId(pub Uuid);

impl FromRequestParts<AppState> for TenantId {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<TenantId>()
            .copied()
            .ok_or(AppError::Unauthorized)
    }
}

const KEY_SCHEME: &str = "hr_live_";

pub async fn require_api_key(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?;

    let (prefix, secret) = parse_key(token).ok_or(AppError::Unauthorized)?;

    let key = hookrelay_store::api_keys::find_active_by_prefix(&state.pool, prefix)
        .await?
        .ok_or(AppError::Unauthorized)?;

    let hash = key.key_hash.clone();
    let secret = secret.to_owned();
    let valid = tokio::task::spawn_blocking(move || verify(&hash, &secret))
        .await
        .map_err(|error| AppError::Internal(error.into()))?;

    if !valid {
        return Err(AppError::Unauthorized);
    }

    match state.limiter.check(prefix).await {
        Ok(RateDecision::Allowed { .. }) => {}
        Ok(RateDecision::Denied { retry_after }) => {
            return Err(AppError::RateLimited(retry_after));
        }
        Err(error) => {
            // Ingest availability matters more than strict limiting; the metric
            // tracks that this path was taken.
            tracing::warn!(error = %error, "rate limiter unavailable, allowing request");
        }
    }

    request.extensions_mut().insert(TenantId(key.tenant_id));

    Ok(next.run(request).await)
}

fn parse_key(token: &str) -> Option<(&str, &str)> {
    let rest = token.strip_prefix(KEY_SCHEME)?;

    rest.rsplit_once('_')
        .filter(|(prefix, secret)| !prefix.is_empty() && !secret.is_empty())
}

fn verify(hash: &str, secret: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };

    Argon2::default()
        .verify_password(secret.as_bytes(), &parsed)
        .is_ok()
}
