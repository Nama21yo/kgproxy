use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::{
    cache::{CachedEntry, ResponseCache},
    cache_key,
    circuit_breaker::{BreakerState, CircuitBreaker},
    logging::{RequestLogEvent, RequestLogger},
    metrics::{EmptyMetricsReader, MetricsReader, MetricsSummary, MetricsTimeseries},
    origin::{DbpediaClient, OriginError},
};

#[derive(Clone)]
pub struct AppState {
    origin: Arc<dyn DbpediaClient>,
    cache: Arc<dyn ResponseCache>,
    cache_ttl: Duration,
    outbound_limiter: Arc<Semaphore>,
    max_outbound_concurrency: usize,
    breaker: Arc<CircuitBreaker>,
    logger: Arc<dyn RequestLogger>,
    metrics: Arc<dyn MetricsReader>,
}

impl AppState {
    pub fn new(
        origin: Arc<dyn DbpediaClient>,
        cache: Arc<dyn ResponseCache>,
        cache_ttl: Duration,
        max_outbound_concurrency: usize,
        logger: Arc<dyn RequestLogger>,
    ) -> Self {
        Self::with_breaker(
            origin,
            cache,
            cache_ttl,
            max_outbound_concurrency,
            Arc::new(CircuitBreaker::new(3, Duration::from_secs(30))),
            logger,
            Arc::new(EmptyMetricsReader),
        )
    }

    pub fn with_breaker(
        origin: Arc<dyn DbpediaClient>,
        cache: Arc<dyn ResponseCache>,
        cache_ttl: Duration,
        max_outbound_concurrency: usize,
        breaker: Arc<CircuitBreaker>,
        logger: Arc<dyn RequestLogger>,
        metrics: Arc<dyn MetricsReader>,
    ) -> Self {
        Self {
            origin,
            cache,
            cache_ttl,
            outbound_limiter: Arc::new(Semaphore::new(max_outbound_concurrency)),
            max_outbound_concurrency,
            breaker,
            logger,
            metrics,
        }
    }

    pub fn with_metrics(
        origin: Arc<dyn DbpediaClient>,
        cache: Arc<dyn ResponseCache>,
        cache_ttl: Duration,
        max_outbound_concurrency: usize,
        logger: Arc<dyn RequestLogger>,
        metrics: Arc<dyn MetricsReader>,
    ) -> Self {
        Self::with_breaker(
            origin,
            cache,
            cache_ttl,
            max_outbound_concurrency,
            Arc::new(CircuitBreaker::new(3, Duration::from_secs(30))),
            logger,
            metrics,
        )
    }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    outbound_available_permits: usize,
    max_outbound_concurrency: usize,
    circuit_breaker_state: &'static str,
    circuit_breaker_failures: u32,
    last_successful_origin_call_unix_secs: Option<u64>,
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
    endpoint: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EndpointParams {
    endpoint: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SparqlRequest {
    query: String,
    endpoint: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("outbound limiter is closed")]
    OutboundLimiterClosed,
    #[error("dbpedia origin is unavailable and no cached response exists")]
    OriginUnavailable,
    #[error(transparent)]
    Metrics(#[from] crate::metrics::MetricsError),
    #[error(transparent)]
    Origin(#[from] OriginError),
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/entity/:id", get(entity))
        .route("/v1/search", get(search))
        .route("/v1/sparql", post(sparql))
        .route("/v1/metrics/summary", get(metrics_summary))
        .route("/v1/metrics/timeseries", get(metrics_timeseries))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let breaker = state.breaker.snapshot().await;

    Json(HealthResponse {
        status: "ok",
        service: "kgproxy",
        outbound_available_permits: state.outbound_limiter.available_permits(),
        max_outbound_concurrency: state.max_outbound_concurrency,
        circuit_breaker_state: breaker.state.as_str(),
        circuit_breaker_failures: breaker.failure_count,
        last_successful_origin_call_unix_secs: breaker.last_success_unix_secs,
    })
}

async fn metrics_summary(State(state): State<AppState>) -> Result<Json<MetricsSummary>, ApiError> {
    Ok(Json(state.metrics.summary(86_400).await?))
}

async fn metrics_timeseries(
    State(state): State<AppState>,
) -> Result<Json<MetricsTimeseries>, ApiError> {
    Ok(Json(state.metrics.timeseries(86_400).await?))
}

async fn entity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<EndpointParams>,
) -> Result<Json<ApiEnvelope>, ApiError> {
    let started = Instant::now();
    let client_identifier = client_identifier(&headers);
    let id = match validate_non_empty(id, "entity id") {
        Ok(id) => id,
        Err(error) => {
            log_error(
                &state,
                "entity",
                "invalid_entity_id".to_owned(),
                error.status_code(),
                started,
                &client_identifier,
            )
            .await;
            return Err(error);
        }
    };
    let endpoint_override = validate_endpoint_override(params.endpoint)?;
    let lang = validate_language(params.lang)?;
    let cache_params = cache_params(endpoint_override.as_deref(), &lang);
    let cache_key = cache_key::entity_key(&id, &cache_params);

    if let Some(envelope) = cached_envelope(&state, &cache_key).await {
        log_request_with_metadata(
            &state,
            "entity",
            cache_key,
            &envelope,
            StatusCode::OK,
            started,
            json!({ "entity_id": id, "lang": lang, "endpoint": endpoint_override }),
            &client_identifier,
        )
        .await;
        return Ok(Json(envelope));
    }

    if state.breaker.before_request().await == BreakerState::Open {
        let result = stale_or_unavailable(&state, &cache_key).await;
        log_result(
            &state,
            "entity",
            cache_key,
            &result,
            started,
            &client_identifier,
        )
        .await;
        return result.map(Json);
    }

    let _permit = state
        .outbound_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::OutboundLimiterClosed)?;

    match state
        .origin
        .entity(&id, endpoint_override.as_deref(), &lang)
        .await
    {
        Ok(data) => {
            state.breaker.record_success().await;
            store_fresh(&state, &cache_key, data.clone()).await;
            let envelope = origin_envelope(data);
            log_request_with_metadata(
                &state,
                "entity",
                cache_key,
                &envelope,
                StatusCode::OK,
                started,
                json!({ "entity_id": id, "lang": lang, "endpoint": endpoint_override }),
                &client_identifier,
            )
            .await;
            Ok(Json(envelope))
        }
        Err(error) => {
            let result = origin_failure(&state, &cache_key, error).await;
            log_result(
                &state,
                "entity",
                cache_key,
                &result,
                started,
                &client_identifier,
            )
            .await;
            result.map(Json)
        }
    }
}

async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<SearchParams>,
) -> Result<Json<ApiEnvelope>, ApiError> {
    let started = Instant::now();
    let client_identifier = client_identifier(&headers);
    let query = match validate_non_empty(params.q, "search query") {
        Ok(query) => query,
        Err(error) => {
            log_error(
                &state,
                "search",
                "invalid_search_query".to_owned(),
                error.status_code(),
                started,
                &client_identifier,
            )
            .await;
            return Err(error);
        }
    };
    let endpoint_override = validate_endpoint_override(params.endpoint)?;
    let lang = validate_language(params.lang)?;
    let cache_params = cache_params(endpoint_override.as_deref(), &lang);
    let cache_key = cache_key::search_key(&query, &cache_params);

    if let Some(envelope) = cached_envelope(&state, &cache_key).await {
        log_request(
            &state,
            "search",
            cache_key,
            &envelope,
            StatusCode::OK,
            started,
            &client_identifier,
        )
        .await;
        return Ok(Json(envelope));
    }

    if state.breaker.before_request().await == BreakerState::Open {
        let result = stale_or_unavailable(&state, &cache_key).await;
        log_result(
            &state,
            "search",
            cache_key,
            &result,
            started,
            &client_identifier,
        )
        .await;
        return result.map(Json);
    }

    let _permit = state
        .outbound_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::OutboundLimiterClosed)?;

    match state
        .origin
        .search(&query, endpoint_override.as_deref(), &lang)
        .await
    {
        Ok(data) => {
            state.breaker.record_success().await;
            store_fresh(&state, &cache_key, data.clone()).await;
            let envelope = origin_envelope(data);
            log_request(
                &state,
                "search",
                cache_key,
                &envelope,
                StatusCode::OK,
                started,
                &client_identifier,
            )
            .await;
            Ok(Json(envelope))
        }
        Err(error) => {
            let result = origin_failure(&state, &cache_key, error).await;
            log_result(
                &state,
                "search",
                cache_key,
                &result,
                started,
                &client_identifier,
            )
            .await;
            result.map(Json)
        }
    }
}

async fn sparql(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SparqlRequest>,
) -> Result<Json<ApiEnvelope>, ApiError> {
    let started = Instant::now();
    let client_identifier = client_identifier(&headers);
    let query = match validate_non_empty(body.query, "sparql query") {
        Ok(query) => query,
        Err(error) => {
            log_error(
                &state,
                "sparql",
                "invalid_sparql_query".to_owned(),
                error.status_code(),
                started,
                &client_identifier,
            )
            .await;
            return Err(error);
        }
    };
    let endpoint_override = validate_endpoint_override(body.endpoint)?;
    let cache_params = cache_params(endpoint_override.as_deref(), "raw");
    let cache_key = cache_key::sparql_key_with_params(&query, &cache_params);

    if let Some(envelope) = cached_envelope(&state, &cache_key).await {
        log_request(
            &state,
            "sparql",
            cache_key,
            &envelope,
            StatusCode::OK,
            started,
            &client_identifier,
        )
        .await;
        return Ok(Json(envelope));
    }

    if state.breaker.before_request().await == BreakerState::Open {
        let result = stale_or_unavailable(&state, &cache_key).await;
        log_result(
            &state,
            "sparql",
            cache_key,
            &result,
            started,
            &client_identifier,
        )
        .await;
        return result.map(Json);
    }

    let _permit = state
        .outbound_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::OutboundLimiterClosed)?;

    match state
        .origin
        .sparql(&query, endpoint_override.as_deref())
        .await
    {
        Ok(data) => {
            state.breaker.record_success().await;
            store_fresh(&state, &cache_key, data.clone()).await;
            let envelope = origin_envelope(data);
            log_request(
                &state,
                "sparql",
                cache_key,
                &envelope,
                StatusCode::OK,
                started,
                &client_identifier,
            )
            .await;
            Ok(Json(envelope))
        }
        Err(error) => {
            let result = origin_failure(&state, &cache_key, error).await;
            log_result(
                &state,
                "sparql",
                cache_key,
                &result,
                started,
                &client_identifier,
            )
            .await;
            result.map(Json)
        }
    }
}

fn validate_non_empty(value: String, field: &'static str) -> Result<String, ApiError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ApiError::BadRequest(format!("{field} is required")));
    }

    Ok(trimmed.to_owned())
}

fn validate_endpoint_override(endpoint: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(endpoint) = endpoint else {
        return Ok(None);
    };
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(ApiError::BadRequest("endpoint required".to_owned()));
    }

    let url = Url::parse(endpoint)
        .map_err(|_| ApiError::BadRequest("endpoint must be a valid URL".to_owned()))?;
    let host = url
        .host_str()
        .ok_or_else(|| ApiError::BadRequest("endpoint must include a host".to_owned()))?;

    if url.scheme() != "https" {
        return Err(ApiError::BadRequest("endpoint must use https".to_owned()));
    }
    let is_dbpedia_endpoint = host == "dbpedia.org"
        || host.ends_with(".dbpedia.org")
        || host == "am.dbpedia.data.dice-research.org";
    if !is_dbpedia_endpoint {
        return Err(ApiError::BadRequest(
            "endpoint must be an approved DBpedia endpoint".to_owned(),
        ));
    }
    if url.path() != "/sparql" {
        return Err(ApiError::BadRequest(
            "endpoint path must be /sparql".to_owned(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(ApiError::BadRequest(
            "endpoint must not include query or fragment".to_owned(),
        ));
    }

    Ok(Some(url.to_string()))
}

fn validate_language(language: Option<String>) -> Result<String, ApiError> {
    let language = language
        .unwrap_or_else(|| "en".to_owned())
        .trim()
        .to_ascii_lowercase();
    let valid = !language.is_empty()
        && language.len() <= 35
        && language.split('-').enumerate().all(|(index, part)| {
            let valid_length = (1..=8).contains(&part.len());
            let valid_chars = part
                .chars()
                .all(|character| character.is_ascii_alphanumeric());
            let valid_first = index == 0
                && part
                    .chars()
                    .all(|character| character.is_ascii_alphabetic());
            valid_length && valid_chars && (index > 0 || valid_first)
        });

    if !valid {
        return Err(ApiError::BadRequest(
            "lang must be a valid BCP-47-style language tag".to_owned(),
        ));
    }

    Ok(language)
}

fn cache_params<'a>(endpoint_override: Option<&'a str>, lang: &'a str) -> Vec<(&'a str, &'a str)> {
    let mut params = vec![("lang", lang)];
    if let Some(endpoint) = endpoint_override {
        params.push(("endpoint", endpoint));
    }
    params
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

fn stale_envelope(data: Value) -> ApiEnvelope {
    ApiEnvelope {
        data,
        cached: true,
        stale: true,
        source: "stale_cache",
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

async fn stale_or_unavailable(state: &AppState, key: &str) -> Result<ApiEnvelope, ApiError> {
    match state.cache.get_stale(key).await {
        Ok(Some(entry)) => Ok(stale_envelope(entry.payload)),
        Ok(None) | Err(_) => Err(ApiError::OriginUnavailable),
    }
}

async fn origin_failure(
    state: &AppState,
    key: &str,
    error: OriginError,
) -> Result<ApiEnvelope, ApiError> {
    state.breaker.record_failure().await;

    match state.cache.get_stale(key).await {
        Ok(Some(entry)) => Ok(stale_envelope(entry.payload)),
        Ok(None) | Err(_) => Err(ApiError::Origin(error)),
    }
}

async fn store_fresh(state: &AppState, key: &str, data: Value) {
    let entry = CachedEntry::fresh(data);
    let _ = state.cache.set(key, &entry, state.cache_ttl).await;
}

async fn log_result(
    state: &AppState,
    route: &'static str,
    query_hash: String,
    result: &Result<ApiEnvelope, ApiError>,
    started: Instant,
    client_identifier: &str,
) {
    match result {
        Ok(envelope) => {
            log_request(
                state,
                route,
                query_hash,
                envelope,
                StatusCode::OK,
                started,
                client_identifier,
            )
            .await
        }
        Err(error) => {
            log_error(
                state,
                route,
                query_hash,
                error.status_code(),
                started,
                client_identifier,
            )
            .await;
        }
    }
}

async fn log_request(
    state: &AppState,
    route: &'static str,
    query_hash: String,
    envelope: &ApiEnvelope,
    status: StatusCode,
    started: Instant,
    client_identifier: &str,
) {
    log_request_with_metadata(
        state,
        route,
        query_hash,
        envelope,
        status,
        started,
        json!({}),
        client_identifier,
    )
    .await;
}

async fn log_request_with_metadata(
    state: &AppState,
    route: &'static str,
    query_hash: String,
    envelope: &ApiEnvelope,
    status: StatusCode,
    started: Instant,
    metadata: Value,
    client_identifier: &str,
) {
    let event = RequestLogEvent::new(
        route,
        query_hash,
        envelope.cached,
        envelope.stale,
        started.elapsed().as_millis(),
        client_identifier,
        status.as_u16(),
    )
    .with_metadata(metadata);

    state.logger.log(event).await;
}

async fn log_error(
    state: &AppState,
    route: &'static str,
    query_hash: String,
    status: StatusCode,
    started: Instant,
    client_identifier: &str,
) {
    state
        .logger
        .log(RequestLogEvent::new(
            route,
            query_hash,
            false,
            false,
            started.elapsed().as_millis(),
            client_identifier,
            status.as_u16(),
        ))
        .await;
}

fn client_identifier(headers: &HeaderMap) -> String {
    header_str(headers, "x-real-ip")
        .and_then(first_forwarded_value)
        .or_else(|| header_str(headers, "x-forwarded-for").and_then(first_forwarded_value))
        .unwrap_or("unknown")
        .to_owned()
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

fn first_forwarded_value(value: &str) -> Option<&str> {
    value
        .split(',')
        .map(str::trim)
        .find(|part| !part.is_empty())
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
            Self::OriginUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "origin_unavailable",
                self.to_string(),
            ),
            Self::Metrics(error) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "metrics_unavailable",
                error.to_string(),
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

impl ApiError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::OutboundLimiterClosed | Self::OriginUnavailable => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::Metrics(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Origin(_) => StatusCode::BAD_GATEWAY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::NoopLogger;
    use async_trait::async_trait;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use reqwest::StatusCode as ReqwestStatusCode;
    use serde_json::json;
    use std::{
        collections::HashMap,
        sync::{
            Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
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
        fail: AtomicBool,
    }

    impl MockOrigin {
        fn with_delay(delay: Duration) -> Self {
            Self {
                delay,
                ..Self::default()
            }
        }

        fn failing() -> Self {
            Self {
                fail: AtomicBool::new(true),
                ..Self::default()
            }
        }

        async fn track_call(&self) -> Result<(), OriginError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);

            if !self.delay.is_zero() {
                sleep(self.delay).await;
            }

            self.active.fetch_sub(1, Ordering::SeqCst);

            if self.fail.load(Ordering::SeqCst) {
                return Err(OriginError::Status(ReqwestStatusCode::BAD_GATEWAY));
            }

            Ok(())
        }
    }

    #[async_trait]
    impl DbpediaClient for MockOrigin {
        async fn entity(
            &self,
            id: &str,
            endpoint_override: Option<&str>,
            lang: &str,
        ) -> Result<Value, OriginError> {
            self.track_call().await?;
            Ok(json!({ "kind": "entity", "id": id, "endpoint": endpoint_override, "lang": lang }))
        }

        async fn search(
            &self,
            query: &str,
            endpoint_override: Option<&str>,
            lang: &str,
        ) -> Result<Value, OriginError> {
            self.track_call().await?;
            Ok(
                json!({ "kind": "search", "query": query, "endpoint": endpoint_override, "lang": lang }),
            )
        }

        async fn sparql(
            &self,
            query: &str,
            endpoint_override: Option<&str>,
        ) -> Result<Value, OriginError> {
            self.track_call().await?;
            Ok(json!({ "kind": "sparql", "query": query, "endpoint": endpoint_override }))
        }
    }

    #[derive(Debug, Default)]
    struct MemoryCache {
        entries: Mutex<HashMap<String, CachedEntry>>,
        stale_entries: Mutex<HashMap<String, CachedEntry>>,
    }

    #[derive(Debug, Default)]
    struct CollectingLogger {
        events: Mutex<Vec<RequestLogEvent>>,
    }

    #[async_trait]
    impl RequestLogger for CollectingLogger {
        async fn log(&self, event: RequestLogEvent) {
            self.events.lock().unwrap().push(event);
        }
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
            Ok(self.stale_entries.lock().unwrap().get(key).cloned())
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
            self.stale_entries
                .lock()
                .unwrap()
                .insert(key.to_owned(), entry.clone());
            Ok(())
        }
    }

    impl MemoryCache {
        fn expire_fresh(&self, key: &str) {
            self.entries.lock().unwrap().remove(key);
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
            Arc::new(NoopLogger),
        )
    }

    fn test_state_with_breaker(
        origin: Arc<MockOrigin>,
        cache: Arc<MemoryCache>,
        breaker: Arc<CircuitBreaker>,
    ) -> AppState {
        AppState::with_breaker(
            origin,
            cache,
            Duration::from_secs(604_800),
            2,
            breaker,
            Arc::new(NoopLogger),
            Arc::new(EmptyMetricsReader),
        )
    }

    fn test_state_with_logger(
        origin: Arc<MockOrigin>,
        cache: Arc<MemoryCache>,
        logger: Arc<CollectingLogger>,
    ) -> AppState {
        AppState::with_breaker(
            origin,
            cache,
            Duration::from_secs(604_800),
            2,
            Arc::new(CircuitBreaker::new(3, Duration::from_secs(30))),
            logger,
            Arc::new(EmptyMetricsReader),
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
        assert_eq!(json["circuit_breaker_state"], "closed");
    }

    #[tokio::test]
    async fn metrics_summary_route_returns_empty_default_summary() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/v1/metrics/summary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert_eq!(json["rolling_window_seconds"], 86_400);
        assert_eq!(json["total_requests"], 0);
        assert_eq!(json["cache_hit_rate"], 0.0);
    }

    #[tokio::test]
    async fn metrics_timeseries_route_returns_empty_default_series() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/v1/metrics/timeseries")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert_eq!(json["rolling_window_seconds"], 86_400);
        assert_eq!(json["bucket_seconds"], 3_600);
        assert_eq!(json["points"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn request_logging_uses_x_real_ip_hash() {
        let origin = Arc::new(MockOrigin::default());
        let cache = Arc::new(MemoryCache::default());
        let logger = Arc::new(CollectingLogger::default());
        let router = build_router(test_state_with_logger(origin, cache, logger.clone()));

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/v1/entity/Albert_Einstein")
                    .header("x-real-ip", "203.0.113.10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let events = logger.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].client_hash,
            RequestLogEvent::new(
                "entity",
                "q".to_owned(),
                false,
                false,
                0,
                "203.0.113.10",
                200
            )
            .client_hash
        );
        assert_ne!(events[0].client_hash, "203.0.113.10");
    }

    #[tokio::test]
    async fn request_logging_falls_back_to_first_forwarded_for_ip() {
        let origin = Arc::new(MockOrigin::default());
        let cache = Arc::new(MemoryCache::default());
        let logger = Arc::new(CollectingLogger::default());
        let router = build_router(test_state_with_logger(origin, cache, logger.clone()));

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/v1/search?q=Albert%20Einstein")
                    .header("x-forwarded-for", "198.51.100.2, 198.51.100.3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let events = logger.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].client_hash,
            RequestLogEvent::new(
                "search",
                "q".to_owned(),
                false,
                false,
                0,
                "198.51.100.2",
                200
            )
            .client_hash
        );
    }

    #[tokio::test]
    async fn request_logging_uses_unknown_without_proxy_headers() {
        let origin = Arc::new(MockOrigin::default());
        let cache = Arc::new(MemoryCache::default());
        let logger = Arc::new(CollectingLogger::default());
        let router = build_router(test_state_with_logger(origin, cache, logger.clone()));

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/v1/entity/Albert_Einstein")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let events = logger.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].client_hash,
            RequestLogEvent::new("entity", "q".to_owned(), false, false, 0, "unknown", 200)
                .client_hash
        );
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
    async fn entity_route_accepts_dbpedia_endpoint_override() {
        let json = request_json(
            Request::builder()
                .uri("/v1/entity/Berlin?endpoint=https%3A%2F%2Fde.dbpedia.org%2Fsparql")
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(json["data"]["kind"], "entity");
        assert_eq!(json["data"]["id"], "Berlin");
        assert_eq!(json["data"]["endpoint"], "https://de.dbpedia.org/sparql");
    }

    #[tokio::test]
    async fn entity_route_accepts_amharic_dbpedia_endpoint_override() {
        let json = request_json(
            Request::builder()
                .uri("/v1/entity/Albert_Einstein?endpoint=https%3A%2F%2Fam.dbpedia.data.dice-research.org%2Fsparql&lang=am")
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(json["data"]["lang"], "am");
        assert_eq!(
            json["data"]["endpoint"],
            "https://am.dbpedia.data.dice-research.org/sparql"
        );
    }

    #[tokio::test]
    async fn entity_route_passes_requested_language_to_origin() {
        let json = request_json(
            Request::builder()
                .uri("/v1/entity/Berlin?lang=de")
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(json["data"]["lang"], "de");
    }

    #[tokio::test]
    async fn language_validation_rejects_invalid_tags() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/v1/search?q=Berlin&lang=de_DE")
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
    async fn endpoint_override_rejects_non_dbpedia_hosts() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/v1/entity/Albert_Einstein?endpoint=https%3A%2F%2Fexample.com%2Fsparql")
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
    async fn sparql_route_accepts_endpoint_override_in_body() {
        let json = request_json(
            Request::builder()
                .method("POST")
                .uri("/v1/sparql")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{ "query": "SELECT * WHERE { ?s ?p ?o } LIMIT 1", "endpoint": "https://fr.dbpedia.org/sparql" }"#,
                ))
                .unwrap(),
        )
        .await;

        assert_eq!(json["data"]["kind"], "sparql");
        assert_eq!(json["data"]["endpoint"], "https://fr.dbpedia.org/sparql");
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

    #[tokio::test]
    async fn origin_failure_serves_stale_cached_response() {
        let origin = Arc::new(MockOrigin::failing());
        let cache = Arc::new(MemoryCache::default());
        let key = cache_key::entity_key("Albert_Einstein", &[("lang", "en")]);
        cache
            .set(
                &key,
                &CachedEntry::fresh(json!({ "kind": "stale-entity" })),
                Duration::from_secs(604_800),
            )
            .await
            .unwrap();
        cache.expire_fresh(&key);

        let response = build_router(test_state(origin, cache))
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
        assert_eq!(json["data"]["kind"], "stale-entity");
        assert_eq!(json["stale"], true);
        assert_eq!(json["source"], "stale_cache");
    }

    #[tokio::test]
    async fn open_breaker_serves_stale_without_forwarding_origin_call() {
        let origin = Arc::new(MockOrigin::default());
        let cache = Arc::new(MemoryCache::default());
        let breaker = Arc::new(CircuitBreaker::new(1, Duration::from_secs(30)));
        breaker.record_failure().await;
        let key = cache_key::entity_key("Albert_Einstein", &[("lang", "en")]);
        cache
            .set(
                &key,
                &CachedEntry::fresh(json!({ "kind": "stale-entity" })),
                Duration::from_secs(604_800),
            )
            .await
            .unwrap();
        cache.expire_fresh(&key);

        let response = build_router(test_state_with_breaker(origin.clone(), cache, breaker))
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
        assert_eq!(json["stale"], true);
        assert_eq!(origin.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn open_breaker_without_cache_returns_unavailable_error() {
        let origin = Arc::new(MockOrigin::default());
        let cache = Arc::new(MemoryCache::default());
        let breaker = Arc::new(CircuitBreaker::new(1, Duration::from_secs(30)));
        breaker.record_failure().await;

        let response = build_router(test_state_with_breaker(origin, cache, breaker))
            .oneshot(
                Request::builder()
                    .uri("/v1/entity/Albert_Einstein")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "origin_unavailable");
    }

    #[tokio::test]
    async fn successful_request_emits_log_event() {
        let origin = Arc::new(MockOrigin::default());
        let cache = Arc::new(MemoryCache::default());
        let logger = Arc::new(CollectingLogger::default());

        let response = build_router(test_state_with_logger(origin, cache, logger.clone()))
            .oneshot(
                Request::builder()
                    .uri("/v1/search?q=Albert%20Einstein")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let events = logger.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].route, "search");
        assert_eq!(events[0].status_code, 200);
        assert!(!events[0].cache_hit);
        assert!(!events[0].stale);
    }

    #[tokio::test]
    async fn validation_error_emits_log_event() {
        let logger = Arc::new(CollectingLogger::default());

        let response = build_router(test_state_with_logger(
            Arc::new(MockOrigin::default()),
            Arc::new(MemoryCache::default()),
            logger.clone(),
        ))
        .oneshot(
            Request::builder()
                .uri("/v1/search?q=%20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let events = logger.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].route, "search");
        assert_eq!(events[0].query_hash, "invalid_search_query");
        assert_eq!(events[0].status_code, 400);
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
