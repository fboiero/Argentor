# Argentor Performance, Scalability, and Usability Roadmap

> Development roadmap for turning Argentor from a functional agent gateway into
> an operable, scalable, and adaptive agent control plane.

## Current Baseline

- REST audit endpoints are mounted under `/api/v1/audit`.
- Audit logs and violations use bounded recent reads instead of scanning the
  whole JSONL file for normal dashboard queries.
- Audit stats are cached by file metadata.
- The audit dashboard is served at `/dashboard/audit` and linked from the main
  dashboard.
- Session SSE broadcast channels are capped and idle channels are pruned.
- Response caching uses a heap-backed LRU eviction path.

## Guiding Principles

- Measure before broad refactors. Add metrics and benchmarks before replacing
  working paths.
- Keep operational APIs bounded by default. Every list endpoint needs a limit,
  cursor, or window.
- Prefer pluggable storage over one-off special cases.
- Make the dashboard an operator cockpit, not a decorative report.
- Avoid high-cardinality metrics labels. Session IDs, user IDs, rule payloads,
  and tenant IDs belong in logs/traces, not Prometheus labels.

## Phase 1: Operational Hardening

**Goal:** make the current single-node gateway diagnosable under real traffic.

- Add Prometheus metrics for audit health and aggregate audit counters.
- Add request latency/error dashboards for `/api/v1/audit/*`.
- Add explicit API error schemas and document them in OpenAPI.
- Add load tests for audit JSONL files with 100k, 1M, and 10M events.
- Add smoke tests that verify `/dashboard`, `/dashboard/audit`, `/metrics`, and
  `/openapi.json` together.

**Success criteria:**

- `/metrics` exposes audit configured state, log size, total events, events
  today, violations today, and block rate.
- Recent logs/violations stay responsive on a 1M-line audit file.
- Operators can identify whether audit is unavailable, empty, or active.

**Initial audit latency budget:**

Baseline command:

```bash
cargo run -p argentor-benchmarks -- audit-scale \
  --events 100000 \
  --page-limit 100 \
  --violation-every 100 \
  --samples 5
```

Observed on local dev hardware:

| Endpoint | 100k-event p95 |
|----------|----------------|
| Logs first page | <= 2 ms |
| Logs second page | <= 2 ms |
| Violations first page | <= 65 ms |
| Stats cold scan | <= 650 ms |
| Stats warm cache hit | <= 1 ms |

Near-term target: keep logs under 10 ms p95 and violations under 100 ms p95
at 100k events. Phase 2 should add a violation index before treating 1M-event
violation queries as production-ready.

Follow-up 1M-event baseline after streaming stats optimization:

| Endpoint | 1M-event p95 |
|----------|--------------|
| Logs first page | <= 2 ms |
| Logs second page | <= 2 ms |
| Violations first page | <= 70 ms |
| Stats cold scan | <= 1.1 s |
| Stats warm cache hit | <= 1 ms |

Stats cold now avoids materializing every audit entry and counts total events via
byte scanning, then parses only today's tail in reverse. A persisted stats index
is still required if cold `/api/v1/audit/stats` must be consistently sub-100 ms
on million-event logs after process restart.

## Phase 2: Scalable Audit Plane

**Goal:** move audit from local JSONL convenience into a production subsystem.

- Add cursor pagination to `/api/v1/audit/logs`.
- Add cursor pagination to `/api/v1/audit/violations`.
- Introduce audit retention, rotation, and compression policy.
- Add pluggable audit sinks: JSONL, SQLite/Postgres, S3-compatible object
  storage, and SIEM webhook/export.
- Add a lightweight violation index for fast policy/security views.

**Success criteria:**

- Dashboard never needs unbounded reads.
- Audit storage can be swapped without changing dashboard code.
- A single tenant can export a bounded audit range without blocking the runtime.

## Phase 3: Distributed Runtime Scalability

**Goal:** support multi-instance gateway deployments without losing session or
streaming semantics.

- Abstract session broadcast behind a local/Redis/NATS implementation.
- Support SSE reconnect with `Last-Event-ID`.
- Add per-tenant and per-API-key backpressure.
- Add circuit breakers for LLM providers and tool backends.
- Add distributed-safe session persistence.

**Success criteria:**

- Multiple gateway replicas can serve the same tenant.
- Stream fanout has explicit limits and observable pressure signals.
- Provider degradation triggers fallback or graceful failure.

## Phase 4: Operator Usability

**Goal:** make day-two operations practical from the dashboard.

- Add audit filters by severity, outcome, action, skill, date range, and text.
- Add detail drawers with compact JSON rendering and copyable correlation IDs.
- Add export actions for CSV/JSON.
- Add health panels for cache hit rate, audit lag, active streams, and stream
  channel pressure.
- Add safe operator actions: scoped cache clear, session revoke, pause/resume
  agent, and policy dry-run.

**Success criteria:**

- An operator can answer "what was blocked and why?" without shell access.
- Large datasets remain usable through pagination and virtualization.
- UI failure states distinguish network failure, empty data, and disabled audit.

## Phase 5: Disruptive Control Plane

**Goal:** move beyond observability into adaptive governance.

- Policy Intelligence Layer: mine audit events for noisy rules, missing
  guardrails, and risky allow patterns.
- Replay Lab: replay historical sessions against new policies, models, and
  routing strategies before rollout.
- Autonomous Reliability Agent: inspect metrics and audit trails, propose
  config changes, and open reviewed patches.
- Adaptive routing: choose model/tool paths by risk, latency, cost, and
  historical quality.
- Compliance Copilot: generate reproducible evidence packages for incidents,
  SOC2-style reviews, and tenant reports.

**Success criteria:**

- Policy changes can be simulated before production rollout.
- Reliability recommendations include evidence, blast radius, and rollback.
- Governance becomes proactive instead of purely forensic.

## Next Sprint

1. Finish audit metrics in `/metrics`.
2. Add cursor pagination design and tests for `/api/v1/audit/logs`.
3. Add a JSONL benchmark fixture generator for audit scale tests.
4. Improve `/dashboard/audit` filters and disabled/empty/error states.
5. Add an operations checklist linking dashboard, OpenAPI, and Prometheus.
