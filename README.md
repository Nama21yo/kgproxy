# KGProxy

KGProxy is a Rust reliability gateway for DBpedia. It exposes a simple REST API,
adds application-level caching, protects the public DBpedia SPARQL endpoint with
bounded outbound concurrency, and records request metrics for review demos.

The current implementation covers:

- Rust service scaffold with `GET /v1/health`
- deterministic cache-key generation
- mockable DBpedia origin client and SPARQL query construction
- public REST routes for entity lookup, search, and raw SPARQL
- Redis read-through caching with stale fallback support
- outbound concurrency limiting and circuit breaker behavior
- asynchronous PostgreSQL request logging
- `GET /v1/metrics/summary`
- local Docker Compose runtime for app, Redis, PostgreSQL, and Nginx
- cache-warmer selection logic

## Local Development

```bash
cd app
cargo fmt
cargo test
cargo run
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
