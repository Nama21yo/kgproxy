# AWS Deployment Runbook

This runbook deploys KGProxy on one AWS EC2 `t3.micro` instance in `us-east-1`.
The production path uses Docker Compose, Nginx, and Let's Encrypt TLS.

## Required Inputs

- EC2 instance: Ubuntu 24.04 LTS, `t3.micro`, 20 GB gp3 EBS.
- Security group:
  - `22/tcp` from admin IP only.
  - `80/tcp` from `0.0.0.0/0`.
  - `443/tcp` from `0.0.0.0/0`.
  - no public `6379/tcp` or `5432/tcp`.
- DNS `A` record pointing the API hostname to the instance public IP.
- GitHub deploy access to `https://github.com/Nama21yo/kgproxy.git`.
- Local `.env` file on the server.

Required `.env` values:

```bash
POSTGRES_PASSWORD=replace-with-generated-password
NGINX_HTTP_PORT=80
CACHE_WARMER_ENABLED=false
CACHE_WARMER_INTERVAL_SECONDS=3600
CACHE_WARMER_TOP_K=25
```

The app service also sets these runtime defaults in `docker-compose.yml`:

```bash
BIND_ADDR=0.0.0.0:8080
REDIS_URL=redis://redis:6379/0
DATABASE_URL=postgres://kgproxy:${POSTGRES_PASSWORD}@postgres:5432/kgproxy
DBPEDIA_SPARQL_URL=https://dbpedia.org/sparql
MAX_OUTBOUND_CONCURRENCY=2
CACHE_TTL_SECONDS=604800
ORIGIN_TIMEOUT_MS=2000
MAX_ORIGIN_RESPONSE_BYTES=102400
```

## Instance Setup

```bash
sudo apt update
sudo apt upgrade -y
sudo apt install -y ca-certificates curl git
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

Log out and back in so Docker group membership applies.

## App Deployment

```bash
sudo mkdir -p /opt/kgproxy
sudo chown "$USER":"$USER" /opt/kgproxy
git clone https://github.com/Nama21yo/kgproxy.git /opt/kgproxy
cd /opt/kgproxy
cp .env.example .env
openssl rand -base64 32
```

Put the generated password in `.env`:

```bash
POSTGRES_PASSWORD=replace-with-generated-password
NGINX_HTTP_PORT=80
```

Build the frontend dashboard:

```bash
cd frontend
bun install
bun run build
cd ..
```

Start the HTTP-capable production stack:

```bash
docker compose -f docker-compose.yml -f docker-compose.prod.yml up --build -d app redis postgres nginx
```

Verify HTTP before issuing certificates:

```bash
curl -fsS http://kgproxy.example.com/v1/health
docker compose -f docker-compose.yml -f docker-compose.prod.yml exec nginx nginx -t
```

## TLS Setup

Issue the Let's Encrypt certificate with the bundled Certbot service:

```bash
docker compose --profile tls run --rm certbot certonly \
  --webroot \
  -w /var/www/certbot \
  -d kgproxy.example.com \
  --email you@example.com \
  --agree-tos \
  --no-eff-email
```

Enable the production Nginx config:

```bash
cp nginx/conf.d/production.conf.example nginx/conf.d/production.conf
sed -i 's/kgproxy.example.com/your-real-domain.example/g' nginx/conf.d/production.conf
docker compose -f docker-compose.yml -f docker-compose.prod.yml restart nginx
```

Validate HTTPS:

```bash
docker compose -f docker-compose.yml -f docker-compose.prod.yml exec nginx nginx -t
curl -fsS https://kgproxy.example.com/v1/health
curl -fsS https://kgproxy.example.com/dashboard/
curl -I http://kgproxy.example.com/v1/health
```

Expected result: HTTPS returns KGProxy health JSON and HTTP redirects to HTTPS.

Certificate renewal cron:

```cron
0 3 * * * cd /opt/kgproxy && docker compose --profile tls run --rm certbot renew --quiet && docker compose -f docker-compose.yml -f docker-compose.prod.yml restart nginx
```

## Operations

Common commands:

```bash
docker compose -f docker-compose.yml -f docker-compose.prod.yml ps
docker compose -f docker-compose.yml -f docker-compose.prod.yml logs -f app
docker stats
curl -fsS https://kgproxy.example.com/v1/health
curl -fsS https://kgproxy.example.com/v1/metrics/summary
curl -fsS https://kgproxy.example.com/dashboard/
```

Upgrade to the latest `main`:

```bash
cd /opt/kgproxy
git pull --ff-only
docker compose -f docker-compose.yml -f docker-compose.prod.yml up --build -d
scripts/e2e-smoke.sh
```

Rollback to the previous commit:

```bash
cd /opt/kgproxy
git log --oneline -5
git checkout <previous-good-commit>
docker compose -f docker-compose.yml -f docker-compose.prod.yml up --build -d
scripts/e2e-smoke.sh
```

Return to `main` after the incident:

```bash
git switch main
```

## Backups

Request logs are analytics data, not source truth, but daily backups make demo
and dashboard history recoverable.

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
- Do not create a NAT Gateway; the MVP does not need one.
- Keep the Elastic IP attached to the running instance.
- Set CloudWatch log retention if shipping logs there.
- Keep Redis at `128mb` with `allkeys-lru`.
- Keep Postgres tuned with `shared_buffers=64MB`, `max_connections=20`, and
  `work_mem=4MB` on `t3.micro`.

## Verification Checklist

- `docker compose -f docker-compose.yml -f docker-compose.prod.yml config`
  succeeds.
- `curl -fsS http://kgproxy.example.com/v1/health` works before TLS.
- Certbot issues a certificate into `certbot/conf`.
- `docker compose -f docker-compose.yml -f docker-compose.prod.yml exec nginx nginx -t`
  passes after TLS config is enabled.
- `curl -fsS https://kgproxy.example.com/v1/health` returns JSON.
- `curl -fsS https://kgproxy.example.com/dashboard/` returns the dashboard HTML.
- `curl -I http://kgproxy.example.com/v1/health` redirects to HTTPS.
- Security group exposes only SSH from admin IP plus public HTTP/HTTPS.
