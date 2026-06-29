CREATE TABLE IF NOT EXISTS request_logs (
    id BIGSERIAL PRIMARY KEY,
    observed_at_unix_secs BIGINT NOT NULL,
    route TEXT NOT NULL,
    query_hash TEXT NOT NULL,
    cache_hit BOOLEAN NOT NULL,
    stale BOOLEAN NOT NULL,
    latency_ms INTEGER NOT NULL,
    client_hash TEXT NOT NULL,
    status_code SMALLINT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS idx_request_logs_observed_at
    ON request_logs (observed_at_unix_secs);

CREATE INDEX IF NOT EXISTS idx_request_logs_query_hash
    ON request_logs (query_hash);

CREATE INDEX IF NOT EXISTS idx_request_logs_route
    ON request_logs (route);
