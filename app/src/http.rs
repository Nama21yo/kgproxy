use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::origin::{DbpediaClient, OriginError};

#[derive(Clone)]
pub struct AppState {
    origin: Arc<dyn DbpediaClient>,
}

impl AppState {
    pub fn new(origin: Arc<dyn DbpediaClient>) -> Self {
        Self { origin }
    }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
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

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "kgproxy",
    })
}

async fn entity(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiEnvelope>, ApiError> {
    let id = validate_non_empty(id, "entity id")?;
    let data = state.origin.entity(&id).await?;

    Ok(Json(origin_envelope(data)))
}

async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<ApiEnvelope>, ApiError> {
    let query = validate_non_empty(params.q, "search query")?;
    let data = state.origin.search(&query).await?;

    Ok(Json(origin_envelope(data)))
}

async fn sparql(
    State(state): State<AppState>,
    Json(body): Json<SparqlRequest>,
) -> Result<Json<ApiEnvelope>, ApiError> {
    let query = validate_non_empty(body.query, "sparql query")?;
    let data = state.origin.sparql(&query).await?;

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

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, "bad_request", message),
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
    use tower::ServiceExt;

    #[derive(Debug, Default)]
    struct MockOrigin;

    #[async_trait]
    impl DbpediaClient for MockOrigin {
        async fn entity(&self, id: &str) -> Result<Value, OriginError> {
            Ok(json!({ "kind": "entity", "id": id }))
        }

        async fn search(&self, query: &str) -> Result<Value, OriginError> {
            Ok(json!({ "kind": "search", "query": query }))
        }

        async fn sparql(&self, query: &str) -> Result<Value, OriginError> {
            Ok(json!({ "kind": "sparql", "query": query }))
        }
    }

    fn test_router() -> Router {
        build_router(AppState::new(Arc::new(MockOrigin)))
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

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["status"], "ok");
        assert_eq!(json["service"], "kgproxy");
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
