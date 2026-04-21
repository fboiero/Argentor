//! End-to-end demo automation for the Argentor framework.
//!
//! Demonstrates real tool execution with a scripted DemoBackend
//! that requires NO API keys. Run with:
//!
//!   cargo run -p argentor-cli --example demo_automation

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
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

// ── ANSI Colors ────────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";
const MAGENTA: &str = "\x1b[35m";
const RED: &str = "\x1b[31m";
const BLUE: &str = "\x1b[34m";

// ── DemoBackend ────────────────────────────────────────────────

/// A scripted LLM backend that returns predetermined responses.
/// No API keys needed — demonstrates the full agentic loop with real tool execution.
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
                "DemoBackend: no more scripted responses".into(),
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

// ── Response Builder ───────────────────────────────────────────

fn build_demo_responses(temp_dir: &str) -> Vec<LlmResponse> {
    let src_path = format!("{temp_dir}/hello_argentor.rs");
    let bin_path = format!("{temp_dir}/hello_argentor");

    vec![
        // Turn 0: Shell — gather system info
        LlmResponse::ToolUse {
            content: Some("I'll start by gathering system information to understand the environment.".into()),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "shell".into(),
                arguments: serde_json::json!({
                    "command": "echo \"OS: $(uname -s) $(uname -m)\" && echo \"User: $(whoami)\" && echo \"Date: $(date '+%Y-%m-%d %H:%M:%S')\" && echo \"Rust: $(rustc --version)\"",
                    "timeout_secs": 10
                }),
            }],
        },
        // Turn 1: FileWrite — create a Rust program
        LlmResponse::ToolUse {
            content: Some("System info collected. Now I'll create a Rust program that demonstrates computation.".into()),
            tool_calls: vec![ToolCall {
                id: "call_2".into(),
                name: "file_write".into(),
                arguments: serde_json::json!({
                    "path": src_path,
                    "content": concat!(
                        "fn main() {\n",
                        "    // Fibonacci sequence\n",
                        "    let mut fibs = vec![0u64, 1];\n",
                        "    for i in 2..20 {\n",
                        "        let next = fibs[i - 1] + fibs[i - 2];\n",
                        "        fibs.push(next);\n",
                        "    }\n",
                        "\n",
                        "    println!(\"Argentor Demo - Fibonacci Calculator\");\n",
                        "    println!(\"=====================================\");\n",
                        "    for (i, f) in fibs.iter().enumerate() {\n",
                        "        println!(\"  fib({:2}) = {}\", i, f);\n",
                        "    }\n",
                        "    println!(\"\\nSum of first 20 Fibonacci numbers: {}\", fibs.iter().sum::<u64>());\n",
                        "}\n"
                    ),
                    "create_dirs": true
                }),
            }],
        },
        // Turn 2: FileRead — verify the file
        LlmResponse::ToolUse {
            content: Some("File written. Let me verify the contents by reading it back.".into()),
            tool_calls: vec![ToolCall {
                id: "call_3".into(),
                name: "file_read".into(),
                arguments: serde_json::json!({
                    "path": src_path
                }),
            }],
        },
        // Turn 3: Shell — compile and run
        LlmResponse::ToolUse {
            content: Some("File verified. Compiling and running the program with rustc.".into()),
            tool_calls: vec![ToolCall {
                id: "call_4".into(),
                name: "shell".into(),
                arguments: serde_json::json!({
                    "command": format!("rustc {src_path} -o {bin_path} && {bin_path}"),
                    "timeout_secs": 30
                }),
            }],
        },
        // Turn 4: MemoryStore — save compilation result
        LlmResponse::ToolUse {
            content: Some("Program compiled and ran successfully! Storing the result in semantic memory.".into()),
            tool_calls: vec![ToolCall {
                id: "call_5".into(),
                name: "memory_store".into(),
                arguments: serde_json::json!({
                    "content": "Successfully compiled and executed a Rust Fibonacci program. The program computed the first 20 Fibonacci numbers and their sum. Compilation was successful with no errors or warnings.",
                    "metadata": {
                        "task": "demo_automation",
                        "language": "rust",
                        "result": "success",
                        "program": "fibonacci"
                    }
                }),
            }],
        },
        // Turn 5: MemoryStore — save system info
        LlmResponse::ToolUse {
            content: Some("Storing system information in memory for future reference.".into()),
            tool_calls: vec![ToolCall {
                id: "call_6".into(),
                name: "memory_store".into(),
                arguments: serde_json::json!({
                    "content": "System environment verified: capable of Rust compilation. Development tools are properly configured. Platform supports full Argentor skill execution.",
                    "metadata": {
                        "task": "demo_automation",
                        "type": "system_info"
                    }
                }),
            }],
        },
        // Turn 6: MemorySearch — retrieve results
        LlmResponse::ToolUse {
            content: Some("Now searching semantic memory to verify stored results.".into()),
            tool_calls: vec![ToolCall {
                id: "call_7".into(),
                name: "memory_search".into(),
                arguments: serde_json::json!({
                    "query": "Rust Fibonacci compilation result",
                    "top_k": 5
                }),
            }],
        },
        // Turn 7: Done — final summary
        LlmResponse::Done(
            "Demo automation complete! Here's what I accomplished:\n\n\
             1. Gathered system info (OS, user, date, Rust version)\n\
             2. Created a Fibonacci calculator in Rust (hello_argentor.rs)\n\
             3. Verified the source file with file_read\n\
             4. Compiled and executed the program with rustc\n\
             5. Stored compilation results in semantic memory\n\
             6. Stored system info in semantic memory\n\
             7. Searched memory and retrieved stored results\n\n\
             All 7 steps executed REAL tools — no mocks, no API keys.\n\
             The agentic loop ran end-to-end with permission checks and audit logging."
                .into(),
        ),
    ]
}

// ── Print Helpers ──────────────────────────────────────────────

fn print_banner(personality: &AgentPersonality, skill_count: usize) {
    println!();
    println!("{BOLD}{CYAN}+==================================================+{RESET}");
    println!("{BOLD}{CYAN}|         ARGENTOR  --  Demo Automation             |{RESET}");
    println!("{BOLD}{CYAN}|    End-to-end agent execution, no API keys       |{RESET}");
    println!("{BOLD}{CYAN}+==================================================+{RESET}");
    println!();
    println!(
        "{BOLD}Agent:{RESET} {} ({}) ",
        personality.name, personality.role
    );
    println!("{BOLD}Skills registered:{RESET} {skill_count}");
    println!(
        "{BOLD}Thinking level:{RESET} {:?}",
        personality.thinking_level
    );
    println!();
}

fn print_section(title: &str) {
    println!(
        "{DIM}-- {title} {}--{RESET}",
        "-".repeat(48_usize.saturating_sub(title.len() + 4))
    );
}

fn print_user(input: &str) {
    print_section("USER");
    println!("{CYAN}{input}{RESET}");
    println!();
}

fn print_turn(turn: u32) {
    print_section(&format!("TURN {turn}"));
}

fn print_thinking(text: &str) {
    println!("{YELLOW}  [thinking] {text}{RESET}");
}

fn print_tool_call(call: &ToolCall) {
    let args_str = serde_json::to_string(&call.arguments).unwrap_or_default();
    let truncated = if args_str.len() > 120 {
        format!("{}...", &args_str[..117])
    } else {
        args_str
    };
    println!(
        "{MAGENTA}  [tool_call] {BOLD}{}{RESET}{MAGENTA} {truncated}{RESET}",
        call.name
    );
}

fn print_tool_result(name: &str, content: &str, is_error: bool) {
    let lines: Vec<&str> = content.lines().collect();
    let preview = if lines.len() > 8 {
        let mut s = lines[..6].join("\n");
        s.push_str(&format!("\n  ... ({} more lines)", lines.len() - 6));
        s
    } else {
        content.to_string()
    };

    if is_error {
        println!("{RED}  [error] {name}: {preview}{RESET}");
    } else {
        println!("{GREEN}  [result] {name}:{RESET}");
        for line in preview.lines() {
            println!("{GREEN}    {line}{RESET}");
        }
    }
    println!();
}

fn print_final_response(text: &str) {
    println!();
    print_section("FINAL RESPONSE");
    for line in text.lines() {
        println!("{BOLD}{GREEN}  {line}{RESET}");
    }
    println!();
}

fn print_stats(session: &Session, duration: std::time::Duration, tools_called: u32) {
    print_section("STATS");
    println!(
        "  Turns: {BOLD}{}{RESET} | Tools called: {BOLD}{}{RESET} | Duration: {BOLD}{:.2}s{RESET}",
        tools_called + 1, // +1 for the Done turn
        tools_called,
        duration.as_secs_f64()
    );
    println!(
        "  Messages in session: {BOLD}{}{RESET} | Session ID: {DIM}{}{RESET}",
        session.message_count(),
        &session.id.to_string()[..8]
    );
    println!();
}

fn print_audit_trail(audit_dir: &Path) {
    let log_path = audit_dir.join("audit.jsonl");
    // Small delay to let the async audit writer flush
    std::thread::sleep(std::time::Duration::from_millis(200));

    if !log_path.exists() {
        println!("{DIM}  (audit log not yet flushed){RESET}");
        return;
    }

    print_section("AUDIT TRAIL");
    match argentor_security::query_audit_log(&log_path, &argentor_security::AuditFilter::all()) {
        Ok(result) => {
            for entry in &result.entries {
                let outcome = match &entry.outcome {
                    AuditOutcome::Success => format!("{GREEN}OK{RESET}"),
                    AuditOutcome::Denied => format!("{RED}DENIED{RESET}"),
                    AuditOutcome::Error => format!("{RED}ERROR{RESET}"),
                };
                let skill = entry.skill_name.as_deref().unwrap_or("-");
                println!(
                    "  {DIM}[{}]{RESET} {outcome}  {BOLD}{:<14}{RESET} {DIM}{}{RESET}",
                    entry.timestamp.format("%H:%M:%S"),
                    entry.action,
                    skill,
                );
            }
            println!();
            println!(
                "  {BLUE}Summary: {} entries | {} success | {} errors | {} unique skills{RESET}",
                result.total_scanned,
                result.stats.success_count,
                result.stats.error_count,
                result.stats.unique_skills,
            );
        }
        Err(e) => {
            println!("{RED}  Failed to read audit log: {e}{RESET}");
        }
    }
    println!();
}

// ── Main ───────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Suppress framework tracing — demo uses its own output
    tracing_subscriber::fmt().with_env_filter("warn").init();

    // 1. Create temp directory for demo artifacts
    let temp_dir = std::env::temp_dir().join(format!("argentor_demo_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir)?;
    let temp_dir_str = temp_dir.to_string_lossy().to_string();
    let audit_dir = temp_dir.join("audit");

    // 2. Build DemoBackend with predetermined responses
    let responses = build_demo_responses(&temp_dir_str);
    let backend = DemoBackend::new(responses);

    // 3. Set up SkillRegistry with memory
    let store: Arc<dyn VectorStore> = Arc::new(InMemoryVectorStore::new());
    let embedder = Arc::new(LocalEmbedding::default());
    let registry = SkillRegistry::new();
    register_builtins_with_memory(&registry, store, embedder);

    // 4. Set up permissions
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

    // 5. Create audit log
    let audit = Arc::new(AuditLog::new(audit_dir.clone()));

    // 6. Create AgentPersonality
    let personality = AgentPersonality {
        name: "Argentor Demo".into(),
        role: "an autonomous automation agent".into(),
        instructions: "You perform multi-step automation tasks using real tools. \
                       Execute shell commands, create files, compile code, and manage semantic memory."
            .into(),
        style: CommunicationStyle {
            tone: "precise and efficient".into(),
            language: Some("English".into()),
            use_markdown: true,
            max_response_length: None,
        },
        constraints: vec![
            "Never execute destructive operations".into(),
            "Always verify results before proceeding".into(),
        ],
        expertise: vec![
            "Rust".into(),
            "automation".into(),
            "system administration".into(),
        ],
        thinking_level: ThinkingLevel::Medium,
    };

    let system_prompt = personality.to_system_prompt();

    // 7. Print banner
    print_banner(&personality, registry.skill_count());

    // 8. Create session
    let mut session = Session::new();

    // 9. User prompt
    let user_input = "Gather system info, create a Rust Fibonacci program, compile and run it, \
                      then store results in semantic memory and search for them.";
    print_user(user_input);

    let user_msg = Message::user(user_input, session.id);
    session.add_message(user_msg);

    // 10. Collect tool descriptors
    let tool_descriptors = registry.list_descriptors();

    // 11. Run the agentic loop
    let start = std::time::Instant::now();
    let mut tools_called = 0u32;
    let max_turns = 10u32;

    for turn in 0..max_turns {
        // Call the "LLM"
        let response = backend
            .chat(Some(&system_prompt), &session.messages, &tool_descriptors)
            .await?;

        match response {
            LlmResponse::Done(text) => {
                let msg = Message::assistant(&text, session.id);
                session.add_message(msg);

                audit.log_action(
                    session.id,
                    "agent_response",
                    None,
                    serde_json::json!({"turn": turn, "type": "final"}),
                    AuditOutcome::Success,
                );

                print_final_response(&text);
                break;
            }

            LlmResponse::Text(text) => {
                print_turn(turn);
                print_thinking(&text);
                let msg = Message::assistant(&text, session.id);
                session.add_message(msg);
            }

            LlmResponse::ToolUse {
                content,
                tool_calls,
            } => {
                print_turn(turn);

                if let Some(text) = &content {
                    print_thinking(text);
                    let msg = Message::assistant(text, session.id);
                    session.add_message(msg);
                }

                for call in tool_calls {
                    print_tool_call(&call);

                    audit.log_action(
                        session.id,
                        "tool_call",
                        Some(call.name.clone()),
                        serde_json::json!({
                            "call_id": call.id,
                            "arguments": call.arguments,
                        }),
                        AuditOutcome::Success,
                    );

                    // Execute the REAL tool
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
                        serde_json::json!({
                            "call_id": result.call_id,
                            "is_error": result.is_error,
                        }),
                        outcome,
                    );

                    print_tool_result(&call.name, &result.content, result.is_error);

                    // Backfill tool result as message
                    let result_content = serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": result.call_id,
                        "content": result.content,
                        "is_error": result.is_error,
                    });
                    let tool_msg = Message::new(Role::User, result_content.to_string(), session.id);
                    session.add_message(tool_msg);
                }
            }
        }
    }

    let duration = start.elapsed();

    // 12. Print stats
    print_stats(&session, duration, tools_called);

    // 13. Print audit trail
    print_audit_trail(&audit_dir);

    // 14. Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);

    println!("{BOLD}{CYAN}Demo finished. All tools executed real operations.{RESET}");
    println!();

    Ok(())
}
