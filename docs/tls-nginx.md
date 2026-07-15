# Production Nginx TLS

This runbook enables HTTPS for KGProxy without changing the local development
flow. Local development keeps using Nginx on `http://127.0.0.1:8081`.
Production uses the same Compose stack with:

```bash
NGINX_HTTP_PORT=80 docker compose -f docker-compose.yml -f docker-compose.prod.yml up -d
```

Replace `kgproxy.example.com` in the examples with the real API hostname.

## Files

- `nginx/conf.d/default.conf`: local HTTP proxy and ACME challenge path.
- `nginx/conf.d/production.conf.example`: production HTTP redirect and HTTPS
  proxy template.
- `docker-compose.prod.yml`: adds host port `443` and mounts certificates.
- `certbot/conf`: local certificate storage, ignored by Git.
- `certbot/www`: ACME webroot challenge storage, ignored by Git.

## Prerequisites

- DNS `A` record points `kgproxy.example.com` to the server public IP.
- Security group allows:
  - `80/tcp` from the internet
  - `443/tcp` from the internet
  - `22/tcp` from the admin IP only
- `.env` contains:

```bash
NGINX_HTTP_PORT=80
POSTGRES_PASSWORD=replace-with-generated-password
```

## First-Time Certificate Issue

Start the stack with HTTP only. Do not enable `production.conf` before the
certificate files exist, because Nginx will fail to start if the configured TLS
paths are missing.

```bash
docker compose -f docker-compose.yml -f docker-compose.prod.yml up --build -d app redis postgres nginx
```

Verify that HTTP reaches Nginx and the app:

```bash
curl -fsS http://kgproxy.example.com/v1/health
```

Issue the certificate with the Certbot service:

```bash
docker compose --profile tls run --rm certbot certonly \
  --webroot \
  -w /var/www/certbot \
  -d kgproxy.example.com \
  --email you@example.com \
  --agree-tos \
  --no-eff-email
```

Enable the production TLS config:

```bash
cp nginx/conf.d/production.conf.example nginx/conf.d/production.conf
sed -i 's/kgproxy.example.com/your-real-domain.example/g' nginx/conf.d/production.conf
docker compose -f docker-compose.yml -f docker-compose.prod.yml restart nginx
```

Validate the Nginx configuration:

```bash
docker compose -f docker-compose.yml -f docker-compose.prod.yml exec nginx nginx -t
```

Verify HTTPS:

```bash
curl -fsS https://kgproxy.example.com/v1/health
curl -I http://kgproxy.example.com/v1/health
```

Expected result:

- HTTPS returns the KGProxy health JSON.
- HTTP returns a redirect to HTTPS, except for the ACME challenge path.

## Renewal

Let's Encrypt certificates expire every 90 days. Add a host cron entry:

```cron
0 3 * * * cd /opt/kgproxy && docker compose --profile tls run --rm certbot renew --quiet && docker compose -f docker-compose.yml -f docker-compose.prod.yml restart nginx
```

Manual renewal test:

```bash
docker compose --profile tls run --rm certbot renew --dry-run
```

## Notes

- Keep `nginx/conf.d/production.conf` uncommitted if it contains the real
  hostname.
- Certificate material under `certbot/conf` is ignored by Git.
- Local development still works with:

```bash
docker compose up --build -d
curl -fsS http://127.0.0.1:8081/v1/health
```
