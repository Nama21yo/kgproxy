use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::{
    cache::{CachedEntry, ResponseCache},
    cache_key,
    origin::{DbpediaClient, OriginError},
};

#[derive(Clone)]
pub struct AppState {
    origin: Arc<dyn DbpediaClient>,
    cache: Arc<dyn ResponseCache>,
    cache_ttl: Duration,
    outbound_limiter: Arc<Semaphore>,
    max_outbound_concurrency: usize,
}

impl AppState {
    pub fn new(
        origin: Arc<dyn DbpediaClient>,
        cache: Arc<dyn ResponseCache>,
        cache_ttl: Duration,
        max_outbound_concurrency: usize,
    ) -> Self {
        Self {
            origin,
            cache,
            cache_ttl,
            outbound_limiter: Arc::new(Semaphore::new(max_outbound_concurrency)),
            max_outbound_concurrency,
        }
    }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    outbound_available_permits: usize,
    max_outbound_concurrency: usize,
}

#[derive(Debug, Serialize)]
struct ApiEnvelope {
    data: Value,
    cached: bool,
    stale: bool,
    source: &'static str,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorEnvelope,
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    code: &'static str,
    message: String,
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    q: String,
}

#[derive(Debug, Deserialize)]
struct SparqlRequest {
    query: String,
}

#[derive(Debug, thiserror::Error)]
enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("outbound limiter is closed")]
    OutboundLimiterClosed,
    #[error(transparent)]
    Origin(#[from] OriginError),
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/entity/:id", get(entity))
        .route("/v1/search", get(search))
        .route("/v1/sparql", post(sparql))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "kgproxy",
        outbound_available_permits: state.outbound_limiter.available_permits(),
        max_outbound_concurrency: state.max_outbound_concurrency,
    })
}

async fn entity(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiEnvelope>, ApiError> {
    let id = validate_non_empty(id, "entity id")?;
    let cache_key = cache_key::entity_key(&id, &[("lang", "en")]);

    if let Some(envelope) = cached_envelope(&state, &cache_key).await {
        return Ok(Json(envelope));
    }

    let _permit = state
        .outbound_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::OutboundLimiterClosed)?;
    let data = state.origin.entity(&id).await?;
    store_fresh(&state, &cache_key, data.clone()).await;

    Ok(Json(origin_envelope(data)))
}

async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<ApiEnvelope>, ApiError> {
    let query = validate_non_empty(params.q, "search query")?;
    let cache_key = cache_key::search_key(&query, &[("lang", "en")]);

    if let Some(envelope) = cached_envelope(&state, &cache_key).await {
        return Ok(Json(envelope));
    }

    let _permit = state
        .outbound_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::OutboundLimiterClosed)?;
    let data = state.origin.search(&query).await?;
    store_fresh(&state, &cache_key, data.clone()).await;

    Ok(Json(origin_envelope(data)))
}

async fn sparql(
    State(state): State<AppState>,
    Json(body): Json<SparqlRequest>,
) -> Result<Json<ApiEnvelope>, ApiError> {
    let query = validate_non_empty(body.query, "sparql query")?;
    let cache_key = cache_key::sparql_key(&query);

    if let Some(envelope) = cached_envelope(&state, &cache_key).await {
        return Ok(Json(envelope));
    }

    let _permit = state
        .outbound_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::OutboundLimiterClosed)?;
    let data = state.origin.sparql(&query).await?;
    store_fresh(&state, &cache_key, data.clone()).await;

    Ok(Json(origin_envelope(data)))
}

fn validate_non_empty(value: String, field: &'static str) -> Result<String, ApiError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ApiError::BadRequest(format!("{field} is required")));
    }

    Ok(trimmed.to_owned())
}

fn origin_envelope(data: Value) -> ApiEnvelope {
    ApiEnvelope {
        data,
        cached: false,
        stale: false,
        source: "origin",
    }
}

fn cache_envelope(data: Value) -> ApiEnvelope {
    ApiEnvelope {
        data,
        cached: true,
        stale: false,
        source: "cache",
    }
}

async fn cached_envelope(state: &AppState, key: &str) -> Option<ApiEnvelope> {
    let mut entry = match state.cache.get(key).await {
        Ok(Some(entry)) => entry,
        Ok(None) | Err(_) => return None,
    };

    entry.record_hit();
    let payload = entry.payload.clone();
    let _ = state.cache.set(key, &entry, state.cache_ttl).await;

    Some(cache_envelope(payload))
}

async fn store_fresh(state: &AppState, key: &str, data: Value) {
    let entry = CachedEntry::fresh(data);
    let _ = state.cache.set(key, &entry, state.cache_ttl).await;
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, "bad_request", message),
            Self::OutboundLimiterClosed => (
                StatusCode::SERVICE_UNAVAILABLE,
                "outbound_limiter_closed",
                self.to_string(),
            ),
            Self::Origin(error) => (StatusCode::BAD_GATEWAY, "origin_error", error.to_string()),
        };

        (
            status,
            Json(ErrorBody {
                error: ErrorEnvelope { code, message },
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use serde_json::json;
    use std::{
        collections::HashMap,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use tokio::time::sleep;
    use tower::ServiceExt;

    #[derive(Debug, Default)]
    struct MockOrigin {
        calls: AtomicUsize,
        active: AtomicUsize,
        max_active: AtomicUsize,
        delay: Duration,
    }

    impl MockOrigin {
        fn with_delay(delay: Duration) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
                delay,
            }
        }

        async fn track_call(&self) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);

            if !self.delay.is_zero() {
                sleep(self.delay).await;
            }

            self.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl DbpediaClient for MockOrigin {
        async fn entity(&self, id: &str) -> Result<Value, OriginError> {
            self.track_call().await;
            Ok(json!({ "kind": "entity", "id": id }))
        }

        async fn search(&self, query: &str) -> Result<Value, OriginError> {
            self.track_call().await;
            Ok(json!({ "kind": "search", "query": query }))
        }

        async fn sparql(&self, query: &str) -> Result<Value, OriginError> {
            self.track_call().await;
            Ok(json!({ "kind": "sparql", "query": query }))
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

    fn test_router() -> Router {
        build_router(test_state(
            Arc::new(MockOrigin::default()),
            Arc::new(MemoryCache::default()),
        ))
    }

    fn test_state(origin: Arc<MockOrigin>, cache: Arc<MemoryCache>) -> AppState {
        test_state_with_limit(origin, cache, 2)
    }

    fn test_state_with_limit(
        origin: Arc<MockOrigin>,
        cache: Arc<MemoryCache>,
        max_outbound_concurrency: usize,
    ) -> AppState {
        AppState::new(
            origin,
            cache,
            Duration::from_secs(604_800),
            max_outbound_concurrency,
        )
    }

    #[tokio::test]
    async fn health_route_returns_ok() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/v1/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;

        assert_eq!(json["status"], "ok");
        assert_eq!(json["service"], "kgproxy");
        assert_eq!(json["outbound_available_permits"], 2);
        assert_eq!(json["max_outbound_concurrency"], 2);
    }

    #[tokio::test]
    async fn entity_route_returns_origin_data_envelope() {
        let json = request_json(
            Request::builder()
                .uri("/v1/entity/Albert_Einstein")
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(json["data"]["kind"], "entity");
        assert_eq!(json["data"]["id"], "Albert_Einstein");
        assert_eq!(json["cached"], false);
        assert_eq!(json["stale"], false);
        assert_eq!(json["source"], "origin");
    }

    #[tokio::test]
    async fn search_route_requires_query_input() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/v1/search?q=%20")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "bad_request");
    }

    #[tokio::test]
    async fn search_route_returns_origin_data() {
        let json = request_json(
            Request::builder()
                .uri("/v1/search?q=Albert%20Einstein")
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(json["data"]["kind"], "search");
        assert_eq!(json["data"]["query"], "Albert Einstein");
    }

    #[tokio::test]
    async fn sparql_route_validates_json_body() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/sparql")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{ "query": "   " }"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "bad_request");
    }

    #[tokio::test]
    async fn sparql_route_returns_origin_data() {
        let json = request_json(
            Request::builder()
                .method("POST")
                .uri("/v1/sparql")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{ "query": "SELECT * WHERE { ?s ?p ?o } LIMIT 1" }"#,
                ))
                .unwrap(),
        )
        .await;

        assert_eq!(json["data"]["kind"], "sparql");
        assert_eq!(json["data"]["query"], "SELECT * WHERE { ?s ?p ?o } LIMIT 1");
    }

    #[tokio::test]
    async fn cache_hit_returns_data_without_origin_call() {
        let origin = Arc::new(MockOrigin::default());
        let cache = Arc::new(MemoryCache::default());
        let key = cache_key::entity_key("Albert_Einstein", &[("lang", "en")]);
        cache
            .set(
                &key,
                &CachedEntry::fresh(json!({ "kind": "cached-entity" })),
                Duration::from_secs(604_800),
            )
            .await
            .unwrap();

        let response = build_router(test_state(origin.clone(), cache))
            .oneshot(
                Request::builder()
                    .uri("/v1/entity/Albert_Einstein")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["data"]["kind"], "cached-entity");
        assert_eq!(json["cached"], true);
        assert_eq!(json["source"], "cache");
        assert_eq!(origin.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cache_miss_calls_origin_once_and_stores_response() {
        let origin = Arc::new(MockOrigin::default());
        let cache = Arc::new(MemoryCache::default());
        let key = cache_key::search_key("Albert Einstein", &[("lang", "en")]);

        let response = build_router(test_state(origin.clone(), cache.clone()))
            .oneshot(
                Request::builder()
                    .uri("/v1/search?q=Albert%20Einstein")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["cached"], false);
        assert_eq!(origin.calls.load(Ordering::SeqCst), 1);

        let stored = cache.get(&key).await.unwrap().unwrap();
        assert_eq!(stored.payload["kind"], "search");
        assert_eq!(stored.hit_count, 0);
    }

    #[tokio::test]
    async fn cache_hit_does_not_acquire_outbound_permit() {
        let origin = Arc::new(MockOrigin::default());
        let cache = Arc::new(MemoryCache::default());
        let key = cache_key::entity_key("Albert_Einstein", &[("lang", "en")]);
        cache
            .set(
                &key,
                &CachedEntry::fresh(json!({ "kind": "cached-entity" })),
                Duration::from_secs(604_800),
            )
            .await
            .unwrap();

        let response = build_router(test_state_with_limit(origin.clone(), cache, 0))
            .oneshot(
                Request::builder()
                    .uri("/v1/entity/Albert_Einstein")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["cached"], true);
        assert_eq!(origin.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cache_misses_are_limited_to_two_concurrent_origin_calls() {
        let origin = Arc::new(MockOrigin::with_delay(Duration::from_millis(40)));
        let cache = Arc::new(MemoryCache::default());
        let router = build_router(test_state(origin.clone(), cache));

        let mut tasks = Vec::new();
        for index in 0..5 {
            let router = router.clone();
            tasks.push(tokio::spawn(async move {
                router
                    .oneshot(
                        Request::builder()
                            .uri(format!("/v1/entity/Entity_{index}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap()
                    .status()
            }));
        }

        for task in tasks {
            assert_eq!(task.await.unwrap(), StatusCode::OK);
        }

        assert_eq!(origin.calls.load(Ordering::SeqCst), 5);
        assert_eq!(origin.max_active.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn permit_is_released_after_origin_call() {
        let origin = Arc::new(MockOrigin::default());
        let cache = Arc::new(MemoryCache::default());
        let state = test_state_with_limit(origin, cache, 1);
        let router = build_router(state);

        for entity in ["First", "Second"] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/v1/entity/{entity}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK);
        }
    }

    async fn request_json(request: Request<Body>) -> Value {
        let response = test_router().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response).await
    }

    async fn body_json(response: axum::response::Response) -> Value {
        let body = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&body).unwrap()
    }
}
