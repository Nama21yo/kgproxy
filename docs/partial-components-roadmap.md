# Partially Implemented Components Roadmap

This document tracks KGProxy components that already have some code, config, or
documentation in the repository, but are not yet complete runtime features.

## 1. Cache Warmer Worker

### Current State

- `app/src/warmer.rs` implements the core cache-warmer logic.
- It can select popular entity requests from PostgreSQL request logs.
- It can refresh those entities from DBpedia and write fresh entries into Redis.
- Unit tests cover refreshing top entities and honoring limits.
- The API process can start the warmer as a background `tokio` task when
  `CACHE_WARMER_ENABLED=true`.
- Runtime controls are available through:
  - `CACHE_WARMER_ENABLED`
  - `CACHE_WARMER_INTERVAL_SECONDS`
  - `CACHE_WARMER_TOP_K`

### Remaining Work

- Add an optional refresh threshold before TTL expiry if we want to avoid
  refreshing entries that were just written.
- Add richer structured logs, such as run duration.
- Consider a separate `kgproxy-warmer` binary only if the warmer needs to scale
  independently from the API service.

### Future Implementation Steps

1. Add run-duration logging around each warmer loop.
2. Add a freshness threshold so only near-expiry entries are refreshed.
3. Add an integration-style test for the full runtime loop with mocked
   source/origin/cache.
4. Revisit separate-process deployment if production traffic requires it.

### Verification

```bash
cd app
cargo test warmer
cargo test
```

If wired into Docker Compose:

```bash
docker compose up --build -d
docker compose logs -f app
```

Expected result: popular entities from request logs are refreshed into Redis
without blocking normal API traffic.

## 2. Production Nginx TLS

### Current State

- `nginx/conf.d/default.conf` provides a local HTTP reverse proxy.
- It includes per-IP `limit_req` rate limiting.
- It forwards `X-Real-IP`, `X-Forwarded-For`, and `X-Forwarded-Proto`.
- `nginx/conf.d/production.conf.example` provides a production TLS template.
- `docker-compose.prod.yml` adds the production `443:443` port binding and
  certificate mount.
- `docker-compose.yml` includes a Certbot service behind the `tls` profile.
- `docs/tls-nginx.md` documents first-time certificate issuance, enabling TLS,
  verification, and renewal.

### Remaining Work

- Replace `kgproxy.example.com` with the real production hostname during
  deployment.
- Issue the first live certificate on the server.
- Install the documented host cron entry for certificate renewal.
- Verify HTTPS against the real DNS name.

### Deployment Steps

1. Set `NGINX_HTTP_PORT=80` in production `.env`.
2. Start the production stack with
   `docker compose -f docker-compose.yml -f docker-compose.prod.yml up -d`.
3. Issue the certificate with
   `docker compose --profile tls run --rm certbot certonly`.
4. Copy `production.conf.example` to `production.conf` and replace the domain.
5. Restart Nginx and validate HTTPS.

### Verification

```bash
docker compose config
docker compose exec nginx nginx -t
curl -fsS https://kgproxy.example.com/v1/health
```

Expected result: HTTPS terminates at Nginx and proxies requests to the Rust app.

## 3. Real Client IP Logging

### Current State

- `RequestLogEvent` supports a hashed client identifier.
- `logging.rs` hashes client identifiers before storage.
- Nginx forwards `X-Real-IP` and `X-Forwarded-For`.
- Rust handlers extract `X-Real-IP`, then the first `X-Forwarded-For` value,
  then fall back to `"unknown"`.
- Request logs store only the hashed identifier, not the raw header value.
- Route tests cover `X-Real-IP`, `X-Forwarded-For`, and no-header fallback.

### Remaining Work

- Add an explicit trusted-proxy mode if the Rust app is ever exposed directly
  without Nginx.
- Optionally parse and validate IP address syntax before hashing.

### Future Implementation Steps

1. Add a config flag such as `TRUST_PROXY_HEADERS=true`.
2. Ignore forwarded headers when that flag is false.
3. Fall back to connection metadata if exposed through Axum/Tower in a future
   refactor.
4. Add tests for trusted-proxy mode.

### Verification

```bash
cd app
cargo test logging
cargo test http
```

Manual check:

```bash
curl -fsS -H 'X-Real-IP: 203.0.113.10' \
  http://127.0.0.1:8080/v1/entity/Albert_Einstein
```

Expected result: PostgreSQL stores a stable hash for the client identifier, not
the raw IP.

## 4. Metrics Dashboard

### Current State

- `GET /v1/metrics/summary` and `/v1/metrics/timeseries` exist.
- Metrics are computed from PostgreSQL request logs.
- The API returns total requests, cache hits, stale responses, origin errors,
  rates, and p95 latency.
- `frontend/` contains the Svelte/Tailwind dashboard.
- `frontend/dist` is mounted into Nginx after `bun run build`.
- Local Nginx serves the dashboard at `http://127.0.0.1:8081/dashboard/`.
- Production Nginx serves the dashboard at `https://<domain>/dashboard/`.
- The dashboard fetches health, summary, and hourly timeseries metrics.
- The dashboard can run language-aware entity lookups, searches, and raw
  SPARQL queries and displays response metadata.

### Remaining Work

- Add CloudWatch dashboard export only if AWS-hosted operational monitoring is
  needed beyond the built-in demo dashboard.
- Add browser-level visual regression checks if the dashboard grows more
  complex.

### Future Implementation Steps

1. Add CloudWatch integration if AWS-hosted monitoring requires it.
2. Add browser-level visual regression checks if the dashboard grows more
   complex.

### Verification

```bash
curl -fsS http://127.0.0.1:8080/v1/metrics/summary
curl -fsS http://127.0.0.1:8080/v1/metrics/timeseries
curl -fsS http://127.0.0.1:8080/v1/health
curl -fsS http://127.0.0.1:8081/dashboard/
```

Expected result: reviewers can see KGProxy health, cache behavior, and origin
error behavior without reading raw JSON manually.

## 5. Language-Aware Endpoint Support

### Current State

- The default endpoint remains `https://dbpedia.org/sparql`.
- API users can override the DBpedia SPARQL endpoint per request.
- Overrides are restricted to `https://dbpedia.org/sparql` or
  `https://*.dbpedia.org/sparql`.
- Cache keys include the selected endpoint.
- Entity and search query templates still filter `lang(?label) = "en"` and
  `lang(?abstract) = "en"`.

### Completed in current implementation

- Entity and search routes accept `lang`, defaulting to `en`.
- Generated label and abstract filters use the requested language.
- Language and endpoint are both included in cache keys.
- Language tags are validated using a conservative BCP-47-style format.

### Verification

```bash
cd app
cargo test origin
cargo test http
```

Manual examples:

```bash
curl -fsS \
  'http://127.0.0.1:8080/v1/entity/Berlin?endpoint=https%3A%2F%2Fde.dbpedia.org%2Fsparql&lang=de'

curl -fsS \
  'http://127.0.0.1:8080/v1/search?q=Paris&endpoint=https%3A%2F%2Ffr.dbpedia.org%2Fsparql&lang=fr'
```

Expected result: endpoint selection and language filters are aligned, and cache
entries stay separated by both endpoint and language.
