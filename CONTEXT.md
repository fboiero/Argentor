# Argentor - Session Context
> Last updated: 2026-05-18 (roadmap/v1.5.0)

## Current Goal
Continue the v1.5.0 performance, scalability, and usability roadmap. Phase 3 distributed runtime scalability is now closed enough to move into Phase 4 operator usability work.

## What's Completed
- Phase 1 operational hardening is mostly closed: audit metrics, OpenAPI audit schemas, operator smoke coverage, and 100k/1M audit benchmark baselines.
- Phase 2 scalable audit plane is mostly closed: cursor pagination, audit rotation/retention, compression, `AuditSink`, SQLite/Webhook sinks, violation index, and persisted stats index.
- 10M audit benchmark result promoted as versionable evidence in `benchmarks/results/audit_scale_20260515_013517.json`.
- Phase 3 has a local `SessionBroadcast` abstraction for session SSE streams.
- Phase 3 now supports local SSE reconnect with `Last-Event-ID`: session broadcast events get monotonic per-session IDs and a bounded replay buffer.
- Phase 3 now has `FileSessionBroadcast` for shared-filesystem multi-replica session SSE streams, with append logs, lock-based event ID assignment, replay from disk, and polling for live events.
- Phase 3 now has optional `RedisSessionBroadcast` behind the `redis-broadcast` feature, using Redis Pub/Sub for live fanout, Redis lists for bounded replay, and an atomic Lua publish path.
- Phase 3 now has local stream backpressure for active SSE subscriptions: global, per-tenant (`X-Tenant-ID` / `X-Tenant`), and per-API-key (`Authorization: Bearer` / `X-API-Key`) limits.
- Phase 3 now has circuit breakers for both LLM providers and tool backends. Tool breaker keys use `tool:<name>` and open on tool execution errors or error results.
- Phase 3 now has distributed-safe local session persistence for shared filesystems: `SqliteSessionStore` uses unique temp files, an interprocess index lock, disk-index refresh before reads, and merge-before-write updates.

## What's Pending
1. Move into Phase 4 operator usability: health panels for cache hit rate, audit lag, active streams, and stream channel pressure.
2. Add NATS `SessionBroadcast` only if the target deployment needs NATS instead of Redis.

## Key Decisions
- Kept SSE reconnect local and bounded before adding Redis/NATS. This exercises the public contract first and avoids coupling the HTTP handler to a specific distributed backend.
- `Last-Event-ID` replay is scoped per session, with event IDs assigned by the broadcast adapter rather than by each subscriber.
- New subscribers without `Last-Event-ID` receive only live events; reconnecting subscribers receive buffered events with IDs greater than the header value.
- Backpressure is held by an RAII permit captured by the SSE stream, so slots are released when the client disconnects or the response stream is dropped.
- Tool circuit breakers are separate from LLM provider circuit breakers, so a bad tool cannot poison provider health and provider failures cannot block unrelated tools.
- Hardened the exported `SqliteSessionStore` instead of adding a new persistence dependency because it is already public, gateway-compatible, and can support the current shared-filesystem deployment model.
- Added `FileSessionBroadcast` before Redis/NATS because it completes the current shared-volume deployment path without new dependencies while keeping the same `SessionBroadcast` contract for future broker adapters.
- Added Redis as an optional gateway feature rather than a default dependency, so normal gateway builds keep their existing dependency footprint while Redis deployments can opt in.
- Track the 10M audit benchmark JSON because it is small, matches existing `benchmarks/results/` practice, and backs the Phase 1/2 scale claims.

## Relevant Files
- `crates/argentor-gateway/src/streaming.rs` - SSE session broadcast abstraction, local replay buffer, file-backed shared-volume broadcast, optional Redis broadcast, `Last-Event-ID` handling, stream backpressure.
- `crates/argentor-gateway/src/lib.rs` - public gateway re-exports, including `FileSessionBroadcast` and feature-gated `RedisSessionBroadcast`.
- `crates/argentor-gateway/Cargo.toml` - optional `redis-broadcast` feature.
- `Cargo.lock` - optional Redis dependency resolution.
- `crates/argentor-gateway/src/server.rs` - wires default `StreamBackpressureLimiter` into gateway streaming state.
- `crates/argentor-agent/src/runner.rs` - LLM and tool backend circuit breaker integration.
- `crates/argentor-session/src/sqlite_store.rs` - local session persistence with atomic writes, index lock, refresh-before-read, and merge-before-write behavior.
- `docs/ROADMAP_PERFORMANCE_SCALABILITY_USABILITY.md` - roadmap progress tracking.
- `benchmarks/results/audit_scale_20260515_013517.json` - 10M audit benchmark evidence to include in the next commit.
