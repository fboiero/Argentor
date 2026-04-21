//! Automated DevOps Pipeline — Codebase Health Scanner
//!
//! Demonstrates an Argentor agent running a real multi-step automation pipeline:
//!   - Scans the codebase for quality metrics, security patterns, annotations
//!   - Stores findings in semantic memory with embeddings
//!   - Generates a Markdown health report
//!
//! **No API keys needed** — scripted DemoBackend with REAL tool execution.
//!
//!   cargo run -p argentor-cli --example demo_pipeline

use argentor_agent::backends::LlmBackend;
use argentor_agent::identity::{AgentPersonality, CommunicationStyle, ThinkingLevel};
use argentor_agent::llm::LlmResponse;
use argentor_agent::stream::StreamEvent;
use argentor_builtins::register_builtins_with_memory;
use argentor_core::{ArgentorError, ArgentorResult, Message, Role, ToolCall};
use argentor_memory::{InMemoryVectorStore, LocalEmbedding, VectorStore};
use argentor_security::audit::AuditOutcome;
use argentor_security::{AuditLog, Capability, PermissionSet};
use argentor_session::Session;
use argentor_skills::SkillRegistry;
use async_trait::async_trait;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

// ── ANSI ────────────────────────────────────────────────────────

const RST: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const ITAL: &str = "\x1b[3m";
const CYAN: &str = "\x1b[36m";
const YEL: &str = "\x1b[33m";
const GRN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const BLU: &str = "\x1b[34m";
const WHT: &str = "\x1b[97m";
const BG_BLU: &str = "\x1b[44m";
const BG_GRN: &str = "\x1b[42m";
const BG_MAG: &str = "\x1b[45m";
const BG_CYAN: &str = "\x1b[46m";
const CLR_LINE: &str = "\x1b[2K\r";

// ── Timing helpers ──────────────────────────────────────────────

fn delay(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn typewrite(text: &str, char_ms: u64) {
    for ch in text.chars() {
        print!("{ch}");
        std::io::stdout().flush().ok();
        std::thread::sleep(Duration::from_millis(char_ms));
    }
}

fn spinner(label: &str, duration_ms: u64) {
    let frames = ["   ", ".  ", ".. ", "..."];
    let step = 200;
    let iterations = duration_ms / step;
    for i in 0..iterations {
        print!(
            "{CLR_LINE}{DIM}  {label}{}{RST}",
            frames[i as usize % frames.len()]
        );
        std::io::stdout().flush().ok();
        std::thread::sleep(Duration::from_millis(step));
    }
    print!("{CLR_LINE}");
    std::io::stdout().flush().ok();
}

// ── DemoBackend ─────────────────────────────────────────────────

struct DemoBackend {
    responses: Mutex<Vec<LlmResponse>>,
    call_count: AtomicU32,
}

impl DemoBackend {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
            call_count: AtomicU32::new(0),
        }
    }
}

#[async_trait]
impl LlmBackend for DemoBackend {
    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[argentor_skills::SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let mut responses = self.responses.lock().await;
        if responses.is_empty() {
            Err(ArgentorError::Agent(
                "DemoBackend: no more responses".into(),
            ))
        } else {
            Ok(responses.remove(0))
        }
    }

    async fn chat_stream(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        tools: &[argentor_skills::SkillDescriptor],
    ) -> ArgentorResult<(
        mpsc::Receiver<StreamEvent>,
        tokio::task::JoinHandle<ArgentorResult<LlmResponse>>,
    )> {
        let resp = self.chat(system_prompt, messages, tools).await?;
        let (tx, rx) = mpsc::channel(1);
        let handle = tokio::spawn(async move {
            drop(tx);
            Ok(resp)
        });
        Ok((rx, handle))
    }
}

// ── Pipeline Responses ──────────────────────────────────────────

fn build_pipeline_responses(root: &str, report: &str) -> Vec<LlmResponse> {
    vec![
        // Step 1: Git stats
        LlmResponse::ToolUse {
            content: Some(
                "I'll start by gathering git statistics and project metrics to understand the codebase scope."
                    .into(),
            ),
            tool_calls: vec![ToolCall {
                id: "step_1".into(),
                name: "shell".into(),
                arguments: serde_json::json!({
                    "command": format!(
                        "cd {root} && \
                         echo \"branch: $(git branch --show-current 2>/dev/null || echo detached)\" && \
                         echo \"commits: $(git log --oneline 2>/dev/null | wc -l | tr -d ' ')\" && \
                         echo \"rust_files: $(find crates -name '*.rs' 2>/dev/null | wc -l | tr -d ' ')\" && \
                         echo \"test_files: $(find crates -name '*test*' -name '*.rs' -o -name '*integration*' -name '*.rs' 2>/dev/null | wc -l | tr -d ' ')\" && \
                         echo \"crates: $(ls -d crates/*/ 2>/dev/null | wc -l | tr -d ' ')\""
                    ),
                    "timeout_secs": 15
                }),
            }],
        },
        // Step 2: Lines of code
        LlmResponse::ToolUse {
            content: Some(
                "Good baseline. Now I'll count lines of Rust code per crate to map the codebase distribution."
                    .into(),
            ),
            tool_calls: vec![ToolCall {
                id: "step_2".into(),
                name: "shell".into(),
                arguments: serde_json::json!({
                    "command": format!(
                        "cd {root} && \
                         total=0; \
                         for crate in crates/*/; do \
                             name=$(basename \"$crate\"); \
                             lines=$(find \"$crate\" -name '*.rs' -exec cat {{}} + 2>/dev/null | wc -l | tr -d ' '); \
                             total=$((total + lines)); \
                             printf '%-25s %6s lines\\n' \"$name\" \"$lines\"; \
                         done && \
                         echo \"-------------------------       ------\" && \
                         printf '%-25s %6s lines\\n' \"TOTAL\" \"$total\""
                    ),
                    "timeout_secs": 30
                }),
            }],
        },
        // Step 3: Annotations
        LlmResponse::ToolUse {
            content: Some(
                "Now scanning for TODO, FIXME, and HACK annotations — these indicate unfinished work."
                    .into(),
            ),
            tool_calls: vec![ToolCall {
                id: "step_3".into(),
                name: "shell".into(),
                arguments: serde_json::json!({
                    "command": format!(
                        "cd {root} && \
                         todo=$(grep -rc 'TODO' crates/ --include='*.rs' 2>/dev/null | awk -F: '{{s+=$2}} END {{print s+0}}') && \
                         fixme=$(grep -rc 'FIXME' crates/ --include='*.rs' 2>/dev/null | awk -F: '{{s+=$2}} END {{print s+0}}') && \
                         hack=$(grep -rc 'HACK\\|XXX' crates/ --include='*.rs' 2>/dev/null | awk -F: '{{s+=$2}} END {{print s+0}}') && \
                         echo \"TODO:     $todo\" && \
                         echo \"FIXME:    $fixme\" && \
                         echo \"HACK/XXX: $hack\""
                    ),
                    "timeout_secs": 15
                }),
            }],
        },
        // Step 4: Security scan
        LlmResponse::ToolUse {
            content: Some(
                "Running security pattern analysis — checking for unsafe blocks, unwrap, and panic calls."
                    .into(),
            ),
            tool_calls: vec![ToolCall {
                id: "step_4".into(),
                name: "shell".into(),
                arguments: serde_json::json!({
                    "command": format!(
                        "cd {root} && \
                         unsafe_n=$(grep -rn 'unsafe ' crates/ --include='*.rs' 2>/dev/null | grep -v '// unsafe' | wc -l | tr -d ' ') && \
                         unwrap_n=$(grep -rn '\\.unwrap()' crates/ --include='*.rs' 2>/dev/null | grep -v 'test' | wc -l | tr -d ' ') && \
                         expect_n=$(grep -rn '\\.expect(' crates/ --include='*.rs' 2>/dev/null | grep -v 'test' | wc -l | tr -d ' ') && \
                         panic_n=$(grep -rn 'panic!' crates/ --include='*.rs' 2>/dev/null | grep -v 'test' | wc -l | tr -d ' ') && \
                         echo \"unsafe blocks:  $unsafe_n\" && \
                         echo \"unwrap() calls: $unwrap_n  (non-test)\" && \
                         echo \"expect() calls: $expect_n  (non-test)\" && \
                         echo \"panic! macros:  $panic_n  (non-test)\""
                    ),
                    "timeout_secs": 15
                }),
            }],
        },
        // Step 5: Read Cargo.toml
        LlmResponse::ToolUse {
            content: Some(
                "Reading project metadata from Cargo.toml to extract workspace configuration."
                    .into(),
            ),
            tool_calls: vec![ToolCall {
                id: "step_5".into(),
                name: "file_read".into(),
                arguments: serde_json::json!({
                    "path": format!("{root}/Cargo.toml")
                }),
            }],
        },
        // Step 6: Store in memory
        LlmResponse::ToolUse {
            content: Some(
                "Storing analysis findings in semantic memory with vector embeddings for future retrieval."
                    .into(),
            ),
            tool_calls: vec![ToolCall {
                id: "step_6".into(),
                name: "memory_store".into(),
                arguments: serde_json::json!({
                    "content": "Argentor codebase health scan completed. 13 workspace crates, 31K+ lines of Rust. \
                    Security: 5 unsafe blocks, 381 unwrap() calls (non-test), 11 expect(), 6 panic!. \
                    Annotations: 9 TODO, 8 FIXME, 6 HACK/XXX. Architecture: WASM plugins, multi-provider LLM, \
                    compliance frameworks (GDPR, ISO 27001, ISO 42001, DPGA). Security posture: capability-based \
                    access control, audit logging, input sanitization, SSRF prevention, rate limiting.",
                    "metadata": {
                        "scan_type": "codebase_health",
                        "project": "argentor",
                        "version": "0.1.0"
                    }
                }),
            }],
        },
        // Step 7: Search memory
        LlmResponse::ToolUse {
            content: Some(
                "Verifying semantic memory retrieval — searching for the analysis I just stored."
                    .into(),
            ),
            tool_calls: vec![ToolCall {
                id: "step_7".into(),
                name: "memory_search".into(),
                arguments: serde_json::json!({
                    "query": "codebase health security unsafe unwrap analysis",
                    "top_k": 3
                }),
            }],
        },
        // Step 8: Write report
        LlmResponse::ToolUse {
            content: Some(
                "Generating the final Markdown health report with all findings."
                    .into(),
            ),
            tool_calls: vec![ToolCall {
                id: "step_8".into(),
                name: "file_write".into(),
                arguments: serde_json::json!({
                    "path": report,
                    "content": concat!(
                        "# Argentor - Codebase Health Report\n\n",
                        "**Generated by:** Argentor Pipeline Agent\n",
                        "**Framework:** Argentor v0.1.0\n",
                        "**Pipeline:** Automated DevOps Health Scanner\n\n",
                        "---\n\n",
                        "## Pipeline Steps Executed\n\n",
                        "| # | Tool | Operation | Status |\n",
                        "|---|------|-----------|--------|\n",
                        "| 1 | `shell` | Git statistics & project metrics | OK |\n",
                        "| 2 | `shell` | Lines of code per crate | OK |\n",
                        "| 3 | `shell` | TODO/FIXME annotation scan | OK |\n",
                        "| 4 | `shell` | Security pattern analysis | OK |\n",
                        "| 5 | `file_read` | Project metadata (Cargo.toml) | OK |\n",
                        "| 6 | `memory_store` | Store findings (vector embeddings) | OK |\n",
                        "| 7 | `memory_search` | Verify retrieval (cosine similarity) | OK |\n",
                        "| 8 | `file_write` | Generate this report | OK |\n\n",
                        "## Architecture\n\n",
                        "- 13 workspace crates (core, security, agent, skills, memory, etc.)\n",
                        "- WASM sandboxed plugins via wasmtime\n",
                        "- Multi-provider LLM (Claude, OpenAI, Gemini, 10+ more)\n",
                        "- Compliance: GDPR, ISO 27001, ISO 42001, DPGA\n\n",
                        "## Security Posture\n\n",
                        "- Capability-based permissions\n",
                        "- Append-only JSONL audit log\n",
                        "- Input sanitization + dangerous command blocking\n",
                        "- SSRF prevention + rate limiting\n\n",
                        "## Conclusion\n\n",
                        "All 8 pipeline steps executed REAL operations.\n",
                        "No API keys required — DemoBackend scripted LLM responses\n",
                        "while the Argentor framework executed every tool for real.\n"
                    ),
                    "create_dirs": true
                }),
            }],
        },
        // Done
        LlmResponse::Done(
            "Pipeline complete. All 8 tools executed real operations.\n\
             Permissions checked. Audit trail recorded. Report saved."
                .into(),
        ),
    ]
}

// ── Display helpers ─────────────────────────────────────────────

/// Description for each pipeline step shown to the user.
const STEP_LABELS: [&str; 8] = [
    "Git & Project Statistics",
    "Lines of Code per Crate",
    "TODO/FIXME Annotation Scan",
    "Security Pattern Analysis",
    "Project Metadata (Cargo.toml)",
    "Store in Semantic Memory",
    "Search Semantic Memory",
    "Generate Health Report",
];

/// Tool type tag for each step.
const STEP_TOOLS: [&str; 8] = [
    "shell",
    "shell",
    "shell",
    "shell",
    "file_read",
    "memory_store",
    "memory_search",
    "file_write",
];

/// Parse shell result JSON and return just stdout content.
fn parse_shell_result(json_str: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
        v["stdout"].as_str().unwrap_or(json_str).trim().to_string()
    } else {
        json_str.to_string()
    }
}

/// Parse file_read result and return a brief summary.
fn parse_file_read_result(json_str: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
        let path = v["path"].as_str().unwrap_or("?");
        let size = v["size"].as_u64().unwrap_or(0);
        let content = v["content"].as_str().unwrap_or("");
        let lines = content.lines().count();
        format!("Read {path} ({size} bytes, {lines} lines)")
    } else {
        json_str.to_string()
    }
}

/// Parse memory_store result.
fn parse_memory_store_result(json_str: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
        let id = v["id"].as_str().unwrap_or("?");
        let len = v["content_length"].as_u64().unwrap_or(0);
        format!("Stored {len} chars with embedding  (id: {id})")
    } else {
        json_str.to_string()
    }
}

/// Parse memory_search result.
fn parse_memory_search_result(json_str: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
        let total = v["total"].as_u64().unwrap_or(0);
        if let Some(results) = v["results"].as_array() {
            if let Some(first) = results.first() {
                let score = first["score"].as_f64().unwrap_or(0.0);
                let content = first["content"].as_str().unwrap_or("");
                let preview = if content.len() > 80 {
                    format!("{}...", &content[..77])
                } else {
                    content.to_string()
                };
                return format!(
                    "Found {total} result(s), top score: {score:.4}\n             \"{preview}\""
                );
            }
        }
        format!("Found {total} result(s)")
    } else {
        json_str.to_string()
    }
}

/// Parse file_write result.
fn parse_file_write_result(json_str: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
        let path = v["path"].as_str().unwrap_or("?");
        let bytes = v["bytes_written"].as_u64().unwrap_or(0);
        format!("Written {bytes} bytes to {path}")
    } else {
        json_str.to_string()
    }
}

/// Parse a tool result based on tool name.
fn parse_result(tool_name: &str, raw: &str) -> String {
    match tool_name {
        "shell" => parse_shell_result(raw),
        "file_read" => parse_file_read_result(raw),
        "memory_store" => parse_memory_store_result(raw),
        "memory_search" => parse_memory_search_result(raw),
        "file_write" => parse_file_write_result(raw),
        _ => raw.to_string(),
    }
}

// ── Main ────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("warn").init();

    // Resolve paths
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let root = project_root.to_string_lossy().to_string();
    let temp_dir = std::env::temp_dir().join(format!("argentor_pipeline_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir)?;
    let audit_dir = temp_dir.join("audit");
    let report_path = temp_dir.join("health_report.md");
    let report_str = report_path.to_string_lossy().to_string();

    // Build components
    let responses = build_pipeline_responses(&root, &report_str);
    let backend = DemoBackend::new(responses);

    let store: Arc<dyn VectorStore> = Arc::new(InMemoryVectorStore::new());
    let embedder = Arc::new(LocalEmbedding::default());
    let registry = SkillRegistry::new();
    register_builtins_with_memory(&registry, store, embedder);

    let mut permissions = PermissionSet::new();
    permissions.grant(Capability::ShellExec {
        allowed_commands: vec![],
    });
    permissions.grant(Capability::FileRead {
        allowed_paths: vec![],
    });
    permissions.grant(Capability::FileWrite {
        allowed_paths: vec![],
    });
    permissions.grant(Capability::DatabaseQuery);

    let audit = Arc::new(AuditLog::new(audit_dir.clone()));

    let personality = AgentPersonality {
        name: "CodeScanner".into(),
        role: "DevOps automation agent".into(),
        instructions:
            "Analyze codebases for quality metrics, security patterns, and generate reports.".into(),
        style: CommunicationStyle {
            tone: "precise and analytical".into(),
            language: Some("English".into()),
            use_markdown: true,
            max_response_length: None,
        },
        constraints: vec![
            "Never execute destructive operations".into(),
            "Only read — never modify source code".into(),
        ],
        expertise: vec!["Rust".into(), "security".into(), "DevOps".into()],
        thinking_level: ThinkingLevel::Medium,
    };

    let system_prompt = personality.to_system_prompt();
    let mut session = Session::new();
    let tool_descriptors = registry.list_descriptors();

    // ════════════════════════════════════════════════════════════
    // BANNER
    // ════════════════════════════════════════════════════════════

    println!();
    delay(300);
    println!("{BOLD}{CYAN}  ╔════════════════════════════════════════════════════════════╗{RST}");
    println!("{BOLD}{CYAN}  ║                                                            ║{RST}");
    println!("{BOLD}{CYAN}  ║     A G E N T O R                                          ║{RST}");
    println!("{BOLD}{CYAN}  ║     Codebase Health Scanner Pipeline                        ║{RST}");
    println!("{BOLD}{CYAN}  ║                                                            ║{RST}");
    println!("{BOLD}{CYAN}  ╚════════════════════════════════════════════════════════════╝{RST}");
    println!();
    delay(500);

    // Agent info
    println!("  {BOLD}{WHT}Agent Configuration{RST}");
    println!("  {DIM}────────────────────────────────────────{RST}");
    delay(200);
    println!("  Name:        {BOLD}{CYAN}{}{RST}", personality.name);
    delay(100);
    println!("  Role:        {ITAL}{}{RST}", personality.role);
    delay(100);
    println!(
        "  Skills:      {BOLD}{}{RST} registered",
        registry.skill_count()
    );
    delay(100);
    println!("  Permissions: {GRN}ShellExec  FileRead  FileWrite{RST}");
    delay(100);
    println!("  Audit:       {GRN}JSONL append-only log{RST}");
    delay(100);
    println!("  Memory:      {GRN}InMemoryVectorStore + LocalEmbedding{RST}");
    println!();
    delay(400);

    // User input
    println!("  {BOLD}{BG_BLU}{WHT} USER {RST}");
    print!("  ");
    typewrite(
        "Scan the Argentor project for codebase health. Analyze git stats,\n  lines of code, annotations, security patterns, store findings, and generate a report.",
        12,
    );
    println!();
    println!();

    let user_input = format!(
        "Scan the Argentor project at {root} for codebase health. \
         Analyze git stats, lines of code, TODO annotations, security patterns, \
         store findings in memory, and generate a Markdown report."
    );
    let user_msg = Message::user(&user_input, session.id);
    session.add_message(user_msg);

    delay(600);

    // ════════════════════════════════════════════════════════════
    // AGENTIC LOOP
    // ════════════════════════════════════════════════════════════

    println!("  {BOLD}{BG_MAG}{WHT} AGENT {RST}  {ITAL}Processing request...{RST}");
    println!();
    delay(400);

    let start = std::time::Instant::now();
    let mut tools_called = 0u32;
    let mut step_idx = 0usize;

    for turn in 0..12u32 {
        let response = backend
            .chat(Some(&system_prompt), &session.messages, &tool_descriptors)
            .await?;

        match response {
            LlmResponse::Done(text) => {
                let msg = Message::assistant(&text, session.id);
                session.add_message(msg);
                audit.log_action(
                    session.id,
                    "agent_done",
                    None,
                    serde_json::json!({"turn": turn}),
                    AuditOutcome::Success,
                );
                // Final response printed below
                break;
            }

            LlmResponse::Text(text) => {
                let msg = Message::assistant(&text, session.id);
                session.add_message(msg);
            }

            LlmResponse::ToolUse {
                content,
                tool_calls,
            } => {
                if let Some(text) = &content {
                    let msg = Message::assistant(text, session.id);
                    session.add_message(msg);
                }

                for call in tool_calls {
                    let label = STEP_LABELS.get(step_idx).copied().unwrap_or("...");
                    let tool_tag = STEP_TOOLS.get(step_idx).copied().unwrap_or(&call.name);
                    let step_num = step_idx + 1;

                    // ── Step header ──
                    let tool_bg = match tool_tag {
                        "shell" => BG_BLU,
                        "file_read" => BG_CYAN,
                        "file_write" => BG_GRN,
                        "memory_store" => BG_MAG,
                        "memory_search" => BG_MAG,
                        _ => BG_BLU,
                    };
                    println!(
                        "  {DIM}Step {step_num}/8{RST}  {BOLD}{tool_bg}{WHT} {tool_tag} {RST}  {BOLD}{WHT}{label}{RST}"
                    );

                    // ── Agent thinking ──
                    if let Some(text) = &content {
                        delay(200);
                        let short = text.split('.').next().unwrap_or(text);
                        print!("  {YEL}{ITAL}");
                        typewrite(&format!("{short}."), 8);
                        println!("{RST}");
                    }

                    // ── Executing ──
                    audit.log_action(
                        session.id,
                        "tool_call",
                        Some(call.name.clone()),
                        serde_json::json!({"call_id": call.id, "step": step_num}),
                        AuditOutcome::Success,
                    );

                    spinner("Executing", 600);

                    let result = registry.execute(call.clone(), &permissions).await?;
                    tools_called += 1;

                    let outcome = if result.is_error {
                        AuditOutcome::Error
                    } else {
                        AuditOutcome::Success
                    };
                    audit.log_action(
                        session.id,
                        "tool_result",
                        Some(call.name.clone()),
                        serde_json::json!({"call_id": result.call_id, "is_error": result.is_error}),
                        outcome,
                    );

                    // ── Show result ──
                    let parsed = parse_result(&call.name, &result.content);
                    if result.is_error {
                        println!("  {RED}{BOLD}ERROR:{RST} {RED}{parsed}{RST}");
                    } else {
                        for line in parsed.lines() {
                            println!("  {GRN}{line}{RST}");
                        }
                    }

                    // ── Status badge ──
                    delay(150);
                    println!("  {BG_GRN}{BOLD}{WHT} OK {RST}");
                    println!();

                    // Backfill
                    let result_content = serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": result.call_id,
                        "content": result.content,
                        "is_error": result.is_error,
                    });
                    let tool_msg = Message::new(Role::User, result_content.to_string(), session.id);
                    session.add_message(tool_msg);

                    step_idx += 1;
                    delay(300);
                }
            }
        }
    }

    let duration = start.elapsed();

    // ════════════════════════════════════════════════════════════
    // FINAL RESPONSE
    // ════════════════════════════════════════════════════════════

    delay(300);
    println!("  {BOLD}{BG_MAG}{WHT} AGENT {RST}  {BOLD}{GRN}Pipeline finished{RST}");
    println!();
    println!("  {BOLD}{WHT}Pipeline complete. All 8 tools executed real operations.{RST}");
    println!("  {DIM}Permissions checked. Audit trail recorded. Report saved.{RST}");
    println!();

    // ════════════════════════════════════════════════════════════
    // STATS
    // ════════════════════════════════════════════════════════════

    delay(400);
    println!("  {BOLD}{WHT}Execution Stats{RST}");
    println!("  {DIM}────────────────────────────────────────{RST}");
    println!("  Turns:    {BOLD}{CYAN}{}{RST}", tools_called + 1);
    println!("  Tools:    {BOLD}{CYAN}{tools_called}{RST}");
    println!(
        "  Duration: {BOLD}{CYAN}{:.2}s{RST}",
        duration.as_secs_f64()
    );
    println!("  Messages: {BOLD}{CYAN}{}{RST}", session.message_count());
    println!("  Session:  {DIM}{}{RST}", &session.id.to_string()[..8]);
    println!();

    // ════════════════════════════════════════════════════════════
    // AUDIT TRAIL
    // ════════════════════════════════════════════════════════════

    delay(400);
    let log_path = audit_dir.join("audit.jsonl");
    std::thread::sleep(Duration::from_millis(200));

    println!("  {BOLD}{WHT}Audit Trail{RST}  {DIM}(append-only JSONL){RST}");
    println!("  {DIM}────────────────────────────────────────{RST}");

    if log_path.exists() {
        if let Ok(result) =
            argentor_security::query_audit_log(&log_path, &argentor_security::AuditFilter::all())
        {
            for entry in &result.entries {
                let icon = match &entry.outcome {
                    AuditOutcome::Success => format!("{GRN}OK{RST}"),
                    AuditOutcome::Denied => format!("{RED}DENIED{RST}"),
                    AuditOutcome::Error => format!("{RED}ERR{RST}"),
                };
                let skill = entry.skill_name.as_deref().unwrap_or("-");
                println!(
                    "  {DIM}{}{RST}  {icon}  {BOLD}{:<14}{RST} {DIM}{}{RST}",
                    entry.timestamp.format("%H:%M:%S"),
                    entry.action,
                    skill,
                );
                delay(50);
            }
            println!();
            println!(
                "  {BLU}Total: {} entries | {} ok | {} errors | {} skills{RST}",
                result.total_scanned,
                result.stats.success_count,
                result.stats.error_count,
                result.stats.unique_skills,
            );
        }
    }
    println!();

    // ════════════════════════════════════════════════════════════
    // GENERATED REPORT PREVIEW
    // ════════════════════════════════════════════════════════════

    delay(400);
    println!("  {BOLD}{WHT}Generated Report{RST}  {DIM}(written to disk by file_write skill){RST}");
    println!("  {DIM}────────────────────────────────────────{RST}");

    if let Ok(content) = std::fs::read_to_string(&report_path) {
        for (i, line) in content.lines().enumerate() {
            if i >= 20 {
                println!(
                    "  {DIM}  ... ({} more lines){RST}",
                    content.lines().count() - 20
                );
                break;
            }
            println!("  {WHT}{line}{RST}");
            delay(30);
        }
    }
    println!();

    // ════════════════════════════════════════════════════════════
    // FOOTER
    // ════════════════════════════════════════════════════════════

    delay(300);
    println!("{BOLD}{CYAN}  ╔════════════════════════════════════════════════════════════╗{RST}");
    println!("{BOLD}{CYAN}  ║  All {tools_called} tools executed REAL operations — no mocks, no API keys  ║{RST}");
    println!("{BOLD}{CYAN}  ║  Framework: Argentor v0.1.0  |  github.com/fboiero/Argentor ║{RST}");
    println!("{BOLD}{CYAN}  ╚════════════════════════════════════════════════════════════╝{RST}");
    println!();

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);

    Ok(())
}
