# KGProxy

KGProxy is a Rust reliability gateway for DBpedia. It exposes a simple REST API,
adds application-level caching, and protects the public DBpedia SPARQL endpoint
with bounded outbound concurrency.

The current implementation covers:

- Rust service scaffold with `GET /v1/health`
- deterministic cache-key generation
- mockable DBpedia origin client and SPARQL query construction
- public REST routes for entity, search, and raw SPARQL
- Redis read-through caching
- local Docker Compose runtime with app, Redis, and PostgreSQL

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
