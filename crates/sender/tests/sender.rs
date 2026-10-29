use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{HeaderValue, LOCATION, RETRY_AFTER};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use hookrelay_sender::{SendOutcome, SendReport, SendRequest, Sender, SenderConfig};
use tokio::sync::Mutex;
use uuid::Uuid;

const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";
const BODY: &[u8] = br#"{"type":"invoice.paid","id":"in_1"}"#;

#[derive(Clone)]
struct MockConfig {
    status: StatusCode,
    delay: Duration,
    body: Vec<u8>,
    retry_after: Option<String>,
    location: Option<String>,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            status: StatusCode::OK,
            delay: Duration::ZERO,
            body: Vec::new(),
            retry_after: None,
            location: None,
        }
    }
}

#[derive(Clone)]
struct Recorded {
    path: String,
    headers: HeaderMap,
}

#[derive(Clone)]
struct AppState {
    config: Arc<MockConfig>,
    seen: Arc<Mutex<Vec<Recorded>>>,
}

async fn handle(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
    _body: Bytes,
) -> Response {
    state.seen.lock().await.push(Recorded {
        path: uri.path().to_owned(),
        headers: headers.clone(),
    });

    if !state.config.delay.is_zero() {
        tokio::time::sleep(state.config.delay).await;
    }

    if let Some(location) = &state.config.location {
        return (
            StatusCode::FOUND,
            [(LOCATION, HeaderValue::from_str(location).unwrap())],
        )
            .into_response();
    }

    let mut response = (state.config.status, state.config.body.clone()).into_response();

    if let Some(value) = &state.config.retry_after {
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from_str(value).unwrap());
    }

    response
}

async fn spawn(config: MockConfig) -> (String, Arc<Mutex<Vec<Recorded>>>) {
    let state = AppState {
        config: Arc::new(config),
        seen: Arc::default(),
    };
    let app = Router::new()
        .route("/hook", post(handle))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{addr}/hook"), state.seen)
}

fn sender(allow_private_targets: bool, request_timeout: Duration, response_cap: usize) -> Sender {
    Sender::new(SenderConfig {
        allow_private_targets,
        request_timeout,
        connect_timeout: request_timeout,
        response_cap,
        ..SenderConfig::default()
    })
}

async fn send(sender: &Sender, url: &str) -> SendReport {
    let delivery_id = Uuid::now_v7();

    sender
        .send(SendRequest {
            url,
            secret: SECRET,
            delivery_id,
            event_id: Uuid::now_v7(),
            attempt: 1,
            body: BODY,
        })
        .await
}

#[tokio::test]
async fn signs_request_and_sends_metadata_headers() {
    let (url, seen) = spawn(MockConfig::default()).await;
    let sender = sender(true, Duration::from_secs(2), 1024);

    let report = send(&sender, &url).await;

    assert!(matches!(
        report.outcome,
        SendOutcome::Delivered { status: 200, .. }
    ));

    let seen = seen.lock().await;
    let headers = &seen[0].headers;
    let signature = headers
        .get("x-hookrelay-signature")
        .unwrap()
        .to_str()
        .unwrap();

    hookrelay_core::signature::verify(SECRET, signature, BODY, chrono::Utc::now()).unwrap();
    assert_eq!(
        headers
            .get("x-hookrelay-attempt")
            .unwrap()
            .to_str()
            .unwrap(),
        "1"
    );
    assert!(headers.get("x-hookrelay-id").is_some());
    assert!(headers.get("x-hookrelay-event").is_some());
    assert!(headers.get("idempotency-key").is_some());
}

#[tokio::test]
async fn maps_500_to_retryable() {
    let (url, _) = spawn(MockConfig {
        status: StatusCode::SERVICE_UNAVAILABLE,
        ..MockConfig::default()
    })
    .await;

    let report = send(&sender(true, Duration::from_secs(2), 1024), &url).await;

    assert!(matches!(
        report.outcome,
        SendOutcome::Retryable {
            status: Some(503),
            retry_after: None,
            ..
        }
    ));
}

#[tokio::test]
async fn maps_404_to_permanent() {
    let (url, _) = spawn(MockConfig {
        status: StatusCode::NOT_FOUND,
        ..MockConfig::default()
    })
    .await;

    let report = send(&sender(true, Duration::from_secs(2), 1024), &url).await;

    assert!(matches!(
        report.outcome,
        SendOutcome::Permanent {
            status: Some(404),
            ..
        }
    ));
}

#[tokio::test]
async fn parses_retry_after_seconds() {
    let (url, _) = spawn(MockConfig {
        status: StatusCode::TOO_MANY_REQUESTS,
        retry_after: Some("120".to_owned()),
        ..MockConfig::default()
    })
    .await;

    let report = send(&sender(true, Duration::from_secs(2), 1024), &url).await;

    assert!(matches!(
        report.outcome,
        SendOutcome::Retryable {
            status: Some(429),
            retry_after: Some(delay),
            ..
        } if delay == Duration::from_secs(120)
    ));
}

#[tokio::test]
async fn caps_response_excerpt_at_configured_limit() {
    let (url, _) = spawn(MockConfig {
        body: vec![b'a'; 200 * 1024],
        ..MockConfig::default()
    })
    .await;

    let report = send(&sender(true, Duration::from_secs(5), 1024), &url).await;

    match report.outcome {
        SendOutcome::Delivered { excerpt, .. } => assert!(excerpt.len() <= 1024),
        other => panic!("expected delivered, got {other:?}"),
    }
}

#[tokio::test]
async fn times_out_slow_receiver() {
    let (url, _) = spawn(MockConfig {
        delay: Duration::from_secs(2),
        ..MockConfig::default()
    })
    .await;

    let started = Instant::now();
    let report = send(&sender(true, Duration::from_millis(100), 1024), &url).await;

    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(matches!(
        report.outcome,
        SendOutcome::Retryable {
            error,
            status: None,
            ..
        } if error.contains("timed out")
    ));
}

#[tokio::test]
async fn refuses_private_target_when_not_allowed() {
    let (url, seen) = spawn(MockConfig::default()).await;
    let https_url = url.replace("http://", "https://");

    let report = send(&sender(false, Duration::from_secs(2), 1024), &https_url).await;

    assert!(matches!(
        report.outcome,
        SendOutcome::Permanent {
            error,
            status: None,
        } if error.contains("blocked address")
    ));
    assert!(seen.lock().await.is_empty());
}

#[tokio::test]
async fn does_not_follow_redirects() {
    let (url, seen) = spawn(MockConfig {
        location: Some("/elsewhere".to_owned()),
        ..MockConfig::default()
    })
    .await;

    let report = send(&sender(true, Duration::from_secs(2), 1024), &url).await;

    assert!(matches!(
        report.outcome,
        SendOutcome::Permanent {
            status: Some(302),
            ..
        }
    ));

    let seen = seen.lock().await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/hook");
}
