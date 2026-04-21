//! CLI binary for the Argentor framework.
//!
//! Provides the `argentor` command-line tool with the following subcommands:
//!
//! - `serve` — Start the HTTP/WebSocket gateway server.
//! - `skill list` — List all registered skills and their capabilities.
//! - `compliance report` — Generate a compliance report across all frameworks.
//! - `a2a` — Interact with remote A2A agents (discover, send, status, cancel, list).
//! - `orchestrate` — Run multi-agent orchestration on a task description.

/// Configuration file hot-reload watcher.
mod config_watcher;
/// Headless mode: NDJSON over stdin/stdout for SDK wrapping.
pub mod headless;
/// Interactive REPL for agent debugging.
pub mod repl;

use argentor_agent::{AgentRunner, ModelConfig};
use argentor_gateway::{AuthConfig, GatewayServer};
use argentor_security::tls;
use argentor_security::{AuditLog, Capability, PermissionSet, RateLimiter};
use argentor_session::FileSessionStore;
use argentor_skills::{MarkdownSkillLoader, SkillConfig, SkillLoader, SkillRegistry, ToolGroup};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "argentor", about = "Argentor — Secure AI Agent Framework")]
struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "argentor.toml")]
    config: PathBuf,

    /// Run in headless mode (NDJSON over stdin/stdout for SDK wrapping)
    #[arg(long)]
    headless: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the gateway server
    Serve {
        /// Host to bind to (overrides config)
        #[arg(long)]
        host: Option<String>,
        /// Port to listen on (overrides config)
        #[arg(short, long)]
        port: Option<u16>,
    },
    /// Manage skills
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Generate a compliance report
    Compliance {
        #[command(subcommand)]
        action: ComplianceAction,
    },
    /// Interact with remote A2A agents
    A2a {
        /// Remote agent URL (e.g., http://localhost:3000)
        #[arg(long)]
        url: String,
        #[command(subcommand)]
        action: A2aAction,
    },
    /// Run multi-agent orchestration on a task
    Orchestrate {
        /// The task description for the orchestrator to decompose and execute
        task: String,
        /// Directory to write output artifacts (code, tests, spec, review)
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
        /// Enable interactive stdin-based human approval for high-risk operations
        #[arg(long)]
        interactive_approval: bool,
        /// Timeout in seconds for interactive approval prompts (default: 300)
        #[arg(long, default_value = "300")]
        approval_timeout: u64,
    },
}

#[derive(Subcommand)]
enum SkillAction {
    /// List registered skills
    List,
}

#[derive(Subcommand)]
enum ComplianceAction {
    /// Generate a compliance report for all frameworks
    Report,
}

/// Actions available when interacting with a remote A2A agent.
#[derive(Subcommand)]
enum A2aAction {
    /// Discover a remote agent's capabilities (fetches the agent card)
    Discover,
    /// Send a task to a remote agent
    Send {
        /// The message to send
        message: String,
        /// Optional session ID to reuse
        #[arg(long)]
        session_id: Option<String>,
    },
    /// Get the status of a task
    Status {
        /// Task ID to check
        id: String,
    },
    /// Cancel a running task
    Cancel {
        /// Task ID to cancel
        id: String,
    },
    /// List tasks, optionally filtered by session
    List {
        /// Filter by session ID
        #[arg(long)]
        session_id: Option<String>,
    },
}

#[derive(Deserialize)]
struct ArgentorConfig {
    model: ModelConfig,
    #[serde(default = "default_data_dir")]
    data_dir: PathBuf,
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    security: SecurityConfig,
    #[serde(default)]
    skills: Vec<SkillConfig>,
    #[serde(default)]
    mcp_servers: Vec<McpServerConfig>,
    /// Directory containing markdown skill files (.md with YAML frontmatter).
    #[serde(default)]
    markdown_skills_dir: Option<PathBuf>,
    /// Custom tool groups for progressive skill disclosure.
    #[serde(default)]
    tool_groups: Vec<ToolGroup>,
}

/// MCP server config — delegates to the manager's type.
type McpServerConfig = argentor_mcp::McpServerConfig;

#[derive(Deserialize)]
struct ServerConfig {
    #[serde(default = "default_host")]
    host: String,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default)]
    tls: TlsConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            tls: TlsConfig::default(),
        }
    }
}

#[derive(Deserialize, Default)]
struct TlsConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    cert_path: String,
    #[serde(default)]
    key_path: String,
    #[serde(default)]
    client_ca_path: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct SecurityConfig {
    #[serde(default = "default_rps")]
    max_requests_per_second: f64,
    #[serde(default = "default_burst")]
    max_burst: f64,
    #[serde(default = "default_max_msg_len")]
    max_message_length: usize,
    #[serde(default)]
    api_keys: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            max_requests_per_second: default_rps(),
            max_burst: default_burst(),
            max_message_length: default_max_msg_len(),
            api_keys: vec![],
        }
    }
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("./data")
}
fn default_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    3000
}
fn default_rps() -> f64 {
    10.0
}
fn default_burst() -> f64 {
    50.0
}
fn default_max_msg_len() -> usize {
    100_000
}

/// Expand `${VAR_NAME}` patterns in a string with environment variable values.
/// Unknown variables are replaced with empty strings.
fn expand_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut var_name = String::new();
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
                var_name.push(c);
            }
            if let Ok(val) = std::env::var(&var_name) {
                result.push_str(&val);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Load markdown skills from the configured directory into the registry.
/// Returns the prompt injection text to append to system prompts.
async fn load_markdown_skills(
    config: &ArgentorConfig,
    config_dir: &std::path::Path,
    registry: &SkillRegistry,
) -> String {
    let dir = match &config.markdown_skills_dir {
        Some(d) => {
            if d.is_absolute() {
                d.clone()
            } else {
                config_dir.join(d)
            }
        }
        None => return String::new(),
    };

    let loader = MarkdownSkillLoader::new(dir);
    match loader.load_all().await {
        Ok(loaded) => {
            let prompt_text = loaded.build_prompt_injection();

            // Register callable markdown skills
            for skill in loaded.callable {
                registry.register(skill);
            }

            info!(
                prompt_skills = loaded.prompts.len(),
                "Markdown skills loaded"
            );
            prompt_text
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to load markdown skills");
            String::new()
        }
    }
}

/// Register custom tool groups from config.
fn load_tool_groups(config: &ArgentorConfig, registry: &SkillRegistry) {
    if !config.tool_groups.is_empty() {
        registry.register_groups(config.tool_groups.clone());
        info!(
            count = config.tool_groups.len(),
            "Custom tool groups registered"
        );
    }
}

/// Handle the `a2a` subcommand by dispatching to the appropriate A2A client method.
async fn handle_a2a(url: &str, action: A2aAction) -> anyhow::Result<()> {
    let client = argentor_a2a::A2AClient::new(url);

    match action {
        A2aAction::Discover => {
            let card = client
                .get_agent_card()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to discover agent: {e}"))?;
            let pretty = serde_json::to_string_pretty(&card)
                .map_err(|e| anyhow::anyhow!("Failed to format agent card: {e}"))?;
            println!("{pretty}");
        }
        A2aAction::Send {
            message,
            session_id,
        } => {
            let task_msg = argentor_a2a::TaskMessage::user_text(&message);
            let task = client
                .send_task(task_msg, session_id)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send task: {e}"))?;
            let pretty = serde_json::to_string_pretty(&task)
                .map_err(|e| anyhow::anyhow!("Failed to format task: {e}"))?;
            println!("{pretty}");
        }
        A2aAction::Status { id } => {
            let task = client
                .get_task(&id)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get task status: {e}"))?;
            let pretty = serde_json::to_string_pretty(&task)
                .map_err(|e| anyhow::anyhow!("Failed to format task: {e}"))?;
            println!("{pretty}");
        }
        A2aAction::Cancel { id } => {
            let task = client
                .cancel_task(&id)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to cancel task: {e}"))?;
            let pretty = serde_json::to_string_pretty(&task)
                .map_err(|e| anyhow::anyhow!("Failed to format task: {e}"))?;
            println!("{pretty}");
        }
        A2aAction::List { session_id } => {
            let tasks = client
                .list_tasks(session_id.as_deref())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to list tasks: {e}"))?;
            let pretty = serde_json::to_string_pretty(&tasks)
                .map_err(|e| anyhow::anyhow!("Failed to format tasks: {e}"))?;
            println!("{pretty}");
        }
    }

    Ok(())
}

/// Wait for a shutdown signal (Ctrl+C or SIGTERM).
async fn shutdown_signal() {
    // Signal handlers cannot propagate errors — if OS signal registration
    // fails the process cannot function, so expect is justified here.
    #[allow(clippy::expect_used)]
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    #[allow(clippy::expect_used)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received Ctrl+C, shutting down..."),
        _ = terminate => info!("Received SIGTERM, shutting down..."),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if present (API keys, tokens)
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    // Headless mode — NDJSON protocol over stdin/stdout, no subcommand needed.
    // Suppress all tracing in headless mode to keep stdout clean.
    let suppress_logs = cli.headless
        || matches!(
            cli.command,
            Some(Commands::Orchestrate { .. }) | Some(Commands::A2a { .. })
        );
    let default_level = if suppress_logs && std::env::var("RUST_LOG").is_err() {
        "warn"
    } else {
        "info"
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level)),
        )
        .json()
        .with_writer(std::io::stderr)
        .init();

    // Handle headless mode early — no config file required
    if cli.headless {
        return headless::run_headless()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"));
    }

    // Require a subcommand when not in headless mode
    let command = match cli.command {
        Some(cmd) => cmd,
        None => {
            eprintln!("Error: a subcommand is required (or use --headless).");
            eprintln!("Run `argentor --help` for usage information.");
            std::process::exit(1);
        }
    };

    // Handle A2A subcommand early — it doesn't require a config file
    if let Commands::A2a { url, action } = command {
        return handle_a2a(&url, action).await;
    }

    // Load config with environment variable expansion
    let config_str = tokio::fs::read_to_string(&cli.config).await.map_err(|e| {
        anyhow::anyhow!(
            "Failed to read config file '{}': {}",
            cli.config.display(),
            e
        )
    })?;
    let config_str = expand_env_vars(&config_str);
    let config: ArgentorConfig = toml::from_str(&config_str)?;

    // Resolve config base directory (for relative skill paths)
    let config_dir = cli
        .config
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();

    match command {
        Commands::Serve { host, port } => {
            let host = host.unwrap_or_else(|| config.server.host.clone());
            let port = port.unwrap_or(config.server.port);

            info!("Starting Argentor gateway on {}:{}", host, port);

            // Initialize security
            let audit = Arc::new(AuditLog::new(config.data_dir.join("audit")));
            let rate_limiter = Arc::new(RateLimiter::new(
                config.security.max_burst,
                config.security.max_requests_per_second,
            ));
            let auth_config = AuthConfig::new(config.security.api_keys.clone());
            if auth_config.is_enabled() {
                info!(
                    keys = config.security.api_keys.len(),
                    "API key auth enabled"
                );
            }

            // Initialize sessions
            let sessions = Arc::new(FileSessionStore::new(config.data_dir.join("sessions")).await?);

            // Initialize vector memory (persistent)
            let memory_path = config.data_dir.join("memory").join("vectors.jsonl");
            let vector_store: Arc<dyn argentor_memory::VectorStore> = Arc::new(
                argentor_memory::FileVectorStore::new(memory_path)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to initialize vector store: {e}"))?,
            );
            let embedder: Arc<dyn argentor_memory::EmbeddingProvider> =
                Arc::new(argentor_memory::LocalEmbedding::default());
            info!("Vector memory initialized");

            // Load skills: builtins (with memory) first, then WASM skills from config
            let registry = SkillRegistry::new();
            argentor_builtins::register_builtins_with_memory(&registry, vector_store, embedder);
            info!(count = registry.skill_count(), "Built-in skills registered");

            if !config.skills.is_empty() {
                let loader = SkillLoader::new()?;
                let loaded = loader.load_all(&config.skills, &config_dir, &registry)?;
                info!(count = loaded, "WASM skills loaded from config");
            }

            // Load markdown skills and tool groups
            let _prompt_injection = load_markdown_skills(&config, &config_dir, &registry).await;
            load_tool_groups(&config, &registry);

            // Connect to MCP servers and register their tools as skills
            let mcp_manager = Arc::new(argentor_mcp::McpServerManager::new());
            if !config.mcp_servers.is_empty() {
                let errors = mcp_manager
                    .connect_all(&config.mcp_servers, &registry)
                    .await;
                if !errors.is_empty() {
                    tracing::warn!(failed = errors.len(), "Some MCP servers failed to connect");
                }
                // Start background health check loop (60s default)
                let mgr = mcp_manager.clone();
                mgr.start_health_loop(std::time::Duration::from_secs(60));
            }

            // Build permissions from all loaded skills' required capabilities
            let mut permissions = PermissionSet::new();
            for desc in registry.list_descriptors() {
                for cap in &desc.required_capabilities {
                    permissions.grant(cap.clone());
                }
            }

            let skills = Arc::new(registry);

            let agent = Arc::new(AgentRunner::new(
                config.model,
                skills.clone(),
                permissions,
                audit,
            ));

            let app = GatewayServer::build_with_middleware(
                agent,
                sessions,
                Some(rate_limiter),
                auth_config,
                None,
                None,
            );

            let addr = format!("{host}:{port}");
            let listener = tokio::net::TcpListener::bind(&addr).await?;

            if config.server.tls.enabled {
                // TLS/mTLS mode
                let tls_config = argentor_security::tls::TlsConfig {
                    enabled: true,
                    cert_path: config.server.tls.cert_path.clone(),
                    key_path: config.server.tls.key_path.clone(),
                    client_ca_path: config.server.tls.client_ca_path.clone(),
                };
                tls::validate_tls_config(&tls_config).await?;
                let acceptor = tls::build_tls_acceptor(&tls_config).await?;

                info!("Argentor gateway listening on {} (TLS enabled)", addr);

                // Graceful shutdown for TLS mode
                let shutdown = shutdown_signal();
                tokio::pin!(shutdown);

                loop {
                    tokio::select! {
                        result = listener.accept() => {
                            let (stream, peer_addr) = result?;
                            let acceptor = acceptor.clone();
                            let app = app.clone();
                            tokio::spawn(async move {
                                match acceptor.accept(stream).await {
                                    Ok(tls_stream) => {
                                        let io = hyper_util::rt::TokioIo::new(tls_stream);
                                        let svc = hyper_util::service::TowerToHyperService::new(app);
                                        let conn = hyper_util::server::conn::auto::Builder::new(
                                            hyper_util::rt::TokioExecutor::new(),
                                        );
                                        if let Err(e) = conn.serve_connection(io, svc).await {
                                            tracing::error!(
                                                peer = %peer_addr,
                                                error = %e,
                                                "TLS connection error"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            peer = %peer_addr,
                                            error = %e,
                                            "TLS handshake failed"
                                        );
                                    }
                                }
                            });
                        }
                        _ = &mut shutdown => {
                            info!("Shutting down TLS server");
                            break;
                        }
                    }
                }
            } else {
                info!("Argentor gateway listening on {}", addr);
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown_signal())
                    .await?;
            }

            info!("Argentor gateway stopped");
        }
        Commands::A2a { .. } => unreachable!("handled above"),
        Commands::Skill { action } => match action {
            SkillAction::List => {
                // Load all skills: builtins + config + markdown
                let registry = SkillRegistry::new();
                argentor_builtins::register_builtins(&registry);
                if !config.skills.is_empty() {
                    let loader = SkillLoader::new()?;
                    let _ = loader.load_all(&config.skills, &config_dir, &registry);
                }
                let _ = load_markdown_skills(&config, &config_dir, &registry).await;
                load_tool_groups(&config, &registry);

                let skills = registry.list_descriptors();
                if skills.is_empty() {
                    println!("No skills registered.");
                    println!("Configure skills in argentor.toml under [[skills]]");
                } else {
                    println!("Registered skills:");
                    for skill in &skills {
                        println!("  {} — {}", skill.name, skill.description);
                        if !skill.required_capabilities.is_empty() {
                            println!("    Capabilities:");
                            for cap in &skill.required_capabilities {
                                match cap {
                                    Capability::FileRead { allowed_paths } => {
                                        println!("      file_read: {allowed_paths:?}");
                                    }
                                    Capability::FileWrite { allowed_paths } => {
                                        println!("      file_write: {allowed_paths:?}");
                                    }
                                    Capability::NetworkAccess { allowed_hosts } => {
                                        println!("      network: {allowed_hosts:?}");
                                    }
                                    Capability::ShellExec { allowed_commands } => {
                                        println!("      shell: {allowed_commands:?}");
                                    }
                                    _ => {
                                        println!("      {cap:?}");
                                    }
                                }
                            }
                        }
                    }
                    println!("\nTotal: {} skill(s)", skills.len());
                }

                // Show tool groups
                let groups = registry.list_groups();
                if !groups.is_empty() {
                    println!("\nTool Groups:");
                    for group in &groups {
                        let available = registry.skills_in_group(&group.name);
                        if group.skills.is_empty() {
                            println!("  {} — {} [all skills]", group.name, group.description);
                        } else {
                            println!(
                                "  {} — {} [{}/{}]",
                                group.name,
                                group.description,
                                available.len(),
                                group.skills.len()
                            );
                        }
                    }
                }
            }
        },
        Commands::Orchestrate {
            task,
            output_dir,
            interactive_approval,
            approval_timeout,
        } => {
            // Initialize security
            let audit = Arc::new(AuditLog::new(config.data_dir.join("audit")));

            // Load skills: builtins + config + markdown
            let registry = SkillRegistry::new();
            if interactive_approval {
                let channel = Arc::new(argentor_builtins::StdinApprovalChannel::new(
                    std::time::Duration::from_secs(approval_timeout),
                ));
                argentor_builtins::register_builtins_with_approval(&registry, channel);
                info!(
                    "Interactive approval enabled (timeout: {}s)",
                    approval_timeout
                );
            } else {
                argentor_builtins::register_builtins(&registry);
            }
            if !config.skills.is_empty() {
                let loader = SkillLoader::new()?;
                let _ = loader.load_all(&config.skills, &config_dir, &registry);
            }
            let _prompt_injection = load_markdown_skills(&config, &config_dir, &registry).await;
            load_tool_groups(&config, &registry);

            // Build permissions
            let mut permissions = PermissionSet::new();
            for desc in registry.list_descriptors() {
                for cap in &desc.required_capabilities {
                    permissions.grant(cap.clone());
                }
            }

            let skills = Arc::new(registry);

            // Build compliance hooks for automated event tracking
            let iso27001_module = Arc::new(argentor_compliance::Iso27001Module::new());
            let iso42001_module = Arc::new(argentor_compliance::Iso42001Module::new());
            let system_id = uuid::Uuid::new_v4();

            let iso27001_hook = Arc::new(argentor_compliance::Iso27001Hook::new(
                iso27001_module.clone(),
            ));
            let iso42001_hook = Arc::new(argentor_compliance::Iso42001Hook::new(
                iso42001_module.clone(),
                system_id,
            ));

            let mut compliance_chain = argentor_compliance::ComplianceHookChain::new();
            compliance_chain.add(iso27001_hook);
            compliance_chain.add(iso42001_hook);
            let compliance_chain = Arc::new(compliance_chain);

            // Create orchestrator with progress callback, compliance hooks, and optional output dir
            let mut orchestrator =
                argentor_orchestrator::Orchestrator::new(&config.model, skills, permissions, audit);
            orchestrator = orchestrator
                .with_progress(|role, msg| {
                    eprintln!("  [{role:>10}] {msg}");
                })
                .with_compliance(compliance_chain.clone());
            if let Some(dir) = output_dir {
                orchestrator = orchestrator.with_output_dir(dir);
            }

            eprintln!("Argentor Multi-Agent Orchestrator");
            eprintln!("================================");
            eprintln!("Task: {task}");
            eprintln!();

            let start = Instant::now();

            // Run pipeline
            match orchestrator.run(&task).await {
                Ok(result) => {
                    let duration = start.elapsed();
                    eprintln!();
                    eprintln!("{} ({:.1}s)", result.summary, duration.as_secs_f64());

                    // Show written files
                    if !result.written_files.is_empty() {
                        eprintln!();
                        eprintln!("Files written:");
                        for f in &result.written_files {
                            eprintln!("  {f}");
                        }
                    }

                    eprintln!();

                    // Print artifacts to stdout (pipeable)
                    for artifact in &result.artifacts {
                        let label = match artifact.kind {
                            argentor_orchestrator::ArtifactKind::Spec => "SPEC",
                            argentor_orchestrator::ArtifactKind::Code => "CODE",
                            argentor_orchestrator::ArtifactKind::Test => "TEST",
                            argentor_orchestrator::ArtifactKind::Review => "REVIEW",
                            argentor_orchestrator::ArtifactKind::Report => "REPORT",
                        };
                        println!("=== {label} ===");
                        println!("{}", artifact.content);
                        println!();
                    }
                }
                Err(e) => {
                    eprintln!("Orchestration failed: {e}");
                    std::process::exit(1);
                }
            }

            // Persist compliance event counts as a summary
            let access_events = iso27001_module.access_event_count().await;
            let transparency_logs = iso42001_module.transparency_log_count().await;
            if access_events > 0 || transparency_logs > 0 {
                eprintln!(
                    "Compliance: {access_events} ISO 27001 access events, {transparency_logs} ISO 42001 transparency logs"
                );
            }
        }
        Commands::Compliance { action } => match action {
            ComplianceAction::Report => {
                use argentor_compliance::{
                    dpga::{assess_argentor_dpga, DpgaInput},
                    gdpr::GdprModule,
                    iso27001::Iso27001Module,
                    iso42001::Iso42001Module,
                };

                println!("Argentor Compliance Report");
                println!("========================\n");

                // GDPR
                let gdpr = GdprModule::new();
                let gdpr_report = gdpr.assess(true, true, true, false);
                println!("{}:", gdpr_report.framework);
                println!("  Status: {:?}", gdpr_report.status);
                for f in &gdpr_report.findings {
                    let icon = if f.compliant { "+" } else { "-" };
                    println!("  [{}] {}: {}", icon, f.title, f.description);
                    if !f.recommendation.is_empty() {
                        println!("      Recommendation: {}", f.recommendation);
                    }
                }

                println!();

                // ISO 27001
                let iso27001 = Iso27001Module::new();
                let iso_report = iso27001.assess(true, true, true, true, false);
                println!("{}:", iso_report.framework);
                println!("  Status: {:?}", iso_report.status);
                for f in &iso_report.findings {
                    let icon = if f.compliant { "+" } else { "-" };
                    println!("  [{}] {}: {}", icon, f.title, f.description);
                    if !f.recommendation.is_empty() {
                        println!("      Recommendation: {}", f.recommendation);
                    }
                }

                println!();

                // ISO 42001
                let iso42001 = Iso42001Module::new();
                let ai_report = iso42001.assess(true, true, true, true, true);
                println!("{}:", ai_report.framework);
                println!("  Status: {:?}", ai_report.status);
                for f in &ai_report.findings {
                    let icon = if f.compliant { "+" } else { "-" };
                    println!("  [{}] {}: {}", icon, f.title, f.description);
                    if !f.recommendation.is_empty() {
                        println!("      Recommendation: {}", f.recommendation);
                    }
                }

                println!();

                // DPGA
                let dpga_input = DpgaInput {
                    has_open_license: true,
                    has_sdg_docs: true,
                    has_open_data: true,
                    has_privacy: true,
                    has_docs: true,
                    has_open_standards: true,
                    has_governance: false,
                    has_do_no_harm: true,
                    has_interop: true,
                };
                let dpga_report = assess_argentor_dpga(&dpga_input);
                println!("{}:", dpga_report.framework);
                println!("  Status: {:?}", dpga_report.status);
                for f in &dpga_report.findings {
                    let icon = if f.compliant { "+" } else { "-" };
                    println!("  [{}] {}: {}", icon, f.title, f.description);
                    if !f.recommendation.is_empty() {
                        println!("      Recommendation: {}", f.recommendation);
                    }
                }

                println!("\n{}", dpga_report.summary);

                // Persist all reports to disk
                let report_dir = config.data_dir.join("compliance_reports");
                let store = argentor_compliance::JsonReportStore::new(&report_dir);
                let reports = [&gdpr_report, &iso_report, &ai_report, &dpga_report];
                let mut saved = 0;
                for report in &reports {
                    match store.save_report(report).await {
                        Ok(path) => {
                            saved += 1;
                            println!("  Saved: {}", path.display());
                        }
                        Err(e) => {
                            eprintln!("  Failed to save {} report: {}", report.framework, e);
                        }
                    }
                }
                if saved > 0 {
                    println!("\n{} report(s) saved to {}", saved, report_dir.display());
                }
            }
        },
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_env_vars_known() {
        std::env::set_var("ARGENTOR_TEST_VAR", "hello123");
        let result = expand_env_vars("key = \"${ARGENTOR_TEST_VAR}\"");
        assert_eq!(result, "key = \"hello123\"");
        std::env::remove_var("ARGENTOR_TEST_VAR");
    }

    #[test]
    fn test_expand_env_vars_unknown() {
        let result = expand_env_vars("key = \"${ARGENTOR_NONEXISTENT_12345}\"");
        assert_eq!(result, "key = \"\"");
    }

    #[test]
    fn test_expand_env_vars_no_vars() {
        let result = expand_env_vars("plain text without vars");
        assert_eq!(result, "plain text without vars");
    }

    #[test]
    fn test_expand_env_vars_multiple() {
        std::env::set_var("ARGENTOR_A", "foo");
        std::env::set_var("ARGENTOR_B", "bar");
        let result = expand_env_vars("${ARGENTOR_A} and ${ARGENTOR_B}");
        assert_eq!(result, "foo and bar");
        std::env::remove_var("ARGENTOR_A");
        std::env::remove_var("ARGENTOR_B");
    }
}
