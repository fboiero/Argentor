# API Reference

## Browsing Locally

Generate and open the full API documentation with:

```bash
cargo doc --workspace --no-deps --open
```

This builds HTML docs for all 17 crates and opens them in your browser.

## docs.rs (once published)

Each crate will be available at:

```
https://docs.rs/argentor-core
https://docs.rs/argentor-security
https://docs.rs/argentor-session
https://docs.rs/argentor-skills
https://docs.rs/argentor-agent
https://docs.rs/argentor-builtins
https://docs.rs/argentor-memory
https://docs.rs/argentor-mcp
https://docs.rs/argentor-orchestrator
https://docs.rs/argentor-compliance
https://docs.rs/argentor-channels
https://docs.rs/argentor-gateway
https://docs.rs/argentor-a2a
https://docs.rs/argentor-tee
https://docs.rs/argentor-cloud
```

All crates are configured with `all-features = true` so every public API is visible.

## Key Entry Points

| What you want | Crate | Type |
|---|---|---|
| Run an agent | `argentor-agent` | `AgentRunner` |
| Cache LLM responses transparently | `argentor-agent` | `CacheLayer`, `CacheConfig` |
| Fire webhook events on agent activity | `argentor-agent` | `WebhookManager`, `WebhookConfig` |
| Add a skill | `argentor-skills` | `Skill` trait, `SkillRegistry` |
| Built-in tools | `argentor-builtins` | `register_builtins()` |
| Multi-agent | `argentor-orchestrator` | `Orchestrator` |
| MCP integration | `argentor-mcp` | `McpClient`, `McpSkill` |
| Permissions and audit | `argentor-security` | `PermissionSet`, `AuditLog`, `AuditRotationConfig` |
| Session state | `argentor-session` | `Session` |
| Core types | `argentor-core` | `Message`, `ToolCall`, `ToolResult` |

## HTTP REST API

The gateway exposes machine-readable surfaces under `http://<host>:<port>/`. The
canonical specification is served at `/openapi.json`; this section is a quick
index of the endpoints most often hit by operators.

### Operator surfaces

| Endpoint | Purpose |
|----------|---------|
| `GET /health` | Basic liveness check |
| `GET /metrics` | Prometheus text exposition, including the `argentor_audit_*` family |
| `GET /openapi.json` | OpenAPI 3.0 specification (self-referenced) |
| `GET /dashboard` | Operator cockpit: deployments, agents, health |
| `GET /dashboard/audit` | Audit explorer with filters, JSON drawer, and CSV/JSON export |

### Audit plane

All audit endpoints accept `limit` (capped at 1000 for logs, 500 for violations)
and `cursor` (byte offset returned in the `x-next-cursor` response header). They
read in bounded blocks and consult persisted indexes for sub-second responses on
multi-million-event audit files.

| Endpoint | Description |
|----------|-------------|
| `GET /api/v1/audit/logs?limit=N&cursor=B` | Recent audit JSONL entries. `x-next-cursor` continues the page |
| `GET /api/v1/audit/violations?limit=N&cursor=B` | Recent guardrail and policy violations, backed by `audit.jsonl.violations.idx` |
| `GET /api/v1/audit/stats` | Aggregate counters for the dashboard summary, served from `audit.jsonl.stats.idx` when fresh |

Latency baselines (release build, single-violation-every-1000 synthetic fixture):

| Endpoint | 1M-event p95 | 10M-event p95 |
|----------|--------------|---------------|
| Logs first page | ≤ 2 ms | 0.20 ms |
| Logs second page | ≤ 2 ms | 0.13 ms |
| Violations first page | ≤ 70 ms | 44.84 ms |
| Stats cold scan | ≤ 1.1 s | 812.96 ms |
| Stats warm cache hit | ≤ 1 ms | 0.05 ms |

Reproduce with:

```bash
cargo run -p argentor-benchmarks --release -- audit-scale \
  --events 10000000 --page-limit 100 --violation-every 1000 --samples 3
```

### Streaming and chat

| Endpoint | Description |
|----------|-------------|
| `POST /api/v1/chat/stream` | Start an agent run; events are published on the per-session broadcast |
| `GET /api/v1/stream/{session_id}` | Server-sent events subscriber: `token`, `tool_call`, and `done` events |

### Control plane and platform

| Endpoint | Description |
|----------|-------------|
| `GET /api/v1/enterprise/readiness` | Runtime posture and recommended next actions |
| `GET /api/v1/sessions`, `POST /api/v1/sessions` | List and create sessions |
| `GET /api/v1/skills` | Registered skills with descriptors |
| `GET /api/v1/control-plane/deployments`, `POST .../deployments` | Deployment list and create, authenticated |

### Error responses

Every `/api/v1` endpoint documents its non-2xx responses in `/openapi.json`. The
wire shape is `{"error": "<message>"}` for 400, 401, and 500 responses.
Authenticated endpoints declare a 401 explicitly; mutating endpoints declare 400
and 500 explicitly.
