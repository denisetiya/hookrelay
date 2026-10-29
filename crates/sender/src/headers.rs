use chrono::Utc;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use uuid::Uuid;

use crate::{SendRequest, SenderError};

const SIGNATURE_HEADER: &str = "x-hookrelay-signature";
const DELIVERY_HEADER: &str = "x-hookrelay-id";
const EVENT_HEADER: &str = "x-hookrelay-event";
const ATTEMPT_HEADER: &str = "x-hookrelay-attempt";
const IDEMPOTENCY_HEADER: &str = "idempotency-key";

pub(crate) fn build(request: &SendRequest<'_>) -> Result<HeaderMap, SenderError> {
    let timestamp = Utc::now().timestamp();
    let signature = hookrelay_core::signature::sign(request.secret, timestamp, request.body);

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    insert(&mut headers, SIGNATURE_HEADER, &signature)?;
    insert(
        &mut headers,
        DELIVERY_HEADER,
        &request.delivery_id.to_string(),
    )?;
    insert(&mut headers, EVENT_HEADER, &request.event_id.to_string())?;
    insert(&mut headers, ATTEMPT_HEADER, &request.attempt.to_string())?;
    insert(
        &mut headers,
        IDEMPOTENCY_HEADER,
        &idempotency_value(request.delivery_id),
    )?;

    Ok(headers)
}

fn insert(headers: &mut HeaderMap, name: &'static str, value: &str) -> Result<(), SenderError> {
    let value = HeaderValue::from_str(value)
        .map_err(|_| SenderError::InvalidHeader(format!("{name} contains invalid bytes")))?;

    headers.insert(name, value);

    Ok(())
}

fn idempotency_value(delivery_id: Uuid) -> String {
    delivery_id.to_string()
}
