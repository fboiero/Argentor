# Deploy Argentor to the Cloud

> Docker build, local Compose dev stack, AWS ECS/Fargate, GCP Cloud Run, and Azure Container Apps — with health checks and monitoring.

For advanced topics (Kubernetes, Helm, mTLS, PostgreSQL-backed sessions) see [Tutorial 9: Production Deployment](./09-deployment.md) and [DEPLOYMENT.md](../DEPLOYMENT.md).

---

## Prerequisites

- Docker 24+ installed locally
- Cloud CLI installed: `aws`, `gcloud`, or `az`
- An LLM API key (`ANTHROPIC_API_KEY` or equivalent)

---

## 1. Build the Docker image

Argentor ships a multi-stage Dockerfile that produces a ~60 MB runtime image:

```bash
git clone https://github.com/fboiero/Agentor.git
cd Agentor
docker build -t argentor:local .
```

Verify it starts:

```bash
docker run --rm \
  -e ANTHROPIC_API_KEY="sk-ant-..." \
  -p 8080:8080 \
  argentor:local serve

curl http://localhost:8080/health
# {"status":"healthy","version":"1.2.0","uptime_seconds":3}
```

Or pull the pre-built image:

```bash
docker pull ghcr.io/fboiero/argentor:latest
```

---

## 2. Local dev with Docker Compose

`docker-compose.production.yml` in the repo root starts the full stack:

```bash
ANTHROPIC_API_KEY="sk-ant-..." \
docker compose -f docker-compose.production.yml up -d
```

| Service     | Port        | Purpose                         |
|-------------|-------------|---------------------------------|
| argentor    | 8080, 9090  | Gateway + metrics               |
| postgres    | 5432        | Session and audit storage       |
| redis       | 6379        | Distributed rate-limit counters |
| prometheus  | 9091        | Metrics scraping                |
| grafana     | 3001        | Pre-built Argentor dashboard    |

Open the dashboard: `http://localhost:8080/dashboard`
Open Grafana: `http://localhost:3001` (admin / admin)

Stop everything:

```bash
docker compose -f docker-compose.production.yml down
```

---

## 3. Environment variables

| Variable               | Required | Default    | Description                                  |
|------------------------|----------|------------|----------------------------------------------|
| `ANTHROPIC_API_KEY`    | yes*     | —          | Claude API key (* or another provider key)   |
| `OPENAI_API_KEY`       | yes*     | —          | OpenAI key (if using OpenAI provider)        |
| `GEMINI_API_KEY`       | yes*     | —          | Gemini key (if using Gemini provider)        |
| `ARGENTOR_LOG_LEVEL`   | no       | `info`     | `error`, `warn`, `info`, `debug`, `trace`    |
| `DATABASE_URL`         | no       | in-memory  | Postgres URL for persistent sessions + audit |
| `REDIS_URL`            | no       | in-memory  | Redis URL for distributed rate limits        |
| `JWT_SECRET`           | no       | random     | Shared secret for JWT auth (set in prod)     |
| `ARGENTOR_BIND`        | no       | `0.0.0.0:8080` | Gateway listen address               |
| `ARGENTOR_METRICS_BIND`| no       | `0.0.0.0:9090` | Prometheus metrics listen address    |

Never commit API keys. Pass them via environment, secrets manager, or Docker secrets.

---

## 4. AWS — ECS / Fargate

### Push the image to ECR

```bash
REGION=us-east-1
ACCOUNT=$(aws sts get-caller-identity --query Account --output text)
REPO=${ACCOUNT}.dkr.ecr.${REGION}.amazonaws.com/argentor

aws ecr get-login-password --region $REGION \
  | docker login --username AWS --password-stdin $REPO

docker tag argentor:local $REPO:latest
docker push $REPO:latest
```

### Store the API key in Secrets Manager

```bash
aws secretsmanager create-secret \
  --name /argentor/prod/anthropic-api-key \
  --secret-string "sk-ant-..."
```

### ECS Task Definition (JSON excerpt)

```json
{
  "family": "argentor",
  "networkMode": "awsvpc",
  "requiresCompatibilities": ["FARGATE"],
  "cpu": "1024",
  "memory": "2048",
  "containerDefinitions": [{
    "name": "argentor",
    "image": "ACCOUNT.dkr.ecr.us-east-1.amazonaws.com/argentor:latest",
    "portMappings": [{"containerPort": 8080}],
    "secrets": [{
      "name": "ANTHROPIC_API_KEY",
      "valueFrom": "arn:aws:secretsmanager:us-east-1:ACCOUNT:secret:/argentor/prod/anthropic-api-key"
    }],
    "environment": [
      {"name": "ARGENTOR_LOG_LEVEL", "value": "info"}
    ],
    "healthCheck": {
      "command": ["CMD-SHELL", "curl -f http://localhost:8080/health || exit 1"],
      "interval": 30,
      "timeout": 5,
      "retries": 3,
      "startPeriod": 15
    },
    "logConfiguration": {
      "logDriver": "awslogs",
      "options": {
        "awslogs-group": "/ecs/argentor",
        "awslogs-region": "us-east-1",
        "awslogs-stream-prefix": "ecs"
      }
    }
  }]
}
```

### Create and run the service

```bash
# Create log group
aws logs create-log-group --log-group-name /ecs/argentor --region $REGION

# Register task definition
aws ecs register-task-definition --cli-input-json file://task-def.json

# Create service (assumes a VPC, subnets, and security group exist)
aws ecs create-service \
  --cluster argentor-cluster \
  --service-name argentor \
  --task-definition argentor \
  --desired-count 2 \
  --launch-type FARGATE \
  --network-configuration "awsvpcConfiguration={subnets=[subnet-xxx],securityGroups=[sg-xxx],assignPublicIp=ENABLED}"
```

---

## 5. GCP — Cloud Run

Cloud Run is the simplest option: no cluster to manage, scales to zero.

### Push to Artifact Registry

```bash
PROJECT=my-gcp-project
REGION=us-central1
IMAGE=$REGION-docker.pkg.dev/$PROJECT/argentor/argentor:latest

gcloud auth configure-docker $REGION-docker.pkg.dev
docker tag argentor:local $IMAGE
docker push $IMAGE
```

### Store the API key in Secret Manager

```bash
echo -n "sk-ant-..." | gcloud secrets create anthropic-api-key --data-file=-
```

### Deploy

```bash
gcloud run deploy argentor \
  --image $IMAGE \
  --region $REGION \
  --platform managed \
  --allow-unauthenticated \
  --port 8080 \
  --memory 2Gi \
  --cpu 2 \
  --min-instances 1 \
  --max-instances 10 \
  --set-secrets ANTHROPIC_API_KEY=anthropic-api-key:latest \
  --set-env-vars ARGENTOR_LOG_LEVEL=info
```

Cloud Run runs the health check against `/health` automatically. The service URL is printed after deploy.

---

## 6. Azure — Container Apps

```bash
RESOURCE_GROUP=argentor-rg
LOCATION=eastus
ENVIRONMENT=argentor-env
APP_NAME=argentor

# Create resource group and environment
az group create --name $RESOURCE_GROUP --location $LOCATION
az containerapp env create \
  --name $ENVIRONMENT \
  --resource-group $RESOURCE_GROUP \
  --location $LOCATION

# Store the secret
az containerapp secret set \
  --name $APP_NAME \
  --resource-group $RESOURCE_GROUP \
  --secrets anthropic-key="sk-ant-..."

# Deploy
az containerapp create \
  --name $APP_NAME \
  --resource-group $RESOURCE_GROUP \
  --environment $ENVIRONMENT \
  --image ghcr.io/fboiero/argentor:latest \
  --target-port 8080 \
  --ingress external \
  --min-replicas 1 \
  --max-replicas 10 \
  --cpu 1.0 \
  --memory 2.0Gi \
  --secrets anthropic-key="sk-ant-..." \
  --env-vars ANTHROPIC_API_KEY=secretref:anthropic-key ARGENTOR_LOG_LEVEL=info
```

---

## 7. Health checks and monitoring

All deployments expose the same endpoints:

```
GET /health        — summary: {"status":"healthy","version":"1.2.0"}
GET /health/live   — liveness: process is alive
GET /health/ready  — readiness: LLM reachable, DB connected, not shutting down
GET /metrics       — Prometheus metrics (port 9090 by default)
```

Key metrics to alert on:

| Metric | Threshold |
|--------|-----------|
| `argentor_requests_total{status="5xx"}` | > 1% of requests |
| `argentor_request_duration_seconds{quantile="0.99"}` | > 5s |
| `argentor_circuit_breaker_state` | != 0 (open or half-open) |
| `argentor_guardrail_violations_total{severity="block"}` | any spike |

Add a Prometheus scrape job:

```yaml
scrape_configs:
  - job_name: argentor
    scrape_interval: 15s
    static_configs:
      - targets: ['argentor:9090']
```

---

## Common issues

**Container exits immediately on startup** — the API key env var is missing. Check the container logs: `docker logs argentor` or the cloud provider's log viewer.

**Health check fails** — `/health/ready` returns 503 when the LLM provider is unreachable. Verify network egress from the container to `api.anthropic.com` (or whichever provider) is allowed.

**"429 Too Many Requests" from LLM provider** — you need a `RetryPolicy`. Set it in `argentor.toml` or pass `ARGENTOR_RETRY_MAX_ATTEMPTS=5`.

**Out of memory on Fargate / Cloud Run** — increase the memory allocation to 4 Gi and set `ARGENTOR_CACHE_CAPACITY` to a lower value (default is high for multi-tenant use).

---

## Next steps

- [Tutorial 9: Production Deployment](./09-deployment.md) — Kubernetes, Helm, HPA, PDB, rolling updates
- [Tutorial 10: Observability](./10-observability.md) — OpenTelemetry traces and distributed debugging
- [DEPLOYMENT.md](../DEPLOYMENT.md) — mTLS, multi-region, PostgreSQL schema reference
