# KGProxy Demo Notes

This file is intentionally local-only and should not be committed.

## Start From A Clean Checkout

```bash
git status --short --branch
just verify
```

## Manual Demo Path

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

Expected story:

- First entity request is usually an origin miss on a fresh Redis volume:
  `"cached":false`.
- Second entity request is a Redis hit: `"cached":true`.
- Raw SPARQL passthrough returns a JSON envelope.
- Metrics shows recent request counts and cache-hit statistics.
