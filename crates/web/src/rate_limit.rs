use std::time::{Duration, SystemTime, UNIX_EPOCH};

use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Script};

#[derive(Clone)]
pub struct RateLimiter {
    redis: Option<ConnectionManager>,
    capacity: u32,
    refill_per_sec: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateDecision {
    Allowed { remaining: u32 },
    Denied { retry_after: Duration },
}

impl RateLimiter {
    pub fn disabled() -> Self {
        Self {
            redis: None,
            capacity: 0,
            refill_per_sec: 0.0,
        }
    }

    pub fn new(redis: ConnectionManager, capacity: u32, refill_per_sec: f64) -> Self {
        Self {
            redis: Some(redis),
            capacity,
            refill_per_sec,
        }
    }

    pub async fn check(&self, key: &str) -> Result<RateDecision, redis::RedisError> {
        let Some(redis) = &self.redis else {
            return Ok(RateDecision::Allowed { remaining: 0 });
        };

        let state = self.state_key(key);
        let ttl = (self.capacity as f64 / self.refill_per_sec).ceil() as u64 + 60;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let mut connection = redis.clone();
        let (allowed, remaining): (i64, i64) = bucket_script()
            .key(state)
            .arg(self.capacity)
            .arg(self.refill_per_sec)
            .arg(now)
            .arg(1)
            .arg(ttl)
            .invoke_async(&mut connection)
            .await?;

        if allowed == 1 {
            return Ok(RateDecision::Allowed {
                remaining: remaining.max(0) as u32,
            });
        }

        let shortfall = (1.0 - remaining as f64).max(0.0);
        let seconds = (shortfall / self.refill_per_sec).ceil().max(1.0);

        Ok(RateDecision::Denied {
            retry_after: Duration::from_secs(seconds as u64),
        })
    }

    /// Resets a bucket. Used by tests and by the admin path when a key is rotated.
    pub async fn reset(&self, key: &str) -> Result<(), redis::RedisError> {
        let Some(redis) = &self.redis else {
            return Ok(());
        };

        let mut connection = redis.clone();
        connection.del(self.state_key(key)).await
    }

    fn state_key(&self, key: &str) -> String {
        format!("rl:key:{key}")
    }
}

fn bucket_script() -> Script {
    Script::new(
        r#"
local key = KEYS[1]
local capacity = tonumber(ARGV[1])
local refill = tonumber(ARGV[2])
local now = tonumber(ARGV[3])
local cost = tonumber(ARGV[4])
local ttl = tonumber(ARGV[5])

local state = redis.call('HMGET', key, 'tokens', 'ts')
local tokens = tonumber(state[1])
local ts = tonumber(state[2])

if tokens == nil then
  tokens = capacity
  ts = now
end

local elapsed = math.max(0, now - ts)
tokens = math.min(capacity, tokens + elapsed * refill)
ts = now

if tokens < cost then
  redis.call('HMSET', key, 'tokens', tokens, 'ts', ts)
  redis.call('EXPIRE', key, ttl)
  return {0, 0}
end

tokens = tokens - cost
redis.call('HMSET', key, 'tokens', tokens, 'ts', ts)
redis.call('EXPIRE', key, ttl)
return {1, math.floor(tokens)}
"#,
    )
}
