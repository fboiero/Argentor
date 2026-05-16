# Argentor v1.4.7 Release Status

Status snapshot for the `v1.4.7` release line.

## Summary

`v1.4.7` is published and usable. The release fixed the crates.io publishing
cycle between `argentor-agent` and `argentor-builtins` by keeping local agent
tests on the builtins path while publishing package metadata against an already
available `argentor-builtins` version.

The product release is complete across package registries and the main runtime
artifacts. The only open item is the macOS Intel binary asset, which is waiting
on GitHub Actions runner availability for `macos-13`.

## Release Links

- Tag: `v1.4.7`
- Commit: `f0bbfa9`
- GitHub Release: <https://github.com/fboiero/Argentor/releases/tag/v1.4.7>
- Release workflow: <https://github.com/fboiero/Argentor/actions/runs/25964337910>

## Artifact Matrix

| Artifact | Status | Notes |
| --- | --- | --- |
| GitHub Release | Complete | Release exists and is public. |
| crates.io packages | Complete | `publish-crates` completed successfully. |
| Python SDK / PyPI | Complete | `Publish Python SDK to PyPI` completed successfully. |
| TypeScript SDK / npm | Complete | `publish-typescript` completed successfully. |
| Docker image / GHCR | Complete | `publish-docker` completed successfully. |
| Linux x86_64 binary | Complete | Asset and checksum uploaded. |
| macOS ARM binary | Complete | Asset and checksum uploaded. |
| macOS Intel binary | External wait | `upload-binaries (x86_64-apple-darwin, macos-13)` is queued on GitHub Actions runner availability. |

## Uploaded GitHub Release Assets

- `argentor-v1.4.7-aarch64-apple-darwin.tar.gz`
- `argentor-v1.4.7-aarch64-apple-darwin.tar.gz.sha256`
- `argentor-v1.4.7-x86_64-unknown-linux-gnu.tar.gz`
- `argentor-v1.4.7-x86_64-unknown-linux-gnu.tar.gz.sha256`

## Verification Completed

- CI on `master` for `f0bbfa9` completed successfully.
- Release workflow `test` completed successfully.
- Release workflow `create-release` completed successfully.
- Release workflow `publish-crates` completed successfully.
- Release workflow `publish-python` completed successfully.
- Release workflow `publish-typescript` completed successfully.
- Release workflow `publish-docker` completed successfully.
- Release workflow uploaded Linux x86_64 and macOS ARM binaries.

## Known Open Item

The macOS Intel asset is not blocked by code. It is queued because the release
workflow uses the `macos-13` runner for `x86_64-apple-darwin`.

Decision required for `v1.5.0`:

- keep macOS Intel as a required artifact and tolerate external queue time;
- mark it as a delayed artifact that can finish after release announcement;
- remove it from the critical path and publish it as best-effort compatibility.

## Follow-Up for v1.5.0

- Add a pre-tag release check that catches package metadata and crates.io
  dependency publication issues before tag creation.
- Add a release artifact matrix template for every tagged release.
- Decide the macOS Intel support policy before the next tag.

