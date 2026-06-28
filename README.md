# KGProxy

KGProxy is a Rust reliability gateway for DBpedia. It exposes a simple REST API,
adds application-level caching, and protects the public DBpedia SPARQL endpoint
with bounded outbound concurrency.

The current implementation covers the first milestones:

- Rust service scaffold with `GET /v1/health`
- deterministic cache-key generation
- mockable DBpedia origin client and SPARQL query construction

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
