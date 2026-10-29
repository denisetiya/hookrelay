use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Tenant {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ApiKey {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub prefix: String,
    pub key_hash: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Endpoint {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub url: String,
    pub description: Option<String>,
    pub secret_current: Vec<u8>,
    pub secret_previous: Option<Vec<u8>>,
    pub secret_rotated_at: Option<DateTime<Utc>>,
    pub event_types: Vec<String>,
    pub status: String,
    pub max_attempts: i16,
    pub rate_limit_per_sec: i32,
    pub max_concurrency: i16,
    pub ordering_mode: String,
    pub created_at: DateTime<Utc>,
}
