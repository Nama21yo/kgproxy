# Deploy KGProxy to AWS EC2

This guide explains how to deploy the current KGProxy application from an AWS
account to one Ubuntu EC2 instance. Follow the sections in order. The result is:

~~~text
Internet → HTTPS/Nginx → KGProxy Rust app → Redis/PostgreSQL
                                      └→ DBpedia SPARQL
~~~

This is the manual first-deployment guide. It uses one EC2 instance, Docker
Compose, Nginx, and Let's Encrypt. After this guide is completed, configure
automatic GitHub Actions deployment with
[`docs/ci-cd-aws.md`](ci-cd-aws.md).

## What you need before starting

Prepare these items:

- An AWS account with permission to create EC2, networking, and Elastic IP
  resources.
- The GitHub repository URL:
  https://github.com/Nama21yo/kgproxy.git
- The registered domain `kgproxy.io`.
- An email address for Let's Encrypt certificate notifications.
- A computer with an SSH client. Linux and macOS already include one; Windows
  can use Windows Terminal, PowerShell, or WSL.

Keep the following values nearby. Replace the example domain everywhere below
with your real hostname.

~~~text
AWS region:       us-east-1
EC2 name:         kgproxy-production
Instance type:    t3.micro
Ubuntu version:   Ubuntu Server 24.04 LTS
Domain:           kgproxy.io
Install folder:   /opt/kgproxy
~~~

## 1. Select the AWS region

1. Open the AWS Console: https://console.aws.amazon.com/
2. Sign in to your AWS account.
3. In the top-right region selector, choose US East (N. Virginia),
   us-east-1.

Use the same region for the EC2 instance and Elastic IP. AWS resources are
regional, so selecting a different region later can make them appear to be
missing.

## 2. Create the EC2 instance

### 2.1 Open the launch page

1. In the AWS search bar, type EC2 and open the EC2 console.
2. In the left menu, select Instances.
3. Click Launch instances.

### 2.2 Configure the instance

Set the fields as follows:

- Name: kgproxy-production
- Application and OS Images: select Ubuntu.
- Choose Ubuntu Server 24.04 LTS for a 64-bit x86 architecture.
- Instance type: t3.micro.

### 2.3 Create or select an SSH key pair

Under Key pair (login):

1. Click Create new key pair if you do not already have one.
2. Name it something recognizable, such as kgproxy-production-key.
3. Keep RSA and choose .pem as the private key format.
4. Click Create key pair.
5. Save the downloaded .pem file somewhere safe. AWS will not let you
   download the same private key again.

If you already have an EC2 key pair, select it instead. You must have the
matching private key to connect to the server.

### 2.4 Configure networking and the security group

In Network settings:

1. Leave the default VPC and subnet selected unless your AWS account already
   has a specific network design.
2. Ensure Auto-assign public IP is enabled.
3. Under Firewall, choose Create security group.
4. Give it a name such as kgproxy-production-sg.
5. Add these inbound rules:

   | Type | Port | Source | Purpose |
   | --- | ---: | --- | --- |
   | SSH | 22 | My IP | Administrative SSH access |
   | HTTP | 80 | 0.0.0.0/0 | HTTP and Let's Encrypt challenges |
   | HTTPS | 443 | 0.0.0.0/0 | Public encrypted traffic |

Do not add inbound rules for ports 8080, 6379, or 5432. The application,
Redis, and PostgreSQL must not be publicly reachable. The default outbound
rule can remain enabled because the server needs to download packages,
Docker images, certificates, and DBpedia responses.

### 2.5 Configure storage and launch

1. Set the root volume to 20 GiB.
2. Keep the volume type as gp3.
3. Leave the remaining advanced settings at their defaults for this MVP.
4. Review the summary on the right.
5. Click Launch instance.

Wait until the instance state is Running and its status checks show 2/2
checks passed.

## 3. Allocate and attach a stable Elastic IP

Without an Elastic IP, the public IP can change when the instance is stopped
and started. Attach one before configuring DNS.

1. In the EC2 console left menu, open Network & Security → Elastic IPs.
2. Click Allocate Elastic IP address.
3. Leave the default scope: IPv4, Amazon's pool of IPv4 addresses.
4. Click Allocate.
5. Select the new Elastic IP.
6. Choose Actions → Associate Elastic IP address.
7. For Resource type, select Instance.
8. Select kgproxy-production.
9. Click Associate.

Record the Elastic IP address. You will use it for DNS and troubleshooting.
Keep it associated with the running instance; AWS may charge for an unused
Elastic IP.

## 4. Point the domain to the instance

Create an IPv4 A record at your DNS provider:

~~~text
Name/host:  @                    (the root domain)
Type:      A
Value:     <your Elastic IP>
TTL:       300 or the provider default
~~~

This should make `kgproxy.io` point to the EC2 Elastic IP.

If your DNS is hosted in Route 53:

1. Open Route 53 in the AWS Console.
2. Select Hosted zones.
3. Open your domain's hosted zone.
4. Click Create record.
5. Enter the subdomain in Record name.
6. Choose A as the record type.
7. Enter the Elastic IP as the value.
8. Click Create records.

Wait for DNS to resolve before requesting the certificate. From your own
computer, check it with:

~~~bash
nslookup kgproxy.io
~~~

The result should contain the Elastic IP address.

## 5. Connect to the EC2 server

On Linux or macOS, protect the downloaded key and connect:

~~~bash
chmod 400 /path/to/kgproxy-production-key.pem
ssh -i /path/to/kgproxy-production-key.pem ubuntu@kgproxy.io
~~~

You can use the Elastic IP instead of the hostname if DNS is not ready:

~~~bash
ssh -i /path/to/kgproxy-production-key.pem ubuntu@<your-elastic-ip>
~~~

On Windows PowerShell, use the same ssh command. If Windows reports a key
permission error, keep the key in your user profile and restrict its access
to your Windows user account.

All commands in the remaining sections run inside the SSH session unless
explicitly stated otherwise.

## 6. Install system tools, Docker, and Bun

Update Ubuntu and install the basic tools:

~~~bash
sudo apt update
sudo apt upgrade -y
sudo apt install -y ca-certificates curl git openssl gnupg
~~~

Install Docker from Docker's official Ubuntu repository:

~~~bash
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
~~~

Log out and reconnect so the Docker group change takes effect:

~~~bash
exit
ssh -i /path/to/kgproxy-production-key.pem ubuntu@kgproxy.io
~~~

Confirm Docker works without sudo:

~~~bash
docker --version
docker compose version
~~~

Install Bun. Bun is required because the dashboard is built from the
frontend/ directory and its generated files are served by Nginx.

~~~bash
curl -fsSL https://bun.sh/install | bash
export BUN_INSTALL="$HOME/.bun"
export PATH="$BUN_INSTALL/bin:$PATH"
bun --version
~~~

If a new SSH session does not recognize bun, run the two export commands
again or add them to ~/.bashrc.

## 7. Download the application and create the production environment file

Create the application directory and clone the repository:

~~~bash
sudo mkdir -p /opt/kgproxy
sudo chown "$USER":"$USER" /opt/kgproxy
git clone https://github.com/Nama21yo/kgproxy.git /opt/kgproxy
cd /opt/kgproxy
~~~

Create the private production environment file:

~~~bash
cp .env.example .env
openssl rand -base64 32
nano .env
~~~

Replace the contents of .env with these values. Replace the password with the
value printed by openssl rand -base64 32 and do not commit this file:

~~~dotenv
POSTGRES_PASSWORD=replace-with-the-generated-password
NGINX_HTTP_PORT=80
CACHE_WARMER_ENABLED=false
CACHE_WARMER_INTERVAL_SECONDS=3600
CACHE_WARMER_TOP_K=25
~~~

Save in nano with Ctrl+O, press Enter, then exit with Ctrl+X.

The Compose file supplies the internal application values automatically:

~~~text
BIND_ADDR=0.0.0.0:8080
REDIS_URL=redis://redis:6379/0
DATABASE_URL=postgres://kgproxy:<your-password>@postgres:5432/kgproxy
DBPEDIA_SPARQL_URL=https://dbpedia.org/sparql
~~~

Do not put the production password in GitHub, the frontend, or a public
document.

## 8. Build the frontend dashboard

The dashboard is not built into a Docker image. Nginx serves the generated
frontend/dist directory, so this step is required on the EC2 server:

~~~bash
cd /opt/kgproxy/frontend
bun install
bun run build
cd /opt/kgproxy
~~~

The build should finish by creating frontend/dist/index.html.

## 9. Start KGProxy over HTTP first

Do not create nginx/conf.d/production.conf yet. Nginx cannot start with the
TLS configuration until Let's Encrypt has created the certificate files.

Start the app, Redis, PostgreSQL, and Nginx:

~~~bash
cd /opt/kgproxy
docker compose -f docker-compose.yml -f docker-compose.prod.yml up --build -d app redis postgres nginx
~~~

Check that all four services are running:

~~~bash
docker compose -f docker-compose.yml -f docker-compose.prod.yml ps
~~~

Check the Nginx configuration and application health:

~~~bash
docker compose -f docker-compose.yml -f docker-compose.prod.yml exec nginx nginx -t
curl -fsS http://kgproxy.io/v1/health
~~~

The health request should return JSON. If it fails, inspect the logs before
continuing:

~~~bash
docker compose -f docker-compose.yml -f docker-compose.prod.yml logs --tail=100 app nginx postgres redis
~~~

## 10. Request the Let's Encrypt certificate

The certificate request must use the same hostname that appears in DNS and in
the Nginx configuration. Replace the example hostname and email below.

~~~bash
docker compose --profile tls run --rm certbot certonly \
  --webroot \
  -w /var/www/certbot \
  -d kgproxy.io \
  --email you@example.com \
  --agree-tos \
  --no-eff-email
~~~

Confirm that certificate files were created:

~~~bash
find /opt/kgproxy/certbot/conf/live -maxdepth 2 -type f
~~~

If Certbot reports that the HTTP challenge failed, check that:

- The DNS A record points to the Elastic IP.
- The EC2 security group allows inbound port 80 from the internet.
- The HTTP stack from the previous step is still running.
- The hostname in the command is exactly the hostname in DNS.

## 11. Enable HTTPS in Nginx

Copy the production Nginx template and replace its placeholder hostname with
your real hostname:

~~~bash
cd /opt/kgproxy
cp nginx/conf.d/production.conf.example nginx/conf.d/production.conf
~~~

Now edit the file:

~~~bash
nano nginx/conf.d/production.conf
~~~

The template contains a placeholder hostname. Replace every occurrence of
`kgproxy.example.com` with `kgproxy.io`. If you change the domain later,
replace `kgproxy.io` with the new hostname instead.
Save with Ctrl+O, press Enter, and exit with Ctrl+X.

Restart Nginx using the production Compose override:

~~~bash
docker compose -f docker-compose.yml -f docker-compose.prod.yml exec nginx nginx -t
docker compose -f docker-compose.yml -f docker-compose.prod.yml restart nginx
~~~

Verify the complete public path:

~~~bash
curl -fsS https://kgproxy.io/v1/health
curl -fsS https://kgproxy.io/dashboard/
curl -I http://kgproxy.io/v1/health
~~~

Expected results:

- HTTPS health returns KGProxy JSON.
- HTTPS /dashboard/ returns the dashboard HTML.
- HTTP returns a 301 or 308 redirect to HTTPS.

## 12. Run the end-to-end verification

Run the smoke test through the public HTTPS hostname, not the local
development port 8081:

~~~bash
cd /opt/kgproxy
BASE_URL=https://kgproxy.io \
DASHBOARD_URL=https://kgproxy.io \
scripts/e2e-smoke.sh
~~~

The script checks health, entity lookup, cache behavior, SPARQL, metrics, and
the dashboard. A successful run ends with:

~~~text
E2E smoke passed
~~~

## 13. Configure certificate renewal

Let's Encrypt certificates expire after 90 days. Add a host cron entry:

~~~bash
crontab -e
~~~

Add this line:

~~~cron
0 3 * * * cd /opt/kgproxy && docker compose --profile tls run --rm certbot renew --quiet && docker compose -f docker-compose.yml -f docker-compose.prod.yml restart nginx
~~~

You can test renewal without changing the live certificate:

~~~bash
cd /opt/kgproxy
docker compose --profile tls run --rm certbot renew --dry-run
~~~

## 14. Configure PostgreSQL backups

Request logs are analytics data, but backups preserve dashboard history and
make recovery possible.

Create a backup directory:

~~~bash
mkdir -p /opt/kgproxy/backups
~~~

Open the user crontab:

~~~bash
crontab -e
~~~

Add these two lines:

~~~cron
15 3 * * * cd /opt/kgproxy && docker compose exec -T postgres pg_dump -U kgproxy kgproxy | gzip > /opt/kgproxy/backups/kgproxy-$(date +\%F).sql.gz
30 3 * * * find /opt/kgproxy/backups -type f -name 'kgproxy-*.sql.gz' -mtime +14 -delete
~~~

To restore a backup later:

~~~bash
cd /opt/kgproxy
gunzip -c /opt/kgproxy/backups/kgproxy-YYYY-MM-DD.sql.gz | \
  docker compose exec -T postgres psql -U kgproxy -d kgproxy
~~~

## 15. Useful operations commands

Run these from /opt/kgproxy:

~~~bash
docker compose -f docker-compose.yml -f docker-compose.prod.yml ps
docker compose -f docker-compose.yml -f docker-compose.prod.yml logs -f app
docker stats
curl -fsS https://kgproxy.io/v1/health
curl -fsS https://kgproxy.io/v1/metrics/summary
curl -fsS https://kgproxy.io/dashboard/
~~~

## 16. Manually deploy a later change

The current repository does not automatically update EC2. After changes are
merged into main, connect to EC2 and run:

~~~bash
cd /opt/kgproxy
git pull --ff-only

cd frontend
bun install
bun run build
cd ..

docker compose -f docker-compose.yml -f docker-compose.prod.yml up --build -d

BASE_URL=https://kgproxy.io \
DASHBOARD_URL=https://kgproxy.io \
scripts/e2e-smoke.sh
~~~

The frontend must be rebuilt because Nginx serves frontend/dist. The backend
is rebuilt by Docker's --build option. Database migrations run automatically
when the application starts.

## 17. Roll back a bad deployment

First find recent commits:

~~~bash
cd /opt/kgproxy
git log --oneline -5
~~~

Stop on the last known-good commit, rebuild both parts, and verify:

~~~bash
git switch --detach <previous-good-commit>

cd frontend
bun install
bun run build
cd ..

docker compose -f docker-compose.yml -f docker-compose.prod.yml up --build -d

BASE_URL=https://kgproxy.io \
DASHBOARD_URL=https://kgproxy.io \
scripts/e2e-smoke.sh
~~~

After the incident, return to the main branch:

~~~bash
git switch main
git pull --ff-only
~~~

Do not delete the PostgreSQL or Redis Docker volumes during a rollback. They
contain application data.

## 18. AWS cost and security checklist

- Create an AWS Budget alert in Billing and Cost Management → Budgets.
- Keep the Elastic IP attached to the running instance.
- Do not create a NAT Gateway for this public-subnet MVP.
- Do not expose ports 8080, 6379, or 5432 in the security group.
- Keep .env, nginx/conf.d/production.conf, and certificate files out of Git.
- Keep Redis at 128mb with allkeys-lru.
- Keep PostgreSQL tuned with shared_buffers=64MB, max_connections=20, and
  work_mem=4MB for the t3.micro.
- Set CloudWatch log retention if logs are shipped there.

## Final completion checklist

- [ ] EC2 instance is running Ubuntu 24.04 in us-east-1.
- [ ] Elastic IP is attached to the instance.
- [ ] DNS points the chosen hostname to the Elastic IP.
- [ ] Security group allows only SSH from the admin IP and public HTTP/HTTPS.
- [ ] Docker and Docker Compose work without sudo.
- [ ] Bun is installed and frontend/dist/index.html exists.
- [ ] Compose shows app, Redis, PostgreSQL, and Nginx running.
- [ ] HTTP health works before TLS.
- [ ] Certbot created the certificate in certbot/conf.
- [ ] Nginx configuration test passes after HTTPS is enabled.
- [ ] HTTPS health and /dashboard/ work.
- [ ] HTTP redirects to HTTPS.
- [ ] The HTTPS end-to-end smoke test passes.
- [ ] Certificate renewal cron is installed and dry-run tested.
- [ ] PostgreSQL backup cron is installed.
- [ ] AWS Budget alert is configured.
