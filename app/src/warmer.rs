use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::Value;
use sqlx::{PgPool, Row};
use thiserror::Error;
use tokio::{task::JoinHandle, time::MissedTickBehavior};

use crate::{
    cache::{CachedEntry, ResponseCache},
    cache_key,
    origin::{DbpediaClient, OriginError},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopularEntity {
    pub id: String,
    pub cache_key: String,
    pub hits: i64,
    pub lang: String,
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarmResult {
    pub refreshed: usize,
    pub failed: usize,
}

#[derive(Debug, Error)]
pub enum WarmerError {
    #[error("postgres error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("origin error: {0}")]
    Origin(#[from] OriginError),
    #[error("cache error: {0}")]
    Cache(#[from] crate::cache::CacheError),
}

#[async_trait]
pub trait PopularEntitySource: Send + Sync {
    async fn top_entities(&self, limit: i64) -> Result<Vec<PopularEntity>, WarmerError>;
}

#[derive(Debug, Clone)]
pub struct PostgresPopularEntitySource {
    pool: PgPool,
}

impl PostgresPopularEntitySource {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PopularEntitySource for PostgresPopularEntitySource {
    async fn top_entities(&self, limit: i64) -> Result<Vec<PopularEntity>, WarmerError> {
        let rows = sqlx::query(
            r#"
            SELECT
                metadata->>'entity_id' AS entity_id,
                query_hash,
                COUNT(*) AS hits,
                COALESCE(metadata->>'lang', 'en') AS lang,
                metadata->>'endpoint' AS endpoint
            FROM request_logs
            WHERE route = 'entity'
              AND metadata ? 'entity_id'
            GROUP BY metadata->>'entity_id', query_hash
            ORDER BY hits DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| PopularEntity {
                id: row.get("entity_id"),
                cache_key: row.get("query_hash"),
                hits: row.get("hits"),
                lang: row.get("lang"),
                endpoint: row.get("endpoint"),
            })
            .collect())
    }
}

pub struct CacheWarmer {
    source: Arc<dyn PopularEntitySource>,
    origin: Arc<dyn DbpediaClient>,
    cache: Arc<dyn ResponseCache>,
    ttl: Duration,
}

impl CacheWarmer {
    pub fn new(
        source: Arc<dyn PopularEntitySource>,
        origin: Arc<dyn DbpediaClient>,
        cache: Arc<dyn ResponseCache>,
        ttl: Duration,
    ) -> Self {
        Self {
            source,
            origin,
            cache,
            ttl,
        }
    }

    pub async fn warm_top_entities(&self, limit: i64) -> Result<WarmResult, WarmerError> {
        let entities = self.source.top_entities(limit).await?;
        let mut result = WarmResult {
            refreshed: 0,
            failed: 0,
        };

        for entity in entities {
            match self.refresh_entity(&entity).await {
                Ok(()) => result.refreshed += 1,
                Err(error) => {
                    result.failed += 1;
                    tracing::warn!(%error, entity_id = %entity.id, "cache warmer refresh failed");
                }
            }
        }

        Ok(result)
    }

    async fn refresh_entity(&self, entity: &PopularEntity) -> Result<(), WarmerError> {
        let payload = self
            .origin
            .entity(&entity.id, entity.endpoint.as_deref(), &entity.lang)
            .await?;
        let entry = CachedEntry::fresh(payload);
        let key = if entity.cache_key.is_empty() {
            cache_key::entity_key(&entity.id, &[("lang", "en")])
        } else {
            entity.cache_key.clone()
        };

        self.cache.set(&key, &entry, self.ttl).await?;
        Ok(())
    }
}

pub fn spawn_cache_warmer(
    warmer: Arc<CacheWarmer>,
    interval: Duration,
    top_k: i64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        run_cache_warmer_loop(warmer, interval, top_k).await;
    })
}

pub async fn run_cache_warmer_loop(warmer: Arc<CacheWarmer>, interval: Duration, top_k: i64) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        match warmer.warm_top_entities(top_k).await {
            Ok(result) => tracing::info!(
                refreshed = result.refreshed,
                failed = result.failed,
                top_k,
                "cache warmer completed"
            ),
            Err(error) => tracing::warn!(%error, top_k, "cache warmer run failed"),
        }
    }
}

#[allow(dead_code)]
fn _payload_type_marker(_: Value) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::OriginError;
    use serde_json::{Value, json};
    use std::{
        collections::HashMap,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    #[derive(Debug)]
    struct StaticPopularSource {
        entities: Vec<PopularEntity>,
    }

    #[async_trait]
    impl PopularEntitySource for StaticPopularSource {
        async fn top_entities(&self, limit: i64) -> Result<Vec<PopularEntity>, WarmerError> {
            Ok(self.entities.iter().take(limit as usize).cloned().collect())
        }
    }

    #[derive(Debug, Default)]
    struct MockOrigin {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl DbpediaClient for MockOrigin {
        async fn entity(
            &self,
            id: &str,
            _endpoint_override: Option<&str>,
            _lang: &str,
        ) -> Result<Value, OriginError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(json!({ "id": id, "refreshed": true }))
        }

        async fn search(
            &self,
            _query: &str,
            _endpoint_override: Option<&str>,
            _lang: &str,
        ) -> Result<Value, OriginError> {
            unreachable!("cache warmer should not search")
        }

        async fn sparql(
            &self,
            _query: &str,
            _endpoint_override: Option<&str>,
        ) -> Result<Value, OriginError> {
            unreachable!("cache warmer should not call raw sparql")
        }
    }

    #[derive(Debug, Default)]
    struct MemoryCache {
        entries: Mutex<HashMap<String, CachedEntry>>,
    }

    #[async_trait]
    impl ResponseCache for MemoryCache {
        async fn get(&self, key: &str) -> Result<Option<CachedEntry>, crate::cache::CacheError> {
            Ok(self.entries.lock().unwrap().get(key).cloned())
        }

        async fn get_stale(
            &self,
            key: &str,
        ) -> Result<Option<CachedEntry>, crate::cache::CacheError> {
            self.get(key).await
        }

        async fn set(
            &self,
            key: &str,
            entry: &CachedEntry,
            _ttl: Duration,
        ) -> Result<(), crate::cache::CacheError> {
            self.entries
                .lock()
                .unwrap()
                .insert(key.to_owned(), entry.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn warmer_refreshes_top_entities_into_cache() {
        let key = cache_key::entity_key("Albert_Einstein", &[("lang", "en")]);
        let source = Arc::new(StaticPopularSource {
            entities: vec![PopularEntity {
                id: "Albert_Einstein".to_owned(),
                cache_key: key.clone(),
                hits: 7,
                lang: "en".to_owned(),
                endpoint: None,
            }],
        });
        let origin = Arc::new(MockOrigin::default());
        let cache = Arc::new(MemoryCache::default());
        let warmer = CacheWarmer::new(
            source,
            origin.clone(),
            cache.clone(),
            Duration::from_secs(604_800),
        );

        let result = warmer.warm_top_entities(10).await.unwrap();

        assert_eq!(result.refreshed, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(origin.calls.load(Ordering::SeqCst), 1);

        let entry = cache.get(&key).await.unwrap().unwrap();
        assert_eq!(entry.payload["id"], "Albert_Einstein");
        assert_eq!(entry.payload["refreshed"], true);
    }

    #[tokio::test]
    async fn warmer_honors_limit() {
        let source = Arc::new(StaticPopularSource {
            entities: vec![
                PopularEntity {
                    id: "First".to_owned(),
                    cache_key: cache_key::entity_key("First", &[("lang", "en")]),
                    hits: 9,
                    lang: "en".to_owned(),
                    endpoint: None,
                },
                PopularEntity {
                    id: "Second".to_owned(),
                    cache_key: cache_key::entity_key("Second", &[("lang", "en")]),
                    hits: 8,
                    lang: "en".to_owned(),
                    endpoint: None,
                },
            ],
        });
        let origin = Arc::new(MockOrigin::default());
        let cache = Arc::new(MemoryCache::default());
        let warmer = CacheWarmer::new(source, origin.clone(), cache, Duration::from_secs(60));

        let result = warmer.warm_top_entities(1).await.unwrap();

        assert_eq!(result.refreshed, 1);
        assert_eq!(origin.calls.load(Ordering::SeqCst), 1);
    }
}
