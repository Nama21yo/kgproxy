# KGProxy

KGProxy is a Rust reliability gateway for DBpedia. It exposes a simple REST API,
adds application-level caching, protects the public DBpedia SPARQL endpoint with
bounded outbound concurrency, and records request metrics for review demos.

The current implementation covers:

- Rust service scaffold with `GET /v1/health`
- deterministic cache-key generation
- mockable DBpedia origin client and SPARQL query construction
- default DBpedia SPARQL endpoint with per-request DBpedia endpoint overrides
- public REST routes for entity lookup, search, and raw SPARQL
- Redis read-through caching with stale fallback support
- outbound concurrency limiting and circuit breaker behavior
- asynchronous PostgreSQL request logging
- `GET /v1/metrics/summary`
- `GET /v1/metrics/timeseries`
- local Docker Compose runtime for app, Redis, PostgreSQL, and Nginx
- optional cache-warmer runtime for popular entity refreshes

## Local Development

```bash
cd app
cargo fmt
cargo test
cargo run
```

Frontend dashboard:

```bash
cd frontend
bun install
bun run dev
```

For the Nginx-served dashboard:

```bash
cd frontend
bun run build
```

Then check:

```bash
curl -fsS http://127.0.0.1:8080/v1/health
```

## Docker Runtime

The repo includes a `Justfile` for common local workflows:

```bash
just test
just compose-config
just e2e
just verify
```

```bash
cp .env.example .env
docker compose up --build -d
scripts/smoke-health.sh
docker compose down
```

Nginx proxies the app locally on port `8081`:

```bash
curl -fsS http://127.0.0.1:8081/v1/health
docker compose exec nginx nginx -t
```

The dashboard is served by Nginx at:

```bash
http://127.0.0.1:8081/dashboard/
```

The root URL redirects to the dashboard:

```bash
http://127.0.0.1:8081/
```

For AWS deployment, see [`docs/deploy-aws.md`](docs/deploy-aws.md). After the
initial manual setup, GitHub Actions can deploy changes pushed to `main` using
AWS Systems Manager. The CI/CD setup is documented in
[`docs/ci-cd-aws.md`](docs/ci-cd-aws.md).

Nginx forwards `X-Real-IP` and `X-Forwarded-For`; KGProxy hashes the selected
client identifier before writing request logs to PostgreSQL.

Production TLS is configured with `docker-compose.prod.yml`,
`nginx/conf.d/production.conf.example`, and the Certbot service. See
`docs/tls-nginx.md` for the first-time certificate and renewal flow.

## Cache Warmer

The cache warmer is disabled by default. Enable it when the service should
periodically refresh popular entity cache entries from PostgreSQL request logs:

```bash
CACHE_WARMER_ENABLED=true
CACHE_WARMER_INTERVAL_SECONDS=3600
CACHE_WARMER_TOP_K=25
```

When enabled, the API process starts a background task that selects the top
requested entity IDs, refreshes them from DBpedia, and stores fresh responses in
Redis with the normal cache TTL. Failed entity refreshes are logged and do not
stop the next warmer run.

## SPARQL Endpoint Selection

KGProxy uses `https://dbpedia.org/sparql` by default. API users can override the
endpoint per request when they need a DBpedia language endpoint. Overrides are
validated so the proxy only forwards to `https://dbpedia.org/sparql` or
`https://*.dbpedia.org/sparql`.

Entity and search routes accept optional `endpoint` and `lang` query parameters:

```bash
curl -fsS \
  'http://127.0.0.1:8080/v1/entity/Berlin?endpoint=https%3A%2F%2Fde.dbpedia.org%2Fsparql'

curl -fsS \
  'http://127.0.0.1:8080/v1/entity/Berlin?endpoint=https%3A%2F%2Fde.dbpedia.org%2Fsparql&lang=de'

curl -fsS \
  'http://127.0.0.1:8080/v1/search?q=Paris&endpoint=https%3A%2F%2Ffr.dbpedia.org%2Fsparql'

curl -fsS \
  'http://127.0.0.1:8080/v1/search?q=Paris&endpoint=https%3A%2F%2Ffr.dbpedia.org%2Fsparql&lang=fr'
```

Raw SPARQL accepts an optional `endpoint` field in the JSON body:

```bash
curl -fsS -H 'content-type: application/json' \
  -d '{"query":"SELECT * WHERE { ?s ?p ?o } LIMIT 1","endpoint":"https://fr.dbpedia.org/sparql"}' \
  http://127.0.0.1:8080/v1/sparql
```

## End-To-End MVP Demo

Run the full local demo from the repository root:

```bash
scripts/e2e-smoke.sh
```

The script starts Docker Compose if the app is not already running, waits for
`/v1/health`, calls `GET /v1/entity/Albert_Einstein` twice, verifies the second
response is served from cache, calls `POST /v1/sparql`, checks
`GET /v1/metrics/summary`, and shuts Compose down only if it started the stack.
If Redis already has a persisted cache entry from a previous run, the first
entity response may also be cached.

Equivalent manual requests:

```bash
docker compose up --build -d
curl -fsS http://127.0.0.1:8080/v1/health
curl -fsS http://127.0.0.1:8080/v1/entity/Albert_Einstein
curl -fsS http://127.0.0.1:8080/v1/entity/Albert_Einstein
curl -fsS -H 'content-type: application/json' \
  -d '{"query":"SELECT * WHERE { ?s ?p ?o } LIMIT 1"}' \
  http://127.0.0.1:8080/v1/sparql
curl -fsS http://127.0.0.1:8080/v1/metrics/summary
curl -fsS http://127.0.0.1:8080/v1/metrics/timeseries
docker compose down
```

Troubleshooting:

- DBpedia timeouts: rerun the script after a short wait; cache hits should stay
  fast once the first entity lookup succeeds.
- Redis connection failures: check `docker compose ps redis` and confirm
  `REDIS_URL=redis://redis:6379/0` inside the app service.
- Postgres migration errors: run `docker compose logs app`; migrations are
  embedded at build time from `app/migrations` and run on app startup.
- Nginx TLS setup: local Compose exposes HTTP on `8081`; production TLS and
  Certbot commands are documented in `docs/tls-nginx.md`.
- AWS deployment: the EC2 runbook is in `docs/deploy-aws.md`.
