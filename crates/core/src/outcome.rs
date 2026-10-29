use std::time::Duration;

use crate::backoff::BackoffPolicy;

/// A receiver can ask for a long pause, but holding a delivery for a day is worse
/// than retrying on our own schedule.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(3_600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusClass {
    Success,
    Retryable { retry_after: Option<Duration> },
    Permanent,
}

pub fn classify_status(status: u16) -> StatusClass {
    match status {
        200..=299 => StatusClass::Success,
        408 | 425 | 429 => StatusClass::Retryable { retry_after: None },
        500..=599 => StatusClass::Retryable { retry_after: None },
        _ => StatusClass::Permanent,
    }
}

/// Returns the delay before the next attempt, or `None` when attempts are exhausted.
/// `attempt` is 1-based: the attempt that just failed.
pub fn next_delay(
    attempt: u8,
    retry_after: Option<Duration>,
    policy: &BackoffPolicy,
    sample: f32,
) -> Option<Duration> {
    if !policy.should_retry(attempt) {
        return None;
    }

    Some(match retry_after {
        Some(delay) => delay.min(MAX_RETRY_AFTER),
        None => policy.jitter(policy.delay(attempt.saturating_sub(1)), sample),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: Duration, expected: Duration) {
        let delta = actual.abs_diff(expected);

        assert!(
            delta <= Duration::from_millis(1),
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn classifies_success() {
        for status in [200, 201, 204, 299] {
            assert_eq!(classify_status(status), StatusClass::Success);
        }
    }

    #[test]
    fn classifies_retryable() {
        for status in [408, 425, 429, 500, 502, 503, 504] {
            assert!(matches!(
                classify_status(status),
                StatusClass::Retryable { .. }
            ));
        }
    }

    #[test]
    fn classifies_permanent() {
        for status in [301, 302, 400, 401, 403, 404, 410, 422] {
            assert_eq!(classify_status(status), StatusClass::Permanent);
        }
    }

    #[test]
    fn honors_retry_after_within_cap() {
        let policy = BackoffPolicy::default();
        let delay = next_delay(1, Some(Duration::from_secs(120)), &policy, 0.5).unwrap();

        assert_eq!(delay, Duration::from_secs(120));
    }

    #[test]
    fn caps_absurd_retry_after() {
        let policy = BackoffPolicy::default();
        let delay = next_delay(1, Some(Duration::from_secs(86_400)), &policy, 0.5).unwrap();

        assert_eq!(delay, MAX_RETRY_AFTER);
    }

    #[test]
    fn falls_back_to_jittered_backoff() {
        let policy = BackoffPolicy::default();

        assert_close(
            next_delay(1, None, &policy, 0.5).unwrap(),
            Duration::from_secs(5),
        );
        assert_close(
            next_delay(3, None, &policy, 0.5).unwrap(),
            Duration::from_secs(20),
        );
    }

    #[test]
    fn stops_when_attempts_exhausted() {
        let policy = BackoffPolicy::default();

        assert_eq!(next_delay(8, None, &policy, 0.5), None);
    }
}
