# Argentor Deployment Guide

This guide covers deploying Argentor in production environments, from single-node quick starts to multi-region Kubernetes clusters with full compliance controls.

## Prerequisites

- Argentor binary or Docker image (`ghcr.io/fboiero/argentor:latest`)
- TLS certificates (self-signed for dev, CA-signed for production)
- PostgreSQL 15+ (optional, for persistent session/audit storage)

---

## 1. Quick Start

Run a single-node Argentor gateway with default settings:

```bash
docker run -d \
  --name argentor \
  -p 8080:8080 \
  -p 9090:9090 \
  -v argentor-data:/var/lib/argentor \
  -e ARGENTOR_LOG_LEVEL=info \
  ghcr.io/fboiero/argentor:latest serve
```

- Port `8080` — WebSocket/REST gateway
- Port `9090` — Metrics and health endpoints
- Volume `argentor-data` — persistent audit logs and session data

Verify the service is running:

```bash
curl http://localhost:9090/health
```

---

## 2. Docker Compose

Full stack with PostgreSQL, Redis (for future distributed sessions), and Prometheus:

```yaml
# docker-compose.yml
version: "3.9"

services:
  argentor:
    image: ghcr.io/fboiero/argentor:latest
    command: serve --bind 0.0.0.0:8080
    ports:
      - "8080:8080"
      - "9090:9090"
    environment:
      ARGENTOR_LOG_LEVEL: info
      ARGENTOR_DB_URL: postgres://argentor:secret@postgres:5432/argentor
      ARGENTOR_TLS_CERT: /certs/server.crt
      ARGENTOR_TLS_KEY: /certs/server.key
      ARGENTOR_DATA_REGION: US
    volumes:
      - argentor-data:/var/lib/argentor
      - ./certs:/certs:ro
    depends_on:
      - postgres

  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: argentor
      POSTGRES_PASSWORD: secret
      POSTGRES_DB: argentor
    volumes:
      - pg-data:/var/lib/postgresql/data
    ports:
      - "5432:5432"

  redis:
    image: redis:7-alpine
    ports:
      - "6379:6379"
    volumes:
      - redis-data:/data

  prometheus:
    image: prom/prometheus:latest
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml:ro
    ports:
      - "9091:9090"

  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
    environment:
      GF_SECURITY_ADMIN_PASSWORD: admin
    volumes:
      - grafana-data:/var/lib/grafana

volumes:
  argentor-data:
  pg-data:
  redis-data:
  grafana-data:
```

Start the stack:

```bash
docker compose up -d
docker compose logs -f argentor
```

---

## 3. Kubernetes / Helm

Argentor ships a Helm chart in `deploy/helm/agentor/`.

### Install with defaults

```bash
helm install argentor ./deploy/helm/agentor \
  --namespace argentor --create-namespace
```

### Production overrides

Create a `values-prod.yaml`:

```yaml
replicaCount: 3

image:
  repository: ghcr.io/fboiero/argentor
  tag: "latest"

service:
  type: LoadBalancer
  port: 8080

ingress:
  enabled: true
  className: nginx
  hosts:
    - host: argentor.example.com
      paths:
        - path: /
          pathType: Prefix
  tls:
    - secretName: argentor-tls
      hosts:
        - argentor.example.com

resources:
  requests:
    cpu: 500m
    memory: 512Mi
  limits:
    cpu: 2000m
    memory: 2Gi

env:
  ARGENTOR_LOG_LEVEL: info
  ARGENTOR_DATA_REGION: EU
  ARGENTOR_ENCRYPTION_AT_REST: "true"
```

Deploy:

```bash
helm upgrade --install argentor ./deploy/helm/agentor \
  -f values-prod.yaml \
  --namespace argentor --create-namespace
```

---

## 4. On-Premises (Bare Metal / VM)

### Binary installation

```bash
# Download the latest release
curl -fsSL https://github.com/fboiero/Agentor/releases/latest/download/argentor-linux-amd64.tar.gz \
  | tar xz -C /usr/local/bin

# Create system user and directories
useradd --system --no-create-home argentor
mkdir -p /var/lib/argentor/{data,audit}
mkdir -p /etc/argentor
chown -R argentor:argentor /var/lib/argentor
```

### systemd service

```ini
# /etc/systemd/system/argentor.service
[Unit]
Description=Argentor AI Agent Gateway
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=argentor
Group=argentor
ExecStart=/usr/local/bin/argentor serve --bind 0.0.0.0:8080 --config /etc/argentor/config.toml
Restart=always
RestartSec=5
LimitNOFILE=65536

Environment=ARGENTOR_LOG_LEVEL=info
Environment=RUST_BACKTRACE=1

[Install]
WantedBy=multi-user.target
```

```bash
systemctl daemon-reload
systemctl enable --now argentor
```

### Nginx reverse proxy

```nginx
# /etc/nginx/sites-available/argentor
upstream argentor_backend {
    server 127.0.0.1:8080;
}

server {
    listen 443 ssl http2;
    server_name argentor.example.com;

    ssl_certificate     /etc/ssl/certs/argentor.crt;
    ssl_certificate_key /etc/ssl/private/argentor.key;
    ssl_protocols       TLSv1.2 TLSv1.3;
    ssl_ciphers         HIGH:!aNULL:!MD5;

    location / {
        proxy_pass http://argentor_backend;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 86400;
    }

    location /metrics {
        proxy_pass http://127.0.0.1:9090/metrics;
        allow 10.0.0.0/8;
        deny all;
    }
}
```

---

## 5. Data Residency Configuration

Argentor supports region-locked deployments via `DataResidencyConfig`. Configure the region in your config file or environment:

| Region | Env Value | Description |
|--------|-----------|-------------|
| EU     | `EU`      | EU-only LLM endpoints, GDPR-ready |
| US     | `US`      | Standard US endpoints |
| LATAM  | `LATAM`   | Latin America, Spanish defaults |
| APAC   | `APAC`    | Asia-Pacific |
| Custom | `custom:me-south-1` | Custom region identifier |

### Example: EU GDPR deployment

```toml
# /etc/argentor/config.toml
[data_residency]
region = "EU"
data_storage_location = "/var/lib/argentor/data/eu"
llm_routing_policy = "SameRegion"
encryption_at_rest = true
encryption_in_transit = true
data_retention_days = 30
pii_handling = "Redact"
cross_border_transfer = false
audit_data_location = "/var/lib/argentor/audit/eu"
```

### Example: HIPAA deployment

```toml
[data_residency]
region = "US"
llm_routing_policy = "SameRegion"
encryption_at_rest = true
encryption_in_transit = true
data_retention_days = 2555
pii_handling = "Encrypt"
cross_border_transfer = false
```

### LLM routing policies

- **AnyRegion** — no restrictions on endpoint location
- **SameRegion** — only providers in the configured region
- **PreferRegion** — prefer same region, allow fallback
- **ExplicitEndpoints** — locked to a specific set of URLs

---

## 6. Security Hardening

### TLS configuration

Always use TLS 1.2+ in production. Generate certificates:

```bash
# Self-signed for development
openssl req -x509 -newkey rsa:4096 -keyout server.key -out server.crt \
  -days 365 -nodes -subj "/CN=argentor.local"

# For production, use Let's Encrypt or your CA
```

### Secrets management

- Store API keys and database credentials in a secrets manager (HashiCorp Vault, AWS Secrets Manager, or Kubernetes Secrets).
- Never pass secrets via command-line arguments (visible in `ps`).
- Use environment variables or mounted secret files.

### Network policies (Kubernetes)

```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: argentor-network-policy
  namespace: argentor
spec:
  podSelector:
    matchLabels:
      app: argentor
  policyTypes:
    - Ingress
    - Egress
  ingress:
    - from:
        - namespaceSelector:
            matchLabels:
              name: ingress
      ports:
        - port: 8080
  egress:
    - to:
        - namespaceSelector:
            matchLabels:
              name: argentor
      ports:
        - port: 5432
    - to:
        - ipBlock:
            cidr: 0.0.0.0/0
      ports:
        - port: 443
```

### Additional hardening

- Enable RBAC for multi-tenant deployments.
- Run containers as non-root (`runAsNonRoot: true`).
- Set read-only root filesystem where possible.
- Rotate TLS certificates automatically (cert-manager on K8s).

---

## 7. Monitoring

### Prometheus scraping

Argentor exposes metrics on port `9090` at `/metrics`. Add to `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: argentor
    scrape_interval: 15s
    static_configs:
      - targets: ["argentor:9090"]
    # For Kubernetes service discovery:
    # kubernetes_sd_configs:
    #   - role: pod
    #     namespaces:
    #       names: [argentor]
```

### Key metrics

| Metric | Type | Description |
|--------|------|-------------|
| `argentor_requests_total` | Counter | Total gateway requests |
| `argentor_request_duration_seconds` | Histogram | Request latency |
| `argentor_active_sessions` | Gauge | Currently active sessions |
| `argentor_llm_calls_total` | Counter | LLM API calls by provider |
| `argentor_tool_executions_total` | Counter | Tool invocations by skill |
| `argentor_errors_total` | Counter | Errors by subsystem |
| `argentor_audit_configured` | Gauge | Whether audit metrics are backed by an audit log |
| `argentor_audit_log_bytes` | Gauge | Current audit log size |
| `argentor_audit_events_total` | Gauge | Total events in the current audit log |
| `argentor_audit_events_today` | Gauge | Audit events recorded today |
| `argentor_audit_violations_today` | Gauge | Audit violations recorded today |
| `argentor_audit_block_rate_percent` | Gauge | Percentage of today's audit events denied by policy |

### Grafana dashboard

Import the bundled dashboard from `deploy/grafana/argentor-dashboard.json` or create panels for the key metrics above. Recommended alerts:

- **High error rate** — `rate(argentor_errors_total[5m]) > 0.05`
- **Latency spike** — `histogram_quantile(0.99, argentor_request_duration_seconds) > 5`
- **Session saturation** — `argentor_active_sessions > 1000`

---

## 8. Backup and Recovery

### Audit logs

Audit logs are critical for compliance. Back them up regularly:

Configure local lifecycle controls before shipping off-host:

```toml
[audit]
path = "/var/lib/argentor/audit.jsonl"
max_size_mb = 500
max_rotated_files = 14
retention_days = 30
compress_rotated = true
```

```bash
# Daily backup to S3
aws s3 sync /var/lib/argentor/audit s3://argentor-backups/audit/$(date +%Y-%m-%d)/

# Or use a cron job
0 2 * * * /usr/local/bin/backup-argentor-audit.sh
```

### Session data

```bash
# PostgreSQL backup
pg_dump -U argentor argentor | gzip > argentor-$(date +%Y%m%d).sql.gz
```

### Recovery procedure

1. Stop the Argentor service.
2. Restore PostgreSQL from backup: `gunzip -c backup.sql.gz | psql -U argentor argentor`
3. Restore audit logs to `/var/lib/argentor/audit`.
4. Start the service and verify with `/health`.

---

## 9. Scaling

### Horizontal scaling

Argentor is stateless at the gateway layer. Scale horizontally behind a load balancer:

```yaml
# Kubernetes HPA
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: argentor-hpa
  namespace: argentor
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: argentor
  minReplicas: 2
  maxReplicas: 10
  metrics:
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: 70
```

### Load balancer considerations

- Use sticky sessions (cookie-based) if WebSocket connections need affinity.
- Health check endpoint: `GET /health` on port `9090`.
- Drain timeout: 30 seconds minimum for in-flight requests.

### Database scaling

- Use PostgreSQL read replicas for audit log queries.
- Consider connection pooling with PgBouncer for high concurrency.

---

## 10. Compliance Checklist

Use this checklist before deploying Argentor in a regulated environment.

### General

- [ ] TLS 1.2+ enabled for all external connections
- [ ] Encryption at rest enabled for all stored data
- [ ] Non-root user for service execution
- [ ] Secrets stored in a secrets manager (not in config files)
- [ ] Network policies restrict ingress/egress to required ports only
- [ ] Audit logging enabled and backed up off-host

### GDPR (EU)

- [ ] Data region set to `EU`
- [ ] LLM routing policy set to `SameRegion` or `ExplicitEndpoints`
- [ ] PII handling set to `Redact` or `Encrypt`
- [ ] Cross-border transfer disabled (or SCCs in place)
- [ ] Data retention policy configured (default: 30 days)
- [ ] Data subject access request (DSAR) procedure documented

### HIPAA (US Healthcare)

- [ ] Data region set to `US`
- [ ] PII handling set to `Encrypt`
- [ ] Cross-border transfer disabled
- [ ] Data retention set to 7+ years (2555 days)
- [ ] BAA (Business Associate Agreement) signed with LLM providers
- [ ] Access logs retained for 6 years minimum

### ISO 27001

- [ ] Information security policy documented
- [ ] Access control with RBAC enabled
- [ ] Incident response procedure in place
- [ ] Regular vulnerability scans scheduled
- [ ] Encryption at rest and in transit enabled

### SOX (Financial)

- [ ] Audit trail immutable and backed up
- [ ] Data retention set to 7+ years
- [ ] Change management process documented
- [ ] Access controls with separation of duties

---

## Troubleshooting

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| Connection refused on 8080 | Service not running | Check `systemctl status argentor` |
| TLS handshake failure | Certificate mismatch or expired | Verify cert CN matches hostname |
| High latency on LLM calls | Wrong region routing | Check `llm_routing_policy` matches deployment region |
| Audit logs missing | Permissions on audit directory | Ensure `argentor` user owns `/var/lib/argentor/audit` |
| Pod OOMKilled | Memory limit too low | Increase to 2Gi+ for production workloads |
