# Argentor v1.5.0 Roadmap

Release planning baseline for the first post-`1.4.x` evolution line.

`v1.4.7` closed the release-operability gap: crates.io, PyPI, npm, Docker,
GitHub Release creation, Linux binaries, and macOS ARM binaries publish from
the tag workflow. The only release task that can remain pending independently is
the macOS Intel asset while GitHub Actions waits for a `macos-13` runner.

## Product Thesis

Argentor should evolve from a high-performance agent gateway into an operator
control plane for governed, measurable, and scalable agent execution.

The next release should not optimize isolated internals without increasing
operability. Every feature in this line must improve at least one of these
outcomes:

- operators can see what agents are doing and why;
- platform teams can scale traffic without losing session semantics;
- developers can diagnose failures without reading source code;
- release engineers can publish without manual recovery work.

## Release Shape

Target: `v1.5.0`

Release type: minor feature release.

Primary theme: performance, scalability, usability, and control-plane
governance.

Non-goals:

- no broad rewrite of the gateway runtime;
- no unbounded dashboard APIs;
- no new storage backend without an explicit benchmark and failure-mode story;
- no marketing-only feature that cannot be verified from tests, metrics, or
  reproducible evidence.

## Current Baseline

Inherited from `v1.4.7`:

- audit REST API under `/api/v1/audit`;
- operator audit dashboard under `/dashboard/audit`;
- Prometheus metrics from `/metrics`;
- OpenAPI coverage for audit endpoints and shared errors;
- bounded cursor pagination for audit logs and violations;
- JSONL audit rotation, retention, compression, violation index, and stats
  index;
- pluggable `AuditSink` with JSONL, SQLite, and SIEM webhook support;
- per-route audit health panels in the dashboard;
- release pipeline publishing crates, SDKs, Docker image, release notes, and
  platform binaries.

Known inherited limits:

- JSONL remains the default local audit store;
- Postgres and S3-compatible audit sinks are not implemented;
- distributed session broadcast is still local-only;
- SSE reconnect/resume semantics are incomplete;
- dashboard health is useful for audit routes, but not yet a full operator
  cockpit;
- release publication depends on a queued macOS Intel runner for the final
  x86 asset.

## Strategic Tracks

### Track 1: Release Control and Supply Chain

Goal: make releases boring and recoverable.

Deliverables:

- Done: `scripts/pretag-release-check.sh` verifies workspace versions, changelog
  section, SDK package versions, release checklist, Docker context coverage, and
  dry-run package metadata before a tag is created.
- A release status document for the latest tag, including artifact matrix and
  known external waits.
- Crates publish dependency validation that fails before tag creation if a crate
  depends on a not-yet-published sibling in a way crates.io cannot resolve.
- A policy for macOS Intel: required artifact, delayed artifact, or best-effort
  compatibility artifact.

Success criteria:

- a release engineer can run one local command before tagging and get a clear
  pass/fail report;
- all package metadata mismatches are caught before the tag workflow;
- external runner queue waits are documented separately from product failures.

### Track 2: Performance Budgets

Goal: turn existing benchmark evidence into enforced budgets.

Deliverables:

- Done: machine-readable benchmark budget file for audit-scale profiles under
  `benchmarks/budgets/performance.json`.
- Done: opt-in CI performance-budget job that runs the 100k audit-scale profile
  and validates the generated JSON against the stored budget.
- Remaining: benchmark budget file entries for core message operations,
  security guardrails, skill registry operations, and SDK smoke paths.
- Remaining: CI gate coverage for selected low-noise non-audit paths.
- regression report format that separates runtime regression, measurement
  noise, and environment variance;
- local `audit-scale` profiles for 100k, 1M, and 10M events.

Initial budgets:

| Area | Budget |
| --- | --- |
| Audit logs first page, 100k events | p95 <= 10 ms |
| Audit violations first page, 100k events | p95 <= 100 ms |
| Audit stats warm cache | p95 <= 1 ms |
| Security shell validation | p95 <= 1 us |
| Skill registry lookup | p95 <= 50 ns |
| Message serde roundtrip | p95 <= 1 us |

Success criteria:

- performance regressions are caught before release;
- dashboards and docs cite reproducible command lines;
- benchmark results are versioned only when intentionally promoted as evidence.

### Track 3: Distributed Runtime Scalability

Goal: support multi-instance deployments without breaking sessions and streams.

Deliverables:

- broadcast abstraction for local, Redis, and NATS adapters;
- SSE reconnect support with `Last-Event-ID`;
- per-tenant and per-API-key backpressure;
- provider and tool backend circuit breakers;
- distributed-safe session persistence design and first implementation slice.

Success criteria:

- two gateway replicas can serve the same tenant in a documented test topology;
- stream fanout has explicit pressure limits and metrics;
- provider degradation produces observable fallback or graceful failure, not
  silent latency spikes.

### Track 4: Operator Usability

Goal: make day-two operations practical without shell access.

Deliverables:

- dashboard panels for cache hit rate, audit lag, active streams, and stream
  channel pressure;
- copyable correlation IDs where audit entries include them;
- empty, disabled, degraded, and network-failure states for every operator
  dashboard data source;
- safe operator actions: scoped cache clear, session revoke, pause/resume agent,
  and policy dry-run;
- troubleshooting pages linked from surfaced errors.

Success criteria:

- an operator can answer "what was blocked, why, and how often?" from the UI;
- destructive actions require explicit scope and show expected blast radius;
- dashboard controls stay usable with large datasets through pagination or
  virtualization.

### Track 5: Disruptive Control Plane

Goal: make Argentor proactive instead of purely forensic.

Deliverables:

- Policy Intelligence Layer that mines audit events for noisy rules, missing
  guardrails, and risky allow patterns;
- Replay Lab for historical sessions against new policies, models, routing, and
  guardrails;
- adaptive routing by risk, latency, cost, provider health, and observed
  quality;
- compliance evidence packages for incidents, tenant reviews, and SOC2-style
  controls;
- reliability agent prototype that proposes reviewed patches from metrics and
  audit evidence.

Success criteria:

- policy changes can be simulated before rollout;
- recommendations include evidence, blast radius, confidence, and rollback;
- governance shifts from incident review to controlled pre-production testing.

## Sprint Plan

### Sprint 1: Control the Release Line

Outcome: `v1.5.0` has a clean engineering runway.

- finalize `v1.4.7` artifact status once macOS Intel resolves or is classified;
- implement the pre-tag release check script;
- add package metadata dependency validation for crates;
- document release artifact matrix;
- choose the macOS Intel support policy.

Exit gate:

- pre-tag script runs locally and fails clearly on a simulated version mismatch;
- release matrix documents every artifact type and its owner workflow.

### Sprint 2: Enforce Performance Budgets

Outcome: performance claims become CI-enforced evidence.

- add benchmark budget config;
- add low-noise benchmark gate for audit scale warm paths;
- normalize benchmark output to JSON;
- document how to promote new benchmark baselines.

Exit gate:

- CI can flag a deliberate audit endpoint regression;
- docs include reproducible commands for each budgeted path.

### Sprint 3: Distributed Sessions First Slice

Outcome: local-only runtime assumptions are isolated behind interfaces.

- introduce broadcast adapter trait;
- keep local adapter as default;
- add Redis or NATS adapter behind feature flag;
- expose stream pressure metrics;
- design SSE resume semantics with event IDs.

Exit gate:

- local behavior remains backward compatible;
- feature-flagged distributed adapter has integration coverage.

### Sprint 4: Operator Cockpit

Outcome: the dashboard supports operational decisions, not just inspection.

- add stream, cache, and audit-lag health panels;
- implement correlation ID copy affordances where data exists;
- add policy dry-run UI backed by bounded API;
- improve error states and troubleshooting links.

Exit gate:

- dashboard can distinguish disabled, empty, degraded, and failed sources;
- operator actions are scoped, auditable, and reversible where applicable.

### Sprint 5: Replay Lab Prototype

Outcome: the disruptive control-plane direction becomes demonstrable.

- define replay input schema from audit/session history;
- run policy-only replay first;
- add report comparing old vs new decisions;
- expose prototype CLI before dashboard integration.

Exit gate:

- one historical session can be replayed against a changed policy;
- report includes changed decisions, risk category, and operator-readable
  rationale.

## Release Gates

`v1.5.0` should not be tagged unless:

- `cargo fmt --check` passes;
- workspace tests pass;
- Python SDK tests pass;
- TypeScript SDK build and tests pass;
- pre-tag release check passes;
- performance budget gate passes or has an approved baseline update;
- release artifact matrix is updated;
- changelog has a `1.5.0` section;
- known limits are documented with owner and next action.

## Immediate Next Actions

1. Decide whether macOS Intel remains a required release artifact.
2. Extend `scripts/pretag-release-check.sh --deep` to cover every publishable
   crate once the dry-run cost is acceptable for release engineers.
3. Add a release artifact matrix template for future tags.
4. Add performance budget entries for core, security, skills, and SDK smoke
   paths.
5. Promote performance-budget CI from opt-in to required once variance is
   measured across several runs.
