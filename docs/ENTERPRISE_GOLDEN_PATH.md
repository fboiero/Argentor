# Argentor Enterprise Golden Path

This document defines the product path for positioning Argentor as a secure
agent runtime with an enterprise gateway.

## Goal

Run one gateway instance that proves the core enterprise loop:

1. The gateway starts with REST, sessions, skills, metrics, OpenAPI, and control
   plane wiring.
2. Operators can query readiness from a stable endpoint.
3. The readiness response separates active runtime wiring from capabilities that
   are available but still need deployment-specific configuration.
4. Release checks fail when SDK or PyPI tests fail instead of masking errors.

## Readiness Endpoint

`GET /api/v1/enterprise/readiness`

Example shape:

```json
{
  "version": "1.3.0",
  "posture": "ready",
  "score": 71,
  "runtime": {
    "skills_registered": 42,
    "active_connections": 0,
    "active_sessions": 0,
    "uptime_seconds": 60
  },
  "checks": [
    {
      "id": "rest_api",
      "category": "runtime",
      "title": "REST API mounted",
      "status": "active",
      "detail": "REST management endpoints are available under /api/v1."
    }
  ],
  "next_actions": [
    "Wire deployment-specific auth, SSO, rate limits, and approval policy.",
    "Run the enterprise golden path smoke test before tagging a release."
  ]
}
```

## Readiness Semantics

- `active`: verified against the running gateway instance.
- `available`: compiled into Argentor and ready to wire for a deployment.
- `attention`: missing runtime wiring or failed runtime check.

The score is intentionally conservative: active checks count more than available
checks. A gateway can be product-ready while still reporting deployment-specific
actions such as SSO policy, approval policy, and rate-limit tuning.

The endpoint is served from the gateway's main application state rather than the
REST sub-router. That lets it report partial deployments honestly: if REST,
sessions, auth, per-key rate limits, metrics, control plane, proxy management,
or A2A are not mounted, the report can mark the specific check as `available` or
`attention` instead of pretending the capability is active.

## Smoke Test

Run the focused gateway contract tests:

```bash
cargo test -p argentor-gateway --test regression_api enterprise_readiness
```

Run the broader baseline before a release checkpoint:

```bash
cargo check --workspace --all-targets
python3 -m pytest tests/ -q
cd ../sdks/python && python3 -m pytest tests/ -q
cd ../typescript && npm test -- --run
```

## Follow-Up Evolutions

1. Add deployment config input so readiness can distinguish configured SSO,
   approval policy, and data-residency policy.
2. Add SDK methods for the readiness endpoint after the REST contract settles.
3. Add a CI smoke job that boots the gateway and asserts readiness posture.
4. Add dashboard UI integration once the endpoint contract is stable.
