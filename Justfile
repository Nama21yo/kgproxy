set dotenv-load := true

default:
    @just --list

fmt:
    cd app && cargo fmt

test:
    cd app && cargo test

compose-config:
    docker compose config >/dev/null

smoke-health:
    scripts/smoke-health.sh

e2e:
    scripts/e2e-smoke.sh

verify: fmt test compose-config
    scripts/e2e-smoke.sh

up:
    docker compose up --build -d

down:
    docker compose down

logs service="app":
    docker compose logs -f {{service}}

nginx-test:
    docker compose exec nginx nginx -t
