# Release Artifact Matrix Template

Copy this template into `docs/RELEASE_STATUS_vX.Y.Z.md` for each release.

## Summary

`vX.Y.Z` status: `Complete`, `Complete with delayed compatibility asset`,
`Degraded`, or `Superseded`.

Primary release workflow:

- URL:
- Commit:
- Tag:

## Artifact Matrix

| Artifact | Status | Blocking | Workflow | Notes |
| --- | --- | --- | --- | --- |
| GitHub Release | Pending | Yes | `release.yml` |  |
| crates.io packages | Pending | Yes | `release.yml` |  |
| Python SDK / PyPI | Pending | Yes | `release.yml` / `publish-pypi.yml` |  |
| TypeScript SDK / npm | Pending | Yes | `release.yml` |  |
| Docker image / GHCR | Pending | Yes | `release.yml` |  |
| Linux x86_64 binary | Pending | Yes | `release.yml` |  |
| macOS ARM binary | Pending | Yes | `release.yml` |  |
| macOS Intel binary | Pending | No | `release-macos-intel.yml` | Delayed compatibility asset. |

## Uploaded GitHub Release Assets

- Pending

## Verification Completed

- Pending

## Known Open Items

- Pending

## Follow-Up

- Pending

