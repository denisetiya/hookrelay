mod client;
mod headers;

use std::time::Duration;

pub use client::{Sender, SenderConfig};

#[derive(Debug, thiserror::Error)]
pub enum SenderError {
    #[error("invalid target url: {0}")]
    InvalidUrl(String),
    #[error("blocked address: {0}")]
    BlockedAddress(String),
    #[error("target host did not resolve: {0}")]
    NoAddress(String),
    #[error("invalid request header: {0}")]
    InvalidHeader(String),
    #[error("http client error: {0}")]
    Client(#[from] reqwest::Error),
}

pub struct SendRequest<'a> {
    pub url: &'a str,
    pub secret: &'a [u8],
    pub delivery_id: uuid::Uuid,
    pub event_id: uuid::Uuid,
    pub attempt: i16,
    pub body: &'a [u8],
}

#[derive(Debug)]
pub struct SendReport {
    pub outcome: SendOutcome,
    pub latency: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Delivered {
        status: u16,
        excerpt: String,
    },
    Retryable {
        status: Option<u16>,
        error: String,
        retry_after: Option<Duration>,
    },
    Permanent {
        status: Option<u16>,
        error: String,
    },
}
