---
name: Bug Report
about: Report a bug or unexpected behavior in Argentor
title: "[Bug] "
labels: bug, needs-triage
assignees: fboiero
---

## Description

A clear and concise description of the bug.

When I do X, Y happens instead of Z.

## Steps to Reproduce

Minimal, deterministic steps to reproduce the issue. Include commands, inputs, and relevant configuration.

1. Run `cargo run --bin argentor -- serve`
2. Send request `...`
3. Observe error `...`

## Expected Behavior

What did you expect to happen?

## Actual Behavior

What actually happened? Include full error messages, stack traces, or panic output.

```
paste error output here
```

## Environment

| Field | Value |
|-------|-------|
| OS | e.g. macOS 14.4 (arm64) |
| Rust version (`rustc --version`) | e.g. rustc 1.80.0 |
| Argentor version | e.g. v1.4.0 / commit 1e3c213 |
| LLM provider (if relevant) | e.g. Claude / Ollama |

## Logs / Output

```
paste relevant log output here
```

## Additional Context

Anything else that might help — screenshots, related issues, workarounds you tried.

## Checklist

- [ ] I searched existing issues and this is not a duplicate
- [ ] I am running a supported version of Argentor (v1.4.0+)
- [ ] For security vulnerabilities, I have read [SECURITY.md](../../SECURITY.md) and will use private disclosure instead of this template
