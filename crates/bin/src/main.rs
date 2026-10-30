mod config;

use anyhow::Context;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHasher};
use hookrelay_sender::{Sender, SenderConfig};
use hookrelay_worker::{Worker, WorkerConfig};
use rand::distributions::Alphanumeric;
use rand::{Rng, RngCore};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use crate::config::{Config, LogFormat};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let cfg = Config::from_env().context("loading configuration")?;
    init_tracing(cfg.log_format);
    tracing::debug!(
        bind_addr = %cfg.bind_addr,
        log_format = ?cfg.log_format,
        allow_private_targets = cfg.allow_private_targets,
        "configuration loaded"
    );

    let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .context("installing metrics recorder")?;

    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "serve".to_owned());

    match command.as_str() {
        "migrate" => migrate(&cfg).await?,
        "bootstrap" => bootstrap(&cfg).await?,
        "serve" => serve(&cfg, metrics_handle).await?,
        "worker" => run_worker(&cfg, metrics_handle).await?,
        other => {
            anyhow::bail!(
                "unknown command `{other}`; expected migrate, bootstrap, serve, or worker"
            );
        }
    }

    Ok(())
}

async fn migrate(cfg: &Config) -> anyhow::Result<()> {
    let pool = hookrelay_store::db::connect(&cfg.database_url).await?;
    hookrelay_store::db::migrate(&pool).await?;
    println!("migrations applied");

    Ok(())
}

async fn bootstrap(cfg: &Config) -> anyhow::Result<()> {
    let name = flag_value("--name").unwrap_or_else(|| "default".to_owned());
    let pool = hookrelay_store::db::connect(&cfg.database_url).await?;
    hookrelay_store::db::migrate(&pool).await?;

    let tenant = hookrelay_store::tenants::create(&pool, &name).await?;

    let mut rng = rand::thread_rng();
    let prefix: String = (&mut rng)
        .sample_iter(&Alphanumeric)
        .take(12)
        .map(char::from)
        .collect();

    let mut secret_bytes = [0_u8; 32];
    rng.fill_bytes(&mut secret_bytes);
    let secret = hex::encode(secret_bytes);

    let mut salt_bytes = [0_u8; 16];
    rng.fill_bytes(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|error| anyhow::anyhow!("{error}"))?;
    let hash = Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .to_string();

    hookrelay_store::api_keys::create(&pool, tenant.id, &prefix, &hash).await?;

    println!("tenant: {}", tenant.name);
    println!("api key: hr_live_{prefix}_{secret}");
    println!("store this key now; it cannot be recovered.");

    Ok(())
}

async fn serve(
    cfg: &Config,
    metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
) -> anyhow::Result<()> {
    let pool = hookrelay_store::db::connect(&cfg.database_url).await?;
    let limiter = build_limiter(&cfg.redis_url).await;
    let state = hookrelay_web::AppState {
        pool,
        allow_http_targets: cfg.allow_private_targets,
        limiter,
        session_secret: cfg.session_secret.as_bytes().to_vec(),
        metrics: Some(metrics_handle),
    };
    let app = hookrelay_web::router(state);
    let listener = tokio::net::TcpListener::bind(cfg.bind_addr).await?;

    tracing::info!(addr = %cfg.bind_addr, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn run_worker(
    cfg: &Config,
    metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
) -> anyhow::Result<()> {
    let pool = hookrelay_store::db::connect(&cfg.database_url).await?;
    let sender = Sender::new(SenderConfig {
        allow_private_targets: cfg.allow_private_targets,
        ..SenderConfig::default()
    });
    let worker_id = flag_value("--id").unwrap_or_else(|| format!("worker-{}", std::process::id()));
    let worker = Worker::new(
        pool,
        sender,
        WorkerConfig {
            worker_id,
            ..WorkerConfig::default()
        },
    );

    tokio::spawn(serve_metrics(metrics_handle, cfg.metrics_addr));

    let token = CancellationToken::new();
    let shutdown_token = token.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        shutdown_token.cancel();
    });

    worker.run(token).await;

    Ok(())
}

async fn serve_metrics(
    handle: metrics_exporter_prometheus::PrometheusHandle,
    addr: std::net::SocketAddr,
) {
    let app = axum::Router::new().route(
        "/metrics",
        axum::routing::get(move || {
            let handle = handle.clone();

            async move { handle.render() }
        }),
    );

    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => {
            tracing::info!(addr = %addr, "worker metrics listening");

            if let Err(error) = axum::serve(listener, app).await {
                tracing::error!(error = %error, "metrics server stopped");
            }
        }
        Err(error) => tracing::error!(addr = %addr, error = %error, "cannot bind metrics address"),
    }
}

async fn build_limiter(redis_url: &str) -> hookrelay_web::rate_limit::RateLimiter {
    let client = match redis::Client::open(redis_url) {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(error = %error, "invalid redis url; rate limiting disabled");
            return hookrelay_web::rate_limit::RateLimiter::disabled();
        }
    };

    match redis::aio::ConnectionManager::new(client).await {
        Ok(connection) => hookrelay_web::rate_limit::RateLimiter::new(connection, 120, 2.0),
        Err(error) => {
            tracing::warn!(error = %error, "redis unavailable; rate limiting disabled");
            hookrelay_web::rate_limit::RateLimiter::disabled()
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install ctrl_c handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install sigterm handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

fn flag_value(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let position = args.iter().position(|arg| arg == flag)?;

    args.get(position + 1).cloned()
}

fn init_tracing(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);

    match format {
        LogFormat::Json => builder.json().init(),
        LogFormat::Pretty => builder.init(),
    }
}
