#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Argentor end-to-end validation script.
# Runs compilation, tests, examples, Python SDK, benchmarks, docs, and security checks.
# Re-runnable (idempotent). Fails fast on first critical error unless noted.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

pass() { echo -e "${GREEN}[PASS]${NC} $*"; }
fail() { echo -e "${RED}[FAIL]${NC} $*"; }
info() { echo -e "${YELLOW}[INFO]${NC} $*"; }

RESULTS=()
OVERALL=0

record() {
    local section="$1" status="$2" detail="$3"
    RESULTS+=("$section|$status|$detail")
    if [[ "$status" == "FAIL" ]]; then
        OVERALL=1
    fi
}

# ---------------------------------------------------------------------------
# 1. Compilation checks
# ---------------------------------------------------------------------------

info "=== Section 1: Compilation ==="

if cargo build --workspace --quiet 2>&1; then
    pass "cargo build --workspace"
    COMPILE_OK="workspace"
else
    fail "cargo build --workspace"
    record "Compilation" "FAIL" "workspace build failed"
    exit 1
fi

if cargo build --workspace --all-features --quiet 2>&1; then
    pass "cargo build --workspace --all-features"
    COMPILE_OK="$COMPILE_OK + features"
else
    fail "cargo build --workspace --all-features"
    record "Compilation" "FAIL" "all-features build failed"
    exit 1
fi

if cargo build --examples --quiet 2>&1; then
    pass "cargo build --examples"
    COMPILE_OK="$COMPILE_OK + examples"
else
    fail "cargo build --examples"
    record "Compilation" "FAIL" "examples build failed"
    exit 1
fi

CLIPPY_OUT="$(cargo clippy --workspace 2>&1 || true)"
CLIPPY_ERRORS=$(echo "$CLIPPY_OUT" | grep -c "^error" || true)
if [[ "$CLIPPY_ERRORS" -eq 0 ]]; then
    pass "cargo clippy --workspace (no errors)"
    COMPILE_OK="$COMPILE_OK + clippy"
else
    fail "cargo clippy --workspace ($CLIPPY_ERRORS error(s))"
    echo "$CLIPPY_OUT" | grep "^error" | head -10
    record "Compilation" "FAIL" "clippy reported $CLIPPY_ERRORS error(s)"
    exit 1
fi

record "Compilation" "PASS" "$COMPILE_OK"

# ---------------------------------------------------------------------------
# 2. Unit + integration test suite
# ---------------------------------------------------------------------------

info "=== Section 2: Unit + Integration Tests ==="

TEST_OUTPUT="$(cargo test --workspace 2>&1)" || {
    fail "cargo test --workspace — some tests FAILED"
    echo "$TEST_OUTPUT" | tail -30
    record "Unit Tests" "FAIL" "test suite had failures"
    exit 1
}

PASSED=$(echo "$TEST_OUTPUT" | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s+0}')
FAILED=$(echo "$TEST_OUTPUT" | grep -oE '[0-9]+ failed' | awk '{s+=$1} END {print s+0}')
IGNORED=$(echo "$TEST_OUTPUT" | grep -oE '[0-9]+ ignored' | awk '{s+=$1} END {print s+0}')

if [[ "$FAILED" -gt 0 ]]; then
    fail "Tests: $PASSED passed, $FAILED failed, $IGNORED ignored"
    record "Unit Tests" "FAIL" "$PASSED passed, $FAILED failed, $IGNORED ignored"
    exit 1
else
    pass "Tests: $PASSED passed, $FAILED failed, $IGNORED ignored"
    record "Unit Tests" "PASS" "$PASSED passed, $FAILED failed, $IGNORED ignored"
fi

# ---------------------------------------------------------------------------
# 3. Example execution
# ---------------------------------------------------------------------------

info "=== Section 3: Example Execution ==="

EXAMPLES_OK=0
EXAMPLES_TOTAL=3

# hello_world — must contain "Argentor"
HW_OUT="$(cargo run --example hello_world -p argentor-cli --quiet 2>&1)"
if echo "$HW_OUT" | grep -qi "argentor"; then
    pass "hello_world (output contains 'Argentor')"
    (( EXAMPLES_OK++ )) || true
else
    fail "hello_world — output does not contain 'Argentor'"
    echo "  Output: $HW_OUT"
fi

# with_tools — must contain tool execution output
WT_OUT="$(cargo run --example with_tools -p argentor-cli --quiet 2>&1)"
if echo "$WT_OUT" | grep -qiE "result|42|tool|skill"; then
    pass "with_tools (tool execution output found)"
    (( EXAMPLES_OK++ )) || true
else
    fail "with_tools — expected tool-execution output not found"
    echo "  Output: $WT_OUT"
fi

# custom_skill — must mention custom skill registration
CS_OUT="$(cargo run --example custom_skill -p argentor-cli --quiet 2>&1)"
if echo "$CS_OUT" | grep -qiE "reverse|skill|registered"; then
    pass "custom_skill (skill registration output found)"
    (( EXAMPLES_OK++ )) || true
else
    fail "custom_skill — expected skill-registration output not found"
    echo "  Output: $CS_OUT"
fi

if [[ "$EXAMPLES_OK" -eq "$EXAMPLES_TOTAL" ]]; then
    record "Examples" "PASS" "$EXAMPLES_OK/$EXAMPLES_TOTAL run successfully"
else
    record "Examples" "FAIL" "$EXAMPLES_OK/$EXAMPLES_TOTAL run successfully"
    OVERALL=1
fi

# ---------------------------------------------------------------------------
# 4. Python SDK validation
# ---------------------------------------------------------------------------

info "=== Section 4: Python SDK ==="

PYTHON_OK=0

# Install SDK quietly
if pip install -e "$REPO_ROOT/python" --quiet 2>/dev/null; then
    pass "pip install -e python/"
else
    info "pip install failed — trying pip3"
    pip3 install -e "$REPO_ROOT/python" --quiet 2>/dev/null || true
fi

# Verify imports
PY_OUT="$(python3 -c "from argentor import Agent, Session, Skill; print('Python SDK: OK')" 2>&1)"
if echo "$PY_OUT" | grep -q "Python SDK: OK"; then
    pass "Python imports: Agent, Session, Skill"
    PYTHON_OK=$((PYTHON_OK + 1))
else
    fail "Python imports failed: $PY_OUT"
fi

# Check py.typed marker (PEP 561)
if [[ -f "$REPO_ROOT/python/argentor/py.typed" ]]; then
    pass "py.typed marker exists (PEP 561)"
    PYTHON_OK=$((PYTHON_OK + 1))
else
    fail "py.typed marker missing at python/argentor/py.typed"
fi

if [[ "$PYTHON_OK" -eq 2 ]]; then
    record "Python SDK" "PASS" "imports OK, py.typed present"
else
    record "Python SDK" "FAIL" "$PYTHON_OK/2 checks passed"
    OVERALL=1
fi

# ---------------------------------------------------------------------------
# 5. Benchmark harness
# ---------------------------------------------------------------------------

info "=== Section 5: Benchmarks ==="

BENCH_OK=0

# Build benchmarks library tests
BENCH_TEST_OUT="$(cargo test -p argentor-benchmarks --lib 2>&1)" || true
BENCH_PASS=$(echo "$BENCH_TEST_OUT" | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s+0}')
BENCH_FAIL=$(echo "$BENCH_TEST_OUT" | grep -oE '[0-9]+ failed' | awk '{s+=$1} END {print s+0}')

if [[ "$BENCH_FAIL" -eq 0 ]]; then
    pass "Benchmark lib tests: $BENCH_PASS passed"
    BENCH_OK=$((BENCH_OK + 1))
else
    fail "Benchmark lib tests: $BENCH_PASS passed, $BENCH_FAIL failed"
fi

# Can list tasks
BENCH_LIST_OUT="$(cargo run -p argentor-benchmarks -- list 2>&1)" || true
if echo "$BENCH_LIST_OUT" | grep -qE "task|Task|\.toml|\.yaml|discovered|found|sec_|t[0-9]_|adv_"; then
    pass "Benchmark list: tasks discovered"
    BENCH_OK=$((BENCH_OK + 1))
else
    info "Benchmark list output: $BENCH_LIST_OUT"
    fail "Benchmark 'list' did not enumerate tasks"
fi

if [[ "$BENCH_OK" -eq 2 ]]; then
    record "Benchmarks" "PASS" "$BENCH_PASS lib tests, task list OK"
else
    record "Benchmarks" "FAIL" "$BENCH_OK/2 checks passed"
    OVERALL=1
fi

# ---------------------------------------------------------------------------
# 6. Documentation validation
# ---------------------------------------------------------------------------

info "=== Section 6: Documentation ==="

DOCS_OK=0
DOCS_TOTAL=0

check_file() {
    local label="$1" path="$2"
    DOCS_TOTAL=$((DOCS_TOTAL + 1))
    if [[ -f "$path" ]]; then
        pass "$label exists"
        DOCS_OK=$((DOCS_OK + 1))
    else
        fail "$label missing: $path"
    fi
}

check_file "README.md"       "$REPO_ROOT/README.md"
check_file "CHANGELOG.md"    "$REPO_ROOT/CHANGELOG.md"
check_file "CONTRIBUTING.md" "$REPO_ROOT/CONTRIBUTING.md"
check_file "SECURITY.md"     "$REPO_ROOT/SECURITY.md"
check_file "docs/GETTING_STARTED.md" "$REPO_ROOT/docs/GETTING_STARTED.md"
check_file "docs/API_REFERENCE.md"   "$REPO_ROOT/docs/API_REFERENCE.md"
check_file "docs/DEPLOYMENT.md"      "$REPO_ROOT/docs/DEPLOYMENT.md"

# README version check (workspace is 1.3.0; README may not yet mention it — note this)
if grep -q "1\.3\.0" "$REPO_ROOT/README.md" 2>/dev/null; then
    pass "README.md mentions v1.3.0"
    DOCS_OK=$((DOCS_OK + 1))
    DOCS_TOTAL=$((DOCS_TOTAL + 1))
else
    info "README.md does not mention v1.3.0 (workspace version is 1.3.0)"
    DOCS_TOTAL=$((DOCS_TOTAL + 1))
fi

# CHANGELOG v1.3.0 entry
if grep -q "1\.3\.0" "$REPO_ROOT/CHANGELOG.md" 2>/dev/null; then
    pass "CHANGELOG.md has v1.3.0 entry"
    DOCS_OK=$((DOCS_OK + 1))
    DOCS_TOTAL=$((DOCS_TOTAL + 1))
else
    fail "CHANGELOG.md missing v1.3.0 entry"
    DOCS_TOTAL=$((DOCS_TOTAL + 1))
fi

if [[ "$DOCS_OK" -eq "$DOCS_TOTAL" ]]; then
    record "Documentation" "PASS" "$DOCS_OK/$DOCS_TOTAL files present"
else
    record "Documentation" "FAIL" "$DOCS_OK/$DOCS_TOTAL checks passed"
    OVERALL=1
fi

# ---------------------------------------------------------------------------
# 7. Security / guardrail validation
# ---------------------------------------------------------------------------

info "=== Section 7: Security (Guardrails) ==="

GUARDRAIL_BIN="$REPO_ROOT/target/debug/e2e_guardrail_check"

# Build the guardrail check binary if needed
if ! cargo build --bin e2e_guardrail_check --quiet 2>&1; then
    fail "Could not build e2e_guardrail_check binary"
    record "Security" "FAIL" "guardrail check binary build failed"
    OVERALL=1
else
    pass "e2e_guardrail_check binary built"
    GR_OUT="$("$GUARDRAIL_BIN" 2>&1)"
    GR_PASS=$(echo "$GR_OUT" | grep -c "\[PASS\]" || true)
    GR_FAIL=$(echo "$GR_OUT" | grep -c "\[FAIL\]" || true)
    echo "$GR_OUT"

    if [[ "$GR_FAIL" -eq 0 && "$GR_PASS" -ge 5 ]]; then
        pass "Guardrail checks: $GR_PASS/5 correct"
        record "Security" "PASS" "$GR_PASS/5 guardrail checks correct"
    else
        fail "Guardrail checks: $GR_PASS passed, $GR_FAIL failed"
        record "Security" "FAIL" "$GR_PASS/5 correct, $GR_FAIL failures"
        OVERALL=1
    fi
fi

# ---------------------------------------------------------------------------
# 8. Final report
# ---------------------------------------------------------------------------

echo ""
echo "==========================================="
echo "   Argentor v1.3.0 Validation Report"
echo "==========================================="

for entry in "${RESULTS[@]}"; do
    IFS='|' read -r section status detail <<< "$entry"
    if [[ "$status" == "PASS" ]]; then
        printf "%-20s ${GREEN}PASS${NC}  (%s)\n" "$section:" "$detail"
    else
        printf "%-20s ${RED}FAIL${NC}  (%s)\n" "$section:" "$detail"
    fi
done

echo "-------------------------------------------"

if [[ "$OVERALL" -eq 0 ]]; then
    echo -e "OVERALL: ${GREEN}PASS ✓${NC}"
else
    echo -e "OVERALL: ${RED}FAIL ✗${NC}"
fi

echo "==========================================="

exit $OVERALL
