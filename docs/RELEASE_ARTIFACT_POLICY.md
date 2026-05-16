# Release Artifact Policy

This document defines which artifacts block an Argentor release and which
artifacts may complete after the release is already usable.

## Critical Path Artifacts

The main `Release` workflow must finish these artifacts for a release to be
considered complete:

| Artifact | Workflow | Blocking |
| --- | --- | --- |
| GitHub Release notes | `release.yml` | Yes |
| crates.io packages | `release.yml` | Yes |
| Python SDK on PyPI | `release.yml` plus `publish-pypi.yml` | Yes |
| TypeScript SDK on npm | `release.yml` | Yes |
| Docker image on GHCR | `release.yml` | Yes |
| Linux x86_64 CLI binary | `release.yml` | Yes |
| macOS ARM CLI binary | `release.yml` | Yes |

If any critical-path artifact fails, the release is degraded until the failure is
fixed or the tag is superseded.

## Delayed Compatibility Artifacts

| Artifact | Workflow | Blocking | Reason |
| --- | --- | --- | --- |
| macOS Intel CLI binary | `release-macos-intel.yml` | No | GitHub `macos-13` runner availability can keep an otherwise healthy release queued for hours. |

The macOS Intel binary remains supported as a compatibility asset, but it does
not block the release announcement or package publication. It can be uploaded by
the tag-triggered compatibility workflow or manually through `workflow_dispatch`
with the release tag.

## Status Language

- `Complete`: all critical-path artifacts are published.
- `Complete with delayed compatibility asset`: all critical-path artifacts are
  published, but one or more delayed compatibility assets are still queued or
  running.
- `Degraded`: one or more critical-path artifacts failed.
- `Superseded`: a newer patch release replaces this tag as the recommended
  version.

## Release Engineer Checklist

Before announcing a tag:

- confirm the main `Release` workflow completed successfully;
- confirm PyPI and npm publish workflows completed successfully when they run
  outside the main release workflow;
- confirm GitHub Release assets include Linux x86_64 and macOS ARM binaries and
  checksums;
- record any delayed compatibility assets in the release status document;
- do not block on macOS Intel unless the release specifically targets Intel
  macOS users.

