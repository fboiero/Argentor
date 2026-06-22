# Argentor v1.5.x Release Checklist

Release readiness checklist for the distributed runtime scalability and
DeepSeek-default release line published as Argentor `1.5.0`.

## Release Scope

- DeepSeek is the default LLM provider for first-run docs and XcapitSFF agent
  profiles.
- DeepSeek uses `DEEPSEEK_API_KEY`, `deepseek-chat`, and the OpenAI-compatible
  chat completions backend.
- OpenAI-compatible providers report provider-specific names for metrics,
  cache keys, and circuit breakers.
- `SessionBroadcast` supports local, shared-filesystem, and optional Redis
  implementations.
- SSE subscriptions support reconnect via `Last-Event-ID` and bounded replay.
- Stream backpressure protects active SSE subscriptions by global, tenant, and
  API-key scopes.
- `SqliteSessionStore` is safe for shared-filesystem multi-instance usage.
- 10M audit benchmark JSON is promoted as release evidence.

## Verification Commands

Run before tagging:

```bash
cargo fmt --check
cargo test -p argentor-agent providers_integration --test providers_integration
cargo test -p argentor-gateway xcapitsff::tests --lib
cargo test -p argentor-gateway streaming::tests --lib
cargo test -p argentor-session sqlite_store::tests --lib
cargo check -p argentor-gateway --features redis-broadcast
cargo test -p argentor-gateway
scripts/pretag-release-check.sh --offline --allow-dirty 1.5.0
```

Optional deep check before creating a public tag:

```bash
scripts/pretag-release-check.sh --deep 1.5.0
```

## Release Gates

- [x] Workspace version is `1.5.0`.
- [x] Python SDK version is `1.5.0`.
- [x] TypeScript SDK version is `1.5.0`.
- [x] Changelog has a `[1.5.0]` section.
- [x] DeepSeek is the default provider in first-run examples.
- [x] XcapitSFF profiles default to DeepSeek and resolve `DEEPSEEK_API_KEY`.
- [x] DeepSeek keeps provider-specific observability identity.
- [x] Redis broadcast compiles behind its optional feature.
- [x] Shared-filesystem session persistence and broadcast have focused tests.
- [x] Release evidence includes the 10M audit benchmark JSON.

## Known Limits

- Redis broadcast is compile-time optional and requires deployment-provided
  Redis infrastructure.
- NATS broadcast is not included in `1.5.0`; add it only for deployments that
  standardize on NATS.
- DeepSeek is the default, but Claude, OpenAI, Gemini, and other providers
  remain supported through explicit config.

## Tagging Notes

- Tag: `v1.5.0`.
- Changelog source: `CHANGELOG.md` `[1.5.0]` section.
- Primary release evidence: this checklist,
  `docs/ROADMAP_PERFORMANCE_SCALABILITY_USABILITY.md`, and
  `benchmarks/results/audit_scale_20260515_013517.json`.
