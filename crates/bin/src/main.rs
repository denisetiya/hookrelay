mod config;

use anyhow::Context;
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

    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "serve".to_owned());

    match command.as_str() {
        "migrate" => {
            let pool = hookrelay_store::db::connect(&cfg.database_url).await?;
            hookrelay_store::db::migrate(&pool).await?;
            println!("migrations applied");
        }
        "serve" | "worker" => {
            anyhow::bail!("{command} is implemented in a later task");
        }
        other => {
            anyhow::bail!("unknown command `{other}`; expected migrate, serve, or worker");
        }
    }

    Ok(())
}

fn init_tracing(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);

    match format {
        LogFormat::Json => builder.json().init(),
        LogFormat::Pretty => builder.init(),
    }
}
