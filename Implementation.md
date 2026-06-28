# KGProxy Implementation Plan

KGProxy is a Rust-based reliability gateway for DBpedia. The project should be
implemented test-first: every milestone begins with failing tests, then the
smallest implementation that makes those tests pass, then one or more atomic
git commits.

This plan assumes `kgproxy/` becomes its own repository or subproject. The
parent directory is not currently a Git repository, so the commit messages below
are the intended commit boundaries once Git is initialized for this folder.

## Development Rules

- Write the test before the feature code.
- Keep each commit focused on one behavior, one infrastructure concern, or one
  documentation change.
- Do not commit generated secrets, local `.env` files, database volumes, or
  Docker data.
- Prefer integration tests around public HTTP behavior and focused unit tests
  for cache keys, stale fallback, query shaping, and circuit-breaker state.
- Every milestone ends with `cargo fmt`, `cargo test`, and the relevant Docker
  Compose smoke test once Compose exists.

## Milestone 0: Repository Skeleton

**Goal:** Create a runnable Rust workspace with one API binary and a basic test
layout.

**Tests first:**

- Add a smoke test that starts the router in-process and expects
  `GET /v1/health` to return HTTP 200.
- Add a config-loading test that verifies defaults for cache TTL, DBpedia URL,
  outbound concurrency, and bind address.

**Implementation:**

- Initialize a Rust binary crate under `app/`.
- Add dependencies: `axum`, `tokio`, `serde`, `serde_json`, `tracing`,
  `tracing-subscriber`, `tower`, and `thiserror`.
- Split code into `config`, `http`, and `main` modules.
- Implement `GET /v1/health` with a static JSON response.
- Add `.gitignore`, `.env.example`, and a root `README.md`.

**Verification:**

```bash
cargo fmt
cargo test
cargo run
```

**Atomic commits:**

```bash
git commit -m "chore: scaffold kgproxy rust service"
git commit -m "test: cover health route and default config"
git commit -m "feat: expose health endpoint"
```

## Milestone 1: Deterministic Cache Keys

**Goal:** Build the cache-key layer before connecting Redis or DBpedia.

**Tests first:**

- Same REST endpoint and same normalized parameters produce the same SHA-256
  key regardless of query parameter order.
- Different endpoints, entity IDs, labels, or SPARQL text produce different
  keys.
- Raw SPARQL keys are based on exact submitted query text after agreed
  whitespace normalization.

**Implementation:**

- Add a `cache_key` module.
- Define cache namespaces for `entity`, `search`, and `sparql`.
- Normalize REST query parameters with structured parsing, not string
  concatenation.
- Return hex-encoded SHA-256 keys.

**Verification:**

```bash
cargo fmt
cargo test cache_key
```

**Atomic commits:**

```bash
git commit -m "test: define cache key normalization behavior"
git commit -m "feat: add deterministic cache key builder"
```

## Milestone 2: DBpedia Origin Client

**Goal:** Implement DBpedia access behind a mockable trait.

**Tests first:**

- Entity lookup for `Albert_Einstein` builds the expected SPARQL query.
- Search lookup adds a bounded `LIMIT`.
- Raw SPARQL passthrough sends the submitted query without semantic rewriting.
- Origin timeouts and non-2xx responses map to typed application errors.

**Implementation:**

- Add an `origin` module with a `DbpediaClient` trait.
- Implement a `ReqwestDbpediaClient`.
- Add request timeout configuration.
- Add response-size protection for origin responses.
- Keep DBpedia-specific query construction isolated from HTTP handlers.

**Verification:**

```bash
cargo fmt
cargo test origin
```

**Atomic commits:**

```bash
git commit -m "test: specify dbpedia query construction"
git commit -m "feat: add dbpedia origin client"
git commit -m "feat: bound origin timeouts and response size"
```

## Milestone 3: REST API Without Cache

**Goal:** Expose the MVP routes with mocked origin behavior.

**Tests first:**

- `GET /v1/entity/{id}` returns normalized JSON from the origin client.
- `GET /v1/search?q=...` validates required query input.
- `POST /v1/sparql` validates a JSON body containing `query`.
- Handler tests verify correct HTTP status codes for invalid input and origin
  errors.

**Implementation:**

- Add route handlers for:
  - `GET /v1/entity/{id}`
  - `GET /v1/search?q=`
  - `POST /v1/sparql`
  - `GET /v1/health`
- Define response envelopes with fields for `data`, `cached`, `stale`, and
  `source`.
- Return JSON errors with stable error codes.

**Verification:**

```bash
cargo fmt
cargo test http
```

**Atomic commits:**

```bash
git commit -m "test: cover public rest api behavior"
git commit -m "feat: add entity search and sparql routes"
git commit -m "feat: return stable json error responses"
```

## Milestone 4: Redis Cache With TTL

**Goal:** Add read-through caching for all public lookup routes.

**Tests first:**

- Cache hit returns data without calling the origin client.
- Cache miss calls origin once, stores the response, and returns `cached:
  false`.
- Stored entries include payload, created timestamp, last successful refresh
  timestamp, and hit count.
- TTL is applied to fresh cache writes.

**Implementation:**

- Add a `cache` module with a mockable trait.
- Implement Redis-backed cache storage.
- Serialize cached responses as JSON.
- Add cache metadata needed for stale fallback later.
- Add Redis configuration through `REDIS_URL` and `CACHE_TTL_SECONDS`.

**Verification:**

```bash
cargo fmt
cargo test cache
docker compose up redis -d
cargo test --test redis_integration
```

**Atomic commits:**

```bash
git commit -m "test: specify read-through cache behavior"
git commit -m "feat: add redis cache adapter"
git commit -m "feat: wire cache into api routes"
```

## Milestone 5: Docker Compose Runtime

**Goal:** Make the service runnable locally with Redis and PostgreSQL.

**Tests first:**

- Add a Compose smoke script or integration test that waits for the API and
  checks `/v1/health`.
- Add a test that verifies the app fails fast when required runtime
  dependencies are unreachable in production mode.

**Implementation:**

- Add `app/Dockerfile` using a multi-stage Rust build.
- Add `docker-compose.yml` with `app`, `redis`, and `postgres`.
- Add Redis memory cap: `128mb` with `allkeys-lru`.
- Add Postgres tuning for small instance memory.
- Add `.env.example` with `POSTGRES_PASSWORD`, `DATABASE_URL`, and
  `REDIS_URL`.

**Verification:**

```bash
cargo fmt
cargo test
docker compose up --build -d
curl -fsS http://127.0.0.1:8080/v1/health
docker compose down
```

**Atomic commits:**

```bash
git commit -m "test: add local runtime smoke coverage"
git commit -m "chore: add docker compose runtime"
git commit -m "chore: add production dockerfile"
```

## Milestone 6: Outbound Concurrency Limit

**Goal:** Protect DBpedia by allowing only two concurrent origin calls.

**Tests first:**

- More than two simultaneous cache misses queue instead of creating more than
  two origin calls at once.
- Cache hits do not acquire outbound permits.
- Permit release is guaranteed on origin success, error, and timeout.

**Implementation:**

- Add a `tokio::sync::Semaphore` around origin calls.
- Configure permits with `MAX_OUTBOUND_CONCURRENCY`, defaulting to `2`.
- Expose current permit pressure in `/v1/health`.

**Verification:**

```bash
cargo fmt
cargo test limiter
cargo test http
```

**Atomic commits:**

```bash
git commit -m "test: specify outbound concurrency limit"
git commit -m "feat: guard origin calls with semaphore"
```

## Milestone 7: Circuit Breaker And Stale Fallback

**Goal:** Serve the last good cached answer when DBpedia is unhealthy.

**Tests first:**

- Repeated origin timeouts or 5xx responses move the breaker from closed to
  open.
- While open, requests with stale cached data return that data with `stale:
  true`.
- While open, requests without cached data return a clear unavailable error.
- After the cooldown, a successful half-open request closes the breaker.
- A failed half-open request reopens the breaker.

**Implementation:**

- Add a small three-state breaker: `closed`, `open`, `half_open`.
- Store failure count, opened-at timestamp, and last successful origin call.
- Allow stale cache reads after normal TTL expiry when the breaker is open or
  the origin fails.
- Add breaker state to `/v1/health`.

**Verification:**

```bash
cargo fmt
cargo test circuit_breaker
cargo test stale
```

**Atomic commits:**

```bash
git commit -m "test: specify circuit breaker transitions"
git commit -m "feat: add circuit breaker state machine"
git commit -m "feat: serve stale cache on origin failure"
```

## Milestone 8: Request Logging To PostgreSQL

**Goal:** Persist request logs without slowing the response path.

**Tests first:**

- Each request produces a log event containing timestamp, route, query hash,
  cache-hit flag, stale flag, latency, hashed client identifier, and status
  code.
- Handler responses do not wait on a slow log writer.
- Database insertion maps fields into the expected schema.

**Implementation:**

- Add SQL migration for `request_logs`.
- Add `sqlx` and a Postgres connection pool.
- Add a buffered `tokio::sync::mpsc` channel for async log writes.
- Hash client IPs before storage.
- Add graceful shutdown behavior for the logging task.

**Verification:**

```bash
cargo fmt
cargo test logging
docker compose up postgres -d
cargo test --test postgres_integration
```

**Atomic commits:**

```bash
git commit -m "test: specify request log events"
git commit -m "feat: add request log schema"
git commit -m "feat: write request logs asynchronously"
```

## Milestone 9: Health And Metrics Surface

**Goal:** Make the product metrics visible for demos and operations.

**Tests first:**

- `/v1/health` returns service status, Redis status, Postgres status, breaker
  state, last successful origin call, and outbound limiter pressure.
- Metrics calculations return cache hit rate, stale response rate, origin error
  rate, and latency percentiles for a known fixture set.

**Implementation:**

- Extend `/v1/health`.
- Add `GET /v1/metrics/summary` for review/demo metrics.
- Query Postgres logs for rolling 24-hour and 7-day aggregates.
- Keep the endpoint JSON-first; no UI is required for MVP.

**Verification:**

```bash
cargo fmt
cargo test metrics
docker compose up --build -d
curl -fsS http://127.0.0.1:8080/v1/health
curl -fsS http://127.0.0.1:8080/v1/metrics/summary
```

**Atomic commits:**

```bash
git commit -m "test: specify health and metrics output"
git commit -m "feat: expose health dependency checks"
git commit -m "feat: add metrics summary endpoint"
```

## Milestone 10: Nginx Edge And Rate Limiting

**Goal:** Add the production edge configuration.

**Tests first:**

- Add config validation using `nginx -t` in the container.
- Add a smoke test or documented manual check that confirms requests proxy from
  Nginx to the app.

**Implementation:**

- Add `nginx/conf.d/kgproxy.conf`.
- Configure HTTP redirect and ACME challenge path.
- Configure HTTPS server block template.
- Add per-IP `limit_req` policy.
- Document Certbot issuance and renewal commands.

**Verification:**

```bash
docker compose config
docker compose up nginx -d
docker compose exec nginx nginx -t
```

**Atomic commits:**

```bash
git commit -m "test: validate nginx configuration"
git commit -m "chore: add nginx reverse proxy config"
git commit -m "docs: document tls issuance and renewal"
```

## Milestone 11: Cache Warmer Worker

**Goal:** Refresh popular entries before users hit cold-cache latency.

**Tests first:**

- Worker selects top-K entity cache keys from request logs.
- Worker skips recently refreshed keys.
- Worker refreshes eligible entries through the same origin limiter.
- Worker records refresh successes and failures.

**Implementation:**

- Add a separate Rust binary or scheduled background task.
- Query Postgres for popular entity lookups.
- Refresh entries before TTL expiry.
- Reuse cache key, origin, limiter, and cache modules.

**Verification:**

```bash
cargo fmt
cargo test warmer
```

**Atomic commits:**

```bash
git commit -m "test: specify cache warmer selection"
git commit -m "feat: add popular entity cache warmer"
```

## Milestone 12: End-To-End MVP Demo

**Goal:** Prove the complete flow works locally and is ready to deploy.

**Tests first:**

- Add an end-to-end test script that:
  - starts Compose,
  - calls an entity endpoint twice,
  - proves the second call is cached,
  - calls raw SPARQL,
  - checks health and metrics,
  - shuts Compose down.

**Implementation:**

- Add `scripts/e2e-smoke.sh`.
- Add sample `curl` requests to `README.md`.
- Add troubleshooting notes for DBpedia timeouts, Redis connection failures,
  Postgres migrations, and Nginx TLS setup.

**Verification:**

```bash
cargo fmt
cargo test
docker compose up --build -d
scripts/e2e-smoke.sh
docker compose down
```

**Atomic commits:**

```bash
git commit -m "test: add end-to-end mvp smoke script"
git commit -m "docs: add local demo workflow"
```

## Milestone 13: AWS Deployment Runbook

**Goal:** Capture the exact steps needed to deploy on the planned EC2 instance.

**Tests first:**

- Treat this as documentation-driven verification:
  - every command is copyable,
  - every required environment variable is listed,
  - rollback and backup steps are present,
  - cost traps are called out.

**Implementation:**

- Add `docs/deploy-aws.md`.
- Include EC2 setup, security group rules, Docker installation, Compose startup,
  Nginx, Certbot, log retention, backups, and budget alerts.
- Include daily `pg_dump` backup guidance.
- Include Redis and Postgres memory settings from the capacity plan.

**Verification:**

```bash
cargo test
docker compose config
```

**Atomic commits:**

```bash
git commit -m "docs: add aws deployment runbook"
git commit -m "docs: add backup and cost controls"
```

## Commit Discipline Checklist

Use this checklist before every commit:

- The commit contains one behavior or one coherent infrastructure change.
- Tests fail without the implementation and pass with it.
- `cargo fmt` has run.
- Relevant tests have run and their command is recorded in the commit notes or
  PR description.
- No `.env`, secrets, database files, Redis dump files, or build artifacts are
  staged.
- The commit message uses an intentional prefix: `test`, `feat`, `fix`,
  `chore`, or `docs`.

## Definition Of MVP Done

The MVP is done when:

- `GET /v1/entity/{id}` works through Redis with TTL caching.
- `POST /v1/sparql` works through the same cache path.
- Cache hits return in the local fast path without origin calls.
- Origin calls are limited to two concurrent requests.
- Circuit breaker and stale fallback are covered by tests.
- Request logs are persisted to Postgres asynchronously.
- `/v1/health` reports cache, database, breaker, and origin status.
- Docker Compose can start the whole stack locally.
- Nginx configuration validates.
- The end-to-end smoke script demonstrates miss, hit, health, and metrics.
