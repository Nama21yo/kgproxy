# AWS Deployment Runbook

This runbook deploys KGProxy on one AWS EC2 `t3.micro` instance in
`us-east-1`, matching the MVP capacity plan.

## Required Inputs

- EC2 instance: Ubuntu 24.04 LTS, `t3.micro`, 20 GB gp3 EBS
- Security group:
  - `22/tcp` from the admin IP only
  - `80/tcp` from `0.0.0.0/0`
  - `443/tcp` from `0.0.0.0/0`
  - no public `6379/tcp` or `5432/tcp`
- DNS `A` record pointing the API hostname to the instance public IP
- GitHub deploy access for `https://github.com/Nama21yo/kgproxy.git`
- Local `.env` file on the server with:
  - `POSTGRES_PASSWORD`

The app service sets these runtime variables in `docker-compose.yml`:

- `BIND_ADDR=0.0.0.0:8080`
- `REDIS_URL=redis://redis:6379/0`
- `DATABASE_URL=postgres://kgproxy:${POSTGRES_PASSWORD}@postgres:5432/kgproxy`
- `DBPEDIA_SPARQL_URL=https://dbpedia.org/sparql`
- `MAX_OUTBOUND_CONCURRENCY=2`
- `CACHE_TTL_SECONDS=604800`
- `ORIGIN_TIMEOUT_MS=2000`
- `MAX_ORIGIN_RESPONSE_BYTES=102400`

## Instance Setup

```bash
sudo apt update
sudo apt upgrade -y
sudo apt install -y ca-certificates curl git nginx certbot
sudo install -m 0755 -d /etc/apt/keyrings
curl -fsSL https://download.docker.com/linux/ubuntu/gpg | \
  sudo gpg --dearmor -o /etc/apt/keyrings/docker.gpg
echo \
  "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu \
  $(. /etc/os-release && echo "$VERSION_CODENAME") stable" | \
  sudo tee /etc/apt/sources.list.d/docker.list >/dev/null
sudo apt update
sudo apt install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
sudo usermod -aG docker "$USER"
```

Log out and back in so the Docker group membership applies.

## App Deployment

```bash
sudo mkdir -p /opt/kgproxy
sudo chown "$USER":"$USER" /opt/kgproxy
git clone https://github.com/Nama21yo/kgproxy.git /opt/kgproxy
cd /opt/kgproxy
cp .env.example .env
openssl rand -base64 32
```

Put the generated value in `.env`:

```bash
POSTGRES_PASSWORD=replace-with-generated-password
```

Start the stack and run the smoke path:

```bash
docker compose up --build -d
scripts/e2e-smoke.sh
```

The first image build is slow on `t3.micro`; later builds reuse Docker layers.

## Nginx And TLS

Local Compose exposes Nginx on `8081` for development, but production should
bind ports `80` and `443`. Update the Compose Nginx port mapping before public
deployment:

```yaml
ports:
  - "80:80"
  - "443:443"
```

Use the template in `docs/tls-nginx.md` for the TLS server block and Certbot
commands. After issuing certificates, validate the edge config:

```bash
docker compose exec nginx nginx -t
curl -fsS https://kgproxy.example.com/v1/health
```

## Operations

Common commands:

```bash
docker compose ps
docker compose logs -f app
docker stats
curl -fsS http://127.0.0.1:8080/v1/health
curl -fsS http://127.0.0.1:8080/v1/metrics/summary
```

Upgrade to the latest `main`:

```bash
cd /opt/kgproxy
git pull --ff-only
docker compose up --build -d
scripts/e2e-smoke.sh
```

Rollback to the previous commit:

```bash
cd /opt/kgproxy
git log --oneline -5
git checkout <previous-good-commit>
docker compose up --build -d
scripts/e2e-smoke.sh
```

Return to `main` after the incident:

```bash
git switch main
```

## Backups

Request logs are analytics data, not the source of truth, but daily backups make
demo and dashboard history recoverable.

Create `/opt/kgproxy/backups`:

```bash
mkdir -p /opt/kgproxy/backups
```

Daily `pg_dump` cron:

```cron
15 3 * * * cd /opt/kgproxy && docker compose exec -T postgres pg_dump -U kgproxy kgproxy | gzip > /opt/kgproxy/backups/kgproxy-$(date +\%F).sql.gz
```

Keep the last 14 local backups:

```cron
30 3 * * * find /opt/kgproxy/backups -type f -name 'kgproxy-*.sql.gz' -mtime +14 -delete
```

Restore a backup:

```bash
gunzip -c /opt/kgproxy/backups/kgproxy-YYYY-MM-DD.sql.gz | \
  docker compose exec -T postgres psql -U kgproxy -d kgproxy
```

## Cost Controls

- Create an AWS Budget alert before leaving the instance running.
- Do not create a NAT Gateway; it is unnecessary for this public-subnet MVP and
  costs more than the instance.
- Keep the Elastic IP attached to the running instance. An unattached Elastic IP
  accrues monthly charges.
- Set CloudWatch log retention if shipping logs there; the default is indefinite
  retention.
- Keep Redis at `128mb` with `allkeys-lru` and Postgres at
  `shared_buffers=64MB`, `max_connections=20`, and `work_mem=4MB` on `t3.micro`.

## Verification Checklist

- `docker compose config` succeeds.
- `scripts/e2e-smoke.sh` passes on the instance.
- `curl -fsS http://127.0.0.1:8080/v1/health` returns JSON.
- `curl -fsS http://127.0.0.1:8080/v1/metrics/summary` returns JSON.
- `docker compose exec nginx nginx -t` passes after TLS configuration.
- Security group exposes only SSH from the admin IP plus public HTTP/HTTPS.
