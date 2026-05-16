#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/pretag-release-check.sh [--deep] [--offline] [--allow-dirty] [--allow-existing-tag] <version>

Examples:
  scripts/pretag-release-check.sh 1.4.7
  scripts/pretag-release-check.sh --deep v1.5.0
  scripts/pretag-release-check.sh --offline --allow-dirty --allow-existing-tag 1.4.7

Checks release metadata before creating a tag. The default mode is local and
fast. --deep additionally runs cargo publish dry-runs for the crates that have
previously caused release failures.
USAGE
}

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ALLOW_DIRTY=false
ALLOW_EXISTING_TAG=false
DEEP=false
OFFLINE=false
VERSION=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --deep)
      DEEP=true
      shift
      ;;
    --offline)
      OFFLINE=true
      shift
      ;;
    --allow-dirty)
      ALLOW_DIRTY=true
      shift
      ;;
    --allow-existing-tag)
      ALLOW_EXISTING_TAG=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      if [[ -n "$VERSION" ]]; then
        echo "ERROR: multiple versions provided: $VERSION and $1" >&2
        usage >&2
        exit 2
      fi
      VERSION="${1#v}"
      shift
      ;;
  esac
done

if [[ -z "$VERSION" ]]; then
  usage >&2
  exit 2
fi

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]]; then
  echo "ERROR: invalid version '$VERSION'" >&2
  exit 2
fi

TAG="v$VERSION"
FAILURES=0
WARNINGS=0

pass() {
  printf 'ok: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  FAILURES=$((FAILURES + 1))
}

warn() {
  printf 'WARN: %s\n' "$1" >&2
  WARNINGS=$((WARNINGS + 1))
}

need_cmd() {
  if command -v "$1" >/dev/null 2>&1; then
    pass "found $1"
  else
    fail "missing required command: $1"
  fi
}

extract_toml_project_version() {
  local file="$1"
  awk '
    /^\[project\]/ { in_project=1; next }
    /^\[/ && in_project { in_project=0 }
    in_project && /^version[[:space:]]*=/ {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' "$file"
}

extract_workspace_version() {
  awk '
    /^\[workspace.package\]/ { in_pkg=1; next }
    /^\[/ && in_pkg { in_pkg=0 }
    in_pkg && /^version[[:space:]]*=/ {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' "$ROOT/Cargo.toml"
}

check_file_contains() {
  local file="$1"
  local pattern="$2"
  local label="$3"
  if grep -Eq "$pattern" "$ROOT/$file"; then
    pass "$label"
  else
    fail "$label"
  fi
}

cd "$ROOT"

echo "Pre-tag release check for $TAG"
echo

need_cmd git
need_cmd cargo
need_cmd awk
need_cmd grep
need_cmd node
echo

if ! git diff --quiet; then
  if [[ "$ALLOW_DIRTY" == true ]]; then
    warn "tracked worktree changes are present"
  else
    fail "tracked worktree changes are present"
  fi
else
  pass "no unstaged tracked changes"
fi

if ! git diff --cached --quiet; then
  if [[ "$ALLOW_DIRTY" == true ]]; then
    warn "staged changes are present"
  else
    fail "staged changes are present"
  fi
else
  pass "no staged changes"
fi

if [[ -n "$(git status --short --untracked-files=normal | grep '^??' || true)" ]]; then
  warn "untracked files are present; they are not release-blocking but should be intentional"
else
  pass "no untracked files"
fi

if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  if [[ "$ALLOW_EXISTING_TAG" == true ]]; then
    warn "local tag already exists: $TAG"
  else
    fail "local tag already exists: $TAG"
  fi
else
  pass "local tag does not exist"
fi

if [[ "$OFFLINE" == true ]]; then
  warn "remote tag verification skipped in offline mode"
else
  set +e
  git ls-remote --exit-code --tags origin "$TAG" >/tmp/argentor-release-ls-remote.out 2>/tmp/argentor-release-ls-remote.err
  ls_remote_status=$?
  set -e
  if [[ "$ls_remote_status" -eq 0 ]]; then
    if [[ "$ALLOW_EXISTING_TAG" == true ]]; then
      warn "remote tag already exists on origin: $TAG"
    else
      fail "remote tag already exists on origin: $TAG"
    fi
  elif [[ "$ls_remote_status" -eq 2 ]]; then
    pass "remote tag does not exist on origin"
  else
    fail "could not verify remote tag on origin: $(tr '\n' ' ' </tmp/argentor-release-ls-remote.err)"
  fi
fi
echo

workspace_version="$(extract_workspace_version)"
if [[ "$workspace_version" == "$VERSION" ]]; then
  pass "workspace version is $VERSION"
else
  fail "workspace version is '$workspace_version', expected '$VERSION'"
fi

if cargo metadata --no-deps --format-version 1 >/tmp/argentor-release-metadata.json; then
  mismatched_packages="$(
    node -e '
      const fs = require("fs");
      const version = process.argv[1];
      const published = new Set([
        "argentor-core",
        "argentor-security",
        "argentor-session",
        "argentor-memory",
        "argentor-skills",
        "argentor-channels",
        "argentor-compliance",
        "argentor-mcp",
        "argentor-agent",
        "argentor-builtins",
        "argentor-a2a",
        "argentor-orchestrator",
        "argentor-gateway",
      ]);
      const metadata = JSON.parse(fs.readFileSync("/tmp/argentor-release-metadata.json", "utf8"));
      const mismatches = metadata.packages
        .filter((pkg) => published.has(pkg.name) && pkg.version !== version)
        .map((pkg) => `${pkg.name}@${pkg.version}`);
      process.stdout.write(mismatches.join("\n"));
    ' "$VERSION"
  )"
  if [[ -z "$mismatched_packages" ]]; then
    pass "all published argentor crates resolve to $VERSION"
  else
    fail "published crate version mismatch: $mismatched_packages"
  fi
else
  fail "cargo metadata failed"
fi

workspace_dep_mismatches="$(
  grep -E '^argentor-[a-z0-9-]+ = \{ path = "crates/[^"]+", version = "' Cargo.toml \
    | grep -v "version = \"$VERSION\"" || true
)"
if [[ -z "$workspace_dep_mismatches" ]]; then
  pass "internal workspace dependency versions match $VERSION"
else
  fail "internal workspace dependency version mismatch: $workspace_dep_mismatches"
fi

agent_builtins_line="$(grep -E '^argentor-builtins = \{ path = "\.\./argentor-builtins", version = "' crates/argentor-agent/Cargo.toml || true)"
if [[ -z "$agent_builtins_line" ]]; then
  fail "argentor-agent dev-dependency on argentor-builtins is missing explicit path+version metadata"
elif [[ "$agent_builtins_line" == *"version = \"$VERSION\""* ]]; then
  fail "argentor-agent dev-depends on argentor-builtins $VERSION, which can recreate the crates.io publish cycle"
else
  pass "argentor-agent dev-dependency on argentor-builtins avoids target-version publish cycle"
fi
echo

python_version="$(extract_toml_project_version "$ROOT/sdks/python/pyproject.toml")"
if [[ "$python_version" == "$VERSION" ]]; then
  pass "Python SDK version is $VERSION"
else
  fail "Python SDK version is '$python_version', expected '$VERSION'"
fi

typescript_version="$(node -p "require('./sdks/typescript/package.json').version")"
if [[ "$typescript_version" == "$VERSION" ]]; then
  pass "TypeScript SDK version is $VERSION"
else
  fail "TypeScript SDK version is '$typescript_version', expected '$VERSION'"
fi
echo

check_file_contains "CHANGELOG.md" "^## \\[$VERSION\\]" "CHANGELOG has section for $VERSION"
check_file_contains "CHANGELOG.md" "^\\[$VERSION\\]: .*v$VERSION" "CHANGELOG has compare link for $VERSION"

major_minor="$(printf '%s' "$VERSION" | awk -F. '{print $1 "." $2}')"
release_checklist="docs/RELEASE_CHECKLIST_v${major_minor}.x.md"
if [[ -f "$release_checklist" ]]; then
  pass "release checklist exists: $release_checklist"
  if grep -q "$VERSION" "$release_checklist"; then
    pass "release checklist mentions $VERSION"
  else
    fail "release checklist does not mention $VERSION"
  fi
else
  fail "release checklist missing: $release_checklist"
fi
echo

for required_copy in \
  'COPY crates/ crates/' \
  'COPY benchmarks/ benchmarks/' \
  'COPY examples/ examples/' \
  'COPY dashboard/ dashboard/'
do
  if grep -Fq "$required_copy" Dockerfile; then
    pass "Dockerfile includes '$required_copy'"
  else
    fail "Dockerfile missing '$required_copy'"
  fi
done

check_file_contains ".github/workflows/release.yml" 'macOS Intel is intentionally excluded from the critical release workflow' "release workflow excludes macOS Intel from the critical path"
check_file_contains ".github/workflows/release-macos-intel.yml" 'runs-on: macos-13' "delayed macOS Intel compatibility workflow uses macos-13"
check_file_contains ".github/workflows/release.yml" 'cargo publish -p "\$crate" --no-verify' "release workflow publishes crates with captured output"
echo

if [[ "$DEEP" == true ]]; then
  echo "Running deep package checks"
  for crate in argentor-core argentor-agent argentor-builtins argentor-gateway; do
    if cargo publish -p "$crate" --dry-run --no-verify; then
      pass "cargo publish dry-run passed for $crate"
    else
      fail "cargo publish dry-run failed for $crate"
    fi
  done
  echo
fi

if [[ "$FAILURES" -gt 0 ]]; then
  echo "Pre-tag check failed with $FAILURES failure(s) and $WARNINGS warning(s)." >&2
  exit 1
fi

echo "Pre-tag check passed with $WARNINGS warning(s)."
