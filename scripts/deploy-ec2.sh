#!/usr/bin/env bash
set -euo pipefail

APP_DIR="/opt/kgproxy"
BUN_BIN="/home/ubuntu/.bun/bin/bun"
BASE_URL="${DEPLOY_BASE_URL:-https://kgproxy.io}"
DASHBOARD_URL="${DEPLOY_DASHBOARD_URL:-${BASE_URL}}"

exec 9>/tmp/kgproxy-deploy.lock
flock 9

cd "${APP_DIR}"

git switch main
git pull --ff-only origin main

if [[ ! -x "${BUN_BIN}" ]]; then
  echo "Bun was not found at ${BUN_BIN}" >&2
  exit 1
fi

cd frontend
"${BUN_BIN}" install --frozen-lockfile
"${BUN_BIN}" run build
cd "${APP_DIR}"

docker compose -f docker-compose.yml -f docker-compose.prod.yml config >/dev/null
docker compose -f docker-compose.yml -f docker-compose.prod.yml up --build -d
docker compose -f docker-compose.yml -f docker-compose.prod.yml exec -T nginx nginx -t
docker compose -f docker-compose.yml -f docker-compose.prod.yml restart nginx

BASE_URL="${BASE_URL}" \
DASHBOARD_URL="${DASHBOARD_URL}" \
scripts/e2e-smoke.sh
