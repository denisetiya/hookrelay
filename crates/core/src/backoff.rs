use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct BackoffPolicy {
    pub base: Duration,
    pub factor: u32,
    pub max: Duration,
    pub max_attempts: u8,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(5),
            factor: 2,
            max: Duration::from_secs(3_600),
            max_attempts: 8,
        }
    }
}

impl BackoffPolicy {
    /// Delay before the next attempt. `attempt` is zero-based: 0 is the first retry.
    pub fn delay(&self, attempt: u8) -> Duration {
        let exponent = u32::from(attempt).min(32);
        let millis = self
            .base
            .as_millis()
            .saturating_mul(u128::from(self.factor).saturating_pow(exponent));

        Duration::from_millis(millis.min(self.max.as_millis()) as u64)
    }

    /// Spreads retries so a failing receiver is not hammered by every worker at once.
    /// `sample` is a uniform value in `[0, 1)`; pass a fresh one per attempt.
    pub fn jitter(&self, delay: Duration, sample: f32) -> Duration {
        let multiplier = 0.8 + 0.4 * sample.clamp(0.0, 1.0);

        delay.mul_f32(multiplier)
    }

    pub fn should_retry(&self, attempt: u8) -> bool {
        attempt < self.max_attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_exponentially_from_base() {
        let policy = BackoffPolicy::default();

        assert_eq!(policy.delay(0), Duration::from_secs(5));
        assert_eq!(policy.delay(1), Duration::from_secs(10));
        assert_eq!(policy.delay(3), Duration::from_secs(40));
        assert_eq!(policy.delay(6), Duration::from_secs(320));
    }

    #[test]
    fn caps_at_max() {
        let policy = BackoffPolicy::default();

        assert_eq!(policy.delay(20), Duration::from_secs(3_600));
        assert_eq!(policy.delay(u8::MAX), Duration::from_secs(3_600));
    }

    fn assert_close(actual: Duration, expected: Duration) {
        let delta = actual.abs_diff(expected);

        assert!(
            delta <= Duration::from_millis(1),
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn jitter_stays_within_bounds() {
        let policy = BackoffPolicy::default();
        let delay = Duration::from_secs(10);

        assert_close(policy.jitter(delay, 0.0), Duration::from_secs(8));
        assert_close(policy.jitter(delay, 0.5), Duration::from_secs(10));
        assert_close(policy.jitter(delay, 1.0), Duration::from_secs(12));
    }

    #[test]
    fn jitter_clamps_out_of_range_samples() {
        let policy = BackoffPolicy::default();
        let delay = Duration::from_secs(10);

        assert_close(policy.jitter(delay, -1.0), Duration::from_secs(8));
        assert_close(policy.jitter(delay, 2.0), Duration::from_secs(12));
    }

    #[test]
    fn retries_stop_at_max_attempts() {
        let policy = BackoffPolicy::default();

        assert!(policy.should_retry(0));
        assert!(policy.should_retry(7));
        assert!(!policy.should_retry(8));
    }
}
