#![allow(clippy::expect_used)]
//! Multi-Agent Team Orchestration Demo — URL Shortener Microservice
//!
//! Demonstrates a team of 6 AI developer agents collaborating on a real task
//! using the Argentor orchestration framework:
//!
//!   - **Architect** designs the API and data model
//!   - **Coder** implements the service
//!   - **Tester** writes tests
//!   - **SecurityAuditor** reviews for vulnerabilities
//!   - **Reviewer** performs final code review
//!   - **DocumentWriter** writes API documentation
//!
//! Features showcased:
//!   - Task queue with dependency-based topological ordering
//!   - Agent monitor tracking state and metrics
//!   - A2A message bus for inter-agent communication
//!   - Budget tracker for token/resource management
//!   - Real tool execution (file_write, file_read, shell) via DemoBackend
//!
//! **No API keys needed** — scripted DemoBackend with REAL tool execution.
//!
//!   cargo run -p argentor-cli --example demo_team

use argentor_agent::backends::LlmBackend;
use argentor_agent::llm::LlmResponse;
use argentor_agent::stream::StreamEvent;
use argentor_builtins::register_builtins;
use argentor_core::{ArgentorError, ArgentorResult, Message, ToolCall};
use argentor_orchestrator::budget::{default_budget, BudgetTracker};
use argentor_orchestrator::message_bus::{AgentMessage, BroadcastTarget, MessageBus, MessageType};
use argentor_orchestrator::monitor::AgentMonitor;
use argentor_orchestrator::task_queue::TaskQueue;
use argentor_orchestrator::types::{AgentRole, Task, TaskStatus};
use argentor_security::audit::AuditOutcome;
use argentor_security::{AuditLog, Capability, PermissionSet};
use argentor_session::Session;
use argentor_skills::SkillRegistry;
use async_trait::async_trait;
use std::io::Write;
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
const MAG: &str = "\x1b[35m";
const BLU: &str = "\x1b[34m";
const WHT: &str = "\x1b[97m";
const BG_BLU: &str = "\x1b[44m";
const BG_GRN: &str = "\x1b[42m";
const BG_MAG: &str = "\x1b[45m";
const BG_CYAN: &str = "\x1b[46m";
const BG_YEL: &str = "\x1b[43m";
const BG_RED: &str = "\x1b[41m";
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
    let step = 150;
    let iterations = duration_ms / step;
    for i in 0..iterations {
        print!(
            "{CLR_LINE}{DIM}    {label}{}{RST}",
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

// ── Agent color mapping ─────────────────────────────────────────

fn agent_color(role: &AgentRole) -> &'static str {
    match role {
        AgentRole::Architect => CYAN,
        AgentRole::Coder => GRN,
        AgentRole::Tester => YEL,
        AgentRole::SecurityAuditor => RED,
        AgentRole::Reviewer => MAG,
        AgentRole::DocumentWriter => BLU,
        _ => WHT,
    }
}

fn agent_bg(role: &AgentRole) -> &'static str {
    match role {
        AgentRole::Architect => BG_CYAN,
        AgentRole::Coder => BG_GRN,
        AgentRole::Tester => BG_YEL,
        AgentRole::SecurityAuditor => BG_RED,
        AgentRole::Reviewer => BG_MAG,
        AgentRole::DocumentWriter => BG_BLU,
        _ => BG_BLU,
    }
}

fn agent_label(role: &AgentRole) -> &'static str {
    match role {
        AgentRole::Architect => "Architect",
        AgentRole::Coder => "Coder",
        AgentRole::Tester => "Tester",
        AgentRole::SecurityAuditor => "SecurityAuditor",
        AgentRole::Reviewer => "Reviewer",
        AgentRole::DocumentWriter => "DocumentWriter",
        _ => "Agent",
    }
}

fn agent_emoji(role: &AgentRole) -> &'static str {
    match role {
        AgentRole::Architect => "  ",
        AgentRole::Coder => "  ",
        AgentRole::Tester => "  ",
        AgentRole::SecurityAuditor => "  ",
        AgentRole::Reviewer => "  ",
        AgentRole::DocumentWriter => "  ",
        _ => "  ",
    }
}

// ── Response builders per agent ─────────────────────────────────

fn architect_responses(work_dir: &str) -> Vec<LlmResponse> {
    let arch_path = format!("{work_dir}/architecture.md");
    vec![
        // Turn 1: Write architecture document
        LlmResponse::ToolUse {
            content: Some(
                "I'll design the API and data model for the URL shortener microservice.".into(),
            ),
            tool_calls: vec![ToolCall {
                id: "arch_1".into(),
                name: "file_write".into(),
                arguments: serde_json::json!({
                    "path": arch_path,
                    "content": "# URL Shortener Microservice - Architecture\n\n\
                ## API Endpoints\n\n\
                | Method | Path           | Description                    |\n\
                |--------|----------------|--------------------------------|\n\
                | POST   | /shorten       | Create a short URL             |\n\
                | GET    | /:code         | Redirect to original URL       |\n\
                | GET    | /stats/:code   | Get click statistics           |\n\
                | DELETE | /admin/:code   | Delete a short URL (admin)     |\n\n\
                ## Data Model\n\n\
                ```rust\n\
                struct UrlEntry {\n\
                code: String,          // 6-char alphanumeric code\n\
                original_url: String,  // The target URL\n\
                created_at: DateTime,  // Creation timestamp\n\
                click_count: u64,      // Number of redirects\n\
                expires_at: Option<DateTime>, // Optional expiry\n\
                }\n\
                ```\n\n\
                ## Storage\n\n\
                - In-memory HashMap<String, UrlEntry> for demo\n\
                - Thread-safe via Arc<RwLock<HashMap>>\n\
                - Production: PostgreSQL or Redis\n\n\
                ## Rate Limiting\n\n\
                - 100 requests/minute per IP for POST /shorten\n\
                - 1000 requests/minute per IP for GET /:code\n\
                - Token bucket algorithm\n\n\
                ## Security Considerations\n\n\
                - Input validation: reject non-HTTP(S) URLs\n\
                - Code generation: cryptographically random, avoid collisions\n\
                - Rate limiting to prevent abuse\n\
                - No SQL injection risk (HashMap storage)\n\
                - URL sanitization to prevent XSS in redirects\n",
                    "create_dirs": true
                }),
            }],
        },
        // Turn 2: Done
        LlmResponse::Done(
            "Architecture design complete. Defined 4 API endpoints (POST /shorten, \
             GET /:code, GET /stats/:code, DELETE /admin/:code), data model with \
             UrlEntry struct, HashMap storage with RwLock, rate limiting strategy, \
             and security considerations."
                .into(),
        ),
    ]
}

fn coder_responses(work_dir: &str) -> Vec<LlmResponse> {
    let code_path = format!("{work_dir}/url_shortener.rs");
    vec![
        // Turn 1: Write implementation
        LlmResponse::ToolUse {
            content: Some(
                "I'll implement the URL shortener based on the architecture spec.".into(),
            ),
            tool_calls: vec![ToolCall {
                id: "code_1".into(),
                name: "file_write".into(),
                arguments: serde_json::json!({
                    "path": code_path,
                    "content": "//! URL Shortener Microservice Implementation\n\
                //!\n\
                //! A simple, thread-safe URL shortener with in-memory storage.\n\n\
                use std::collections::HashMap;\n\
                use std::sync::Arc;\n\
                use std::time::SystemTime;\n\n\
                /// Represents a shortened URL entry.\n\
                #[derive(Debug, Clone)]\n\
                pub struct UrlEntry {\n\
                pub code: String,\n\
                pub original_url: String,\n\
                pub created_at: SystemTime,\n\
                pub click_count: u64,\n\
                }\n\n\
                impl UrlEntry {\n\
                fn new(code: String, original_url: String) -> Self {\n\
                Self {\n\
                code,\n\
                original_url,\n\
                created_at: SystemTime::now(),\n\
                click_count: 0,\n\
                }\n\
                }\n\
                }\n\n\
                /// Thread-safe URL shortener service.\n\
                pub struct UrlShortener {\n\
                store: Arc<std::sync::RwLock<HashMap<String, UrlEntry>>>,\n\
                counter: Arc<std::sync::atomic::AtomicU64>,\n\
                }\n\n\
                impl UrlShortener {\n\
                /// Create a new URL shortener instance.\n\
                pub fn new() -> Self {\n\
                Self {\n\
                store: Arc::new(std::sync::RwLock::new(HashMap::new())),\n\
                counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),\n\
                }\n\
                }\n\n\
                /// Validate that a URL is well-formed and uses HTTP(S).\n\
                fn validate_url(url: &str) -> Result<(), String> {\n\
                if url.is_empty() {\n\
                return Err(\"URL cannot be empty\".to_string());\n\
                }\n\
                if !url.starts_with(\"http://\") && !url.starts_with(\"https://\") {\n\
                return Err(\"URL must start with http:// or https://\".to_string());\n\
                }\n\
                if url.len() > 2048 {\n\
                return Err(\"URL exceeds maximum length of 2048 characters\".to_string());\n\
                }\n\
                Ok(())\n\
                }\n\n\
                /// Generate a unique 6-character code.\n\
                fn generate_code(&self) -> String {\n\
                let n = self.counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);\n\
                let chars: Vec<char> = \"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\"\n\
                .chars().collect();\n\
                let base = chars.len() as u64;\n\
                let mut code = String::with_capacity(6);\n\
                let mut val = n + 100_000; // offset to ensure 6 chars\n\
                for _ in 0..6 {\n\
                code.push(chars[(val % base) as usize]);\n\
                val /= base;\n\
                }\n\
                code\n\
                }\n\n\
                /// Shorten a URL and return the generated code.\n\
                pub fn shorten(&self, url: &str) -> Result<String, String> {\n\
                Self::validate_url(url)?;\n\
                let code = self.generate_code();\n\
                let entry = UrlEntry::new(code.clone(), url.to_string());\n\
                let mut store = self.store.write()\n\
                .map_err(|e| format!(\"Lock error: {e}\"))?;\n\
                store.insert(code.clone(), entry);\n\
                Ok(code)\n\
                }\n\n\
                /// Resolve a short code to its original URL.\n\
                /// Increments the click counter.\n\
                pub fn resolve(&self, code: &str) -> Result<String, String> {\n\
                let mut store = self.store.write()\n\
                .map_err(|e| format!(\"Lock error: {e}\"))?;\n\
                match store.get_mut(code) {\n\
                Some(entry) => {\n\
                entry.click_count += 1;\n\
                Ok(entry.original_url.clone())\n\
                }\n\
                None => Err(format!(\"Code '{code}' not found\")),\n\
                }\n\
                }\n\n\
                /// Get statistics for a short code.\n\
                pub fn stats(&self, code: &str) -> Result<(String, u64), String> {\n\
                let store = self.store.read()\n\
                .map_err(|e| format!(\"Lock error: {e}\"))?;\n\
                match store.get(code) {\n\
                Some(entry) => Ok((entry.original_url.clone(), entry.click_count)),\n\
                None => Err(format!(\"Code '{code}' not found\")),\n\
                }\n\
                }\n\n\
                /// Delete a short URL (admin operation).\n\
                pub fn delete(&self, code: &str) -> Result<bool, String> {\n\
                let mut store = self.store.write()\n\
                .map_err(|e| format!(\"Lock error: {e}\"))?;\n\
                Ok(store.remove(code).is_some())\n\
                }\n\n\
                /// Get the total number of shortened URLs.\n\
                pub fn count(&self) -> usize {\n\
                self.store.read()\n\
                .map(|s| s.len())\n\
                .unwrap_or(0)\n\
                }\n\
                }\n\n\
                impl Default for UrlShortener {\n\
                fn default() -> Self {\n\
                Self::new()\n\
                }\n\
                }\n",
                    "create_dirs": true
                }),
            }],
        },
        // Turn 2: Run wc -l on the file
        LlmResponse::ToolUse {
            content: Some("Let me verify the implementation by counting lines of code.".into()),
            tool_calls: vec![ToolCall {
                id: "code_2".into(),
                name: "shell".into(),
                arguments: serde_json::json!({
                    "command": format!("wc -l {}", code_path),
                    "timeout_secs": 10
                }),
            }],
        },
        // Turn 3: Done
        LlmResponse::Done(
            "Implementation complete. Created url_shortener.rs with:\n\
             - UrlEntry struct with code, original_url, created_at, click_count\n\
             - UrlShortener with HashMap<String, UrlEntry> + RwLock\n\
             - Methods: shorten(), resolve(), stats(), delete(), count()\n\
             - Input validation: empty URL, non-HTTP(S), length > 2048\n\
             - Thread-safe via Arc<RwLock> and AtomicU64 counter"
                .into(),
        ),
    ]
}

fn tester_responses(work_dir: &str) -> Vec<LlmResponse> {
    let test_path = format!("{work_dir}/url_shortener_test.rs");
    vec![
        // Turn 1: Write tests
        LlmResponse::ToolUse {
            content: Some("I'll write comprehensive tests for the URL shortener.".into()),
            tool_calls: vec![ToolCall {
                id: "test_1".into(),
                name: "file_write".into(),
                arguments: serde_json::json!({
                    "path": test_path,
                    "content": "//! Tests for the URL Shortener Microservice\n\n\
                #[cfg(test)]\n\
                mod tests {\n\
                use super::*;\n\n\
                #[test]\n\
                fn test_shorten_valid_url() {\n\
                let shortener = UrlShortener::new();\n\
                let code = shortener.shorten(\"https://example.com\").unwrap();\n\
                assert_eq!(code.len(), 6);\n\
                }\n\n\
                #[test]\n\
                fn test_shorten_rejects_empty_url() {\n\
                let shortener = UrlShortener::new();\n\
                let result = shortener.shorten(\"\");\n\
                assert!(result.is_err());\n\
                assert!(result.unwrap_err().contains(\"empty\"));\n\
                }\n\n\
                #[test]\n\
                fn test_shorten_rejects_non_http_url() {\n\
                let shortener = UrlShortener::new();\n\
                assert!(shortener.shorten(\"ftp://example.com\").is_err());\n\
                assert!(shortener.shorten(\"javascript:alert(1)\").is_err());\n\
                }\n\n\
                #[test]\n\
                fn test_shorten_rejects_long_url() {\n\
                let shortener = UrlShortener::new();\n\
                let long_url = format!(\"https://example.com/{}\", \"a\".repeat(2048));\n\
                assert!(shortener.shorten(&long_url).is_err());\n\
                }\n\n\
                #[test]\n\
                fn test_resolve_existing_code() {\n\
                let shortener = UrlShortener::new();\n\
                let code = shortener.shorten(\"https://rust-lang.org\").unwrap();\n\
                let url = shortener.resolve(&code).unwrap();\n\
                assert_eq!(url, \"https://rust-lang.org\");\n\
                }\n\n\
                #[test]\n\
                fn test_resolve_nonexistent_code() {\n\
                let shortener = UrlShortener::new();\n\
                assert!(shortener.resolve(\"nonexistent\").is_err());\n\
                }\n\n\
                #[test]\n\
                fn test_resolve_increments_click_count() {\n\
                let shortener = UrlShortener::new();\n\
                let code = shortener.shorten(\"https://example.com\").unwrap();\n\
                shortener.resolve(&code).unwrap();\n\
                shortener.resolve(&code).unwrap();\n\
                let (_, clicks) = shortener.stats(&code).unwrap();\n\
                assert_eq!(clicks, 2);\n\
                }\n\n\
                #[test]\n\
                fn test_stats_returns_correct_data() {\n\
                let shortener = UrlShortener::new();\n\
                let code = shortener.shorten(\"https://docs.rs\").unwrap();\n\
                let (url, clicks) = shortener.stats(&code).unwrap();\n\
                assert_eq!(url, \"https://docs.rs\");\n\
                assert_eq!(clicks, 0);\n\
                }\n\n\
                #[test]\n\
                fn test_delete_existing_code() {\n\
                let shortener = UrlShortener::new();\n\
                let code = shortener.shorten(\"https://example.com\").unwrap();\n\
                assert!(shortener.delete(&code).unwrap());\n\
                assert!(shortener.resolve(&code).is_err());\n\
                }\n\n\
                #[test]\n\
                fn test_delete_nonexistent_code() {\n\
                let shortener = UrlShortener::new();\n\
                assert!(!shortener.delete(\"nope\").unwrap());\n\
                }\n\n\
                #[test]\n\
                fn test_count() {\n\
                let shortener = UrlShortener::new();\n\
                assert_eq!(shortener.count(), 0);\n\
                shortener.shorten(\"https://a.com\").unwrap();\n\
                shortener.shorten(\"https://b.com\").unwrap();\n\
                assert_eq!(shortener.count(), 2);\n\
                }\n\n\
                #[test]\n\
                fn test_unique_codes() {\n\
                let shortener = UrlShortener::new();\n\
                let code1 = shortener.shorten(\"https://a.com\").unwrap();\n\
                let code2 = shortener.shorten(\"https://b.com\").unwrap();\n\
                assert_ne!(code1, code2);\n\
                }\n\
                }\n",
                    "create_dirs": true
                }),
            }],
        },
        // Turn 2: Done
        LlmResponse::Done(
            "Test suite complete. 12 test cases covering:\n\
             - Valid URL shortening\n\
             - Empty URL rejection\n\
             - Non-HTTP URL rejection\n\
             - Long URL rejection (>2048 chars)\n\
             - Code resolution (existing and nonexistent)\n\
             - Click count tracking\n\
             - Statistics retrieval\n\
             - Deletion (existing and nonexistent)\n\
             - Count tracking\n\
             - Unique code generation"
                .into(),
        ),
    ]
}

fn security_auditor_responses(work_dir: &str) -> Vec<LlmResponse> {
    let code_path = format!("{work_dir}/url_shortener.rs");
    vec![
        // Turn 1: Read the code
        LlmResponse::ToolUse {
            content: Some(
                "I'll review the URL shortener implementation for security vulnerabilities.".into(),
            ),
            tool_calls: vec![ToolCall {
                id: "sec_1".into(),
                name: "file_read".into(),
                arguments: serde_json::json!({
                    "path": code_path
                }),
            }],
        },
        // Turn 2: Done with findings
        LlmResponse::Done(
            "Security Audit Report - URL Shortener\n\
             ======================================\n\n\
             SEVERITY: LOW RISK\n\n\
             Findings:\n\
             1. [PASS] Input validation: URLs validated for scheme (HTTP/HTTPS only)\n\
             2. [PASS] Length limiting: URLs capped at 2048 characters\n\
             3. [PASS] No SQL injection risk: uses HashMap, not a database\n\
             4. [PASS] Thread safety: proper use of RwLock and AtomicU64\n\
             5. [WARN] Code generation uses sequential counter, not cryptographic random.\n\
                Recommendation: Use rand::thread_rng() for production.\n\
             6. [WARN] No rate limiting implemented at the service level.\n\
                Recommendation: Add token bucket rate limiter.\n\
             7. [INFO] No XSS protection on redirect — consider validating URL characters.\n\
             8. [PASS] No unwrap() in production paths — proper error handling with Result.\n\n\
             Overall: APPROVED with minor recommendations."
                .into(),
        ),
    ]
}

fn reviewer_responses(work_dir: &str) -> Vec<LlmResponse> {
    let code_path = format!("{work_dir}/url_shortener.rs");
    vec![
        // Turn 1: Read the code
        LlmResponse::ToolUse {
            content: Some(
                "I'll perform a final code review of the URL shortener implementation.".into(),
            ),
            tool_calls: vec![ToolCall {
                id: "rev_1".into(),
                name: "file_read".into(),
                arguments: serde_json::json!({
                    "path": code_path
                }),
            }],
        },
        // Turn 2: Done with review
        LlmResponse::Done(
            "Code Review Report - URL Shortener\n\
             ====================================\n\n\
             VERDICT: APPROVED with minor suggestions\n\n\
             Strengths:\n\
             + Clean, idiomatic Rust with proper error handling\n\
             + Good separation of concerns (validation, storage, generation)\n\
             + Thread-safe design with RwLock and AtomicU64\n\
             + Comprehensive input validation\n\
             + Well-documented with doc comments\n\
             + Default trait implementation\n\n\
             Minor suggestions:\n\
             - Consider adding #[must_use] to public methods returning Result\n\
             - The generate_code() offset of 100_000 is a magic number; extract to const\n\
             - Consider implementing Display for UrlEntry for logging\n\
             - Add #[derive(PartialEq)] to UrlEntry for easier testing\n\n\
             Code quality: 8.5/10\n\
             Security: 9/10 (per SecurityAuditor review)\n\
             Test coverage: Comprehensive (12 tests)\n\n\
             RECOMMENDATION: Merge to main."
                .into(),
        ),
    ]
}

fn doc_writer_responses(work_dir: &str) -> Vec<LlmResponse> {
    let doc_path = format!("{work_dir}/API_DOCS.md");
    vec![
        // Turn 1: Write API docs
        LlmResponse::ToolUse {
            content: Some("I'll write the API documentation for the URL shortener service.".into()),
            tool_calls: vec![ToolCall {
                id: "doc_1".into(),
                name: "file_write".into(),
                arguments: serde_json::json!({
                    "path": doc_path,
                    "content": "# URL Shortener API Documentation\n\n\
                ## Overview\n\n\
                A simple, fast URL shortener microservice built in Rust.\n\
                Thread-safe, zero-dependency storage with in-memory HashMap.\n\n\
                ## Base URL\n\n\
                ```\n\
                http://localhost:3000\n\
                ```\n\n\
                ## Endpoints\n\n\
                ### POST /shorten\n\n\
                Create a shortened URL.\n\n\
                **Request Body:**\n\
                ```json\n\
                {\n\
                \"url\": \"https://example.com/very/long/path\"\n\
                }\n\
                ```\n\n\
                **Response (201 Created):**\n\
                ```json\n\
                {\n\
                \"code\": \"aB3xYz\",\n\
                \"short_url\": \"http://localhost:3000/aB3xYz\",\n\
                \"original_url\": \"https://example.com/very/long/path\"\n\
                }\n\
                ```\n\n\
                **Errors:**\n\
                - 400: Invalid URL (empty, non-HTTP(S), or > 2048 chars)\n\
                - 429: Rate limit exceeded\n\n\
                ### GET /:code\n\n\
                Redirect to the original URL.\n\n\
                **Response:** 302 Redirect to original URL\n\n\
                **Errors:**\n\
                - 404: Code not found\n\n\
                ### GET /stats/:code\n\n\
                Get statistics for a shortened URL.\n\n\
                **Response (200 OK):**\n\
                ```json\n\
                {\n\
                \"code\": \"aB3xYz\",\n\
                \"original_url\": \"https://example.com/very/long/path\",\n\
                \"click_count\": 42,\n\
                \"created_at\": \"2024-01-15T10:30:00Z\"\n\
                }\n\
                ```\n\n\
                ### DELETE /admin/:code\n\n\
                Delete a shortened URL (admin only).\n\n\
                **Response:** 204 No Content\n\n\
                **Errors:**\n\
                - 404: Code not found\n\
                - 403: Unauthorized\n\n\
                ## Rate Limits\n\n\
                | Endpoint      | Limit              |\n\
                |---------------|--------------------|\n\
                | POST /shorten | 100 req/min per IP |\n\
                | GET /:code    | 1000 req/min per IP|\n\n\
                ## Error Format\n\n\
                ```json\n\
                {\n\
                \"error\": \"Description of the error\",\n\
                \"code\": 400\n\
                }\n\
                ```\n",
                    "create_dirs": true
                }),
            }],
        },
        // Turn 2: Done
        LlmResponse::Done(
            "API documentation complete. Created API_DOCS.md covering:\n\
             - Service overview\n\
             - All 4 endpoints with request/response examples\n\
             - Error codes and format\n\
             - Rate limiting table\n\
             - JSON schema for all payloads"
                .into(),
        ),
    ]
}

// ── Parse tool result helpers ───────────────────────────────────

fn parse_shell_result(json_str: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
        v["stdout"].as_str().unwrap_or(json_str).trim().to_string()
    } else {
        json_str.to_string()
    }
}

fn parse_file_read_result(json_str: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
        let path = v["path"].as_str().unwrap_or("?");
        let size = v["size"].as_u64().unwrap_or(0);
        let content = v["content"].as_str().unwrap_or("");
        let lines = content.lines().count();
        let filename = path.rsplit('/').next().unwrap_or(path);
        format!("Read {filename} ({size} bytes, {lines} lines)")
    } else {
        json_str.to_string()
    }
}

fn parse_file_write_result(json_str: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
        let path = v["path"].as_str().unwrap_or("?");
        let bytes = v["bytes_written"].as_u64().unwrap_or(0);
        let filename = path.rsplit('/').next().unwrap_or(path);
        format!("Written {bytes} bytes to {filename}")
    } else {
        json_str.to_string()
    }
}

fn parse_result(tool_name: &str, raw: &str) -> String {
    match tool_name {
        "shell" => parse_shell_result(raw),
        "file_read" => parse_file_read_result(raw),
        "file_write" => parse_file_write_result(raw),
        _ => raw.to_string(),
    }
}

// ── Token simulation ────────────────────────────────────────────

fn simulated_tokens(role: &AgentRole, turn: u32) -> (u64, u64) {
    match role {
        AgentRole::Architect => match turn {
            0 => (400, 800),
            _ => (200, 300),
        },
        AgentRole::Coder => match turn {
            0 => (500, 1500),
            1 => (300, 200),
            _ => (200, 300),
        },
        AgentRole::Tester => match turn {
            0 => (600, 1200),
            _ => (200, 300),
        },
        AgentRole::SecurityAuditor => match turn {
            0 => (400, 300),
            _ => (300, 800),
        },
        AgentRole::Reviewer => match turn {
            0 => (400, 300),
            _ => (200, 500),
        },
        AgentRole::DocumentWriter => match turn {
            0 => (300, 1000),
            _ => (200, 300),
        },
        _ => (200, 200),
    }
}

// ── Execute a single agent ──────────────────────────────────────

struct AgentExecution {
    role: AgentRole,
    turns: u32,
    tool_calls: u32,
    total_tokens: u64,
}

#[allow(clippy::too_many_arguments)]
async fn execute_agent(
    role: AgentRole,
    backend: DemoBackend,
    registry: &SkillRegistry,
    permissions: &PermissionSet,
    monitor: &AgentMonitor,
    budget_tracker: &BudgetTracker,
    _message_bus: &MessageBus,
    task_id: uuid::Uuid,
    audit: &AuditLog,
) -> Result<(AgentExecution, String), Box<dyn std::error::Error>> {
    let color = agent_color(&role);
    let bg = agent_bg(&role);
    let label = agent_label(&role);

    println!("  {bg}{BOLD}{WHT} {label} {RST}  {color}{BOLD}Starting task...{RST}");
    delay(200);

    // Set up monitor and budget
    monitor.start_task(role.clone(), task_id).await;
    budget_tracker.start_tracking(role.clone()).await;
    budget_tracker
        .set_budget(role.clone(), default_budget(&role))
        .await;

    let session = Session::new();
    let tool_descriptors = registry.list_descriptors();

    let mut turns = 0u32;
    let mut tool_calls_count = 0u32;
    let mut total_tokens = 0u64;
    let mut final_response = String::new();

    for _turn in 0..10u32 {
        let response = backend
            .chat(None, &session.messages, &tool_descriptors)
            .await?;

        let (input_tok, output_tok) = simulated_tokens(&role, turns);
        budget_tracker
            .record_tokens(&role, input_tok, output_tok)
            .await;
        total_tokens += input_tok + output_tok;
        turns += 1;

        match response {
            LlmResponse::Done(text) => {
                monitor
                    .record_turn(role.clone(), 0, input_tok + output_tok)
                    .await;

                // Show done message (truncated)
                let preview = if text.len() > 100 {
                    format!("{}...", &text[..97])
                } else {
                    text.clone()
                };
                println!("    {DIM}{ITAL}{preview}{RST}");

                final_response = text;
                audit.log_action(
                    session.id,
                    "agent_done",
                    None,
                    serde_json::json!({"role": role.to_string(), "turns": turns}),
                    AuditOutcome::Success,
                );
                break;
            }

            LlmResponse::Text(_text) => {
                monitor
                    .record_turn(role.clone(), 0, input_tok + output_tok)
                    .await;
            }

            LlmResponse::ToolUse {
                content,
                tool_calls,
            } => {
                if let Some(text) = &content {
                    let short = text.split('.').next().unwrap_or(text);
                    print!("    {color}{ITAL}");
                    typewrite(&format!("{short}."), 8);
                    println!("{RST}");
                }

                for call in tool_calls {
                    let tool_bg = match call.name.as_str() {
                        "shell" => BG_BLU,
                        "file_read" => BG_CYAN,
                        "file_write" => BG_GRN,
                        _ => BG_BLU,
                    };
                    println!("    {tool_bg}{WHT}{BOLD} {} {RST}", call.name);

                    spinner("Executing", 400);

                    let result = registry.execute(call.clone(), permissions).await?;
                    tool_calls_count += 1;
                    budget_tracker.record_tool_call(&role).await;

                    let parsed = parse_result(&call.name, &result.content);
                    if result.is_error {
                        println!("    {RED}{BOLD}ERROR:{RST} {RED}{parsed}{RST}");
                    } else {
                        // Show truncated output
                        for (i, line) in parsed.lines().enumerate() {
                            if i >= 3 {
                                println!(
                                    "    {DIM}  ... ({} more lines){RST}",
                                    parsed.lines().count() - 3
                                );
                                break;
                            }
                            println!("    {GRN}{line}{RST}");
                        }
                    }

                    let outcome = if result.is_error {
                        AuditOutcome::Error
                    } else {
                        AuditOutcome::Success
                    };
                    audit.log_action(
                        session.id,
                        "tool_call",
                        Some(call.name.clone()),
                        serde_json::json!({"role": role.to_string(), "call_id": call.id}),
                        outcome,
                    );

                    monitor
                        .record_turn(role.clone(), 1, input_tok + output_tok)
                        .await;
                }
            }
        }

        delay(100);
    }

    monitor.finish_task(role.clone()).await;

    println!(
        "    {BG_GRN}{BOLD}{WHT} DONE {RST}  {DIM}{turns} turns, {tool_calls_count} tools, ~{total_tokens} tokens{RST}"
    );
    println!();

    Ok((
        AgentExecution {
            role,
            turns,
            tool_calls: tool_calls_count,
            total_tokens,
        },
        final_response,
    ))
}

// ── Main ────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("warn").init();

    let start = std::time::Instant::now();

    // ── Setup work directory ────────────────────────────────────
    let temp_dir = std::env::temp_dir().join(format!("argentor_team_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir)?;
    let work_dir = temp_dir.to_string_lossy().to_string();
    let audit_dir = temp_dir.join("audit");

    // ── Setup infrastructure ────────────────────────────────────
    let registry = SkillRegistry::new();
    register_builtins(&registry);

    let mut permissions = PermissionSet::new();
    permissions.grant(Capability::ShellExec {
        allowed_commands: vec![
            "wc".to_string(),
            "cat".to_string(),
            "ls".to_string(),
            "echo".to_string(),
        ],
    });
    permissions.grant(Capability::FileRead {
        allowed_paths: vec![work_dir.clone()],
    });
    permissions.grant(Capability::FileWrite {
        allowed_paths: vec![work_dir.clone()],
    });

    let audit = Arc::new(AuditLog::new(audit_dir.clone()));
    let monitor = AgentMonitor::new();
    let budget_tracker = BudgetTracker::new();
    let message_bus = MessageBus::new();
    let mut task_queue = TaskQueue::new();

    // ═══════════════════════════════════════════════════════════
    // BANNER
    // ═══════════════════════════════════════════════════════════

    println!();
    delay(300);
    println!("{BOLD}{CYAN}  ╔══════════════════════════════════════════════════════════════╗{RST}");
    println!("{BOLD}{CYAN}  ║                                                              ║{RST}");
    println!("{BOLD}{CYAN}  ║     A R G E N T O R   T E A M   O R C H E S T R A T I O N   ║{RST}");
    println!(
        "{BOLD}{CYAN}  ║     URL Shortener Microservice                                ║{RST}"
    );
    println!("{BOLD}{CYAN}  ║                                                              ║{RST}");
    println!("{BOLD}{CYAN}  ╚══════════════════════════════════════════════════════════════╝{RST}");
    println!();
    delay(400);

    // ═══════════════════════════════════════════════════════════
    // TEAM ROSTER
    // ═══════════════════════════════════════════════════════════

    println!("  {BOLD}{WHT}Team Roster{RST}");
    println!("  {DIM}────────────────────────────────────────────{RST}");
    delay(100);

    let team_roles = [
        (AgentRole::Architect, "Designs API and data model"),
        (AgentRole::Coder, "Implements the service"),
        (AgentRole::Tester, "Writes and runs tests"),
        (AgentRole::SecurityAuditor, "Reviews for vulnerabilities"),
        (AgentRole::Reviewer, "Final code review"),
        (AgentRole::DocumentWriter, "Writes API documentation"),
    ];

    for (role, desc) in &team_roles {
        let color = agent_color(role);
        let emoji = agent_emoji(role);
        let label = agent_label(role);
        println!("  {emoji}{color}{BOLD}{label:<18}{RST} {DIM}{desc}{RST}");
        delay(80);
    }
    println!();
    delay(300);

    // ═══════════════════════════════════════════════════════════
    // PHASE 1: PLANNING — Task decomposition with dependencies
    // ═══════════════════════════════════════════════════════════

    println!("  {BOLD}{BG_BLU}{WHT} PHASE 1 {RST}  {BOLD}{WHT}Task Decomposition & Dependency Graph{RST}");
    println!("  {DIM}────────────────────────────────────────────{RST}");
    println!();
    delay(200);

    // Create tasks with dependencies
    let architect_task = Task::new(
        "Design API endpoints, data model, and system architecture",
        AgentRole::Architect,
    );
    let architect_id = architect_task.id;

    let coder_task = Task::new("Implement URL shortener service in Rust", AgentRole::Coder)
        .with_dependencies(vec![architect_id]);
    let coder_id = coder_task.id;

    let tester_task = Task::new("Write comprehensive test suite", AgentRole::Tester)
        .with_dependencies(vec![coder_id]);
    let tester_id = tester_task.id;

    let security_task = Task::new(
        "Security audit and vulnerability analysis",
        AgentRole::SecurityAuditor,
    )
    .with_dependencies(vec![coder_id]);
    let security_id = security_task.id;

    let reviewer_task = Task::new("Final code review and approval", AgentRole::Reviewer)
        .with_dependencies(vec![tester_id, security_id]);
    let reviewer_id = reviewer_task.id;

    let doc_task = Task::new("Write API documentation", AgentRole::DocumentWriter)
        .with_dependencies(vec![reviewer_id]);
    let _doc_id = doc_task.id;

    task_queue.add(architect_task);
    task_queue.add(coder_task);
    task_queue.add(tester_task);
    task_queue.add(security_task);
    task_queue.add(reviewer_task);
    task_queue.add(doc_task);

    // Verify no cycles
    assert!(!task_queue.has_cycle(), "Task graph must be acyclic");

    // Print dependency graph
    println!("  {BOLD}Dependency Graph:{RST}");
    println!();
    println!("    {CYAN}Architect{RST}");
    println!("        {DIM}|{RST}");
    println!("    {GRN}Coder{RST}");
    println!("      {DIM}/ \\{RST}");
    println!("  {YEL}Tester{RST}   {RED}SecurityAuditor{RST}   {DIM}(parallel){RST}");
    println!("      {DIM}\\ /{RST}");
    println!("    {MAG}Reviewer{RST}");
    println!("        {DIM}|{RST}");
    println!("   {BLU}DocumentWriter{RST}");
    println!();

    println!(
        "  {DIM}Total tasks: {}  |  Pending: {}  |  No cycles: {GRN}OK{RST}",
        task_queue.total_count(),
        task_queue.pending_count(),
    );
    println!();
    delay(400);

    // ═══════════════════════════════════════════════════════════
    // PHASE 2: EXECUTION — Run agents in dependency order
    // ═══════════════════════════════════════════════════════════

    println!("  {BOLD}{BG_MAG}{WHT} PHASE 2 {RST}  {BOLD}{WHT}Agent Execution{RST}");
    println!("  {DIM}────────────────────────────────────────────{RST}");
    println!();
    delay(200);

    let mut agent_results: Vec<AgentExecution> = Vec::new();
    let mut _a2a_messages = 0u32;
    let mut files_generated = 0u32;

    // ── Step 1: Architect ───────────────────────────────────────

    let ready = task_queue.all_ready();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].assigned_to, AgentRole::Architect);
    task_queue.mark_running(architect_id);

    let arch_backend = DemoBackend::new(architect_responses(&work_dir));
    let (exec, _resp) = execute_agent(
        AgentRole::Architect,
        arch_backend,
        &registry,
        &permissions,
        &monitor,
        &budget_tracker,
        &message_bus,
        architect_id,
        &audit,
    )
    .await?;
    agent_results.push(exec);
    files_generated += 1;

    task_queue.mark_completed(architect_id);

    // A2A: Architect notifies Coder
    message_bus
        .send(AgentMessage::new(
            AgentRole::Architect,
            BroadcastTarget::Direct(AgentRole::Coder),
            "Architecture design complete. API spec and data model ready for implementation."
                .to_string(),
            MessageType::ArtifactNotification,
        ))
        .await;
    _a2a_messages += 1;
    println!("  {DIM}  [A2A] Architect -> Coder: Architecture ready{RST}");
    delay(150);

    // ── Step 2: Coder ───────────────────────────────────────────

    let ready = task_queue.all_ready();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].assigned_to, AgentRole::Coder);
    task_queue.mark_running(coder_id);

    // Coder receives message from Architect
    let inbox = message_bus.receive(&AgentRole::Coder).await;
    if !inbox.is_empty() {
        println!(
            "  {DIM}  [A2A] Coder received {} message(s) from inbox{RST}",
            inbox.len()
        );
    }

    let coder_backend = DemoBackend::new(coder_responses(&work_dir));
    let (exec, _resp) = execute_agent(
        AgentRole::Coder,
        coder_backend,
        &registry,
        &permissions,
        &monitor,
        &budget_tracker,
        &message_bus,
        coder_id,
        &audit,
    )
    .await?;
    agent_results.push(exec);
    files_generated += 1;

    task_queue.mark_completed(coder_id);

    // A2A: Coder notifies Tester and SecurityAuditor
    message_bus
        .send(AgentMessage::new(
            AgentRole::Coder,
            BroadcastTarget::Direct(AgentRole::Tester),
            "Implementation complete. url_shortener.rs ready for testing.".to_string(),
            MessageType::ArtifactNotification,
        ))
        .await;
    _a2a_messages += 1;
    println!("  {DIM}  [A2A] Coder -> Tester: Implementation ready{RST}");

    message_bus
        .send(AgentMessage::new(
            AgentRole::Coder,
            BroadcastTarget::Direct(AgentRole::SecurityAuditor),
            "Implementation complete. url_shortener.rs ready for security review.".to_string(),
            MessageType::ArtifactNotification,
        ))
        .await;
    _a2a_messages += 1;
    println!("  {DIM}  [A2A] Coder -> SecurityAuditor: Code ready for audit{RST}");
    delay(150);

    // ── Step 3: Tester + SecurityAuditor (PARALLEL) ─────────────

    let ready = task_queue.all_ready();
    assert_eq!(
        ready.len(),
        2,
        "Tester and SecurityAuditor should be ready in parallel"
    );

    println!();
    println!(
        "  {BOLD}{BG_YEL}{WHT} PARALLEL {RST}  {BOLD}Tester + SecurityAuditor running concurrently{RST}"
    );
    println!();
    delay(200);

    task_queue.mark_running(tester_id);
    task_queue.mark_running(security_id);

    // Tester
    let tester_inbox = message_bus.receive(&AgentRole::Tester).await;
    if !tester_inbox.is_empty() {
        println!(
            "  {DIM}  [A2A] Tester received {} message(s){RST}",
            tester_inbox.len()
        );
    }

    let tester_backend = DemoBackend::new(tester_responses(&work_dir));
    let (exec, _resp) = execute_agent(
        AgentRole::Tester,
        tester_backend,
        &registry,
        &permissions,
        &monitor,
        &budget_tracker,
        &message_bus,
        tester_id,
        &audit,
    )
    .await?;
    agent_results.push(exec);
    files_generated += 1;

    task_queue.mark_completed(tester_id);

    // SecurityAuditor
    let sec_inbox = message_bus.receive(&AgentRole::SecurityAuditor).await;
    if !sec_inbox.is_empty() {
        println!(
            "  {DIM}  [A2A] SecurityAuditor received {} message(s){RST}",
            sec_inbox.len()
        );
    }

    let sec_backend = DemoBackend::new(security_auditor_responses(&work_dir));
    let (exec, _resp) = execute_agent(
        AgentRole::SecurityAuditor,
        sec_backend,
        &registry,
        &permissions,
        &monitor,
        &budget_tracker,
        &message_bus,
        security_id,
        &audit,
    )
    .await?;
    agent_results.push(exec);

    task_queue.mark_completed(security_id);

    // A2A: Both notify Reviewer
    message_bus
        .send(AgentMessage::new(
            AgentRole::Tester,
            BroadcastTarget::Direct(AgentRole::Reviewer),
            "Test suite complete: 12 tests, all passing.".to_string(),
            MessageType::StatusUpdate,
        ))
        .await;
    _a2a_messages += 1;
    println!("  {DIM}  [A2A] Tester -> Reviewer: Tests complete{RST}");

    message_bus
        .send(AgentMessage::new(
            AgentRole::SecurityAuditor,
            BroadcastTarget::Direct(AgentRole::Reviewer),
            "Security audit complete: LOW RISK, APPROVED with minor recommendations.".to_string(),
            MessageType::StatusUpdate,
        ))
        .await;
    _a2a_messages += 1;
    println!("  {DIM}  [A2A] SecurityAuditor -> Reviewer: Audit complete{RST}");
    delay(150);

    // ── Step 4: Reviewer ────────────────────────────────────────

    let ready = task_queue.all_ready();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].assigned_to, AgentRole::Reviewer);
    task_queue.mark_running(reviewer_id);

    let rev_inbox = message_bus.receive(&AgentRole::Reviewer).await;
    if !rev_inbox.is_empty() {
        println!(
            "  {DIM}  [A2A] Reviewer received {} message(s){RST}",
            rev_inbox.len()
        );
    }

    let rev_backend = DemoBackend::new(reviewer_responses(&work_dir));
    let (exec, _resp) = execute_agent(
        AgentRole::Reviewer,
        rev_backend,
        &registry,
        &permissions,
        &monitor,
        &budget_tracker,
        &message_bus,
        reviewer_id,
        &audit,
    )
    .await?;
    agent_results.push(exec);

    task_queue.mark_completed(reviewer_id);

    // A2A: Reviewer notifies DocumentWriter
    message_bus
        .send(AgentMessage::new(
            AgentRole::Reviewer,
            BroadcastTarget::Direct(AgentRole::DocumentWriter),
            "Code review APPROVED. Ready for documentation.".to_string(),
            MessageType::StatusUpdate,
        ))
        .await;
    _a2a_messages += 1;
    println!("  {DIM}  [A2A] Reviewer -> DocumentWriter: Approved{RST}");
    delay(150);

    // ── Step 5: DocumentWriter ──────────────────────────────────

    let ready = task_queue.all_ready();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].assigned_to, AgentRole::DocumentWriter);
    let doc_id_actual = ready[0].id;
    task_queue.mark_running(doc_id_actual);

    let doc_inbox = message_bus.receive(&AgentRole::DocumentWriter).await;
    if !doc_inbox.is_empty() {
        println!(
            "  {DIM}  [A2A] DocumentWriter received {} message(s){RST}",
            doc_inbox.len()
        );
    }

    let doc_backend = DemoBackend::new(doc_writer_responses(&work_dir));
    let (exec, _resp) = execute_agent(
        AgentRole::DocumentWriter,
        doc_backend,
        &registry,
        &permissions,
        &monitor,
        &budget_tracker,
        &message_bus,
        doc_id_actual,
        &audit,
    )
    .await?;
    agent_results.push(exec);
    files_generated += 1;

    task_queue.mark_completed(doc_id_actual);

    // Broadcast completion
    message_bus
        .broadcast(
            AgentRole::DocumentWriter,
            "All tasks complete. URL Shortener microservice is ready!".to_string(),
            MessageType::StatusUpdate,
        )
        .await;
    _a2a_messages += 1;

    let duration = start.elapsed();

    // ═══════════════════════════════════════════════════════════
    // PHASE 3: SYNTHESIS — Dashboard
    // ═══════════════════════════════════════════════════════════

    delay(400);
    println!("  {BOLD}{BG_GRN}{WHT} PHASE 3 {RST}  {BOLD}{WHT}Results & Dashboard{RST}");
    println!("  {DIM}────────────────────────────────────────────{RST}");
    println!();
    delay(200);

    // Verify all tasks completed
    assert!(task_queue.is_done(), "All tasks should be completed");
    assert_eq!(task_queue.completed_count(), 6);

    // Budget summary
    let budget_summary = budget_tracker.summary().await;
    let total_a2a = message_bus.message_count().await;

    // ═══════════════════════════════════════════════════════════
    // DASHBOARD
    // ═══════════════════════════════════════════════════════════

    delay(300);
    println!(
        "{BOLD}{CYAN}\
  ╔══════════════════════════════════════════════════════════════════╗\n\
  ║            ARGENTOR TEAM ORCHESTRATION DEMO                     ║\n\
  ║            URL Shortener Microservice                           ║\n\
  ╠══════════════════════════════════════════════════════════════════╣{RST}"
    );
    println!(
        "{BOLD}{CYAN}\
  ║{RST}  {BOLD}Agent{RST}              {BOLD}Status{RST}      {BOLD}Turns{RST}   {BOLD}Tools{RST}   {BOLD}Tokens{RST}      {BOLD}{CYAN}║{RST}"
    );
    println!(
        "{BOLD}{CYAN}\
  ║{RST}  {DIM}─────────────────  ────────  ─────  ─────  ──────────{RST} {BOLD}{CYAN}║{RST}"
    );

    for exec in &agent_results {
        let color = agent_color(&exec.role);
        let label = agent_label(&exec.role);
        println!(
            "{BOLD}{CYAN}  ║{RST}  {color}{BOLD}{:<17}{RST}  {GRN}Complete{RST}   {BOLD}{:>3}{RST}     {BOLD}{:>3}{RST}    {BOLD}{:>6}{RST}      {BOLD}{CYAN}║{RST}",
            label, exec.turns, exec.tool_calls, exec.total_tokens,
        );
    }

    let _total_turns: u32 = agent_results.iter().map(|e| e.turns).sum();
    let total_tools: u32 = agent_results.iter().map(|e| e.tool_calls).sum();
    let total_tokens: u64 = agent_results.iter().map(|e| e.total_tokens).sum();

    println!(
        "{BOLD}{CYAN}\
  ╠══════════════════════════════════════════════════════════════════╣{RST}"
    );
    println!(
        "{BOLD}{CYAN}  ║{RST}  Total Tasks: {BOLD}6{RST}    Completed: {GRN}{BOLD}6{RST}    Failed: {GRN}{BOLD}0{RST}               {BOLD}{CYAN}║{RST}"
    );
    println!(
        "{BOLD}{CYAN}  ║{RST}  Total Tokens: {BOLD}~{total_tokens}{RST}   Duration: {BOLD}{:.1}s{RST}                   {BOLD}{CYAN}║{RST}",
        duration.as_secs_f64(),
    );
    println!(
        "{BOLD}{CYAN}  ║{RST}  Files Generated: {BOLD}{files_generated}{RST}   A2A Messages: {BOLD}{total_a2a}{RST}                  {BOLD}{CYAN}║{RST}"
    );
    println!(
        "{BOLD}{CYAN}\
  ╚══════════════════════════════════════════════════════════════════╝{RST}"
    );
    println!();

    // ═══════════════════════════════════════════════════════════
    // BUDGET SUMMARY
    // ═══════════════════════════════════════════════════════════

    delay(200);
    println!("  {BOLD}{WHT}Budget Summary{RST}");
    println!("  {DIM}────────────────────────────────────────────{RST}");
    println!(
        "  Input tokens:  {BOLD}{}{RST}",
        budget_summary.total_input_tokens
    );
    println!(
        "  Output tokens: {BOLD}{}{RST}",
        budget_summary.total_output_tokens
    );
    println!(
        "  Tool calls:    {BOLD}{}{RST}",
        budget_summary.total_tool_calls
    );
    println!(
        "  Agents:        {BOLD}{}{RST}",
        budget_summary.per_agent.len()
    );
    println!();

    // ═══════════════════════════════════════════════════════════
    // GENERATED FILES
    // ═══════════════════════════════════════════════════════════

    delay(200);
    println!("  {BOLD}{WHT}Generated Files{RST}  {DIM}(written to disk by file_write skill){RST}");
    println!("  {DIM}────────────────────────────────────────────{RST}");

    let files = [
        "architecture.md",
        "url_shortener.rs",
        "url_shortener_test.rs",
        "API_DOCS.md",
    ];
    for fname in &files {
        let fpath = temp_dir.join(fname);
        if fpath.exists() {
            let size = std::fs::metadata(&fpath).map(|m| m.len()).unwrap_or(0);
            println!("  {GRN}  {fname}{RST}  {DIM}({size} bytes){RST}");
        } else {
            println!("  {RED}  {fname}{RST}  {DIM}(not found){RST}");
        }
    }
    println!();

    // ═══════════════════════════════════════════════════════════
    // TASK QUEUE FINAL STATE
    // ═══════════════════════════════════════════════════════════

    delay(200);
    println!("  {BOLD}{WHT}Task Queue Final State{RST}");
    println!("  {DIM}────────────────────────────────────────────{RST}");
    for task in task_queue.all_tasks() {
        let status_str = match &task.status {
            TaskStatus::Completed => format!("{GRN}Completed{RST}"),
            TaskStatus::Pending => format!("{YEL}Pending{RST}"),
            TaskStatus::Running => format!("{CYAN}Running{RST}"),
            TaskStatus::Failed { reason } => format!("{RED}Failed: {reason}{RST}"),
            TaskStatus::NeedsHumanReview => format!("{MAG}NeedsReview{RST}"),
        };
        let color = agent_color(&task.assigned_to);
        let label = agent_label(&task.assigned_to);
        println!(
            "  {color}  {label:<18}{RST} {status_str}  {DIM}{}{RST}",
            &task.id.to_string()[..8]
        );
    }
    println!();

    // ═══════════════════════════════════════════════════════════
    // AUDIT TRAIL
    // ═══════════════════════════════════════════════════════════

    delay(200);
    let log_path = audit_dir.join("audit.jsonl");
    std::thread::sleep(Duration::from_millis(100));

    println!("  {BOLD}{WHT}Audit Trail{RST}  {DIM}(append-only JSONL){RST}");
    println!("  {DIM}────────────────────────────────────────────{RST}");

    if log_path.exists() {
        if let Ok(result) =
            argentor_security::query_audit_log(&log_path, &argentor_security::AuditFilter::all())
        {
            println!(
                "  {BLU}Entries: {} | OK: {} | Errors: {} | Skills: {}{RST}",
                result.total_scanned,
                result.stats.success_count,
                result.stats.error_count,
                result.stats.unique_skills,
            );
        }
    } else {
        println!("  {DIM}(audit log not available){RST}");
    }
    println!();

    // ═══════════════════════════════════════════════════════════
    // FOOTER
    // ═══════════════════════════════════════════════════════════

    delay(300);
    println!(
        "{BOLD}{CYAN}  ╔══════════════════════════════════════════════════════════════════╗{RST}"
    );
    println!("{BOLD}{CYAN}  ║  6 agents, {total_tools} tools executed REAL — no mocks, no API keys        ║{RST}");
    println!(
        "{BOLD}{CYAN}  ║  Framework: Argentor v0.1.0  |  github.com/fboiero/Agentor      ║{RST}"
    );
    println!(
        "{BOLD}{CYAN}  ╚══════════════════════════════════════════════════════════════════╝{RST}"
    );
    println!();

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);

    Ok(())
}
