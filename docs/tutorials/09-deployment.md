# Tutorial 9: Production Deployment

> Docker, Kubernetes, Helm, health probes, Prometheus metrics, graceful shutdown. Deploy Argentor like a grown-up service.

Running `cargo run` is great for prototyping. Production needs containers, orchestration, health checks, observability, and graceful handling of restarts. This tutorial walks you through the whole deployment stack.

See also [DEPLOYMENT.md](../DEPLOYMENT.md) for the full reference (TLS setup, multi-region, PostgreSQL-backed sessions).

---

## Prerequisites

- Docker + docker-compose installed
- Optional: `kubectl` + access to a Kubernetes cluster (k3d / Minikube / EKS / GKE)
- Optional: `helm` 3.x
- An `ANTHROPIC_API_KEY` or other LLM provider key

---

## 1. Docker Quick Start

Argentor ships a multi-stage Dockerfile that produces a minimal runtime image (~60 MB):

```bash
# Build from source
docker build -t argentor:local .

# Or use the published image
docker pull ghcr.io/fboiero/argentor:latest
```

Run a single-node gateway:

```bash
docker run -d \
  --name argentor \
  -p 8080:8080 \
  -p 9090:9090 \
  -v argentor-data:/var/lib/argentor \
  -e ANTHROPIC_API_KEY="sk-ant-..." \
  -e ARGENTOR_LOG_LEVEL="info" \
  ghcr.io/fboiero/argentor:latest serve
```

Ports:

- `8080` — REST API, WebSocket gateway, dashboard, playground
- `9090` — Prometheus metrics, health endpoints

Verify:

```bash
curl http://localhost:8080/health
# {"status":"healthy","version":"1.0.0","uptime_seconds":42}

curl -X POST http://localhost:8080/api/v1/agent/chat \
  -H "Content-Type: application/json" \
  -d '{"message":"Hello!"}'
```

---

## 2. Docker Compose (Full Stack)

`docker-compose.production.yml` brings up Argentor + PostgreSQL (sessions/audit) + Redis (rate limits) + Prometheus + Grafana:

```bash
docker compose -f docker-compose.production.yml up -d
```

Services:

| Service | Port | Purpose |
|---------|------|---------|
| argentor | 8080, 9090 | Gateway |
| postgres | 5432 | Session + audit storage |
| redis | 6379 | Distributed rate-limit counters |
| prometheus | 9091 | Metrics scraping |
| grafana | 3001 | Pre-built Argentor dashboard |

Security hardening baked in:

```yaml
# excerpt from docker-compose.production.yml
services:
  argentor:
    image: ghcr.io/fboiero/argentor:latest
    read_only: true                       # immutable filesystem
    cap_drop: [ALL]                       # drop all Linux capabilities
    security_opt:
      - no-new-privileges:true
    tmpfs:
      - /tmp                              # only writable mount
    user: "10001:10001"                   # non-root UID
    deploy:
      resources:
        limits:
          cpus: "2"
          memory: "4G"
```

Verify:

```bash
docker compose ps
curl http://localhost:3001   # Grafana login admin/admin
```

---

## 3. Kubernetes Deployment

Argentor ships a Helm chart in `deploy/helm/argentor/`:

```bash
helm install argentor ./deploy/helm/argentor \
  --namespace argentor \
  --create-namespace \
  --set image.tag=latest \
  --set secrets.anthropicApiKey="sk-ant-..." \
  --set ingress.enabled=true \
  --set ingress.hostname=agents.example.com
```

The chart templates include:

- **Deployment** — rolling updates with `maxSurge: 25%`, `maxUnavailable: 0`
- **Service** — `ClusterIP` for the gateway + a separate metrics service
- **Ingress** — TLS terminated via cert-manager integration
- **HPA** — horizontal pod autoscaler on CPU/memory
- **PVC** — persistent volume for audit logs
- **ServiceAccount** + RBAC for pod identity
- **PodDisruptionBudget** — `minAvailable: 1` during rolling updates
- **NetworkPolicy** — default-deny ingress except from listed sources

### values.yaml highlights

```yaml
replicaCount: 3

image:
  repository: ghcr.io/fboiero/argentor
  tag: "latest"
  pullPolicy: IfNotPresent

resources:
  requests:
    cpu: "500m"
    memory: "512Mi"
  limits:
    cpu: "2"
    memory: "2Gi"

autoscaling:
  enabled: true
  minReplicas: 3
  maxReplicas: 20
  targetCPUUtilizationPercentage: 70

persistence:
  enabled: true
  size: 20Gi
  storageClassName: "standard"

ingress:
  enabled: false
  className: "nginx"
  annotations:
    cert-manager.io/cluster-issuer: "letsencrypt-prod"
    nginx.ingress.kubernetes.io/ssl-redirect: "true"
```

Upgrade:

```bash
helm upgrade argentor ./deploy/helm/argentor \
  --namespace argentor \
  --set image.tag=1.1.0 \
  --reuse-values
```

Rollback:

```bash
helm rollback argentor 1 --namespace argentor
```

---

## 4. Health Probes

Argentor exposes two liveness/readiness endpoints:

```
GET /health            — overall health (200 OK or 503)
GET /health/live       — liveness: is the process alive?
GET /health/ready      — readiness: can it serve traffic?
```

`/health/ready` checks:

- LLM provider connectivity (cached 30s)
- Database connection pool healthy (if DB configured)
- Vector store reachable (if remote)
- No graceful-shutdown in progress

Kubernetes probe config (already in Helm chart):

```yaml
livenessProbe:
  httpGet:
    path: /health/live
    port: http
  initialDelaySeconds: 30
  periodSeconds: 10
  failureThreshold: 3

readinessProbe:
  httpGet:
    path: /health/ready
    port: http
  initialDelaySeconds: 5
  periodSeconds: 5
  failureThreshold: 2
```

---

## 5. Prometheus Metrics

The `/metrics` endpoint on port 9090 emits 40+ metrics in Prometheus format:

```
# Request counters
argentor_requests_total{endpoint="/api/v1/agent/chat",status="200"} 14823
argentor_requests_total{endpoint="/api/v1/agent/chat",status="429"} 42

# Latency histograms
argentor_request_duration_seconds_bucket{endpoint="/api/v1/agent/chat",le="0.1"} 2100
argentor_request_duration_seconds_bucket{endpoint="/api/v1/agent/chat",le="1.0"} 12500

# Agent metrics
argentor_agent_turns_total{role="orchestrator"} 3821
argentor_agent_tokens_total{role="coder",direction="input"} 891412
argentor_agent_tokens_total{role="coder",direction="output"} 234881
argentor_agent_cost_usd_total{provider="claude"} 148.72

# Tool calls
argentor_tool_calls_total{skill="web_search",outcome="success"} 2413
argentor_tool_calls_total{skill="web_search",outcome="error"} 18

# Circuit breakers
argentor_circuit_breaker_state{provider="openai"} 0  # 0=closed, 1=open, 2=half-open

# Guardrails
argentor_guardrail_violations_total{rule="pii_detection",severity="block"} 7
```

Scrape config:

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'argentor'
    scrape_interval: 15s
    static_configs:
      - targets: ['argentor:9090']
    metric_relabel_configs:
      - source_labels: [__name__]
        regex: 'argentor_.*'
        action: keep
```

---

## 6. Graceful Shutdown

Argentor implements a 4-phase shutdown when it receives SIGTERM:

1. **PreDrain** — mark `/health/ready` as 503 so load balancers stop sending traffic
2. **Drain** — let in-flight requests finish (configurable timeout, default 30s)
3. **Cleanup** — flush audit log buffers, close DB pools, disconnect MCP servers
4. **Final** — exit with status 0

Kubernetes `terminationGracePeriodSeconds` should be ≥ sum of drain + cleanup (default 60s in the Helm chart).

Configure in `argentor.toml`:

```toml
[shutdown]
drain_timeout_secs = 30
cleanup_timeout_secs = 15
force_exit_after_secs = 60
```

When rolling out updates:

```
[rollout] new pod ready (readiness=OK)
[rollout] terminate old pod with SIGTERM
[old] phase=PreDrain readiness=503
[lb] stops routing to old pod
[old] phase=Drain in_flight=4
[old] in_flight=0 phase=Cleanup
[old] phase=Final exit_code=0
```

No dropped connections.

---

## 7. Configuration Management

Argentor loads configuration from `argentor.toml` with environment variable interpolation:

```toml
[model]
provider = "claude"
model_id = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"

[server]
bind = "0.0.0.0:8080"
metrics_bind = "0.0.0.0:9090"

[server.tls]
cert_path = "/certs/tls.crt"
key_path = "/certs/tls.key"
client_auth = false

[auth]
jwt_secret = "${JWT_SECRET}"
algorithm = "HS256"

[database]
url = "${DATABASE_URL}"
pool_size = 20

[rate_limit]
requests_per_minute = 120
burst = 30

[audit]
path = "/var/lib/argentor/audit.jsonl"
max_size_mb = 500
max_rotated_files = 14
retention_days = 30
compress_rotated = true
```

Config hot-reload is enabled by default — edit the file and Argentor reloads within 500ms (via the `notify` crate file watcher). No restart needed.

---

## 8. Secrets Management

Never bake secrets into images. Use:

- **Kubernetes Secrets** — Helm chart generates them from values; better, use external-secrets-operator backed by AWS Secrets Manager / Vault / Parameter Store
- **Docker secrets** — `docker secret create` + reference in Compose
- **HashiCorp Vault** — Argentor can fetch at boot via the Vault agent sidecar
- **AWS IAM roles for service accounts** — IRSA on EKS; no static creds at all

Example with external-secrets-operator:

```yaml
apiVersion: external-secrets.io/v1beta1
kind: ExternalSecret
metadata:
  name: argentor-llm-keys
spec:
  secretStoreRef:
    name: aws-secrets
    kind: SecretStore
  target:
    name: argentor-secrets
  data:
    - secretKey: ANTHROPIC_API_KEY
      remoteRef:
        key: /argentor/prod/anthropic
```

---

## 9. Multi-Region Deployment

For data residency (GDPR, local regulations), Argentor supports region-aware routing:

```toml
[data_residency]
default_region = "EU"
regions = ["EU", "US", "APAC"]
routing_rules = [
    { match = "user_country_code = 'DE'", region = "EU" },
    { match = "user_country_code IN ('US','CA')", region = "US" },
]
```

Deploy one stack per region. The Helm chart accepts a `region` value:

```bash
helm install argentor-eu ./deploy/helm/argentor \
  --set global.region=EU \
  --set persistence.storageClassName=gp3-eu-west

helm install argentor-us ./deploy/helm/argentor \
  --set global.region=US \
  --set persistence.storageClassName=gp3-us-east
```

Cross-region traffic gets rejected automatically based on the user's region label.

---

## 10. Operations Checklist

Before going live, verify:

- [ ] TLS enabled (cert-manager in K8s, Let's Encrypt in Compose)
- [ ] JWT secret set and rotated via your secrets manager
- [ ] Rate limits configured (per tier / per API key)
- [ ] Audit log rotation configured
- [ ] Prometheus scraping `/metrics` every 15s
- [ ] Grafana dashboards imported (see `deploy/grafana/dashboards/`)
- [ ] Alerts configured on error rate, p99 latency, circuit breaker trips
- [ ] Backups scheduled for PostgreSQL (audit + sessions)
- [ ] Graceful-shutdown timeout verified with rolling updates
- [ ] PodDisruptionBudget in place
- [ ] NetworkPolicy restricts egress (Argentor should only talk to LLM providers + internal services)
- [ ] Image signed (cosign) and SBOM attached
- [ ] Runbook documented (oncall handbook)

---

## Common Issues

**"Health check failing after deploy"**
Usually `/health/ready` returns 503 because the LLM provider is unreachable. Check the audit log — the readiness check does a minimal ping that is logged.

**"OOMKilled" in Kubernetes**
Default limits (2Gi) are fine for ~100 concurrent sessions. Bump memory and `ARGENTOR_CACHE_CAPACITY` together, since the response cache is the biggest in-memory consumer.

**Pods stuck in `Terminating`**
Graceful shutdown exceeded `terminationGracePeriodSeconds`. Increase it in `values.yaml` or lower `drain_timeout_secs` in `argentor.toml`.

**TLS handshake failure**
Cert and key file paths may be wrong. Mount the secret into the pod with `defaultMode: 0400` and verify inside the pod with `openssl x509 -in tls.crt -noout -text`.

**Rate limits applied inconsistently across pods**
Distributed rate limits need Redis. Without it, each pod tracks its own counter — multiply the per-pod limit by replica count, or deploy Redis for shared state.

**"unable to find data_residency region"**
Region label missing on the incoming request. Add a middleware layer (Ingress annotations, CloudFront Lambda@Edge) to inject `X-Argentor-Region: EU` based on client geo.

---

## What You Built

- Production Docker image with minimal attack surface
- Docker Compose stack with PostgreSQL, Redis, Prometheus, Grafana
- Kubernetes deployment via Helm chart with HPA and PDB
- Health probes for liveness/readiness
- Prometheus metrics for every agent-level dimension
- Graceful 4-phase shutdown with no dropped connections
- Multi-region routing for data residency
- A pre-launch operations checklist

---

## Next Steps

- **[Tutorial 10: Observability](./10-observability.md)** — wire OpenTelemetry and distributed tracing.
- **[DEPLOYMENT.md](../DEPLOYMENT.md)** — reference for advanced topics (mTLS, PostgreSQL schema, multi-region failover).
- **[Tutorial 6: Guardrails & Security](./06-guardrails-security.md)** — harden the agent layer (complement to the deployment layer).
