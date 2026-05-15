//! XcapitSFF integration — agent execution endpoints, webhook proxy, and health checks.
//!
//! Provides REST endpoints for XcapitSFF (Python/FastAPI) to invoke Argentor agents
//! by role, execute batch tasks, proxy webhooks with compliance, and cross-check health.
//!
//! # Endpoints
//!
//! - `POST /api/v1/agent/run-task` — Execute a single agent task by role
//! - `POST /api/v1/agent/batch` — Execute multiple agent tasks in parallel
//! - `POST /api/v1/proxy/webhook` — Proxy external webhooks with HMAC validation and audit
//! - `GET /api/v1/health` — Extended health check including XcapitSFF status

use argentor_agent::evaluator::ResponseEvaluator;
use argentor_agent::guardrails::{GuardrailEngine, RuleSeverity as GuardrailSeverity};
use argentor_agent::prompt_manager::PromptManager;
use argentor_agent::{AgentRunner, ModelConfig, StreamEvent};
use argentor_memory::conversation::{ConversationMemory, ConversationSummarizer};
use argentor_orchestrator::workflow::{
    lead_qualification_workflow, support_ticket_workflow, WorkflowEngine,
};
use argentor_security::audit::AuditOutcome;
use argentor_security::tenant_limits::{TenantLimitManager, TenantPlan};
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::SkillRegistry;
use axum::{
    extract::{Json, Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event as SseEvent, Sse},
        IntoResponse,
    },
    routing::{get, post},
    Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{error, info};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Agent profiles
// ---------------------------------------------------------------------------

/// Pre-configured agent profile for XcapitSFF integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XcapitAgentProfile {
    /// Role identifier (e.g. "sales_qualifier").
    pub role: String,
    /// Primary model configuration.
    pub model: ModelConfig,
    /// System prompt for this role.
    pub system_prompt: String,
}

/// Build the 4 default XcapitSFF agent profiles.
pub fn default_xcapit_profiles() -> HashMap<String, XcapitAgentProfile> {
    let mut profiles = HashMap::new();

    // -- sales_qualifier --
    profiles.insert("sales_qualifier".to_string(), XcapitAgentProfile {
        role: "sales_qualifier".to_string(),
        model: ModelConfig {
            provider: argentor_agent::config::LlmProvider::Claude,
            model_id: "claude-sonnet-4-6-20250514".to_string(),
            api_key: String::new(), // resolved from env
            api_base_url: None,
            temperature: 0.3,
            max_tokens: 2048,
            max_turns: 5,
            max_context_tokens: 200_000,
            fallback_models: vec![ModelConfig {
                provider: argentor_agent::config::LlmProvider::OpenAi,
                model_id: "gpt-4o-mini".to_string(),
                api_key: String::new(),
                api_base_url: None,
                temperature: 0.3,
                max_tokens: 2048,
                max_turns: 5,
                max_context_tokens: 200_000,
                fallback_models: vec![],
                retry_policy: None,
            }],
            retry_policy: None,
        },
        system_prompt: "Sos el agente de calificación de ventas de Xcapit. Evaluás leads usando ICP scoring (Region, C-Level, Afinidad, Score). Clasificás como hot (>=70), warm (>=45), cool (>=25), cold (<25). Para cada lead, respondé con: score, clasificación, acción recomendada, prioridad de outreach. Sé conciso y accionable.".to_string(),
    });

    // -- outreach_composer --
    profiles.insert("outreach_composer".to_string(), XcapitAgentProfile {
        role: "outreach_composer".to_string(),
        model: ModelConfig {
            provider: argentor_agent::config::LlmProvider::Claude,
            model_id: "claude-sonnet-4-6-20250514".to_string(),
            api_key: String::new(),
            api_base_url: None,
            temperature: 0.7,
            max_tokens: 4096,
            max_turns: 5,
            max_context_tokens: 200_000,
            fallback_models: vec![ModelConfig {
                provider: argentor_agent::config::LlmProvider::OpenAi,
                model_id: "gpt-4o-mini".to_string(),
                api_key: String::new(),
                api_base_url: None,
                temperature: 0.7,
                max_tokens: 4096,
                max_turns: 5,
                max_context_tokens: 200_000,
                fallback_models: vec![],
                retry_policy: None,
            }],
            retry_policy: None,
        },
        system_prompt: "Sos el agente de outreach de Xcapit, empresa de tecnología financiera (inversión automatizada, gestión de activos digitales, DeFi). Componés mensajes personalizados según canal (email/linkedin/whatsapp), región (LATAM=español neutro, Iberia=español peninsular), seniority (C-Level=ROI estratégico, otros=operativo), y afinidad (HIGH=directo, MEDIUM=educativo, LOW=nurturing). Siempre incluí mensaje principal, variante A/B, y siguiente paso sugerido.".to_string(),
    });

    // -- support_responder --
    profiles.insert("support_responder".to_string(), XcapitAgentProfile {
        role: "support_responder".to_string(),
        model: ModelConfig {
            provider: argentor_agent::config::LlmProvider::Claude,
            model_id: "claude-sonnet-4-6-20250514".to_string(),
            api_key: String::new(),
            api_base_url: None,
            temperature: 0.4,
            max_tokens: 4096,
            max_turns: 5,
            max_context_tokens: 200_000,
            fallback_models: vec![ModelConfig {
                provider: argentor_agent::config::LlmProvider::OpenAi,
                model_id: "gpt-4o-mini".to_string(),
                api_key: String::new(),
                api_base_url: None,
                temperature: 0.4,
                max_tokens: 4096,
                max_turns: 5,
                max_context_tokens: 200_000,
                fallback_models: vec![],
                retry_policy: None,
            }],
            retry_policy: None,
        },
        system_prompt: "Sos el agente de soporte al cliente de Xcapit (fintech, inversión automatizada, activos digitales). Resolvés tickets de forma empática, precisa y rápida. Si no tenés certeza, decilo. Si es tema de fondos/dinero, NUNCA dar instrucciones sin verificación. Si es bug, documentar pasos de reproducción. Siempre ofrecer siguiente paso claro. Español LATAM por defecto. Respondé con: respuesta al cliente, notas internas, y si hay que escalar.".to_string(),
    });

    // -- ticket_router --
    profiles.insert("ticket_router".to_string(), XcapitAgentProfile {
        role: "ticket_router".to_string(),
        model: ModelConfig {
            provider: argentor_agent::config::LlmProvider::Claude,
            model_id: "claude-sonnet-4-6-20250514".to_string(),
            api_key: String::new(),
            api_base_url: None,
            temperature: 0.2,
            max_tokens: 1024,
            max_turns: 3,
            max_context_tokens: 200_000,
            fallback_models: vec![ModelConfig {
                provider: argentor_agent::config::LlmProvider::OpenAi,
                model_id: "gpt-4o-mini".to_string(),
                api_key: String::new(),
                api_base_url: None,
                temperature: 0.2,
                max_tokens: 1024,
                max_turns: 3,
                max_context_tokens: 200_000,
                fallback_models: vec![],
                retry_policy: None,
            }],
            retry_policy: None,
        },
        system_prompt: "Clasificás tickets de soporte por categoría (billing, technical, account, crypto, general) y prioridad (urgent, high, medium, low). Respondé SOLO con JSON: {\"category\": \"...\", \"priority\": \"...\", \"confidence\": 0.0-1.0, \"requires_human_review\": true/false, \"reasoning\": \"...\"}."
            .to_string(),
    });

    profiles
}

/// Resolve API keys from environment into the profile's model config.
///
/// If the corresponding environment variable is missing or empty, a warning is
/// logged and the provider will be unavailable (the key stays empty).
fn resolve_api_keys(config: &mut ModelConfig) {
    if config.api_key.is_empty() {
        let (env_var, provider_label) = match config.provider {
            argentor_agent::config::LlmProvider::Claude
            | argentor_agent::config::LlmProvider::ClaudeCode => {
                ("ANTHROPIC_API_KEY", "Anthropic/Claude")
            }
            argentor_agent::config::LlmProvider::OpenAi => ("OPENAI_API_KEY", "OpenAI"),
            argentor_agent::config::LlmProvider::Gemini => ("GEMINI_API_KEY", "Gemini"),
            _ => ("OPENAI_API_KEY", "OpenAI-compatible"),
        };

        match std::env::var(env_var) {
            Ok(key) if !key.is_empty() => {
                config.api_key = key;
            }
            _ => {
                tracing::warn!(
                    provider = provider_label,
                    env_var = env_var,
                    model = %config.model_id,
                    "API key not found — {} provider will be unavailable. \
                     Set {} to enable it.",
                    provider_label,
                    env_var,
                );
                // Leave api_key empty; callers must check before using.
            }
        }
    }
    for fallback in &mut config.fallback_models {
        resolve_api_keys(fallback);
    }
}

// ---------------------------------------------------------------------------
// XcapitSFF health state
// ---------------------------------------------------------------------------

/// Health status of XcapitSFF backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XcapitSffHealth {
    /// Base URL of XcapitSFF.
    pub url: String,
    /// Current status ("ok", "unreachable", "degraded").
    pub status: String,
    /// Last successful health check timestamp.
    pub last_check: Option<DateTime<Utc>>,
    /// Last error message, if any.
    pub last_error: Option<String>,
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Configuration for the XcapitSFF integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XcapitConfig {
    /// Base URL of XcapitSFF (e.g. "http://localhost:8000").
    pub url: String,
    /// Health check interval in seconds.
    pub health_check_interval: u64,
    /// Allowed webhook sources.
    pub allowed_webhook_sources: Vec<String>,
    /// HMAC secret for webhook validation.
    pub webhook_hmac_secret: String,
    /// CORS allowed origins.
    pub cors_origins: Vec<String>,
}

impl Default for XcapitConfig {
    fn default() -> Self {
        Self {
            url: std::env::var("XCAPITSFF_URL")
                .unwrap_or_else(|_| "http://localhost:8000".to_string()),
            health_check_interval: 30,
            allowed_webhook_sources: vec![
                "hubspot".to_string(),
                "salesforce".to_string(),
                "stripe".to_string(),
                "intercom".to_string(),
            ],
            webhook_hmac_secret: std::env::var("WEBHOOK_SECRET").unwrap_or_else(|_| {
                tracing::warn!(
                    "WEBHOOK_SECRET not set — webhook HMAC validation disabled. \
                     Set WEBHOOK_SECRET for production use."
                );
                String::new()
            }),
            cors_origins: std::env::var("XCAPITSFF_CORS_ORIGINS")
                .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_else(|_| {
                    let base = std::env::var("XCAPITSFF_URL")
                        .unwrap_or_else(|_| "http://localhost:8000".to_string());
                    vec![base, "http://xcapitsff:8000".to_string()]
                }),
        }
    }
}

/// Shared state for XcapitSFF integration endpoints.
pub struct XcapitState {
    /// Agent profiles indexed by role.
    pub profiles: HashMap<String, XcapitAgentProfile>,
    /// Shared skill registry.
    pub skills: Arc<SkillRegistry>,
    /// Permissions for agent execution.
    pub permissions: PermissionSet,
    /// Audit log for compliance.
    pub audit: Arc<AuditLog>,
    /// Integration configuration.
    pub config: XcapitConfig,
    /// XcapitSFF health state (updated by background task).
    pub xcapitsff_health: Arc<RwLock<XcapitSffHealth>>,
    /// HTTP client for outbound requests.
    pub http_client: reqwest::Client,
    /// Per-tenant usage tracker for cost monitoring.
    pub usage_tracker: TenantUsageTracker,
    /// Per-tenant agent personas, keyed by (tenant_id, agent_role).
    pub personas: Arc<RwLock<HashMap<(String, String), PersonaConfig>>>,
    /// AI guardrails engine for input/output validation.
    pub guardrails: GuardrailEngine,
    /// Per-tenant rate limit enforcement.
    pub tenant_limits: TenantLimitManager,
    /// Conversation memory for cross-session context.
    pub conversation_memory: ConversationMemory,
    /// Prompt template manager.
    pub prompt_manager: PromptManager,
    /// Analytics engine for business metrics.
    pub analytics: crate::analytics::AnalyticsEngine,
    /// Workflow engine for automated business pipelines.
    pub workflow_engine: WorkflowEngine,
}

impl XcapitState {
    /// Create a new XcapitState with default profiles.
    pub fn new(
        skills: Arc<SkillRegistry>,
        permissions: PermissionSet,
        audit: Arc<AuditLog>,
        config: XcapitConfig,
    ) -> Self {
        let health = XcapitSffHealth {
            url: config.url.clone(),
            status: "unknown".to_string(),
            last_check: None,
            last_error: None,
        };

        Self {
            profiles: default_xcapit_profiles(),
            skills,
            permissions,
            audit,
            config,
            xcapitsff_health: Arc::new(RwLock::new(health)),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            usage_tracker: TenantUsageTracker::new(),
            personas: Arc::new(RwLock::new(HashMap::new())),
            guardrails: GuardrailEngine::new(),
            tenant_limits: TenantLimitManager::new(),
            conversation_memory: ConversationMemory::new(),
            prompt_manager: {
                let pm = PromptManager::new();
                argentor_agent::prompt_manager::register_xcapit_templates(&pm);
                pm
            },
            analytics: crate::analytics::AnalyticsEngine::new(),
            workflow_engine: WorkflowEngine::new(),
        }
    }

    /// Register default workflow definitions (lead qualification, support ticket).
    /// Must be called after construction since workflow registration is async.
    pub async fn init_workflows(&self) {
        self.workflow_engine
            .register_workflow(lead_qualification_workflow())
            .await;
        self.workflow_engine
            .register_workflow(support_ticket_workflow())
            .await;
    }

    /// Start the background health check loop for XcapitSFF.
    pub fn start_health_loop(self: &Arc<Self>) {
        let state = self.clone();
        let interval = std::time::Duration::from_secs(state.config.health_check_interval);
        tokio::spawn(async move {
            loop {
                let url = format!("{}/health", state.config.url);
                let result = state.http_client.get(&url).send().await;
                let mut health = state.xcapitsff_health.write().await;
                match result {
                    Ok(resp) if resp.status().is_success() => {
                        health.status = "ok".to_string();
                        health.last_check = Some(Utc::now());
                        health.last_error = None;
                    }
                    Ok(resp) => {
                        health.status = "degraded".to_string();
                        health.last_check = Some(Utc::now());
                        health.last_error = Some(format!("HTTP {}", resp.status()));
                    }
                    Err(e) => {
                        health.status = "unreachable".to_string();
                        health.last_check = Some(Utc::now());
                        health.last_error = Some(e.to_string());
                    }
                }
                tokio::time::sleep(interval).await;
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

// -- run-task --

/// Request body for POST /api/v1/agent/run-task.
#[derive(Debug, Deserialize)]
pub struct RunTaskRequest {
    /// Agent role (e.g. "sales_qualifier", "outreach_composer").
    pub agent_role: String,
    /// Optional system prompt override (uses profile default if omitted).
    pub system_prompt: Option<String>,
    /// User context / input for the agent.
    pub context: String,
    /// Optional session ID for conversation continuity.
    pub session_id: Option<String>,
    /// Max tokens for the response (overrides profile if set).
    pub max_tokens: Option<u32>,
    /// Temperature (overrides profile if set).
    pub temperature: Option<f32>,
    /// Optional tenant ID for multi-tenant usage tracking and persona injection.
    pub tenant_id: Option<String>,
    /// Optional routing hint to override the model selection.
    /// Values: "fast_cheap", "balanced", "quality_max".
    pub routing_hint: Option<String>,
}

/// Response for POST /api/v1/agent/run-task.
#[derive(Debug, Serialize)]
pub struct RunTaskResponse {
    /// Agent response text.
    pub response: String,
    /// Session ID (new or existing).
    pub session_id: String,
    /// Model that produced the response.
    pub model_used: String,
    /// Estimated input tokens.
    pub tokens_input: u64,
    /// Estimated output tokens.
    pub tokens_output: u64,
    /// Tool calls made during execution (if any).
    pub tool_calls: Vec<String>,
    /// Compliance flags raised during execution.
    pub compliance_flags: Vec<String>,
    /// Total wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

// -- batch --

/// A single task in a batch request.
#[derive(Debug, Deserialize)]
pub struct BatchTaskItem {
    /// Agent role.
    pub agent_role: String,
    /// User context / input.
    pub context: String,
    /// Optional system prompt override.
    pub system_prompt: Option<String>,
    /// Max tokens override.
    pub max_tokens: Option<u32>,
}

/// Request body for POST /api/v1/agent/batch.
#[derive(Debug, Deserialize)]
pub struct BatchRequest {
    /// List of tasks to execute.
    pub tasks: Vec<BatchTaskItem>,
    /// Maximum concurrent executions (default: 5).
    pub max_concurrent: Option<usize>,
}

/// A single result in a batch response.
#[derive(Debug, Serialize)]
pub struct BatchResultItem {
    /// Index in the original task list.
    pub index: usize,
    /// Whether the task succeeded.
    pub success: bool,
    /// Agent response (empty on failure).
    pub response: String,
    /// Error message (empty on success).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Estimated input tokens.
    pub tokens_input: u64,
    /// Estimated output tokens.
    pub tokens_output: u64,
}

/// Response for POST /api/v1/agent/batch.
#[derive(Debug, Serialize)]
pub struct BatchResponse {
    /// Individual results.
    pub results: Vec<BatchResultItem>,
    /// Total tasks in the batch.
    pub total: usize,
    /// Number of successful tasks.
    pub succeeded: usize,
    /// Number of failed tasks.
    pub failed: usize,
    /// Total tokens consumed.
    pub total_tokens: u64,
    /// Total wall-clock duration in milliseconds.
    pub total_duration_ms: u64,
}

// -- webhook proxy --

/// Request body for POST /api/v1/proxy/webhook.
#[derive(Debug, Deserialize, Serialize)]
pub struct WebhookProxyRequest {
    /// Event type (e.g. "lead.created").
    pub event: String,
    /// Event payload.
    pub data: serde_json::Value,
    /// Source system (e.g. "hubspot").
    pub source: String,
}

/// Response for webhook proxy.
#[derive(Debug, Serialize)]
pub struct WebhookProxyResponse {
    /// Whether the forward succeeded.
    pub forwarded: bool,
    /// HTTP status from XcapitSFF.
    pub upstream_status: Option<u16>,
    /// Audit log entry ID.
    pub audit_id: String,
    /// Error message, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// -- health --

/// Extended health check response.
#[derive(Debug, Serialize)]
pub struct ExtendedHealthResponse {
    /// Overall status.
    pub status: String,
    /// Argentor version.
    pub version: String,
    /// Sub-system checks.
    pub checks: HealthChecks,
    /// XcapitSFF health detail.
    pub xcapitsff: XcapitSffHealth,
}

/// Individual health checks.
#[derive(Debug, Serialize)]
pub struct HealthChecks {
    /// LLM backends status.
    pub llm_backends: String,
    /// XcapitSFF connectivity.
    pub xcapitsff: String,
    /// Compliance modules.
    pub compliance: String,
}

// ---------------------------------------------------------------------------
// Cost tracking per tenant
// ---------------------------------------------------------------------------

/// A single usage record for a tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    /// Agent role that produced this usage.
    pub agent_role: String,
    /// Model used for this request.
    pub model: String,
    /// Input tokens consumed.
    pub tokens_in: u64,
    /// Output tokens consumed.
    pub tokens_out: u64,
    /// Estimated cost in USD.
    pub cost_usd: f64,
    /// Timestamp of the usage event.
    pub timestamp: DateTime<Utc>,
}

/// Time period for usage queries.
#[derive(Debug, Clone, Default, Deserialize)]
pub enum UsagePeriod {
    /// Last N hours.
    #[serde(rename = "hours")]
    Hours(u64),
    /// Last N days.
    #[serde(rename = "days")]
    Days(u64),
    /// All recorded usage.
    #[serde(rename = "all")]
    #[default]
    All,
}

/// Aggregated usage breakdown by agent role and model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageBreakdown {
    /// Tenant ID.
    pub tenant_id: String,
    /// Total input tokens across all requests.
    pub total_tokens_in: u64,
    /// Total output tokens across all requests.
    pub total_tokens_out: u64,
    /// Total estimated cost in USD.
    pub total_cost_usd: f64,
    /// Number of requests recorded.
    pub request_count: usize,
    /// Breakdown by agent role.
    pub by_agent: HashMap<String, AgentUsageSummary>,
    /// Breakdown by model.
    pub by_model: HashMap<String, ModelUsageSummary>,
}

/// Usage summary for a specific agent role.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentUsageSummary {
    /// Total input tokens for this agent.
    pub tokens_in: u64,
    /// Total output tokens for this agent.
    pub tokens_out: u64,
    /// Total cost for this agent.
    pub cost_usd: f64,
    /// Number of requests.
    pub count: usize,
}

/// Usage summary for a specific model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelUsageSummary {
    /// Total input tokens for this model.
    pub tokens_in: u64,
    /// Total output tokens for this model.
    pub tokens_out: u64,
    /// Total cost for this model.
    pub cost_usd: f64,
    /// Number of requests.
    pub count: usize,
}

/// Thread-safe tenant usage tracker.
///
/// Stores usage records per tenant and provides aggregated queries.
pub struct TenantUsageTracker {
    /// Usage records indexed by tenant ID.
    records: Arc<RwLock<HashMap<String, Vec<UsageRecord>>>>,
}

impl TenantUsageTracker {
    /// Create a new empty usage tracker.
    pub fn new() -> Self {
        Self {
            records: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record a usage event for a tenant.
    pub async fn record(
        &self,
        tenant_id: &str,
        agent_role: &str,
        model: &str,
        tokens_in: u64,
        tokens_out: u64,
        cost_usd: f64,
    ) {
        let record = UsageRecord {
            agent_role: agent_role.to_string(),
            model: model.to_string(),
            tokens_in,
            tokens_out,
            cost_usd,
            timestamp: Utc::now(),
        };
        let mut records = self.records.write().await;
        records
            .entry(tenant_id.to_string())
            .or_default()
            .push(record);
    }

    /// Query aggregated usage for a tenant within a time period.
    pub async fn get_usage(&self, tenant_id: &str, period: &UsagePeriod) -> UsageBreakdown {
        let records = self.records.read().await;
        let empty = vec![];
        let tenant_records = records.get(tenant_id).unwrap_or(&empty);

        let cutoff = match period {
            UsagePeriod::Hours(h) => Some(Utc::now() - chrono::Duration::hours(*h as i64)),
            UsagePeriod::Days(d) => Some(Utc::now() - chrono::Duration::days(*d as i64)),
            UsagePeriod::All => None,
        };

        let filtered: Vec<&UsageRecord> = tenant_records
            .iter()
            .filter(|r| match cutoff {
                Some(ref c) => r.timestamp >= *c,
                None => true,
            })
            .collect();

        let mut by_agent: HashMap<String, AgentUsageSummary> = HashMap::new();
        let mut by_model: HashMap<String, ModelUsageSummary> = HashMap::new();
        let mut total_tokens_in: u64 = 0;
        let mut total_tokens_out: u64 = 0;
        let mut total_cost_usd: f64 = 0.0;

        for r in &filtered {
            total_tokens_in += r.tokens_in;
            total_tokens_out += r.tokens_out;
            total_cost_usd += r.cost_usd;

            let agent_entry = by_agent.entry(r.agent_role.clone()).or_default();
            agent_entry.tokens_in += r.tokens_in;
            agent_entry.tokens_out += r.tokens_out;
            agent_entry.cost_usd += r.cost_usd;
            agent_entry.count += 1;

            let model_entry = by_model.entry(r.model.clone()).or_default();
            model_entry.tokens_in += r.tokens_in;
            model_entry.tokens_out += r.tokens_out;
            model_entry.cost_usd += r.cost_usd;
            model_entry.count += 1;
        }

        UsageBreakdown {
            tenant_id: tenant_id.to_string(),
            total_tokens_in,
            total_tokens_out,
            total_cost_usd,
            request_count: filtered.len(),
            by_agent,
            by_model,
        }
    }
}

impl Default for TenantUsageTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Agent personas per tenant
// ---------------------------------------------------------------------------

/// Configuration for an agent persona.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaConfig {
    /// Display name for the persona.
    pub name: String,
    /// Communication tone (e.g. "friendly", "professional", "casual").
    pub tone: String,
    /// Language style (e.g. "es_latam", "en_us", "pt_br").
    pub language_style: String,
    /// Optional signature appended to responses.
    #[serde(default)]
    pub signature: String,
    /// Custom instructions injected into the system prompt.
    #[serde(default)]
    pub custom_instructions: String,
}

/// Request body for POST /api/v1/agent/personas.
#[derive(Debug, Deserialize)]
pub struct CreatePersonaRequest {
    /// Tenant ID that owns this persona.
    pub tenant_id: String,
    /// Agent role this persona applies to.
    pub agent_role: String,
    /// Persona configuration.
    pub persona: PersonaConfig,
}

/// Response for persona creation.
#[derive(Debug, Serialize)]
pub struct CreatePersonaResponse {
    /// Whether the persona was created successfully.
    pub created: bool,
    /// Tenant ID.
    pub tenant_id: String,
    /// Agent role.
    pub agent_role: String,
    /// Persona name.
    pub persona_name: String,
}

/// Response for listing personas.
#[derive(Debug, Serialize)]
pub struct ListPersonasResponse {
    /// Tenant ID.
    pub tenant_id: String,
    /// Personas indexed by agent role.
    pub personas: HashMap<String, PersonaConfig>,
}

// ---------------------------------------------------------------------------
// Response quality scoring
// ---------------------------------------------------------------------------

/// Request body for POST /api/v1/agent/evaluate.
#[derive(Debug, Deserialize)]
pub struct EvaluateRequest {
    /// The response text to evaluate.
    pub response: String,
    /// The context or question that produced the response.
    pub context: String,
    /// Quality criteria to evaluate (e.g. "relevance", "helpfulness", "accuracy", "tone").
    #[serde(default)]
    pub criteria: Vec<String>,
}

/// Response for POST /api/v1/agent/evaluate.
#[derive(Debug, Serialize)]
pub struct EvaluateResponse {
    /// Overall quality score (0.0 - 1.0).
    pub overall_score: f32,
    /// Scores broken down by criterion.
    pub by_criteria: HashMap<String, f32>,
    /// Improvement suggestions.
    pub suggestions: Vec<String>,
}

// ---------------------------------------------------------------------------
// Usage query request
// ---------------------------------------------------------------------------

/// Query parameters for GET /api/v1/usage/tenant/{tenant_id}.
#[derive(Debug, Deserialize)]
pub struct UsageQueryParams {
    /// Period filter: "hours:N", "days:N", or "all".
    #[serde(default)]
    pub period: Option<String>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the XcapitSFF integration router.
pub fn xcapitsff_router(state: Arc<XcapitState>) -> Router {
    Router::new()
        .route("/api/v1/agent/run-task", post(run_task_handler))
        .route(
            "/api/v1/agent/run-task-stream",
            post(run_task_stream_handler),
        )
        .route("/api/v1/agent/batch", post(batch_handler))
        .route("/api/v1/agent/evaluate", post(evaluate_handler))
        .route("/api/v1/agent/personas", post(create_persona_handler))
        .route(
            "/api/v1/agent/personas/{tenant_id}",
            get(list_personas_handler),
        )
        .route("/api/v1/agent/profiles", get(list_profiles_handler))
        .route("/api/v1/proxy/webhook", post(webhook_proxy_handler))
        .route(
            "/api/v1/usage/tenant/{tenant_id}",
            get(tenant_usage_handler),
        )
        .route("/api/v1/health", get(extended_health_handler))
        .route(
            "/api/v1/tenants/{tenant_id}/register",
            post(register_tenant_handler),
        )
        .route(
            "/api/v1/tenants/{tenant_id}/status",
            get(tenant_status_handler),
        )
        .route("/api/v1/workflows/runs", get(list_workflow_runs_handler))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Resolve a routing hint to a model ID override.
///
/// Maps human-friendly hints to concrete model identifiers:
/// - `"fast_cheap"` → `gpt-4o-mini`
/// - `"balanced"` → `claude-sonnet-4-6-20250514` (default, no change)
/// - `"quality_max"` → `claude-opus-4-6-20250514`
fn resolve_routing_hint(hint: &str) -> Option<(String, argentor_agent::config::LlmProvider)> {
    match hint {
        "fast_cheap" => Some((
            "gpt-4o-mini".to_string(),
            argentor_agent::config::LlmProvider::OpenAi,
        )),
        "balanced" => None, // Keep current model
        "quality_max" => Some((
            "claude-opus-4-6-20250514".to_string(),
            argentor_agent::config::LlmProvider::Claude,
        )),
        _ => None,
    }
}

/// Estimate cost in USD based on token counts and model ID.
fn estimate_cost_usd(model_id: &str, tokens_in: u64, tokens_out: u64) -> f64 {
    // Approximate pricing per 1M tokens (USD)
    let (in_price, out_price) = match model_id {
        m if m.contains("gpt-4o-mini") => (0.15, 0.60),
        m if m.contains("opus") => (15.0, 75.0),
        m if m.contains("sonnet") => (3.0, 15.0),
        _ => (3.0, 15.0), // default to sonnet pricing
    };
    (tokens_in as f64 * in_price + tokens_out as f64 * out_price) / 1_000_000.0
}

/// POST /api/v1/agent/run-task — Execute a single agent task.
///
/// Full integrated pipeline:
///   1. Tenant rate limit check
///   2. Input guardrails (PII, prompt injection, toxicity)
///   3. Profile lookup + model routing
///   4. Persona injection
///   5. Conversation memory injection
///   6. Agent execution (with circuit breaker + cache)
///   7. Output guardrails
///   8. Quality scoring
///   9. Conversation memory recording
///  10. Usage tracking + analytics + audit
async fn run_task_handler(
    State(state): State<Arc<XcapitState>>,
    headers: HeaderMap,
    Json(req): Json<RunTaskRequest>,
) -> impl IntoResponse {
    let start = Instant::now();
    let mut compliance_flags: Vec<String> = Vec::new();

    // ── Step 0: Resolve tenant ──────────────────────────────────
    let tenant_id = req.tenant_id.clone().or_else(|| {
        headers
            .get("X-Tenant-ID")
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    });

    // ── Step 1: Tenant rate limit check ─────────────────────────
    if let Some(ref tid) = tenant_id {
        let check = state.tenant_limits.check_request(tid);
        if !check.allowed {
            let reason = check.reason.unwrap_or_else(|| "rate_limited".to_string());
            compliance_flags.push(format!("rate_limited:{reason}"));
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "error": format!("Rate limit exceeded: {reason}"),
                    "tenant_id": tid,
                    "retry_after_seconds": 60,
                })),
            )
                .into_response();
        }
    }

    // ── Step 2: Input guardrails ────────────────────────────────
    let input_check = state.guardrails.check_input(&req.context);
    if !input_check.passed {
        let blocking: Vec<String> = input_check
            .violations
            .iter()
            .filter(|v| v.severity == GuardrailSeverity::Block)
            .map(|v| format!("{}: {}", v.rule_name, v.message))
            .collect();

        if !blocking.is_empty() {
            compliance_flags.push("input_blocked".to_string());
            state.audit.log_action(
                Uuid::new_v4(),
                "guardrail_blocked",
                Some(req.agent_role.clone()),
                serde_json::json!({"violations": blocking}),
                AuditOutcome::Error,
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "Input blocked by guardrails",
                    "violations": blocking,
                })),
            )
                .into_response();
        }

        // Warnings are noted but don't block
        for v in &input_check.violations {
            compliance_flags.push(format!("input_warn:{}", v.rule_name));
        }
    }

    // Use sanitized input if guardrails cleaned it
    let safe_context = input_check
        .sanitized_text
        .unwrap_or_else(|| req.context.clone());

    // ── Step 3: Profile lookup + model routing ──────────────────
    let profile = match state.profiles.get(&req.agent_role) {
        Some(p) => p.clone(),
        None => {
            let available: Vec<&String> = state.profiles.keys().collect();
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("Unknown agent_role: '{}'. Available: {:?}", req.agent_role, available)
                })),
            )
                .into_response();
        }
    };

    let mut model_config = profile.model.clone();
    resolve_api_keys(&mut model_config);

    if let Some(max_tokens) = req.max_tokens {
        model_config.max_tokens = max_tokens;
    }
    if let Some(temp) = req.temperature {
        model_config.temperature = temp;
    }

    if let Some(ref hint) = req.routing_hint {
        if let Some((model_id, provider)) = resolve_routing_hint(hint) {
            model_config.model_id = model_id;
            model_config.provider = provider;
            resolve_api_keys(&mut model_config);
        }
    }

    // ── Step 4: Persona injection ───────────────────────────────
    let mut system_prompt = req
        .system_prompt
        .unwrap_or_else(|| profile.system_prompt.clone());

    if let Some(ref tid) = tenant_id {
        let personas = state.personas.read().await;
        if let Some(persona) = personas.get(&(tid.clone(), req.agent_role.clone())) {
            let persona_prefix = format!(
                "[Persona: {} | Tono: {} | Estilo: {}]\n{}\n\n",
                persona.name, persona.tone, persona.language_style, persona.custom_instructions
            );
            system_prompt = format!("{persona_prefix}{system_prompt}");
        }
    }

    // ── Step 5: Conversation memory injection ───────────────────
    let customer_id = headers
        .get("X-Customer-ID")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    if let Some(ref cid) = customer_id {
        let context_str = ConversationSummarizer::build_context(
            &state.conversation_memory,
            cid,
            500, // max tokens for history injection
        )
        .await;
        if !context_str.is_empty() {
            system_prompt = format!("{system_prompt}\n\n{context_str}");
        }
    }

    let model_id = model_config.model_id.clone();

    // ── Step 6: Agent execution ─────────────────────────────────
    let runner = AgentRunner::new(
        model_config,
        state.skills.clone(),
        state.permissions.clone(),
        state.audit.clone(),
    )
    .with_system_prompt(&system_prompt);

    let mut session = Session::new();
    let session_id = req.session_id.unwrap_or_else(|| session.id.to_string());

    state.audit.log_action(
        session.id,
        "xcapitsff_run_task",
        Some(req.agent_role.clone()),
        serde_json::json!({
            "role": req.agent_role,
            "context_len": safe_context.len(),
            "tenant_id": tenant_id,
            "customer_id": customer_id,
            "model": model_id,
        }),
        AuditOutcome::Success,
    );

    info!(role = %req.agent_role, session_id = %session_id, "XcapitSFF run-task started (full pipeline)");

    let result = runner.run(&mut session, &safe_context).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(mut response) => {
            // ── Step 7: Output guardrails ────────────────────────
            let output_check = state
                .guardrails
                .check_output(&response, Some(&safe_context));
            if !output_check.passed {
                for v in &output_check.violations {
                    compliance_flags.push(format!("output_{:?}:{}", v.severity, v.rule_name));
                }
                // Use sanitized output if available
                if let Some(sanitized) = output_check.sanitized_text {
                    response = sanitized;
                }
            }

            let tokens_input = (safe_context.len() / 4) as u64;
            let tokens_output = (response.len() / 4) as u64;

            // ── Step 8: Quality scoring ─────────────────────────
            let evaluator = ResponseEvaluator::with_defaults();
            let quality_score = evaluator.evaluate_heuristic(&safe_context, &response, &[]);
            let quality = quality_score.overall;

            if quality < 0.5 {
                compliance_flags.push(format!("low_quality:{quality:.2}"));
            }

            // ── Step 9: Conversation memory recording ───────────
            if let Some(ref cid) = customer_id {
                state
                    .conversation_memory
                    .record_turn(cid, &session_id, "user", &safe_context, HashMap::new())
                    .await;
                let mut meta = HashMap::new();
                meta.insert("agent_role".to_string(), req.agent_role.clone());
                meta.insert("model".to_string(), model_id.clone());
                meta.insert("quality".to_string(), format!("{quality:.2}"));
                state
                    .conversation_memory
                    .record_turn(cid, &session_id, "assistant", &response, meta)
                    .await;
            }

            // ── Step 10: Usage + analytics + audit ──────────────
            if let Some(ref tid) = tenant_id {
                let cost = estimate_cost_usd(&model_id, tokens_input, tokens_output);
                state
                    .usage_tracker
                    .record(
                        tid,
                        &req.agent_role,
                        &model_id,
                        tokens_input,
                        tokens_output,
                        cost,
                    )
                    .await;

                state
                    .tenant_limits
                    .record_usage(tid, tokens_input, tokens_output, cost);

                state
                    .analytics
                    .record_interaction(crate::analytics::InteractionEvent {
                        tenant_id: tid.clone(),
                        agent_role: req.agent_role.clone(),
                        channel: "api".to_string(),
                        customer_id: customer_id.clone(),
                        outcome: crate::analytics::InteractionOutcome::Resolved,
                        duration_ms,
                        tokens_used: tokens_input + tokens_output,
                        timestamp: Utc::now(),
                    })
                    .await;

                state
                    .analytics
                    .record_quality_score(crate::analytics::QualityEvent {
                        tenant_id: tid.clone(),
                        agent_role: req.agent_role.clone(),
                        overall_score: quality,
                        criteria_scores: HashMap::new(),
                        timestamp: Utc::now(),
                    })
                    .await;
            }

            state.audit.log_action(
                session.id,
                "xcapitsff_run_task_complete",
                Some(req.agent_role.clone()),
                serde_json::json!({
                    "duration_ms": duration_ms,
                    "tokens_input": tokens_input,
                    "tokens_output": tokens_output,
                    "quality_score": quality,
                    "compliance_flags": compliance_flags,
                }),
                AuditOutcome::Success,
            );

            // ── Step 11: Workflow triggering ────────────────────────
            // Auto-trigger workflows based on agent role and output
            if req.agent_role == "sales_qualifier" {
                // Check if the response contains HOT classification
                let response_upper = response.to_uppercase();
                if response_upper.contains("HOT") || response_upper.contains("\u{1f525}") {
                    let trigger_data = serde_json::json!({
                        "agent_role": "sales_qualifier",
                        "tenant_id": tenant_id,
                        "customer_id": customer_id,
                        "qualification_result": &response,
                        "session_id": &session_id,
                    });
                    if let Some(run_id) = state
                        .workflow_engine
                        .start("lead_qualification", trigger_data)
                        .await
                    {
                        info!(workflow = "lead_qualification", run_id = %run_id, "Auto-triggered lead qualification workflow");
                        compliance_flags
                            .push(format!("workflow_triggered:lead_qualification:{run_id}"));
                    }
                }
            } else if req.agent_role == "ticket_router" {
                // Auto-trigger support workflow for urgent tickets
                let response_lower = response.to_lowercase();
                if response_lower.contains("\"priority\":\"urgent\"")
                    || response_lower.contains("\"priority\": \"urgent\"")
                {
                    let trigger_data = serde_json::json!({
                        "agent_role": "ticket_router",
                        "tenant_id": tenant_id,
                        "routing_result": &response,
                        "session_id": &session_id,
                    });
                    if let Some(run_id) = state
                        .workflow_engine
                        .start("support_ticket", trigger_data)
                        .await
                    {
                        info!(workflow = "support_ticket", run_id = %run_id, "Auto-triggered support ticket workflow");
                        compliance_flags
                            .push(format!("workflow_triggered:support_ticket:{run_id}"));
                    }
                }
            }

            info!(role = %req.agent_role, duration_ms, quality, "XcapitSFF run-task completed (full pipeline)");

            (
                StatusCode::OK,
                Json(
                    serde_json::to_value(RunTaskResponse {
                        response,
                        session_id,
                        model_used: model_id,
                        tokens_input,
                        tokens_output,
                        tool_calls: vec![],
                        compliance_flags,
                        duration_ms,
                    })
                    .unwrap_or_default(),
                ),
            )
                .into_response()
        }
        Err(e) => {
            error!(role = %req.agent_role, error = %e, "XcapitSFF run-task failed");

            // Record failure in analytics
            if let Some(ref tid) = tenant_id {
                state
                    .analytics
                    .record_interaction(crate::analytics::InteractionEvent {
                        tenant_id: tid.clone(),
                        agent_role: req.agent_role.clone(),
                        channel: "api".to_string(),
                        customer_id: customer_id.clone(),
                        outcome: crate::analytics::InteractionOutcome::Escalated,
                        duration_ms,
                        tokens_used: 0,
                        timestamp: Utc::now(),
                    })
                    .await;
            }

            state.audit.log_action(
                session.id,
                "xcapitsff_run_task_error",
                Some(req.agent_role),
                serde_json::json!({"error": e.to_string(), "duration_ms": duration_ms}),
                AuditOutcome::Error,
            );

            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": e.to_string(),
                    "duration_ms": duration_ms,
                })),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/agent/batch — Execute multiple agent tasks in parallel.
async fn batch_handler(
    State(state): State<Arc<XcapitState>>,
    Json(req): Json<BatchRequest>,
) -> impl IntoResponse {
    let start = Instant::now();
    let max_concurrent = req.max_concurrent.unwrap_or(5);
    let total = req.tasks.len();

    info!(total, max_concurrent, "XcapitSFF batch started");

    // Use semaphore for concurrency limiting
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));
    let mut handles = Vec::with_capacity(total);

    for (index, task) in req.tasks.into_iter().enumerate() {
        let state = state.clone();
        let sem = semaphore.clone();

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await;

            // Input guardrails
            let input_check = state.guardrails.check_input(&task.context);
            if !input_check.passed {
                let blocking: Vec<String> = input_check
                    .violations
                    .iter()
                    .filter(|v| v.severity == GuardrailSeverity::Block)
                    .map(|v| v.message.clone())
                    .collect();
                if !blocking.is_empty() {
                    return BatchResultItem {
                        index,
                        success: false,
                        response: String::new(),
                        error: Some(format!("Guardrail blocked: {}", blocking.join("; "))),
                        tokens_input: 0,
                        tokens_output: 0,
                    };
                }
            }

            let safe_context = input_check
                .sanitized_text
                .unwrap_or_else(|| task.context.clone());

            let profile = match state.profiles.get(&task.agent_role) {
                Some(p) => p.clone(),
                None => {
                    return BatchResultItem {
                        index,
                        success: false,
                        response: String::new(),
                        error: Some(format!("Unknown role: {}", task.agent_role)),
                        tokens_input: 0,
                        tokens_output: 0,
                    };
                }
            };

            let mut model_config = profile.model.clone();
            resolve_api_keys(&mut model_config);
            if let Some(mt) = task.max_tokens {
                model_config.max_tokens = mt;
            }

            let system_prompt = task
                .system_prompt
                .unwrap_or_else(|| profile.system_prompt.clone());

            let runner = AgentRunner::new(
                model_config,
                state.skills.clone(),
                state.permissions.clone(),
                state.audit.clone(),
            )
            .with_system_prompt(&system_prompt);

            let mut session = Session::new();
            match runner.run(&mut session, &safe_context).await {
                Ok(mut response) => {
                    // Output guardrails
                    let output_check = state
                        .guardrails
                        .check_output(&response, Some(&safe_context));
                    if let Some(sanitized) = output_check.sanitized_text {
                        response = sanitized;
                    }

                    let ti = (safe_context.len() / 4) as u64;
                    let to = (response.len() / 4) as u64;
                    BatchResultItem {
                        index,
                        success: true,
                        response,
                        error: None,
                        tokens_input: ti,
                        tokens_output: to,
                    }
                }
                Err(e) => BatchResultItem {
                    index,
                    success: false,
                    response: String::new(),
                    error: Some(e.to_string()),
                    tokens_input: 0,
                    tokens_output: 0,
                },
            }
        });

        handles.push(handle);
    }

    // Collect results
    let mut results = Vec::with_capacity(total);
    for handle in handles {
        match handle.await {
            Ok(item) => results.push(item),
            Err(e) => results.push(BatchResultItem {
                index: results.len(),
                success: false,
                response: String::new(),
                error: Some(format!("Task panicked: {e}")),
                tokens_input: 0,
                tokens_output: 0,
            }),
        }
    }

    // Sort by original index
    results.sort_by_key(|r| r.index);

    let succeeded = results.iter().filter(|r| r.success).count();
    let failed = total - succeeded;
    let total_tokens: u64 = results
        .iter()
        .map(|r| r.tokens_input + r.tokens_output)
        .sum();
    let total_duration_ms = start.elapsed().as_millis() as u64;

    info!(
        total,
        succeeded, failed, total_duration_ms, "XcapitSFF batch completed"
    );

    (
        StatusCode::OK,
        Json(
            serde_json::to_value(BatchResponse {
                results,
                total,
                succeeded,
                failed,
                total_tokens,
                total_duration_ms,
            })
            .unwrap_or_default(),
        ),
    )
        .into_response()
}

/// POST /api/v1/proxy/webhook — Proxy webhooks to XcapitSFF with compliance.
async fn webhook_proxy_handler(
    State(state): State<Arc<XcapitState>>,
    headers: HeaderMap,
    Json(req): Json<WebhookProxyRequest>,
) -> impl IntoResponse {
    let audit_id = Uuid::new_v4().to_string();

    // Validate webhook source
    if !state.config.allowed_webhook_sources.contains(&req.source) {
        state.audit.log_action(
            Uuid::new_v4(),
            "webhook_proxy_rejected",
            Some(req.source.clone()),
            serde_json::json!({"event": req.event, "reason": "source_not_allowed"}),
            AuditOutcome::Error,
        );

        return (
            StatusCode::FORBIDDEN,
            Json(
                serde_json::to_value(WebhookProxyResponse {
                    forwarded: false,
                    upstream_status: None,
                    audit_id,
                    error: Some(format!("Source '{}' not in allowed list", req.source)),
                })
                .unwrap_or_default(),
            ),
        )
            .into_response();
    }

    // Validate HMAC if secret is configured
    if !state.config.webhook_hmac_secret.is_empty() {
        let provided_sig = headers
            .get("x-webhook-secret")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if provided_sig.is_empty() {
            state.audit.log_action(
                Uuid::new_v4(),
                "webhook_proxy_rejected",
                Some(req.source.clone()),
                serde_json::json!({"event": req.event, "reason": "missing_hmac"}),
                AuditOutcome::Error,
            );

            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "forwarded": false,
                    "audit_id": audit_id,
                    "error": "Missing X-Webhook-Secret header",
                })),
            )
                .into_response();
        }

        // Compute expected HMAC-SHA256
        use sha2::Digest;
        let body_bytes = serde_json::to_vec(&req).unwrap_or_default();
        let mut mac = sha2::Sha256::new();
        mac.update(state.config.webhook_hmac_secret.as_bytes());
        mac.update(&body_bytes);
        let expected = hex::encode(mac.finalize());

        if provided_sig != expected {
            state.audit.log_action(
                Uuid::new_v4(),
                "webhook_proxy_rejected",
                Some(req.source.clone()),
                serde_json::json!({"event": req.event, "reason": "invalid_hmac"}),
                AuditOutcome::Error,
            );

            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "forwarded": false,
                    "audit_id": audit_id,
                    "error": "Invalid HMAC signature",
                })),
            )
                .into_response();
        }
    }

    // Audit: webhook received
    state.audit.log_action(
        Uuid::new_v4(),
        "webhook_proxy_received",
        Some(req.source.clone()),
        serde_json::json!({"event": &req.event, "source": &req.source}),
        AuditOutcome::Success,
    );

    // Forward to XcapitSFF
    let forward_url = format!("{}/api/v1/webhooks/generic", state.config.url);
    let result = state.http_client.post(&forward_url).json(&req).send().await;

    match result {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let success = resp.status().is_success();

            state.audit.log_action(
                Uuid::new_v4(),
                "webhook_proxy_forwarded",
                Some(req.source.clone()),
                serde_json::json!({
                    "event": req.event,
                    "upstream_status": status,
                    "success": success,
                }),
                if success {
                    AuditOutcome::Success
                } else {
                    AuditOutcome::Error
                },
            );

            info!(
                source = %req.source,
                event = %req.event,
                upstream_status = status,
                "Webhook proxied to XcapitSFF"
            );

            (
                StatusCode::OK,
                Json(
                    serde_json::to_value(WebhookProxyResponse {
                        forwarded: success,
                        upstream_status: Some(status),
                        audit_id,
                        error: if success {
                            None
                        } else {
                            Some(format!("Upstream returned HTTP {status}"))
                        },
                    })
                    .unwrap_or_default(),
                ),
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to forward webhook to XcapitSFF");

            state.audit.log_action(
                Uuid::new_v4(),
                "webhook_proxy_error",
                Some(req.source),
                serde_json::json!({"error": e.to_string()}),
                AuditOutcome::Error,
            );

            (
                StatusCode::BAD_GATEWAY,
                Json(
                    serde_json::to_value(WebhookProxyResponse {
                        forwarded: false,
                        upstream_status: None,
                        audit_id,
                        error: Some(e.to_string()),
                    })
                    .unwrap_or_default(),
                ),
            )
                .into_response()
        }
    }
}

/// GET /api/v1/health — Extended health check with XcapitSFF status.
async fn extended_health_handler(State(state): State<Arc<XcapitState>>) -> impl IntoResponse {
    let xcapitsff_health = state.xcapitsff_health.read().await.clone();

    let overall = if xcapitsff_health.status == "ok" {
        "ok"
    } else {
        "degraded"
    };

    let response = ExtendedHealthResponse {
        status: overall.to_string(),
        version: "0.1.0".to_string(),
        checks: HealthChecks {
            llm_backends: "ok".to_string(),
            xcapitsff: xcapitsff_health.status.clone(),
            compliance: "ok".to_string(),
        },
        xcapitsff: xcapitsff_health,
    };

    (
        StatusCode::OK,
        Json(serde_json::to_value(response).unwrap_or_default()),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Feature 1: Streaming SSE endpoint
// ---------------------------------------------------------------------------

/// POST /api/v1/agent/run-task-stream — Execute a task with Server-Sent Events streaming.
///
/// Returns `text/event-stream` with token-by-token output.
/// Each token: `data: {"type": "token", "content": "..."}\n\n`
/// Final event: `data: {"type": "done", "session_id": "...", "tokens_input": N, "tokens_output": N}\n\n`
async fn run_task_stream_handler(
    State(state): State<Arc<XcapitState>>,
    headers: HeaderMap,
    Json(req): Json<RunTaskRequest>,
) -> impl IntoResponse {
    // Resolve tenant ID
    let tenant_id = req.tenant_id.clone().or_else(|| {
        headers
            .get("X-Tenant-ID")
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    });

    // Look up profile
    let profile = match state.profiles.get(&req.agent_role) {
        Some(p) => p.clone(),
        None => {
            let available: Vec<String> = state.profiles.keys().cloned().collect();
            let role = req.agent_role.clone();
            return Sse::new(futures_util::stream::once(async move {
                Ok::<_, Infallible>(
                    SseEvent::default()
                        .data(serde_json::json!({
                            "type": "error",
                            "message": format!("Unknown agent_role: '{}'. Available: {:?}", role, available)
                        }).to_string()),
                )
            }))
            .into_response();
        }
    };

    // Build model config
    let mut model_config = profile.model.clone();
    resolve_api_keys(&mut model_config);

    if let Some(max_tokens) = req.max_tokens {
        model_config.max_tokens = max_tokens;
    }
    if let Some(temp) = req.temperature {
        model_config.temperature = temp;
    }

    // Apply routing hint
    if let Some(ref hint) = req.routing_hint {
        if let Some((model_id, provider)) = resolve_routing_hint(hint) {
            model_config.model_id = model_id;
            model_config.provider = provider;
            resolve_api_keys(&mut model_config);
        }
    }

    // Build system prompt with optional persona
    let mut system_prompt = req
        .system_prompt
        .unwrap_or_else(|| profile.system_prompt.clone());

    if let Some(ref tid) = tenant_id {
        let personas = state.personas.read().await;
        if let Some(persona) = personas.get(&(tid.clone(), req.agent_role.clone())) {
            let persona_prefix = format!(
                "[Persona: {} | Tono: {} | Estilo: {}]\n{}\n\n",
                persona.name, persona.tone, persona.language_style, persona.custom_instructions
            );
            system_prompt = format!("{persona_prefix}{system_prompt}");
        }
    }

    let model_id = model_config.model_id.clone();
    let agent_role = req.agent_role.clone();
    let context = req.context.clone();

    let runner = AgentRunner::new(
        model_config,
        state.skills.clone(),
        state.permissions.clone(),
        state.audit.clone(),
    )
    .with_system_prompt(&system_prompt);

    let mut session = Session::new();
    let session_id = req.session_id.unwrap_or_else(|| session.id.to_string());

    info!(role = %agent_role, session_id = %session_id, "XcapitSFF run-task-stream started");

    // Create channel for streaming events
    let (event_tx, event_rx) = mpsc::unbounded_channel::<StreamEvent>();

    let state_clone = state.clone();
    let model_id_clone = model_id.clone();
    let agent_role_clone = agent_role.clone();
    let tenant_id_clone = tenant_id.clone();
    let context_len = context.len();

    // Spawn the agent runner in a background task
    tokio::spawn(async move {
        let result = runner
            .run_streaming(&mut session, &context, event_tx.clone())
            .await;

        match result {
            Ok(response) => {
                let tokens_input = (context_len / 4) as u64;
                let tokens_output = (response.len() / 4) as u64;

                // Track usage per tenant
                if let Some(ref tid) = tenant_id_clone {
                    let cost = estimate_cost_usd(&model_id_clone, tokens_input, tokens_output);
                    state_clone
                        .usage_tracker
                        .record(
                            tid,
                            &agent_role_clone,
                            &model_id_clone,
                            tokens_input,
                            tokens_output,
                            cost,
                        )
                        .await;
                }

                info!(role = %agent_role_clone, "XcapitSFF run-task-stream completed");
            }
            Err(e) => {
                error!(role = %agent_role_clone, error = %e, "XcapitSFF run-task-stream failed");
                let _ = event_tx.send(StreamEvent::Error {
                    message: e.to_string(),
                });
            }
        }
    });

    // Convert the channel receiver into an SSE stream
    let rx_stream = UnboundedReceiverStream::new(event_rx);
    let session_id_for_stream = session_id.clone();
    let context_len_for_stream = context_len;

    let sse_stream = futures_util::stream::unfold(
        (
            rx_stream,
            session_id_for_stream,
            context_len_for_stream,
            0usize,
        ),
        |(mut rx, sid, ctx_len, mut output_chars)| async move {
            use futures_util::StreamExt;
            match rx.next().await {
                Some(StreamEvent::TextDelta { text }) => {
                    output_chars += text.len();
                    let data = serde_json::json!({
                        "type": "token",
                        "content": text,
                    });
                    let event = Ok::<_, Infallible>(SseEvent::default().data(data.to_string()));
                    Some((event, (rx, sid, ctx_len, output_chars)))
                }
                Some(StreamEvent::Done) => {
                    let tokens_input = (ctx_len / 4) as u64;
                    let tokens_output = (output_chars / 4) as u64;
                    let data = serde_json::json!({
                        "type": "done",
                        "session_id": sid,
                        "tokens_input": tokens_input,
                        "tokens_output": tokens_output,
                    });
                    let event = Ok::<_, Infallible>(SseEvent::default().data(data.to_string()));
                    Some((event, (rx, sid, ctx_len, output_chars)))
                }
                Some(StreamEvent::Error { message }) => {
                    let data = serde_json::json!({
                        "type": "error",
                        "message": message,
                    });
                    let event = Ok::<_, Infallible>(SseEvent::default().data(data.to_string()));
                    Some((event, (rx, sid, ctx_len, output_chars)))
                }
                Some(StreamEvent::ToolCallStart { id, name }) => {
                    let data = serde_json::json!({
                        "type": "tool_call_start",
                        "id": id,
                        "name": name,
                    });
                    let event = Ok::<_, Infallible>(SseEvent::default().data(data.to_string()));
                    Some((event, (rx, sid, ctx_len, output_chars)))
                }
                Some(StreamEvent::ToolCallDelta {
                    id,
                    arguments_delta,
                }) => {
                    let data = serde_json::json!({
                        "type": "tool_call_delta",
                        "id": id,
                        "arguments_delta": arguments_delta,
                    });
                    let event = Ok::<_, Infallible>(SseEvent::default().data(data.to_string()));
                    Some((event, (rx, sid, ctx_len, output_chars)))
                }
                Some(StreamEvent::ToolCallEnd { id }) => {
                    let data = serde_json::json!({
                        "type": "tool_call_end",
                        "id": id,
                    });
                    let event = Ok::<_, Infallible>(SseEvent::default().data(data.to_string()));
                    Some((event, (rx, sid, ctx_len, output_chars)))
                }
                None => None, // Channel closed, end stream
            }
        },
    );

    Sse::new(sse_stream).into_response()
}

// ---------------------------------------------------------------------------
// Feature 2: Tenant usage endpoint
// ---------------------------------------------------------------------------

/// Parse a period string like "hours:24", "days:7", or "all" into a UsagePeriod.
fn parse_usage_period(input: &str) -> UsagePeriod {
    if input == "all" || input.is_empty() {
        return UsagePeriod::All;
    }
    if let Some(hours) = input.strip_prefix("hours:") {
        if let Ok(h) = hours.parse::<u64>() {
            return UsagePeriod::Hours(h);
        }
    }
    if let Some(days) = input.strip_prefix("days:") {
        if let Ok(d) = days.parse::<u64>() {
            return UsagePeriod::Days(d);
        }
    }
    UsagePeriod::All
}

/// GET /api/v1/usage/tenant/{tenant_id} — Query tenant usage data.
async fn tenant_usage_handler(
    State(state): State<Arc<XcapitState>>,
    Path(tenant_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<UsageQueryParams>,
) -> impl IntoResponse {
    let period = parse_usage_period(&params.period.unwrap_or_default());
    let usage = state.usage_tracker.get_usage(&tenant_id, &period).await;

    info!(tenant_id = %tenant_id, "Tenant usage queried");

    (
        StatusCode::OK,
        Json(serde_json::to_value(usage).unwrap_or_default()),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Feature 4: Response quality scoring
// ---------------------------------------------------------------------------

/// POST /api/v1/agent/evaluate — Evaluate response quality using heuristic scoring.
///
/// Uses the `ResponseEvaluator` from `argentor_agent::evaluator` to score the
/// response across multiple criteria and provide improvement suggestions.
async fn evaluate_handler(
    State(_state): State<Arc<XcapitState>>,
    Json(req): Json<EvaluateRequest>,
) -> impl IntoResponse {
    let evaluator = ResponseEvaluator::with_defaults();

    // Use the heuristic evaluator for base scores
    let quality = evaluator.evaluate_heuristic(&req.context, &req.response, &[]);

    // Map requested criteria to scores, with custom heuristics for non-standard criteria
    let mut by_criteria: HashMap<String, f32> = HashMap::new();

    let criteria = if req.criteria.is_empty() {
        vec![
            "relevance".to_string(),
            "helpfulness".to_string(),
            "accuracy".to_string(),
            "tone".to_string(),
        ]
    } else {
        req.criteria.clone()
    };

    for criterion in &criteria {
        let score = match criterion.as_str() {
            "relevance" => quality.relevance,
            "accuracy" | "consistency" => quality.consistency,
            "completeness" => quality.completeness,
            "clarity" => quality.clarity,
            "helpfulness" => {
                // Helpfulness: combination of relevance and completeness
                (quality.relevance * 0.5 + quality.completeness * 0.5).min(1.0)
            }
            "tone" => {
                // Tone heuristic: presence of polite markers, absence of aggressive language
                score_tone(&req.response)
            }
            _ => 0.5, // Unknown criteria get neutral score
        };
        by_criteria.insert(criterion.clone(), score);
    }

    // Compute overall as average of requested criteria
    let overall_score = if by_criteria.is_empty() {
        quality.overall
    } else {
        let sum: f32 = by_criteria.values().sum();
        sum / by_criteria.len() as f32
    };

    // Generate suggestions based on low-scoring criteria
    let mut suggestions = Vec::new();
    for (criterion, &score) in &by_criteria {
        if score < 0.5 {
            suggestions.push(format!(
                "Improve {criterion}: current score is {score:.2}, consider enhancing this aspect."
            ));
        }
    }
    if suggestions.is_empty() && overall_score < 0.7 {
        suggestions.push(
            "Response quality is below threshold. Consider providing more detail and structure."
                .to_string(),
        );
    }

    let response = EvaluateResponse {
        overall_score,
        by_criteria,
        suggestions,
    };

    info!(overall_score, "Response evaluation completed");

    (
        StatusCode::OK,
        Json(serde_json::to_value(response).unwrap_or_default()),
    )
        .into_response()
}

/// Heuristic tone scorer for response evaluation.
///
/// Checks for polite phrases, exclamation marks (enthusiasm), and absence of
/// aggressive or negative language markers.
fn score_tone(response: &str) -> f32 {
    let lower = response.to_lowercase();
    let mut score: f32 = 0.5;

    // Polite / friendly markers
    let polite_markers = [
        "por favor",
        "gracias",
        "please",
        "thank you",
        "happy to help",
        "con gusto",
        "espero que",
        "hope this helps",
        "let me know",
        "no dudes en",
        "feel free",
    ];
    for marker in &polite_markers {
        if lower.contains(marker) {
            score += 0.1;
        }
    }

    // Negative markers
    let negative_markers = ["error", "wrong", "bad", "terrible", "stupid", "idiota"];
    for marker in &negative_markers {
        if lower.contains(marker) {
            score -= 0.1;
        }
    }

    score.clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Feature 5: Agent personas per tenant
// ---------------------------------------------------------------------------

/// POST /api/v1/agent/personas — Create or update a persona for a tenant + role.
async fn create_persona_handler(
    State(state): State<Arc<XcapitState>>,
    Json(req): Json<CreatePersonaRequest>,
) -> impl IntoResponse {
    let key = (req.tenant_id.clone(), req.agent_role.clone());
    let persona_name = req.persona.name.clone();

    let mut personas = state.personas.write().await;
    personas.insert(key, req.persona);

    info!(
        tenant_id = %req.tenant_id,
        agent_role = %req.agent_role,
        persona_name = %persona_name,
        "Persona created/updated"
    );

    let response = CreatePersonaResponse {
        created: true,
        tenant_id: req.tenant_id,
        agent_role: req.agent_role,
        persona_name,
    };

    (
        StatusCode::CREATED,
        Json(serde_json::to_value(response).unwrap_or_default()),
    )
        .into_response()
}

/// GET /api/v1/agent/personas/{tenant_id} — List all personas for a tenant.
async fn list_personas_handler(
    State(state): State<Arc<XcapitState>>,
    Path(tenant_id): Path<String>,
) -> impl IntoResponse {
    let personas = state.personas.read().await;
    let tenant_personas: HashMap<String, PersonaConfig> = personas
        .iter()
        .filter(|((tid, _), _)| tid == &tenant_id)
        .map(|((_, role), persona)| (role.clone(), persona.clone()))
        .collect();

    info!(tenant_id = %tenant_id, count = tenant_personas.len(), "Listed tenant personas");

    let response = ListPersonasResponse {
        tenant_id,
        personas: tenant_personas,
    };

    (
        StatusCode::OK,
        Json(serde_json::to_value(response).unwrap_or_default()),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// New endpoint handlers (Phase 40)
// ---------------------------------------------------------------------------

/// GET /api/v1/agent/profiles — List all available agent profiles.
async fn list_profiles_handler(State(state): State<Arc<XcapitState>>) -> impl IntoResponse {
    let profiles: Vec<serde_json::Value> = state
        .profiles
        .iter()
        .map(|(role, profile)| {
            serde_json::json!({
                "role": role,
                "model": profile.model.model_id,
                "temperature": profile.model.temperature,
                "max_tokens": profile.model.max_tokens,
                "system_prompt_preview": &profile.system_prompt[..profile.system_prompt.len().min(100)],
                "has_fallback": !profile.model.fallback_models.is_empty(),
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "profiles": profiles,
            "total": profiles.len(),
        })),
    )
        .into_response()
}

/// POST /api/v1/tenants/{tenant_id}/register — Register a tenant with a plan.
async fn register_tenant_handler(
    State(state): State<Arc<XcapitState>>,
    Path(tenant_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let plan_name = body["plan"].as_str().unwrap_or("free");
    let plan = match plan_name {
        "free" => TenantPlan::Free,
        "pro" => TenantPlan::Pro,
        "enterprise" => TenantPlan::Enterprise,
        _ => TenantPlan::Free,
    };

    state.tenant_limits.register_tenant(&tenant_id, plan);

    state.audit.log_action(
        Uuid::new_v4(),
        "tenant_registered",
        None,
        serde_json::json!({"tenant_id": &tenant_id, "plan": plan_name}),
        AuditOutcome::Success,
    );

    info!(tenant_id = %tenant_id, plan = %plan_name, "Tenant registered");

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "tenant_id": tenant_id,
            "plan": plan_name,
            "status": "active",
        })),
    )
        .into_response()
}

/// GET /api/v1/tenants/{tenant_id}/status — Get tenant usage status and limits.
async fn tenant_status_handler(
    State(state): State<Arc<XcapitState>>,
    Path(tenant_id): Path<String>,
) -> impl IntoResponse {
    let limit_status = state.tenant_limits.get_status(&tenant_id);
    let usage = state
        .usage_tracker
        .get_usage(&tenant_id, &UsagePeriod::All)
        .await;

    match limit_status {
        Some(status) => {
            (StatusCode::OK, Json(serde_json::json!({
                "tenant_id": tenant_id,
                "plan": status.plan,
                "limits": {
                    "daily_requests": format!("{}/{}", status.daily_requests, status.daily_limit),
                    "monthly_tokens": format!("{}/{}", status.monthly_tokens, status.monthly_limit),
                    "monthly_cost": format!("${:.4}/${:.2}", status.monthly_cost_usd, status.monthly_budget_usd),
                    "utilization_percent": status.utilization_percent,
                    "is_throttled": status.is_throttled,
                },
                "usage": {
                    "total_requests": usage.request_count,
                    "total_tokens": usage.total_tokens_in + usage.total_tokens_out,
                    "total_cost_usd": usage.total_cost_usd,
                    "by_agent": usage.by_agent,
                    "by_model": usage.by_model,
                },
            }))).into_response()
        }
        None => {
            (StatusCode::NOT_FOUND, Json(serde_json::json!({
                "error": format!("Tenant '{}' not registered", tenant_id),
                "hint": "POST /api/v1/tenants/{tenant_id}/register to register",
            }))).into_response()
        }
    }
}

/// GET /api/v1/workflows/runs — List all workflow runs.
async fn list_workflow_runs_handler(State(state): State<Arc<XcapitState>>) -> impl IntoResponse {
    let lead_runs = state.workflow_engine.list_runs("lead_qualification").await;
    let support_runs = state.workflow_engine.list_runs("support_ticket").await;

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "lead_qualification": lead_runs.iter().map(|r| serde_json::json!({
                "run_id": r.run_id,
                "status": format!("{:?}", r.status),
                "created_at": r.created_at.to_rfc3339(),
                "current_step": r.current_step_index,
                "total_steps": r.step_results.len(),
            })).collect::<Vec<_>>(),
            "support_ticket": support_runs.iter().map(|r| serde_json::json!({
                "run_id": r.run_id,
                "status": format!("{:?}", r.status),
                "created_at": r.created_at.to_rfc3339(),
                "current_step": r.current_step_index,
                "total_steps": r.step_results.len(),
            })).collect::<Vec<_>>(),
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_default_profiles_has_four() {
        let profiles = default_xcapit_profiles();
        assert_eq!(profiles.len(), 4);
        assert!(profiles.contains_key("sales_qualifier"));
        assert!(profiles.contains_key("outreach_composer"));
        assert!(profiles.contains_key("support_responder"));
        assert!(profiles.contains_key("ticket_router"));
    }

    #[test]
    fn test_profile_temperatures() {
        let profiles = default_xcapit_profiles();
        assert!((profiles["sales_qualifier"].model.temperature - 0.3).abs() < 0.01);
        assert!((profiles["outreach_composer"].model.temperature - 0.7).abs() < 0.01);
        assert!((profiles["support_responder"].model.temperature - 0.4).abs() < 0.01);
        assert!((profiles["ticket_router"].model.temperature - 0.2).abs() < 0.01);
    }

    #[test]
    fn test_profile_max_tokens() {
        let profiles = default_xcapit_profiles();
        assert_eq!(profiles["sales_qualifier"].model.max_tokens, 2048);
        assert_eq!(profiles["outreach_composer"].model.max_tokens, 4096);
        assert_eq!(profiles["support_responder"].model.max_tokens, 4096);
        assert_eq!(profiles["ticket_router"].model.max_tokens, 1024);
    }

    #[test]
    fn test_profile_has_fallback() {
        let profiles = default_xcapit_profiles();
        for profile in profiles.values() {
            assert_eq!(
                profile.model.fallback_models.len(),
                1,
                "Profile {} should have 1 fallback",
                profile.role
            );
            assert_eq!(profile.model.fallback_models[0].model_id, "gpt-4o-mini");
        }
    }

    #[test]
    fn test_default_config() {
        let config = XcapitConfig::default();
        assert!(config.url.contains("localhost:8000") || config.url.contains("xcapitsff"));
        assert_eq!(config.health_check_interval, 30);
        assert_eq!(config.allowed_webhook_sources.len(), 4);
    }

    #[test]
    fn test_config_allowed_sources() {
        let config = XcapitConfig::default();
        assert!(config
            .allowed_webhook_sources
            .contains(&"hubspot".to_string()));
        assert!(config
            .allowed_webhook_sources
            .contains(&"salesforce".to_string()));
        assert!(config
            .allowed_webhook_sources
            .contains(&"stripe".to_string()));
        assert!(config
            .allowed_webhook_sources
            .contains(&"intercom".to_string()));
    }

    #[test]
    fn test_run_task_response_serialization() {
        let resp = RunTaskResponse {
            response: "Score: 82".to_string(),
            session_id: "ses_abc".to_string(),
            model_used: "claude-sonnet-4-6".to_string(),
            tokens_input: 100,
            tokens_output: 50,
            tool_calls: vec![],
            compliance_flags: vec![],
            duration_ms: 1200,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"model_used\":\"claude-sonnet-4-6\""));
        assert!(json.contains("\"duration_ms\":1200"));
    }

    #[test]
    fn test_batch_response_serialization() {
        let resp = BatchResponse {
            results: vec![BatchResultItem {
                index: 0,
                success: true,
                response: "ok".to_string(),
                error: None,
                tokens_input: 100,
                tokens_output: 50,
            }],
            total: 1,
            succeeded: 1,
            failed: 0,
            total_tokens: 150,
            total_duration_ms: 500,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"succeeded\":1"));
    }

    #[test]
    fn test_webhook_proxy_response_serialization() {
        let resp = WebhookProxyResponse {
            forwarded: true,
            upstream_status: Some(200),
            audit_id: "audit-123".to_string(),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"forwarded\":true"));
        assert!(!json.contains("\"error\"")); // skip_serializing_if
    }

    #[test]
    fn test_health_response_serialization() {
        let resp = ExtendedHealthResponse {
            status: "ok".to_string(),
            version: "0.1.0".to_string(),
            checks: HealthChecks {
                llm_backends: "ok".to_string(),
                xcapitsff: "ok".to_string(),
                compliance: "ok".to_string(),
            },
            xcapitsff: XcapitSffHealth {
                url: "http://localhost:8000".to_string(),
                status: "ok".to_string(),
                last_check: Some(Utc::now()),
                last_error: None,
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"version\":\"0.1.0\""));
    }

    #[test]
    fn test_resolve_api_keys_from_env() {
        std::env::set_var("ANTHROPIC_API_KEY", "test-key-123");
        let mut config = default_xcapit_profiles()["sales_qualifier"].model.clone();
        resolve_api_keys(&mut config);
        assert_eq!(config.api_key, "test-key-123");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    fn test_system_prompts_non_empty() {
        let profiles = default_xcapit_profiles();
        for (name, profile) in &profiles {
            assert!(
                !profile.system_prompt.is_empty(),
                "Profile {name} has empty system prompt"
            );
            assert!(
                profile.system_prompt.len() > 50,
                "Profile {name} prompt is too short"
            );
        }
    }

    // -----------------------------------------------------------------------
    // New feature tests
    // -----------------------------------------------------------------------

    // -- Routing hint tests --

    #[test]
    fn test_resolve_routing_hint_fast_cheap() {
        let result = resolve_routing_hint("fast_cheap");
        assert!(result.is_some());
        let (model, provider) = result.unwrap();
        assert_eq!(model, "gpt-4o-mini");
        assert!(matches!(
            provider,
            argentor_agent::config::LlmProvider::OpenAi
        ));
    }

    #[test]
    fn test_resolve_routing_hint_quality_max() {
        let result = resolve_routing_hint("quality_max");
        assert!(result.is_some());
        let (model, provider) = result.unwrap();
        assert_eq!(model, "claude-opus-4-6-20250514");
        assert!(matches!(
            provider,
            argentor_agent::config::LlmProvider::Claude
        ));
    }

    #[test]
    fn test_resolve_routing_hint_balanced_returns_none() {
        let result = resolve_routing_hint("balanced");
        assert!(
            result.is_none(),
            "balanced should return None (keep current model)"
        );
    }

    #[test]
    fn test_resolve_routing_hint_unknown_returns_none() {
        assert!(resolve_routing_hint("unknown_hint").is_none());
        assert!(resolve_routing_hint("").is_none());
    }

    // -- Cost estimation tests --

    #[test]
    fn test_estimate_cost_usd_sonnet() {
        let cost = estimate_cost_usd("claude-sonnet-4-6-20250514", 1000, 500);
        // 1000 * 3.0 / 1M + 500 * 15.0 / 1M = 0.003 + 0.0075 = 0.0105
        assert!(
            (cost - 0.0105).abs() < 0.0001,
            "Expected ~0.0105, got {cost}"
        );
    }

    #[test]
    fn test_estimate_cost_usd_gpt4o_mini() {
        let cost = estimate_cost_usd("gpt-4o-mini", 10000, 5000);
        // 10000 * 0.15 / 1M + 5000 * 0.60 / 1M = 0.0015 + 0.003 = 0.0045
        assert!(
            (cost - 0.0045).abs() < 0.0001,
            "Expected ~0.0045, got {cost}"
        );
    }

    #[test]
    fn test_estimate_cost_usd_opus() {
        let cost = estimate_cost_usd("claude-opus-4-6-20250514", 1000, 500);
        // 1000 * 15.0 / 1M + 500 * 75.0 / 1M = 0.015 + 0.0375 = 0.0525
        assert!(
            (cost - 0.0525).abs() < 0.0001,
            "Expected ~0.0525, got {cost}"
        );
    }

    // -- TenantUsageTracker tests --

    #[tokio::test]
    async fn test_tenant_usage_tracker_record_and_query() {
        let tracker = TenantUsageTracker::new();

        tracker
            .record(
                "t_001",
                "sales_qualifier",
                "claude-sonnet-4-6",
                100,
                50,
                0.01,
            )
            .await;
        tracker
            .record("t_001", "outreach_composer", "gpt-4o-mini", 200, 100, 0.005)
            .await;
        tracker
            .record("t_002", "ticket_router", "claude-sonnet-4-6", 50, 25, 0.005)
            .await;

        let usage = tracker.get_usage("t_001", &UsagePeriod::All).await;
        assert_eq!(usage.tenant_id, "t_001");
        assert_eq!(usage.request_count, 2);
        assert_eq!(usage.total_tokens_in, 300);
        assert_eq!(usage.total_tokens_out, 150);
        assert!((usage.total_cost_usd - 0.015).abs() < 0.0001);
        assert_eq!(usage.by_agent.len(), 2);
        assert!(usage.by_agent.contains_key("sales_qualifier"));
        assert!(usage.by_agent.contains_key("outreach_composer"));
    }

    #[tokio::test]
    async fn test_tenant_usage_tracker_empty_tenant() {
        let tracker = TenantUsageTracker::new();
        let usage = tracker.get_usage("nonexistent", &UsagePeriod::All).await;
        assert_eq!(usage.request_count, 0);
        assert_eq!(usage.total_tokens_in, 0);
        assert_eq!(usage.total_cost_usd, 0.0);
    }

    #[tokio::test]
    async fn test_tenant_usage_tracker_by_model_breakdown() {
        let tracker = TenantUsageTracker::new();
        tracker
            .record(
                "t_100",
                "sales_qualifier",
                "claude-sonnet-4-6",
                100,
                50,
                0.01,
            )
            .await;
        tracker
            .record("t_100", "sales_qualifier", "gpt-4o-mini", 100, 50, 0.002)
            .await;

        let usage = tracker.get_usage("t_100", &UsagePeriod::All).await;
        assert_eq!(usage.by_model.len(), 2);
        assert!(usage.by_model.contains_key("claude-sonnet-4-6"));
        assert!(usage.by_model.contains_key("gpt-4o-mini"));
        assert_eq!(usage.by_model["claude-sonnet-4-6"].count, 1);
        assert_eq!(usage.by_model["gpt-4o-mini"].count, 1);
    }

    // -- PersonaConfig tests --

    #[test]
    fn test_persona_config_serialization() {
        let persona = PersonaConfig {
            name: "Sofía".to_string(),
            tone: "friendly".to_string(),
            language_style: "es_latam".to_string(),
            signature: "— Sofía, equipo Xcapit".to_string(),
            custom_instructions: "Siempre saludar con nombre del cliente".to_string(),
        };
        let json = serde_json::to_string(&persona).unwrap();
        assert!(json.contains("\"name\":\"Sofía\""));
        assert!(json.contains("\"tone\":\"friendly\""));
        assert!(json.contains("\"language_style\":\"es_latam\""));

        // Round-trip
        let deserialized: PersonaConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "Sofía");
        assert_eq!(deserialized.signature, "— Sofía, equipo Xcapit");
    }

    #[test]
    fn test_persona_config_default_fields() {
        // Deserialize with missing optional fields
        let json = r#"{"name": "Carlos", "tone": "professional", "language_style": "es_iberia"}"#;
        let persona: PersonaConfig = serde_json::from_str(json).unwrap();
        assert_eq!(persona.name, "Carlos");
        assert!(persona.signature.is_empty());
        assert!(persona.custom_instructions.is_empty());
    }

    // -- Evaluate response tests --

    #[test]
    fn test_evaluate_response_serialization() {
        let mut by_criteria = HashMap::new();
        by_criteria.insert("relevance".to_string(), 0.8_f32);
        by_criteria.insert("tone".to_string(), 0.7_f32);

        let resp = EvaluateResponse {
            overall_score: 0.75,
            by_criteria,
            suggestions: vec!["Consider adding more detail.".to_string()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"overall_score\":0.75"));
    }

    // -- Tone scorer tests --

    #[test]
    fn test_score_tone_polite() {
        let score = score_tone(
            "Gracias por tu consulta. Por favor, no dudes en preguntar si necesitás más ayuda.",
        );
        assert!(score > 0.5, "Polite text should score > 0.5, got {score}");
    }

    #[test]
    fn test_score_tone_neutral() {
        let score = score_tone("The result is 42.");
        assert!(
            (score - 0.5).abs() < f32::EPSILON,
            "Neutral text should score 0.5, got {score}"
        );
    }

    #[test]
    fn test_score_tone_negative() {
        let score = score_tone("This is a terrible and stupid response with bad results.");
        assert!(score < 0.5, "Negative text should score < 0.5, got {score}");
    }

    // -- Parse usage period tests --

    #[test]
    fn test_parse_usage_period_hours() {
        let period = parse_usage_period("hours:24");
        assert!(matches!(period, UsagePeriod::Hours(24)));
    }

    #[test]
    fn test_parse_usage_period_days() {
        let period = parse_usage_period("days:7");
        assert!(matches!(period, UsagePeriod::Days(7)));
    }

    #[test]
    fn test_parse_usage_period_all() {
        let period = parse_usage_period("all");
        assert!(matches!(period, UsagePeriod::All));
    }

    #[test]
    fn test_parse_usage_period_empty_defaults_to_all() {
        let period = parse_usage_period("");
        assert!(matches!(period, UsagePeriod::All));
    }

    #[test]
    fn test_parse_usage_period_invalid_defaults_to_all() {
        let period = parse_usage_period("invalid:foo");
        assert!(matches!(period, UsagePeriod::All));
    }

    // -- CreatePersonaResponse tests --

    #[test]
    fn test_create_persona_response_serialization() {
        let resp = CreatePersonaResponse {
            created: true,
            tenant_id: "t_123".to_string(),
            agent_role: "support_responder".to_string(),
            persona_name: "Sofía".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"created\":true"));
        assert!(json.contains("\"tenant_id\":\"t_123\""));
    }

    // -- ListPersonasResponse tests --

    #[test]
    fn test_list_personas_response_serialization() {
        let mut personas = HashMap::new();
        personas.insert(
            "support_responder".to_string(),
            PersonaConfig {
                name: "Sofía".to_string(),
                tone: "friendly".to_string(),
                language_style: "es_latam".to_string(),
                signature: String::new(),
                custom_instructions: String::new(),
            },
        );
        let resp = ListPersonasResponse {
            tenant_id: "t_456".to_string(),
            personas,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"tenant_id\":\"t_456\""));
        assert!(json.contains("\"support_responder\""));
    }

    // -- UsageBreakdown tests --

    #[test]
    fn test_usage_breakdown_serialization() {
        let breakdown = UsageBreakdown {
            tenant_id: "t_999".to_string(),
            total_tokens_in: 5000,
            total_tokens_out: 2500,
            total_cost_usd: 0.05,
            request_count: 10,
            by_agent: HashMap::new(),
            by_model: HashMap::new(),
        };
        let json = serde_json::to_string(&breakdown).unwrap();
        assert!(json.contains("\"total_tokens_in\":5000"));
        assert!(json.contains("\"request_count\":10"));
    }
}
