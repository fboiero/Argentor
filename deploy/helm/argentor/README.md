# Argentor Helm Chart

Kubernetes Helm chart for [Argentor](https://github.com/fboiero/Argentor) — a secure AI agent framework with WASM sandboxed plugins.

## Prerequisites

- Kubernetes 1.25+
- Helm 3.10+

## Install

```bash
# Basic install — uses Chart.appVersion as the image tag
helm install argentor ./deploy/helm/argentor

# With an Anthropic API key
helm install argentor ./deploy/helm/argentor \
  --set secrets.anthropicApiKey=sk-...

# Pin image tag, scale up, enable HPA
helm install argentor ./deploy/helm/argentor \
  --set image.tag=v1.4.0 \
  --set replicaCount=3 \
  --set autoscaling.enabled=true
```

## Upgrade / Uninstall

```bash
helm upgrade argentor ./deploy/helm/argentor --set image.tag=v1.5.0
helm uninstall argentor
```

## Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| `replicaCount` | `1` | Pod replicas (ignored when `autoscaling.enabled = true`) |
| `image.repository` | `ghcr.io/fboiero/argentor` | Container image |
| `image.tag` | `""` (falls back to `Chart.appVersion`) | Image tag |
| `image.pullPolicy` | `IfNotPresent` | Image pull policy |
| `service.type` | `ClusterIP` | Service type |
| `service.port` | `3000` | Service port |
| `resources.limits.cpu` | `"1"` | CPU limit |
| `resources.limits.memory` | `256Mi` | Memory limit |
| `resources.requests.cpu` | `250m` | CPU request |
| `resources.requests.memory` | `64Mi` | Memory request |
| `autoscaling.enabled` | `false` | Enable HPA |
| `autoscaling.minReplicas` | `1` | HPA min replicas |
| `autoscaling.maxReplicas` | `10` | HPA max replicas |
| `autoscaling.targetCPUUtilizationPercentage` | `80` | Target CPU utilization |
| `persistence.enabled` | `true` | Mount a PVC at `/app/data` |
| `persistence.size` | `1Gi` | PVC size |
| `persistence.storageClass` | `""` | Storage class (cluster default if empty) |
| `ingress.enabled` | `false` | Enable Ingress |
| `ingress.className` | `""` | Ingress class |
| `config.logLevel` | `info` | Sets `RUST_LOG` |
| `config.bind` | `0.0.0.0:3000` | Sets `ARGENTOR_BIND` |
| `secrets.anthropicApiKey` | `""` | Anthropic API key (rendered as `ANTHROPIC_API_KEY`) |
| `secrets.openaiApiKey` | `""` | OpenAI API key (rendered as `OPENAI_API_KEY`) |
| `secrets.groqApiKey` | `""` | Groq API key (rendered as `GROQ_API_KEY`) |
| `env` | `[]` | Extra env vars (core/v1 `EnvVar` list) |
| `envFrom` | `[]` | Extra envFrom sources |

### Secrets

Any non-empty value in the `secrets` block is rendered into a Kubernetes `Secret` and injected into the pod via `envFrom` — so the agent sees the value as an environment variable with the documented name. The Secret is only created when at least one `secrets.*` value is non-empty.

### Config injection

The chart auto-wires two sources into the container's `envFrom`:

1. The generated `{release}-gateway` ConfigMap (always present): exposes `RUST_LOG` and `ARGENTOR_BIND`.
2. The generated `{release}-secrets` Secret (only when one or more `secrets.*` keys are set).

Anything you add to `env` or `envFrom` in values is appended on top.

### Pod security

The chart ships with hardened defaults: `runAsNonRoot: true`, `readOnlyRootFilesystem: true`, `allowPrivilegeEscalation: false`, all Linux capabilities dropped, and `seccompProfile: RuntimeDefault`. `/tmp` and `/app/data` are writable via dedicated volumes.

## Enable Ingress

```bash
helm install argentor ./deploy/helm/argentor \
  --set ingress.enabled=true \
  --set ingress.className=nginx \
  --set ingress.hosts[0].host=argentor.example.com \
  --set ingress.hosts[0].paths[0].path=/ \
  --set ingress.hosts[0].paths[0].pathType=Prefix
```

## Validate locally

```bash
helm lint ./deploy/helm/argentor
helm template argentor ./deploy/helm/argentor \
  --set secrets.anthropicApiKey=sk-test \
  --set autoscaling.enabled=true \
  --set ingress.enabled=true
```

## License

AGPL-3.0-only
