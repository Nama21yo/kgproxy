use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CachedEntry {
    pub payload: Value,
    pub created_at_unix_secs: u64,
    pub last_success_unix_secs: u64,
    pub hit_count: u64,
}

impl CachedEntry {
    pub fn fresh(payload: Value) -> Self {
        let now = now_unix_secs();

        Self {
            payload,
            created_at_unix_secs: now,
            last_success_unix_secs: now,
            hit_count: 0,
        }
    }

    pub fn record_hit(&mut self) {
        self.hit_count += 1;
    }
}

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("cache serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[async_trait]
pub trait ResponseCache: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<CachedEntry>, CacheError>;
    async fn get_stale(&self, key: &str) -> Result<Option<CachedEntry>, CacheError>;
    async fn set(&self, key: &str, entry: &CachedEntry, ttl: Duration) -> Result<(), CacheError>;
}

#[derive(Debug, Clone)]
pub struct RedisCache {
    client: redis::Client,
}

impl RedisCache {
    pub fn new(redis_url: &str) -> Result<Self, CacheError> {
        Ok(Self {
            client: redis::Client::open(redis_url)?,
        })
    }
}

#[async_trait]
impl ResponseCache for RedisCache {
    async fn get(&self, key: &str) -> Result<Option<CachedEntry>, CacheError> {
        let mut connection = self.client.get_multiplexed_async_connection().await?;
        let value: Option<String> = connection.get(key).await?;

        value
            .map(|serialized| serde_json::from_str(&serialized).map_err(CacheError::from))
            .transpose()
    }

    async fn get_stale(&self, key: &str) -> Result<Option<CachedEntry>, CacheError> {
        let mut connection = self.client.get_multiplexed_async_connection().await?;
        let value: Option<String> = connection.get(stale_key(key)).await?;

        value
            .map(|serialized| serde_json::from_str(&serialized).map_err(CacheError::from))
            .transpose()
    }

    async fn set(&self, key: &str, entry: &CachedEntry, ttl: Duration) -> Result<(), CacheError> {
        let mut connection = self.client.get_multiplexed_async_connection().await?;
        let serialized = serde_json::to_string(entry)?;
        let _: () = connection.set_ex(key, serialized, ttl.as_secs()).await?;
        let _: () = connection
            .set(stale_key(key), serde_json::to_string(entry)?)
            .await?;

        Ok(())
    }
}

fn stale_key(key: &str) -> String {
    format!("stale:{key}")
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
    use serde_json::json;

    #[test]
    fn fresh_entry_stores_payload_and_metadata() {
        let entry = CachedEntry::fresh(json!({ "id": "Albert_Einstein" }));

        assert_eq!(entry.payload["id"], "Albert_Einstein");
        assert_eq!(entry.hit_count, 0);
        assert!(entry.created_at_unix_secs > 0);
        assert_eq!(entry.created_at_unix_secs, entry.last_success_unix_secs);
    }

    #[test]
    fn hit_count_can_be_incremented() {
        let mut entry = CachedEntry::fresh(json!({ "ok": true }));

        entry.record_hit();
        entry.record_hit();

        assert_eq!(entry.hit_count, 2);
    }
}
