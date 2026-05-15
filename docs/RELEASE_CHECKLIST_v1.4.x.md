# Argentor v1.4.x Release Checklist

Release readiness checklist for the audit-plane hardening work published as
Argentor `1.4.1`.

## Release Scope

- REST audit API is mounted under `/api/v1/audit`.
- Operator audit dashboard is available at `/dashboard/audit`.
- Prometheus audit metrics are exposed from `/metrics`.
- OpenAPI documents audit endpoints, cursors, and JSON error responses.
- Local JSONL audit storage supports path configuration, rotation, retention,
  optional Zstandard compression, violation indexing, and persisted stats
  indexing.

## Verification Commands

Run before tagging:

```bash
cargo fmt --check
env CARGO_TARGET_DIR=/private/tmp/agentor-target-rest \
  cargo test -p argentor-security audit --no-default-features
env CARGO_TARGET_DIR=/private/tmp/agentor-target-rest \
  cargo test -p argentor-gateway rest_api --no-default-features
env CARGO_TARGET_DIR=/private/tmp/agentor-target-rest \
  cargo test -p argentor-gateway test_release_operability_smoke \
  --test router_integration --no-default-features
env CARGO_TARGET_DIR=/private/tmp/agentor-target-rest \
  cargo check -p argentor-cli --no-default-features
```

Optional scale evidence:

```bash
cargo run -p argentor-benchmarks --release -- audit-scale \
  --events 1000000 --page-limit 100 --violation-every 1000 --samples 5

cargo run -p argentor-benchmarks --release -- audit-scale \
  --events 10000000 --page-limit 100 --violation-every 1000 --samples 3
```

## Release Gates

- [x] Workspace version is `1.4.1` for the current hardening release line.
- [x] Audit dashboard is linked from the main dashboard.
- [x] `/health`, `/dashboard`, `/dashboard/audit`, `/metrics`, and
  `/openapi.json` are covered by release smoke tests.
- [x] Audit list endpoints use bounded pagination and cursor headers.
- [x] Audit stats and violations avoid repeated unbounded reads on unchanged
  JSONL files.
- [x] Audit lifecycle configuration is documented in `argentor.toml`,
  deployment docs, and tutorials.
- [x] Known remaining work is explicitly classified as post-release.

## Known Limits

- Local JSONL remains the default audit store. Pluggable durable sinks
  (SQLite/Postgres, S3-compatible object storage, SIEM webhook/export) are
  post-release work.
- Dashboard request latency/error panels are still post-release. Prometheus
  exposes audit health and aggregate counters, but not dedicated dashboard
  panels per audit route.
- The 10M-event benchmark is synthetic local evidence, not a substitute for a
  production disk, tenant, and retention profile.

## Tagging Notes

- Tag: `v1.4.1`.
- Changelog source: `CHANGELOG.md` `[1.4.1]` section.
- Primary release evidence: this checklist, `docs/ROADMAP_PERFORMANCE_SCALABILITY_USABILITY.md`,
  and benchmark JSON under `benchmarks/results/` when intentionally included.
