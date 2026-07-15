#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:8080}"
DASHBOARD_URL="${DASHBOARD_URL:-http://127.0.0.1:8081}"
ENTITY_ID="${ENTITY_ID:-Albert_Einstein}"
COMPOSE_STARTED=0

cleanup() {
  if [[ "${COMPOSE_STARTED}" == "1" ]]; then
    docker compose down
  fi
}
trap cleanup EXIT

if ! docker compose ps --services --status running | grep -qx app; then
  COMPOSE_STARTED=1
  docker compose up --build -d
fi

wait_for_health() {
  for _ in $(seq 1 60); do
    if curl -fsS "${BASE_URL}/v1/health" >/dev/null; then
      return 0
    fi
    sleep 1
  done

  echo "KGProxy did not become healthy at ${BASE_URL}/v1/health" >&2
  return 1
}

assert_contains() {
  local file="$1"
  local expected="$2"
  local label="$3"

  if ! grep -q "${expected}" "${file}"; then
    echo "Expected ${label} response to contain ${expected}" >&2
    echo "Response body:" >&2
    cat "${file}" >&2
    return 1
  fi
}

wait_for_health

first_entity="$(mktemp)"
second_entity="$(mktemp)"
sparql_response="$(mktemp)"
metrics_response="$(mktemp)"
dashboard_response="$(mktemp)"

curl -fsS "${BASE_URL}/v1/entity/${ENTITY_ID}" >"${first_entity}"
assert_contains "${first_entity}" '"cached":' "first entity"

curl -fsS "${BASE_URL}/v1/entity/${ENTITY_ID}" >"${second_entity}"
assert_contains "${second_entity}" '"cached":true' "second entity"

curl -fsS \
  -H 'content-type: application/json' \
  -d '{"query":"SELECT * WHERE { ?s ?p ?o } LIMIT 1"}' \
  "${BASE_URL}/v1/sparql" >"${sparql_response}"
assert_contains "${sparql_response}" '"source":' "sparql"

curl -fsS "${BASE_URL}/v1/metrics/summary" >"${metrics_response}"
assert_contains "${metrics_response}" '"total_requests":' "metrics"

curl -fsS "${DASHBOARD_URL}/dashboard/" >"${dashboard_response}"
assert_contains "${dashboard_response}" 'KGProxy Dashboard' "dashboard"

echo "E2E smoke passed: health, entity miss, entity hit, SPARQL passthrough, metrics, and dashboard all responded."
