use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use hookrelay_core::backoff::BackoffPolicy;
use hookrelay_sender::{SendRequest, Sender};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::claim::claim;
use crate::result::{reap_expired, record_result, release_leases};

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub worker_id: String,
    pub batch_size: i64,
    pub lease: Duration,
    pub poll_interval: Duration,
    pub max_concurrency: usize,
    pub backoff: BackoffPolicy,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: "worker-1".to_owned(),
            batch_size: 20,
            lease: Duration::from_secs(30),
            poll_interval: Duration::from_millis(200),
            max_concurrency: 8,
            backoff: BackoffPolicy::default(),
        }
    }
}

#[derive(Clone)]
pub struct Worker {
    pool: sqlx::PgPool,
    sender: Sender,
    config: WorkerConfig,
}

impl Worker {
    pub fn new(pool: sqlx::PgPool, sender: Sender, config: WorkerConfig) -> Self {
        Self {
            pool,
            sender,
            config,
        }
    }

    pub async fn run(self, shutdown: CancellationToken) {
        let mut poll = tokio::time::interval(self.config.poll_interval);
        let mut reap = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = poll.tick() => {
                    match self.run_once().await {
                        Ok(0) => {}
                        Ok(count) => tracing::debug!(deliveries = count, "processed batch"),
                        Err(error) => tracing::error!(error = %error, "claim failed"),
                    }
                }
                _ = reap.tick() => {
                    match reap_expired(&self.pool).await {
                        Ok(0) => {}
                        Ok(count) => {
                            metrics::counter!("hookrelay_lease_expired_total").increment(count);
                            tracing::warn!(deliveries = count, "reaped expired leases");
                        }
                        Err(error) => tracing::error!(error = %error, "lease reaper failed"),
                    }
                }
            }
        }

        match release_leases(&self.pool, &self.config.worker_id).await {
            Ok(0) => {}
            Ok(count) => tracing::info!(deliveries = count, "released leases on shutdown"),
            Err(error) => tracing::error!(error = %error, "failed to release leases"),
        }
    }

    pub async fn run_once(&self) -> Result<usize, sqlx::Error> {
        let claim_started = std::time::Instant::now();
        let claimed = claim(
            &self.pool,
            &self.config.worker_id,
            self.config.batch_size,
            self.config.lease,
        )
        .await?;
        metrics::histogram!("hookrelay_claim_batch_seconds")
            .record(claim_started.elapsed().as_secs_f64());

        if let Ok(depth) = sqlx::query_scalar::<_, i64>(
            "select count(*) from deliveries where status in ('pending', 'retrying')",
        )
        .fetch_one(&self.pool)
        .await
        {
            metrics::gauge!("hookrelay_queue_depth").set(depth as f64);
        }

        let count = claimed.len();
        if count == 0 {
            return Ok(0);
        }

        let semaphore = Arc::new(Semaphore::new(self.config.max_concurrency.max(1)));
        let mut tasks = JoinSet::new();

        for item in claimed {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore is never closed");
            let sender = self.sender.clone();
            let pool = self.pool.clone();
            let backoff = self.config.backoff;

            tasks.spawn(async move {
                let _permit = permit;
                let started_at = Utc::now();
                let body =
                    serde_json::to_vec(&item.payload).expect("serde_json::Value is serializable");

                let report = sender
                    .send(SendRequest {
                        url: &item.url,
                        secret: &item.secret_current,
                        delivery_id: item.delivery_id,
                        event_id: item.event_id,
                        attempt: item.attempt,
                        body: &body,
                    })
                    .await;

                let sample = rand::random::<f32>();

                match record_result(&pool, &item, &report, &backoff, sample, started_at).await {
                    Ok(status) => {
                        tracing::debug!(delivery_id = %item.delivery_id, ?status, "attempt recorded");
                    }
                    Err(error) => {
                        tracing::error!(
                            delivery_id = %item.delivery_id,
                            error = %error,
                            "failed to record attempt"
                        );
                    }
                }
            });
        }

        while let Some(joined) = tasks.join_next().await {
            if let Err(error) = joined {
                tracing::error!(error = %error, "delivery task panicked");
            }
        }

        Ok(count)
    }
}
