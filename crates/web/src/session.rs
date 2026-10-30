use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use uuid::Uuid;

const COOKIE_NAME: &str = "hr_session";
const MAX_AGE_SECONDS: u64 = 86_400;

#[derive(Debug, Clone)]
pub struct Session {
    pub tenant_id: Uuid,
    pub prefix: String,
}

pub fn issue(secret: &[u8], tenant_id: Uuid, prefix: &str) -> String {
    let expires = now() + MAX_AGE_SECONDS;
    let payload = format!("{tenant_id}|{prefix}|{expires}");
    let signature = hex::encode(mac(secret, payload.as_bytes()));

    format!("{payload}|{signature}")
}

pub fn verify(secret: &[u8], value: &str) -> Option<Session> {
    let (payload, signature) = value.rsplit_once('|')?;
    let provided = hex::decode(signature).ok()?;
    let expected = mac(secret, payload.as_bytes());

    if provided.ct_eq(&expected).unwrap_u8() != 1 {
        return None;
    }

    let mut parts = payload.split('|');
    let tenant_id = Uuid::parse_str(parts.next()?).ok()?;
    let prefix = parts.next()?.to_owned();
    let expires: u64 = parts.next()?.parse().ok()?;

    if parts.next().is_some() || expires < now() {
        return None;
    }

    Some(Session { tenant_id, prefix })
}

pub fn from_headers(secret: &[u8], headers: &HeaderMap) -> Option<Session> {
    let cookie = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    let value = cookie
        .split(';')
        .find_map(|item| item.trim().strip_prefix("hr_session="))?;

    verify(secret, value)
}

pub fn cookie(value: &str) -> String {
    format!("{COOKIE_NAME}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={MAX_AGE_SECONDS}")
}

pub fn cleared_cookie() -> String {
    format!("{COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}

fn mac(secret: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(payload);

    mac.finalize().into_bytes().to_vec()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn round_trips() {
        let tenant_id = Uuid::now_v7();
        let value = issue(SECRET, tenant_id, "abcdefghijkl");
        let session = verify(SECRET, &value).unwrap();

        assert_eq!(session.tenant_id, tenant_id);
        assert_eq!(session.prefix, "abcdefghijkl");
    }

    #[test]
    fn rejects_tampered_payload() {
        let value = issue(SECRET, Uuid::now_v7(), "abcdefghijkl");
        let tampered = value.replacen("abcdefghijkl", "zzzzzzzzzzzz", 1);

        assert!(verify(SECRET, &tampered).is_none());
    }

    #[test]
    fn rejects_other_secret() {
        let value = issue(SECRET, Uuid::now_v7(), "abcdefghijkl");

        assert!(verify(b"another-secret-another-secret-12", &value).is_none());
    }

    #[test]
    fn rejects_expired() {
        let tenant_id = Uuid::now_v7();
        let payload = format!("{tenant_id}|abcdefghijkl|{}", now() - 10);
        let signature = hex::encode(mac(SECRET, payload.as_bytes()));
        let value = format!("{payload}|{signature}");

        assert!(verify(SECRET, &value).is_none());
    }
}
