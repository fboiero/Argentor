# Argentor v1.5.0 Release Status

Status snapshot for the `v1.5.0` release line.

## Summary

`v1.5.0` is prepared as the DeepSeek-default and distributed-runtime
scalability release. The main code gates are local verification, pre-tag
metadata checks, tag creation, and the GitHub release workflow.

## Release Links

- Tag: `v1.5.0`
- Commit: current release commit
- GitHub Release: pending
- Release workflow: pending

## Artifact Matrix

| Artifact | Status | Blocking | Notes |
| --- | --- | --- | --- |
| GitHub Release | Pending | Yes | Created by `release.yml` after tag push. |
| crates.io packages | Pending | Yes | Published by `release.yml`. |
| Python SDK / PyPI | Pending | Yes | Version bumped to `1.5.0`. |
| TypeScript SDK / npm | Pending | Yes | Version bumped to `1.5.0`. |
| Docker image / GHCR | Pending | Yes | Published by `release.yml`. |
| Linux x86_64 binary | Pending | Yes | Built by `release.yml`. |
| macOS ARM binary | Pending | Yes | Built by `release.yml`. |
| macOS Intel binary | Delayed compatibility | No | Use `release-macos-intel.yml` outside the critical path. |

## Verification Planned

- `cargo fmt --check`
- `cargo test -p argentor-agent providers_integration --test providers_integration`
- `cargo test -p argentor-gateway xcapitsff::tests --lib`
- `cargo test -p argentor-gateway streaming::tests --lib`
- `cargo test -p argentor-session sqlite_store::tests --lib`
- `cargo check -p argentor-gateway --features redis-broadcast`
- `cargo test -p argentor-gateway`
- `scripts/pretag-release-check.sh --offline --allow-dirty 1.5.0`

## Known Open Items

- Commit, tag, and remote release workflow are not created yet.
- macOS Intel remains a delayed compatibility asset by policy.
