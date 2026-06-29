# Nginx And TLS

The local Compose runtime starts Nginx on port `8081` and proxies to the app on
the internal Docker network. It validates with:

```bash
docker compose exec nginx nginx -t
```

For production, point DNS at the host and issue a certificate with Certbot
webroot mode:

```bash
docker compose run --rm --entrypoint certbot nginx certonly \
  --webroot -w /var/www/certbot -d kgproxy.example.com
```

After certificates exist, add a `server` block listening on `443 ssl` and mount:

```text
./certbot/conf:/etc/letsencrypt:ro
./certbot/www:/var/www/certbot:ro
```

Keep the existing `limit_req` settings unless traffic targets change. They cap
per-IP edge traffic before requests reach the Rust service.
