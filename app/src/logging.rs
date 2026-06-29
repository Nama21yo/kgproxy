use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgPoolOptions, PgPool};
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq)]
pub struct RequestLogEvent {
    pub observed_at_unix_secs: u64,
    pub route: &'static str,
    pub query_hash: String,
    pub cache_hit: bool,
    pub stale: bool,
    pub latency_ms: u128,
    pub client_hash: String,
    pub status_code: u16,
    pub metadata: Value,
}

impl RequestLogEvent {
    pub fn new(
        route: &'static str,
        query_hash: String,
        cache_hit: bool,
        stale: bool,
        latency_ms: u128,
        client_identifier: &str,
        status_code: u16,
    ) -> Self {
        Self {
            observed_at_unix_secs: now_unix_secs(),
            route,
            query_hash,
            cache_hit,
            stale,
            latency_ms,
            client_hash: hash_client_identifier(client_identifier),
            status_code,
            metadata: json!({}),
        }
    }
}

#[derive(Debug, Error)]
pub enum LoggingError {
    #[error("postgres error: {0}")]
    Sqlx(#[from] sqlx::Error),
}

#[async_trait]
pub trait RequestLogger: Send + Sync {
    async fn log(&self, event: RequestLogEvent);
}

#[derive(Debug, Default)]
pub struct NoopLogger;

#[async_trait]
impl RequestLogger for NoopLogger {
    async fn log(&self, _event: RequestLogEvent) {}
}

#[derive(Debug, Clone)]
pub struct ChannelLogger {
    sender: mpsc::Sender<RequestLogEvent>,
}

impl ChannelLogger {
    pub fn spawn(pool: PgPool, capacity: usize) -> Self {
        let (sender, mut receiver) = mpsc::channel(capacity);

        tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                if let Err(error) = insert_request_log(&pool, event).await {
                    tracing::warn!(%error, "failed to write request log");
                }
            }
        });

        Self { sender }
    }
}

#[async_trait]
impl RequestLogger for ChannelLogger {
    async fn log(&self, event: RequestLogEvent) {
        let _ = self.sender.try_send(event);
    }
}

pub fn postgres_pool(database_url: &str) -> Result<PgPool, LoggingError> {
    Ok(PgPoolOptions::new()
        .max_connections(5)
        .connect_lazy(database_url)?)
}

pub async fn insert_request_log(pool: &PgPool, event: RequestLogEvent) -> Result<(), LoggingError> {
    sqlx::query(
        r#"
        INSERT INTO request_logs (
            observed_at_unix_secs,
            route,
            query_hash,
            cache_hit,
            stale,
            latency_ms,
            client_hash,
            status_code,
            metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(event.observed_at_unix_secs as i64)
    .bind(event.route)
    .bind(event.query_hash)
    .bind(event.cache_hit)
    .bind(event.stale)
    .bind(i32::try_from(event.latency_ms).unwrap_or(i32::MAX))
    .bind(event.client_hash)
    .bind(i16::try_from(event.status_code).unwrap_or(i16::MAX))
    .bind(event.metadata)
    .execute(pool)
    .await?;

    Ok(())
}

pub fn hash_client_identifier(identifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(identifier.as_bytes());
    hex::encode(hasher.finalize())
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_log_event_contains_required_fields() {
        let event = RequestLogEvent::new(
            "entity",
            "query-hash".to_owned(),
            true,
            false,
            15,
            "127.0.0.1",
            200,
        );

        assert!(event.observed_at_unix_secs > 0);
        assert_eq!(event.route, "entity");
        assert_eq!(event.query_hash, "query-hash");
        assert!(event.cache_hit);
        assert!(!event.stale);
        assert_eq!(event.latency_ms, 15);
        assert_eq!(event.status_code, 200);
        assert_eq!(event.client_hash.len(), 64);
    }

    #[test]
    fn client_identifier_hash_is_stable_and_not_plaintext() {
        let first = hash_client_identifier("127.0.0.1");
        let second = hash_client_identifier("127.0.0.1");

        assert_eq!(first, second);
        assert_ne!(first, "127.0.0.1");
    }
}
