# Automatic AWS Deployment with GitHub Actions

This repository deploys automatically to the KGProxy EC2 instance after a
change is pushed to main. Pull requests run verification only. Deployment
uses GitHub Actions, GitHub OpenID Connect (OIDC), and AWS Systems Manager
(SSM); no AWS access key or EC2 private SSH key is stored in GitHub.

The workflow is in .github/workflows/ci-cd.yml. It performs this sequence:

~~~text
Pull request → backend/frontend verification
Push to main → verification → AWS OIDC → SSM → EC2 deployment → smoke test
~~~

The EC2 deployment script is scripts/deploy-ec2.sh. It switches to main,
pulls the latest code, builds the frontend, rebuilds the backend image,
restarts Compose, and runs the HTTPS end-to-end smoke test.

## Before configuring GitHub

The initial manual deployment must already be complete. Confirm on EC2:

~~~bash
cd /opt/kgproxy
test -f .env
test -x /home/ubuntu/.bun/bin/bun
docker compose -f docker-compose.yml -f docker-compose.prod.yml ps
curl -fsS https://kgproxy.io/v1/health
~~~

The EC2 instance must be large enough to build Rust. t3.small or larger is
recommended. The Dockerfile limits Cargo to one build job to reduce memory
pressure on small instances. A swap file may still be needed on t3.micro.

## 1. Attach the SSM role to EC2

In AWS Console:

1. Open IAM → Roles → Create role.
2. Select AWS service as the trusted entity.
3. Select EC2 as the use case.
4. Attach the policy AmazonSSMManagedInstanceCore.
5. Name the role KGProxyEC2SSMRole.
6. Create the role.
7. Open EC2 → Instances and select the KGProxy instance.
8. Choose Actions → Security → Modify IAM role.
9. Select KGProxyEC2SSMRole and save.

The Ubuntu EC2 AMI normally includes the SSM Agent. Check it on the server:

~~~bash
sudo systemctl status amazon-ssm-agent
~~~

The instance must appear as Online under Systems Manager → Fleet Manager →
Managed nodes. The instance needs outbound HTTPS access so the SSM Agent can
reach AWS endpoints.

## 2. Create the GitHub OIDC provider

In AWS Console:

1. Open IAM → Identity providers.
2. Click Add provider.
3. Provider type: OpenID Connect.
4. Provider URL:

~~~text
https://token.actions.githubusercontent.com
~~~

5. Audience:

~~~text
sts.amazonaws.com
~~~

6. Click Add provider.

If this provider already exists, use the existing provider.

## 3. Create the GitHub deployment role

Create an IAM role trusted by the repository's GitHub Actions workflow:

1. Open IAM → Roles → Create role.
2. Select Custom trust policy.
3. Use this trust policy. Replace AWS_ACCOUNT_ID with the AWS account ID:

~~~json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Principal": {
        "Federated": "arn:aws:iam::AWS_ACCOUNT_ID:oidc-provider/token.actions.githubusercontent.com"
      },
      "Action": "sts:AssumeRoleWithWebIdentity",
      "Condition": {
        "StringEquals": {
          "token.actions.githubusercontent.com:aud": "sts.amazonaws.com"
        },
        "StringLike": {
          "token.actions.githubusercontent.com:sub": "repo:Nama21yo/kgproxy:ref:refs/heads/main"
        }
      }
    }
  ]
}
~~~

4. Name the role KGProxyGitHubDeployRole.
5. Create the role.

Add this inline permissions policy to KGProxyGitHubDeployRole under
Permissions → Add permissions → Create inline policy → JSON. Replace
AWS_ACCOUNT_ID and keep the instance ID exactly as shown:

~~~json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": [
        "ssm:SendCommand"
      ],
      "Resource": [
        "arn:aws:ssm:us-east-1::document/AWS-RunShellScript",
        "arn:aws:ec2:us-east-1:AWS_ACCOUNT_ID:instance/i-08488fefebe5f2044"
      ]
    },
    {
      "Effect": "Allow",
      "Action": [
        "ssm:GetCommandInvocation",
        "ssm:ListCommandInvocations",
        "ssm:ListCommands"
      ],
      "Resource": "*"
    }
  ]
}
~~~

Name the policy KGProxyEC2Deployment.

## 4. Add GitHub Actions secrets

In GitHub, open:

Nama21yo/kgproxy → Settings → Secrets and variables → Actions → New
repository secret

Create these secrets:

~~~text
AWS_DEPLOY_ROLE_ARN
arn:aws:iam::AWS_ACCOUNT_ID:role/KGProxyGitHubDeployRole

EC2_INSTANCE_ID
i-08488fefebe5f2044
~~~

No AWS access key, AWS secret key, SSH private key, or database password is
needed in GitHub. The production .env file remains on EC2.

## 5. Configure deployment URLs

In the same **Settings → Secrets and variables → Actions** page, open the
**Variables** tab and create:

~~~text
DEPLOY_BASE_URL
DEPLOY_DASHBOARD_URL
~~~

While the domain and HTTPS are not ready, use the Elastic IP for both values:

~~~text
DEPLOY_BASE_URL=http://44.194.182.146
DEPLOY_DASHBOARD_URL=http://44.194.182.146
~~~

After `kgproxy.io` DNS and Let's Encrypt are working, change both values to:

~~~text
DEPLOY_BASE_URL=https://kgproxy.io
DEPLOY_DASHBOARD_URL=https://kgproxy.io
~~~

## 6. Test the workflow

After the workflow is pushed:

1. Open the repository's Actions tab.
2. Select CI and deploy.
3. Click Run workflow.
4. Select the main branch.
5. Click Run workflow.

The verify job must pass before the deploy job starts. The deployment job uses
SSM to run /opt/kgproxy/scripts/deploy-ec2.sh as the ubuntu user.

For later changes, merge or push to main. GitHub Actions will repeat the same
process automatically. Deployment jobs are serialized so two production
deployments cannot run at the same time.

## Troubleshooting

### SSM says the instance is not online

Check:

~~~bash
sudo systemctl status amazon-ssm-agent
~~~

Then check Systems Manager → Fleet Manager → Managed nodes. The instance
needs outbound HTTPS and the AmazonSSMManagedInstanceCore EC2 role.

### GitHub cannot assume the AWS role

Check:

- The OIDC provider URL is exactly
  https://token.actions.githubusercontent.com.
- The audience is exactly sts.amazonaws.com.
- The trust policy contains repo:Nama21yo/kgproxy:ref:refs/heads/main.
- The workflow has id-token: write permission.
- AWS_DEPLOY_ROLE_ARN contains the correct account ID and role name.

### Deployment fails while compiling Rust

Check memory on EC2:

~~~bash
free -h
sudo dmesg -T | grep -i -E 'out of memory|killed process' | tail
~~~

Use a t3.small or larger instance, or add swap. The Dockerfile already limits
Cargo to one compiler job.

### The smoke test fails after deployment

Check the public endpoints:

~~~bash
curl -fsS https://kgproxy.io/v1/health
curl -fsS https://kgproxy.io/
curl -fsS https://kgproxy.io/dashboard/
~~~

Then inspect the EC2 services:

~~~bash
cd /opt/kgproxy
docker compose -f docker-compose.yml -f docker-compose.prod.yml ps
docker compose -f docker-compose.yml -f docker-compose.prod.yml logs --tail=100 app nginx
~~~
