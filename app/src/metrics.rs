use async_trait::async_trait;
use serde::Serialize;
use sqlx::{PgPool, Row};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MetricsSummary {
    pub rolling_window_seconds: u64,
    pub total_requests: u64,
    pub cache_hits: u64,
    pub stale_responses: u64,
    pub origin_errors: u64,
    pub cache_hit_rate: f64,
    pub stale_response_rate: f64,
    pub origin_error_rate: f64,
    pub p95_latency_ms: u128,
}

#[derive(Debug, Clone)]
pub struct RequestMetricRow {
    pub cache_hit: bool,
    pub stale: bool,
    pub latency_ms: u128,
    pub status_code: u16,
}

#[derive(Debug, Error)]
pub enum MetricsError {
    #[error("postgres error: {0}")]
    Sqlx(#[from] sqlx::Error),
}

#[async_trait]
pub trait MetricsReader: Send + Sync {
    async fn summary(&self, window_seconds: u64) -> Result<MetricsSummary, MetricsError>;
}

#[derive(Debug, Default)]
pub struct EmptyMetricsReader;

#[async_trait]
impl MetricsReader for EmptyMetricsReader {
    async fn summary(&self, window_seconds: u64) -> Result<MetricsSummary, MetricsError> {
        Ok(summarize_rows(window_seconds, &[]))
    }
}

#[derive(Debug, Clone)]
pub struct PostgresMetricsReader {
    pool: PgPool,
}

impl PostgresMetricsReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MetricsReader for PostgresMetricsReader {
    async fn summary(&self, window_seconds: u64) -> Result<MetricsSummary, MetricsError> {
        let rows = sqlx::query(
            r#"
            SELECT cache_hit, stale, latency_ms, status_code
            FROM request_logs
            WHERE observed_at_unix_secs >= EXTRACT(EPOCH FROM NOW())::BIGINT - $1
            "#,
        )
        .bind(i64::try_from(window_seconds).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;

        let rows = rows
            .into_iter()
            .map(|row| RequestMetricRow {
                cache_hit: row.get("cache_hit"),
                stale: row.get("stale"),
                latency_ms: row.get::<i32, _>("latency_ms") as u128,
                status_code: row.get::<i16, _>("status_code") as u16,
            })
            .collect::<Vec<_>>();

        Ok(summarize_rows(window_seconds, &rows))
    }
}

pub fn summarize_rows(window_seconds: u64, rows: &[RequestMetricRow]) -> MetricsSummary {
    let total_requests = rows.len() as u64;
    let cache_hits = rows.iter().filter(|row| row.cache_hit).count() as u64;
    let stale_responses = rows.iter().filter(|row| row.stale).count() as u64;
    let origin_errors = rows
        .iter()
        .filter(|row| row.status_code >= 500 && !row.stale)
        .count() as u64;
    let p95_latency_ms = percentile_95(rows.iter().map(|row| row.latency_ms).collect());

    MetricsSummary {
        rolling_window_seconds: window_seconds,
        total_requests,
        cache_hits,
        stale_responses,
        origin_errors,
        cache_hit_rate: ratio(cache_hits, total_requests),
        stale_response_rate: ratio(stale_responses, total_requests),
        origin_error_rate: ratio(origin_errors, total_requests),
        p95_latency_ms,
    }
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        return 0.0;
    }

    numerator as f64 / denominator as f64
}

fn percentile_95(mut values: Vec<u128>) -> u128 {
    if values.is_empty() {
        return 0;
    }

    values.sort_unstable();
    let index = ((values.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
    values[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_calculates_rates_and_latency_percentile() {
        let rows = vec![
            RequestMetricRow {
                cache_hit: true,
                stale: false,
                latency_ms: 10,
                status_code: 200,
            },
            RequestMetricRow {
                cache_hit: false,
                stale: false,
                latency_ms: 100,
                status_code: 200,
            },
            RequestMetricRow {
                cache_hit: true,
                stale: true,
                latency_ms: 12,
                status_code: 200,
            },
            RequestMetricRow {
                cache_hit: false,
                stale: false,
                latency_ms: 250,
                status_code: 502,
            },
        ];

        let summary = summarize_rows(86_400, &rows);

        assert_eq!(summary.total_requests, 4);
        assert_eq!(summary.cache_hits, 2);
        assert_eq!(summary.stale_responses, 1);
        assert_eq!(summary.origin_errors, 1);
        assert_eq!(summary.cache_hit_rate, 0.5);
        assert_eq!(summary.stale_response_rate, 0.25);
        assert_eq!(summary.origin_error_rate, 0.25);
        assert_eq!(summary.p95_latency_ms, 250);
    }

    #[test]
    fn empty_summary_returns_zeroes() {
        let summary = summarize_rows(86_400, &[]);

        assert_eq!(summary.total_requests, 0);
        assert_eq!(summary.cache_hit_rate, 0.0);
        assert_eq!(summary.p95_latency_ms, 0);
    }
}
