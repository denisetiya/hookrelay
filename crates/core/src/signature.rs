use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

pub const SIGNATURE_VERSION: &str = "v1";

/// Receivers reject timestamps older or newer than this, which bounds replay of a
/// captured request even before the delivery record is replayed.
pub const MAX_CLOCK_SKEW_SECONDS: i64 = 300;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SignatureError {
    #[error("signature header is malformed")]
    MalformedHeader,
    #[error("signature version {0} is not supported")]
    UnsupportedVersion(String),
    #[error("timestamp is outside the allowed clock skew")]
    TimestampOutOfRange,
    #[error("signature does not match")]
    Mismatch,
}

/// Produces `v1,<unix-timestamp>,<hex-digest>` over `timestamp.body`.
pub fn sign(secret: &[u8], timestamp: i64, body: &[u8]) -> String {
    let digest = hex::encode(compute(secret, timestamp, body));
    format!("{SIGNATURE_VERSION},{timestamp},{digest}")
}

pub fn verify(
    secret: &[u8],
    header: &str,
    body: &[u8],
    now: DateTime<Utc>,
) -> Result<(), SignatureError> {
    let mut parts = header.split(',');
    let version = parts.next().ok_or(SignatureError::MalformedHeader)?;
    let timestamp = parts
        .next()
        .ok_or(SignatureError::MalformedHeader)?
        .parse::<i64>()
        .map_err(|_| SignatureError::MalformedHeader)?;
    let provided = parts.next().ok_or(SignatureError::MalformedHeader)?;

    if parts.next().is_some() {
        return Err(SignatureError::MalformedHeader);
    }
    if version != SIGNATURE_VERSION {
        return Err(SignatureError::UnsupportedVersion(version.to_owned()));
    }

    let provided = hex::decode(provided).map_err(|_| SignatureError::MalformedHeader)?;

    if (now.timestamp() - timestamp).abs() > MAX_CLOCK_SKEW_SECONDS {
        return Err(SignatureError::TimestampOutOfRange);
    }

    let expected = compute(secret, timestamp, body);

    if provided.ct_eq(&expected).into() {
        Ok(())
    } else {
        Err(SignatureError::Mismatch)
    }
}

fn compute(secret: &[u8], timestamp: i64, body: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts keys of any length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const SECRET: &[u8] = b"super-secret";
    const BODY: &[u8] = br#"{"type":"invoice.paid"}"#;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
    }

    #[test]
    fn round_trips() {
        let header = sign(SECRET, now().timestamp(), BODY);
        assert!(verify(SECRET, &header, BODY, now()).is_ok());
    }

    #[test]
    fn header_shape() {
        let header = sign(SECRET, 1_700_000_000, BODY);
        let parts: Vec<_> = header.split(',').collect();

        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], SIGNATURE_VERSION);
        assert_eq!(parts[1], "1700000000");
        assert_eq!(parts[2].len(), 64);
    }

    #[test]
    fn rejects_tampered_body() {
        let header = sign(SECRET, now().timestamp(), BODY);
        let err = verify(SECRET, &header, b"{}", now()).unwrap_err();

        assert_eq!(err, SignatureError::Mismatch);
    }

    #[test]
    fn rejects_wrong_secret() {
        let header = sign(b"other-secret", now().timestamp(), BODY);

        assert_eq!(
            verify(SECRET, &header, BODY, now()).unwrap_err(),
            SignatureError::Mismatch
        );
    }

    #[test]
    fn rejects_stale_timestamp() {
        let stale = now().timestamp() - MAX_CLOCK_SKEW_SECONDS - 1;
        let header = sign(SECRET, stale, BODY);

        assert_eq!(
            verify(SECRET, &header, BODY, now()).unwrap_err(),
            SignatureError::TimestampOutOfRange
        );
    }

    #[test]
    fn rejects_future_timestamp_beyond_skew() {
        let future = now().timestamp() + MAX_CLOCK_SKEW_SECONDS + 1;
        let header = sign(SECRET, future, BODY);

        assert_eq!(
            verify(SECRET, &header, BODY, now()).unwrap_err(),
            SignatureError::TimestampOutOfRange
        );
    }

    #[test]
    fn rejects_unknown_version() {
        let header = sign(SECRET, now().timestamp(), BODY).replacen("v1", "v2", 1);

        assert_eq!(
            verify(SECRET, &header, BODY, now()).unwrap_err(),
            SignatureError::UnsupportedVersion("v2".to_owned())
        );
    }

    #[test]
    fn rejects_malformed_headers() {
        for header in ["", "v1", "v1,abc,ff", "v1,1,zz", "v1,1,ff,extra"] {
            assert_eq!(
                verify(SECRET, header, BODY, now()).unwrap_err(),
                SignatureError::MalformedHeader,
                "header: {header}"
            );
        }
    }
}
