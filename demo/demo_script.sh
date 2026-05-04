#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Argentor v1.3.0 — Feature Demo Script
# Record with: asciinema rec demo/demo.cast --command="bash demo/demo_script.sh"

set -euo pipefail

# ---------------------------------------------------------------------------
# Colors & helpers
# ---------------------------------------------------------------------------
RESET='\033[0m'
BOLD='\033[1m'
DIM='\033[2m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
MAGENTA='\033[0;35m'
WHITE='\033[1;37m'
RED='\033[0;31m'

# Simulate human typing: echo each character with a small delay
type_slow() {
    local text="$1"
    local delay="${2:-0.03}"
    printf "${CYAN}${BOLD}\$ ${RESET}${WHITE}"
    for (( i=0; i<${#text}; i++ )); do
        printf '%s' "${text:$i:1}"
        sleep "$delay"
    done
    printf "${RESET}\n"
}

# Wait for Enter or auto-continue after N seconds (default 2s in non-interactive mode)
pause() {
    local msg="${1:-[Press Enter to continue...]}"
    if [ -t 0 ]; then
        printf "\n${DIM}${msg}${RESET}"
        read -r _
    else
        # Non-interactive (e.g., piped to bash for testing)
        sleep 1
        echo ""
    fi
}

# Boxed section header
section() {
    local title="$1"
    local len=${#title}
    local border
    border=$(printf '═%.0s' $(seq 1 $((len + 4))))
    echo ""
    printf "${BOLD}${BLUE}╔${border}╗${RESET}\n"
    printf "${BOLD}${BLUE}║  ${WHITE}${title}${BLUE}  ║${RESET}\n"
    printf "${BOLD}${BLUE}╚${border}╝${RESET}\n"
    echo ""
}

# Print a labeled line
info() {
    printf "  ${GREEN}▶${RESET} ${1}\n"
}

# ---------------------------------------------------------------------------
# INTRO
# ---------------------------------------------------------------------------
clear
printf "${BOLD}${MAGENTA}"
cat << 'BANNER'
   ___                     _
  / _ \   _ __  __ _  ___ | |_  ___   _ __
 / /_\ \ | '__|/ _` |/ _ \| __|/ _ \ | '__|
/ /   \ \| |  | (_| |  __/| |_| (_) || |
\_|   |_/|_|   \__, |\___| \__|\___/ |_|
                |___/
BANNER
printf "${RESET}"
echo ""
printf "${WHITE}${BOLD}  The Secure AI Agent Framework${RESET}\n"
printf "${DIM}  17 crates  |  220K+ LOC  |  5,000+ tests  |  AGPL-3.0-only${RESET}\n"
echo ""
printf "${DIM}  Built in Rust. WASM-sandboxed plugins. Enterprise-grade guardrails.${RESET}\n"

pause "[Press Enter to start the demo...]"

# ---------------------------------------------------------------------------
# 1. QUICK START — Hello World
# ---------------------------------------------------------------------------
section "1. Quick Start — Hello World"
info "Simplest possible agent: mock backend, no API key required"
echo ""
type_slow "cargo run --example hello_world -p argentor-cli 2>/dev/null"
cargo run --example hello_world -p argentor-cli 2>/dev/null

pause

# ---------------------------------------------------------------------------
# 2. TOOL CALLING
# ---------------------------------------------------------------------------
section "2. Tool Calling — Built-in Skills"
info "Register skills (calculator, hash, uuid), wire a mock LLM that calls them"
echo ""
type_slow "cargo run --example with_tools -p argentor-cli 2>/dev/null"
cargo run --example with_tools -p argentor-cli 2>/dev/null

pause

# ---------------------------------------------------------------------------
# 3. CUSTOM SKILLS
# ---------------------------------------------------------------------------
section "3. Custom Skills — Implement the Skill Trait"
info "Author a custom ReverseSkill in ~30 lines, register, and run it"
echo ""
type_slow "cargo run --example custom_skill -p argentor-cli 2>/dev/null"
cargo run --example custom_skill -p argentor-cli 2>/dev/null

pause

# ---------------------------------------------------------------------------
# 4. MULTI-AGENT ORCHESTRATION
# ---------------------------------------------------------------------------
section "4. Multi-Agent Orchestration — Orchestrator-Workers"
info "Orchestrator decomposes a task → dispatches to Spec/Coder/Tester/Reviewer workers"
echo ""
type_slow "cargo run --example multi_agent -p argentor-cli 2>/dev/null"
cargo run --example multi_agent -p argentor-cli 2>/dev/null

pause

# ---------------------------------------------------------------------------
# 5. SECURITY GUARDRAILS
# ---------------------------------------------------------------------------
section "5. Security — 6-Layer Guardrail Engine"
info "Testing: injection, PII, shell, base64-encoded payload, benign query"
echo ""
type_slow "cargo build --bin e2e_guardrail_check --quiet 2>/dev/null && ./target/debug/e2e_guardrail_check"
cargo build --bin e2e_guardrail_check --quiet 2>/dev/null
./target/debug/e2e_guardrail_check

pause

# ---------------------------------------------------------------------------
# 6. PYTHON SDK
# ---------------------------------------------------------------------------
section "6. Python SDK — Pure-Python Client"
info "Drop-in SDK with sync/async API, tool registration, session management"
echo ""
type_slow "python3 -c \"from argentor import Agent; print('Agent class:', Agent.__name__); print('Import OK — SDK ready')\""
(cd /Users/fboiero/Documents/GitHub/Agentor/python && \
    python3 -c "from argentor import Agent; print('Agent class:', Agent.__name__); print('Import OK — SDK ready')")

pause

# ---------------------------------------------------------------------------
# 7. TEST SUITE
# ---------------------------------------------------------------------------
section "7. Test Suite — 5,000+ Tests Across 17 Crates"
info "Running full workspace test suite..."
echo ""
type_slow "cargo test --workspace --quiet 2>&1 | tail -5"
RESULT=$(cargo test --workspace --quiet 2>&1 | tail -5)
echo "$RESULT"

pause

# ---------------------------------------------------------------------------
# 8. COMPETITIVE BENCHMARKS
# ---------------------------------------------------------------------------
section "8. Competitive Benchmarks — Why Argentor Wins"
echo ""
printf "${BOLD}${WHITE}"
printf "  ┌──────────────────┬──────────────┬──────────────┬──────────────┐\n"
printf "  │ Dimension        │ Argentor     │ LangChain    │ CrewAI       │\n"
printf "  ├──────────────────┼──────────────┼──────────────┼──────────────┤\n"
printf "${RESET}"
printf "  │ Latency          │ ${GREEN}~2ms${RESET}         │ ${RED}~22ms${RESET}        │ ${RED}~55ms${RESET}        │\n"
printf "  │ Cost (50 tools)  │ ${GREEN}350 tok${RESET}      │ ${RED}2,750 tok${RESET}    │ ${RED}4,250 tok${RESET}    │\n"
printf "  │ Security guardr. │ ${GREEN}58.3%% block${RESET} │ ${RED}0%%${RESET}           │ ${RED}0%%${RESET}           │\n"
printf "  │ Compliance       │ ${GREEN}GDPR+ISO${RESET}     │ ${RED}None${RESET}         │ ${RED}None${RESET}         │\n"
printf "  │ SIEM export      │ ${GREEN}CEF/LEEF${RESET}     │ ${RED}None${RESET}         │ ${RED}None${RESET}         │\n"
printf "  │ WASM sandboxing  │ ${GREEN}Yes${RESET}          │ ${RED}No${RESET}           │ ${RED}No${RESET}           │\n"
printf "${BOLD}${WHITE}"
printf "  └──────────────────┴──────────────┴──────────────┴──────────────┘\n"
printf "${RESET}"

pause

# ---------------------------------------------------------------------------
# 9. ARCHITECTURE — 17 CRATES
# ---------------------------------------------------------------------------
section "9. Architecture — 17 Crates, Clean Boundaries"
echo ""
printf "  ${BOLD}${CYAN}Core layer${RESET}\n"
info "argentor-core       — types, errors, Message, ToolCall, ToolResult"
info "argentor-security   — Capability, PermissionSet, RateLimiter, AuditLog, TLS"
info "argentor-session    — Session, FileSessionStore"
echo ""
printf "  ${BOLD}${CYAN}Agent layer${RESET}\n"
info "argentor-agent      — AgentRunner, 8 LLM backends, guardrails, intelligence"
info "argentor-skills     — Skill trait, SkillRegistry, WasmSkillRuntime"
info "argentor-builtins   — 50+ built-in skills (echo, time, calculator, ...)"
echo ""
printf "  ${BOLD}${CYAN}Memory & knowledge${RESET}\n"
info "argentor-memory     — VectorStore, FileVectorStore, LocalEmbedding, RAG"
echo ""
printf "  ${BOLD}${CYAN}Network & protocols${RESET}\n"
info "argentor-gateway    — axum WebSocket gateway, ConnectionManager, MessageRouter"
info "argentor-channels   — Channel trait, pluggable transports"
info "argentor-mcp        — McpClient (JSON-RPC 2.0 stdio), McpSkill"
echo ""
printf "  ${BOLD}${CYAN}Multi-agent${RESET}\n"
info "argentor-orchestrator — Multi-agent engine, TaskQueue, AgentMonitor"
info "argentor-a2a          — Agent-to-Agent protocol"
echo ""
printf "  ${BOLD}${CYAN}Enterprise${RESET}\n"
info "argentor-compliance — GDPR, ISO 27001, ISO 42001, DPGA modules"
info "argentor-cloud      — Cloud-native deployment helpers"
info "argentor-tee        — Trusted Execution Environment support"
echo ""
printf "  ${BOLD}${CYAN}CLI${RESET}\n"
info "argentor-cli        — CLI binary (serve, skill list, REPL)"

pause

# ---------------------------------------------------------------------------
# OUTRO
# ---------------------------------------------------------------------------
clear
printf "${BOLD}${MAGENTA}"
cat << 'BANNER'
   ___                     _
  / _ \   _ __  __ _  ___ | |_  ___   _ __
 / /_\ \ | '__|/ _` |/ _ \| __|/ _ \ | '__|
/ /   \ \| |  | (_| |  __/| |_| (_) || |
\_|   |_/|_|   \__, |\___| \__|\___/ |_|
                |___/
BANNER
printf "${RESET}"
echo ""
printf "${WHITE}${BOLD}  Open Source. Secure. Fast. Production-Ready.${RESET}\n"
echo ""
info "${BOLD}GitHub  ${RESET}  github.com/fboiero/Agentor"
info "${BOLD}Crates  ${RESET}  crates.io/crates/argentor-agent"
info "${BOLD}License ${RESET}  AGPL-3.0-only"
info "${BOLD}Docs    ${RESET}  fboiero.github.io/Agentor"
echo ""
printf "${YELLOW}${BOLD}  Star the repo if you like what you see! ★${RESET}\n"
echo ""
printf "${DIM}  Demo recorded with asciinema — try it yourself:${RESET}\n"
printf "${DIM}  git clone https://github.com/fboiero/Agentor && cargo test --workspace${RESET}\n"
echo ""
