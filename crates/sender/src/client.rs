use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hookrelay_core::outcome::{StatusClass, classify_status};
use hookrelay_core::ssrf::is_blocked_ip;
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, Url};
use tokio::net::lookup_host;
use tokio::sync::Mutex;

use crate::{SendOutcome, SendReport, SendRequest, SenderError, headers};

#[derive(Debug, Clone)]
pub struct SenderConfig {
    pub allow_private_targets: bool,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub response_cap: usize,
    pub cache_ttl: Duration,
    pub cache_capacity: usize,
}

impl Default for SenderConfig {
    fn default() -> Self {
        Self {
            allow_private_targets: false,
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(10),
            response_cap: 64 * 1024,
            cache_ttl: Duration::from_secs(60),
            cache_capacity: 256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    host: String,
    port: u16,
    ip: IpAddr,
}

struct CachedClient {
    client: Client,
    stored_at: Instant,
}

struct Prepared {
    url: Url,
    host: String,
    addr: SocketAddr,
}

#[derive(Clone)]
pub struct Sender {
    config: SenderConfig,
    cache: Arc<Mutex<HashMap<CacheKey, CachedClient>>>,
}

impl Sender {
    pub fn new(config: SenderConfig) -> Self {
        Self {
            config,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn send(&self, request: SendRequest<'_>) -> SendReport {
        let started = Instant::now();

        let prepared = match self.prepare(request.url).await {
            Ok(prepared) => prepared,
            Err(error) => {
                return permanent(error.to_string(), started);
            }
        };

        let signed_headers = match headers::build(&request) {
            Ok(headers) => headers,
            Err(error) => {
                return permanent(error.to_string(), started);
            }
        };

        let client = match self.client_for(&prepared).await {
            Ok(client) => client,
            Err(error) => {
                return permanent(error.to_string(), started);
            }
        };

        let response = client
            .post(prepared.url)
            .headers(signed_headers)
            .body(request.body.to_vec())
            .send()
            .await;

        match response {
            Ok(response) => {
                let status = response.status().as_u16();
                let retry_after = response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_retry_after);
                let excerpt = read_excerpt(response, self.config.response_cap).await;
                let latency = started.elapsed();

                let outcome = match classify_status(status) {
                    StatusClass::Success => SendOutcome::Delivered { status, excerpt },
                    StatusClass::Retryable { .. } => SendOutcome::Retryable {
                        status: Some(status),
                        error: format!("receiver returned {status}"),
                        retry_after,
                    },
                    StatusClass::Permanent => SendOutcome::Permanent {
                        status: Some(status),
                        error: format!("receiver returned {status}"),
                    },
                };

                SendReport { outcome, latency }
            }
            Err(error) => SendReport {
                outcome: SendOutcome::Retryable {
                    status: None,
                    error: describe(&error),
                    retry_after: None,
                },
                latency: started.elapsed(),
            },
        }
    }

    async fn prepare(&self, raw_url: &str) -> Result<Prepared, SenderError> {
        let url =
            Url::parse(raw_url).map_err(|error| SenderError::InvalidUrl(error.to_string()))?;

        match url.scheme() {
            "https" => {}
            "http" if self.config.allow_private_targets => {}
            other => {
                return Err(SenderError::InvalidUrl(format!(
                    "unsupported scheme `{other}`"
                )));
            }
        }

        let host = url
            .host_str()
            .ok_or_else(|| SenderError::InvalidUrl("missing host".to_owned()))?
            .to_owned();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| SenderError::InvalidUrl("missing port".to_owned()))?;

        let addresses: Vec<SocketAddr> = lookup_host((host.as_str(), port))
            .await
            .map_err(|error| SenderError::InvalidUrl(format!("resolve failed: {error}")))?
            .collect();

        let Some(first) = addresses.first().copied() else {
            return Err(SenderError::NoAddress(host));
        };

        // Checking every answer, not just the chosen one, closes the
        // mixed-answer DNS trick.
        if !self.config.allow_private_targets
            && let Some(blocked) = addresses.iter().find(|addr| is_blocked_ip(addr.ip()))
        {
            return Err(SenderError::BlockedAddress(blocked.ip().to_string()));
        }

        Ok(Prepared {
            url,
            host,
            addr: first,
        })
    }

    async fn client_for(&self, prepared: &Prepared) -> Result<Client, SenderError> {
        let key = CacheKey {
            host: prepared.host.clone(),
            port: prepared.addr.port(),
            ip: prepared.addr.ip(),
        };
        let mut cache = self.cache.lock().await;

        if let Some(entry) = cache.get(&key)
            && entry.stored_at.elapsed() < self.config.cache_ttl
        {
            return Ok(entry.client.clone());
        }

        cache.remove(&key);

        let client = Client::builder()
            .connect_timeout(self.config.connect_timeout)
            .timeout(self.config.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&prepared.host, prepared.addr)
            .build()?;

        if self.config.cache_capacity == 0 {
            return Ok(client);
        }

        cache.retain(|_, entry| entry.stored_at.elapsed() < self.config.cache_ttl);
        let oldest = (cache.len() >= self.config.cache_capacity)
            .then(|| {
                cache
                    .iter()
                    .min_by_key(|(_, entry)| entry.stored_at)
                    .map(|(key, _)| key.clone())
            })
            .flatten();
        if let Some(oldest) = oldest {
            cache.remove(&oldest);
        }

        cache.insert(
            key,
            CachedClient {
                client: client.clone(),
                stored_at: Instant::now(),
            },
        );

        Ok(client)
    }
}

fn permanent(error: String, started: Instant) -> SendReport {
    SendReport {
        outcome: SendOutcome::Permanent {
            status: None,
            error,
        },
        latency: started.elapsed(),
    }
}

fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "request timed out".to_owned()
    } else {
        format!("request failed: {error}")
    }
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

async fn read_excerpt(mut response: reqwest::Response, cap: usize) -> String {
    let mut buffer = Vec::new();

    loop {
        if buffer.len() >= cap {
            break;
        }

        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = cap - buffer.len();
                let take = chunk.len().min(remaining);
                buffer.extend_from_slice(&chunk[..take]);
            }
            Ok(None) | Err(_) => break,
        }
    }

    String::from_utf8_lossy(&buffer).into_owned()
}
