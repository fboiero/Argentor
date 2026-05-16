//! REST API endpoints for managing agents, sessions, skills, and connections.
//!
//! Provides a comprehensive JSON API mounted under `/api/v1/` that complements
//! the existing WebSocket and health endpoints.

use crate::connection::ConnectionManager;
use crate::router::{InboundMessage, MessageRouter};
use axum::{
    extract::{Json, Path, State},
    http::{HeaderValue, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use tokio::task;
use tracing::{info, warn};
use uuid::Uuid;

use argentor_security::audit::{AuditEntry, AuditOutcome};
use argentor_security::{query_audit_log, AuditFilter};
use argentor_session::SessionStore;
use argentor_skills::SkillRegistry;

const AUDIT_NEXT_CURSOR_HEADER: &str = "x-next-cursor";

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Extended application state that holds everything the REST API needs.
///
/// This wraps the components already present in `AppState` (router, connections)
/// and adds references to the session store, skill registry, and a start
/// timestamp so the REST handlers can serve rich responses without modifying
/// `server::AppState`.
pub struct RestApiState {
    /// The message router that handles agent interactions.
    pub router: Arc<MessageRouter>,
    /// Tracks active WebSocket connections.
    pub connections: Arc<ConnectionManager>,
    /// Session persistence backend.
    pub sessions: Arc<dyn SessionStore>,
    /// Central skill registry.
    pub skills: Arc<SkillRegistry>,
    /// Timestamp when the server was started (for uptime calculation).
    pub started_at: DateTime<Utc>,
    /// Optional path to the append-only audit JSONL file.
    pub audit_log_path: Option<PathBuf>,
    /// Cached audit stats keyed by audit file metadata.
    pub audit_stats_cache: Arc<RwLock<Option<AuditStatsCacheEntry>>>,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Unified error type for all REST API handlers.
#[derive(Debug)]
pub enum ApiError {
    /// The requested resource was not found.
    NotFound(String),
    /// The request body or parameters were invalid.
    BadRequest(String),
    /// An internal error occurred while processing the request.
    Internal(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(msg) => write!(f, "Not found: {msg}"),
            Self::BadRequest(msg) => write!(f, "Bad request: {msg}"),
            Self::Internal(msg) => write!(f, "Internal error: {msg}"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match &self {
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            Self::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
        };

        let body = serde_json::json!({ "error": message });
        (status, Json(body)).into_response()
    }
}

impl From<argentor_core::ArgentorError> for ApiError {
    fn from(err: argentor_core::ArgentorError) -> Self {
        Self::Internal(err.to_string())
    }
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

// --- Sessions ---

/// Summary of a session returned in list responses.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Unique identifier of the session.
    pub session_id: Uuid,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// Total number of messages in the session.
    pub message_count: usize,
}

/// Detailed session information including messages and metadata.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionDetail {
    /// Unique identifier of the session.
    pub session_id: Uuid,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session was last updated.
    pub updated_at: DateTime<Utc>,
    /// Total number of messages in the session.
    pub message_count: usize,
    /// All messages in the session.
    pub messages: Vec<MessageSummary>,
    /// Arbitrary session metadata.
    pub metadata: serde_json::Value,
}

/// Lightweight representation of a message within a session.
#[derive(Debug, Serialize, Deserialize)]
pub struct MessageSummary {
    /// Unique identifier of the message.
    pub id: Uuid,
    /// Role of the message author (user, assistant, system, tool).
    pub role: String,
    /// Textual content of the message.
    pub content: String,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Response returned after deleting a session.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteSessionResponse {
    /// Whether the session was successfully deleted.
    pub deleted: bool,
    /// The session that was deleted.
    pub session_id: Uuid,
}

// --- Skills ---

/// Summary of a skill returned in list responses.
#[derive(Debug, Serialize, Deserialize)]
pub struct SkillSummary {
    /// Name of the skill.
    pub name: String,
    /// Human-readable description of the skill.
    pub description: String,
}

/// Detailed information about a single skill.
#[derive(Debug, Serialize, Deserialize)]
pub struct SkillDetail {
    /// Name of the skill.
    pub name: String,
    /// Human-readable description of the skill.
    pub description: String,
    /// JSON Schema describing the skill's parameters.
    pub parameters_schema: serde_json::Value,
    /// Security capabilities required to execute this skill.
    pub required_capabilities: Vec<String>,
}

// --- Agent ---

/// Request body for the synchronous chat endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatRequest {
    /// The user message to send to the agent.
    pub message: String,
    /// Optional session ID to continue an existing conversation.
    pub session_id: Option<Uuid>,
}

/// Response from the synchronous chat endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatResponse {
    /// The agent's response text.
    pub response: String,
    /// The session ID for this conversation.
    pub session_id: Uuid,
}

/// Agent readiness status.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentStatus {
    /// Whether the agent is ready to accept requests.
    pub ready: bool,
    /// Number of skills currently loaded in the registry.
    pub skills_loaded: usize,
}

// --- Connections ---

/// Summary of active WebSocket connections.
#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectionsInfo {
    /// Number of active WebSocket connections.
    pub count: usize,
    /// Unique session IDs associated with active connections.
    pub session_ids: Vec<Uuid>,
}

// --- Metrics ---

/// Basic server metrics.
#[derive(Debug, Serialize, Deserialize)]
pub struct MetricsResponse {
    /// Number of active WebSocket connections.
    pub active_connections: usize,
    /// Number of active sessions (via WebSocket).
    pub active_sessions: usize,
    /// Server uptime in seconds.
    pub uptime_seconds: i64,
    /// Number of skills registered in the registry.
    pub skills_registered: usize,
    /// When the server was started.
    pub started_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the REST API sub-router.
///
/// All routes are nested under `/api/v1/` and return JSON responses.
pub fn api_router(state: Arc<RestApiState>) -> Router {
    Router::new()
        // Sessions
        .route("/api/v1/sessions", get(list_sessions))
        .route("/api/v1/sessions/{id}", get(get_session))
        .route("/api/v1/sessions/{id}", delete(delete_session))
        // Skills
        .route("/api/v1/skills", get(list_skills))
        .route("/api/v1/skills/{name}", get(get_skill))
        // Agent
        .route("/api/v1/agent/chat", post(agent_chat))
        .route("/api/v1/agent/status", get(agent_status))
        // Connections
        .route("/api/v1/connections", get(list_connections))
        // Metrics
        .route("/api/v1/metrics", get(get_metrics))
        // Audit
        .route("/api/v1/audit/logs", get(audit_logs))
        .route("/api/v1/audit/violations", get(audit_violations))
        .route("/api/v1/audit/stats", get(audit_stats))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers — Sessions
// ---------------------------------------------------------------------------

async fn list_sessions(
    State(state): State<Arc<RestApiState>>,
) -> Result<Json<Vec<SessionSummary>>, ApiError> {
    let ids = state.sessions.list().await?;
    let mut summaries = Vec::with_capacity(ids.len());

    for id in ids {
        if let Some(session) = state.sessions.get(id).await? {
            summaries.push(SessionSummary {
                session_id: session.id,
                created_at: session.created_at,
                message_count: session.message_count(),
            });
        }
    }

    Ok(Json(summaries))
}

async fn get_session(
    State(state): State<Arc<RestApiState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<SessionDetail>, ApiError> {
    let session = state
        .sessions
        .get(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Session {id} not found")))?;

    let messages = session
        .messages
        .iter()
        .map(|m| MessageSummary {
            id: m.id,
            role: format!("{:?}", m.role).to_lowercase(),
            content: m.content.clone(),
            timestamp: m.timestamp,
        })
        .collect();

    let detail = SessionDetail {
        session_id: session.id,
        created_at: session.created_at,
        updated_at: session.updated_at,
        message_count: session.message_count(),
        messages,
        metadata: serde_json::to_value(&session.metadata).unwrap_or_else(|_| serde_json::json!({})),
    };

    Ok(Json(detail))
}

async fn delete_session(
    State(state): State<Arc<RestApiState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeleteSessionResponse>, ApiError> {
    // Verify the session exists first
    let exists = state.sessions.get(id).await?.is_some();
    if !exists {
        return Err(ApiError::NotFound(format!("Session {id} not found")));
    }

    state.sessions.delete(id).await?;
    info!(session_id = %id, "Session deleted via REST API");

    Ok(Json(DeleteSessionResponse {
        deleted: true,
        session_id: id,
    }))
}

// ---------------------------------------------------------------------------
// Handlers — Skills
// ---------------------------------------------------------------------------

async fn list_skills(
    State(state): State<Arc<RestApiState>>,
) -> Result<Json<Vec<SkillSummary>>, ApiError> {
    let descriptors = state.skills.list_descriptors();

    let summaries: Vec<SkillSummary> = descriptors
        .into_iter()
        .map(|d| SkillSummary {
            name: d.name.clone(),
            description: d.description.clone(),
        })
        .collect();

    Ok(Json(summaries))
}

async fn get_skill(
    State(state): State<Arc<RestApiState>>,
    Path(name): Path<String>,
) -> Result<Json<SkillDetail>, ApiError> {
    let skill = state
        .skills
        .get(&name)
        .ok_or_else(|| ApiError::NotFound(format!("Skill '{name}' not found")))?;

    let descriptor = skill.descriptor();
    let detail = SkillDetail {
        name: descriptor.name.clone(),
        description: descriptor.description.clone(),
        parameters_schema: descriptor.parameters_schema.clone(),
        required_capabilities: descriptor
            .required_capabilities
            .iter()
            .map(|c| format!("{c:?}"))
            .collect(),
    };

    Ok(Json(detail))
}

// ---------------------------------------------------------------------------
// Handlers — Agent
// ---------------------------------------------------------------------------

async fn agent_chat(
    State(state): State<Arc<RestApiState>>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, ApiError> {
    if req.message.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Message must not be empty".to_string(),
        ));
    }

    let session_id = req.session_id.unwrap_or_else(Uuid::new_v4);

    info!(
        session_id = %session_id,
        "REST API chat request"
    );

    // Build a fake connection_id — we will not actually push through WebSocket,
    // instead we use MessageRouter::handle_message logic inline.
    let inbound = InboundMessage {
        session_id: Some(session_id),
        content: req.message.clone(),
    };

    // Get or create the session
    let mut session = match state.sessions.get(session_id).await? {
        Some(s) => s,
        None => {
            let mut s = argentor_session::Session::new();
            s.id = session_id;
            s
        }
    };

    // Add user message
    let user_msg = argentor_core::Message::user(&inbound.content, session_id);
    session.add_message(user_msg);

    // We cannot call agent.run() directly since we do not hold a reference to
    // AgentRunner — it lives inside MessageRouter. Instead, we create a
    // temporary connection, route the message through the existing MessageRouter
    // pipeline, and collect the result.
    //
    // However, for simplicity and directness, we duplicate the core logic:
    // save session, send response. The MessageRouter already has sanitization
    // and routing. We call handle_message and collect the response through a
    // channel.
    use tokio::sync::mpsc;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let conn_id = Uuid::new_v4();

    let conn = crate::connection::Connection {
        id: conn_id,
        session_id,
        tx,
    };
    state.connections.add(conn).await;

    // Route the message through the existing pipeline
    let router = state.router.clone();
    let route_result = router.handle_message(inbound, conn_id).await;

    // Collect the response from the channel
    state.connections.remove(conn_id).await;

    // Close the sender side by dropping the connection, then drain the channel
    let mut response_text = String::new();
    while let Ok(msg) = rx.try_recv() {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&msg) {
            if let Some(content) = parsed.get("content").and_then(|c| c.as_str()) {
                response_text = content.to_string();
            }
        }
    }

    if let Err(e) = route_result {
        warn!(error = %e, "Agent chat failed");
        return Err(ApiError::Internal(format!("Agent error: {e}")));
    }

    Ok(Json(ChatResponse {
        response: response_text,
        session_id,
    }))
}

async fn agent_status(
    State(state): State<Arc<RestApiState>>,
) -> Result<Json<AgentStatus>, ApiError> {
    let skills_loaded = state.skills.skill_count();

    Ok(Json(AgentStatus {
        ready: true,
        skills_loaded,
    }))
}

// ---------------------------------------------------------------------------
// Handlers — Connections
// ---------------------------------------------------------------------------

async fn list_connections(
    State(state): State<Arc<RestApiState>>,
) -> Result<Json<ConnectionsInfo>, ApiError> {
    let count = state.connections.connection_count().await;
    let session_ids = state.connections.session_ids().await;

    Ok(Json(ConnectionsInfo { count, session_ids }))
}

// ---------------------------------------------------------------------------
// Handlers — Metrics
// ---------------------------------------------------------------------------

async fn get_metrics(
    State(state): State<Arc<RestApiState>>,
) -> Result<Json<MetricsResponse>, ApiError> {
    let now = Utc::now();
    let uptime = now.signed_duration_since(state.started_at);
    let active_connections = state.connections.connection_count().await;
    let active_sessions = state.connections.session_ids().await.len();
    let skills_registered = state.skills.skill_count();

    Ok(Json(MetricsResponse {
        active_connections,
        active_sessions,
        uptime_seconds: uptime.num_seconds(),
        skills_registered,
        started_at: state.started_at,
    }))
}

// ---------------------------------------------------------------------------
// Audit API — response types
// ---------------------------------------------------------------------------

/// A single audit log entry as returned by the REST API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    /// ISO 8601 timestamp of the action.
    pub timestamp: String,
    /// Session the action belongs to.
    pub session_id: String,
    /// Human-readable action label (e.g. "tool_call", "login").
    pub action: String,
    /// Skill involved, if any.
    pub skill_name: Option<String>,
    /// Structured details (free-form JSON).
    pub details: serde_json::Value,
    /// Outcome: "success", "denied", or "error".
    pub outcome: String,
}

/// A guardrail/policy violation entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViolationEntry {
    /// ISO 8601 timestamp.
    pub timestamp: String,
    /// Session that triggered the violation.
    pub session_id: String,
    /// Name of the rule that fired (e.g. "pii_detection", "injection_guard").
    pub rule: String,
    /// Severity level: "warn" or "block".
    pub severity: String,
    /// Human-readable description.
    pub message: String,
}

/// Summary statistics for the audit dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditStats {
    /// Total audit events recorded today.
    pub events_today: u64,
    /// Number of guardrail violations today.
    pub violations_today: u64,
    /// Percentage of requests blocked today (0–100).
    pub block_rate_percent: f64,
    /// Total audit events since server start.
    pub total_events: u64,
}

/// Cached audit stats for an unchanged audit log file.
#[derive(Debug, Clone)]
pub struct AuditStatsCacheEntry {
    /// File size in bytes when these stats were computed.
    pub file_len: u64,
    /// File modification timestamp when these stats were computed.
    pub modified: SystemTime,
    /// Computed stats.
    pub stats: AuditStats,
}

#[derive(Debug)]
struct AuditPage {
    entries: Vec<AuditEntry>,
    next_cursor: Option<u64>,
}

/// Export audit health and aggregate counters in Prometheus text format.
///
/// This intentionally keeps labels out of the audit metrics to avoid cardinality
/// growth from sessions, users, rules, or tenants.
pub async fn audit_prometheus_export(state: &RestApiState) -> String {
    let mut body = String::new();
    body.push_str("# HELP argentor_audit_configured Whether an audit log path is configured.\n");
    body.push_str("# TYPE argentor_audit_configured gauge\n");

    // "Configured" means the operator wired a path, regardless of whether the
    // file already exists on disk. The audit log file is created lazily on the
    // first event; we want the metric family to be stable from process start.
    let Some(path) = state.audit_log_path.clone() else {
        body.push_str("argentor_audit_configured 0\n");
        return body;
    };

    body.push_str("argentor_audit_configured 1\n");

    // The audit log file is created lazily on the first event. Emit the full
    // gauge family with zero values when the file is missing so operator
    // dashboards have stable series from process start onward.
    let log_bytes = match tokio::fs::metadata(&path).await {
        Ok(metadata) => metadata.len(),
        Err(_) => 0,
    };
    body.push_str("# HELP argentor_audit_log_bytes Audit log file size in bytes.\n");
    body.push_str("# TYPE argentor_audit_log_bytes gauge\n");
    let _ = writeln!(body, "argentor_audit_log_bytes {log_bytes}");

    let stats = match read_audit_stats(path, state.audit_stats_cache.clone()).await {
        Ok(stats) => stats,
        Err(err) => {
            // File missing or unreadable. Report the failure as a counter but
            // still emit zeroed gauges so the family stays continuous.
            warn!(error = %err, "Failed to read audit stats for Prometheus export");
            body.push_str(
                "# HELP argentor_audit_export_errors_total Audit metrics export errors.\n",
            );
            body.push_str("# TYPE argentor_audit_export_errors_total counter\n");
            body.push_str("argentor_audit_export_errors_total 1\n");
            AuditStats {
                events_today: 0,
                violations_today: 0,
                block_rate_percent: 0.0,
                total_events: 0,
            }
        }
    };

    body.push_str("# HELP argentor_audit_events_today Audit events recorded today.\n");
    body.push_str("# TYPE argentor_audit_events_today gauge\n");
    let _ = writeln!(body, "argentor_audit_events_today {}", stats.events_today);

    body.push_str("# HELP argentor_audit_violations_today Audit violations recorded today.\n");
    body.push_str("# TYPE argentor_audit_violations_today gauge\n");
    let _ = writeln!(
        body,
        "argentor_audit_violations_today {}",
        stats.violations_today
    );

    body.push_str("# HELP argentor_audit_block_rate_percent Percentage of today's audit events that were denied.\n");
    body.push_str("# TYPE argentor_audit_block_rate_percent gauge\n");
    let _ = writeln!(
        body,
        "argentor_audit_block_rate_percent {:.6}",
        stats.block_rate_percent
    );

    body.push_str(
        "# HELP argentor_audit_events_total Total audit events in the current audit log.\n",
    );
    body.push_str("# TYPE argentor_audit_events_total gauge\n");
    let _ = writeln!(body, "argentor_audit_events_total {}", stats.total_events);

    body
}

// ---------------------------------------------------------------------------
// Audit API — handlers
// ---------------------------------------------------------------------------

/// `GET /api/v1/audit/logs?limit=N&cursor=BYTE_OFFSET`
///
/// Returns recent audit log entries. When more older entries are available, the
/// response includes an `x-next-cursor` header for the next page.
async fn audit_logs(
    State(state): State<Arc<RestApiState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<axum::response::Response, ApiError> {
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100)
        .min(1000);
    let cursor = parse_audit_cursor(&params)?;

    let Some(path) = audit_log_path(&state) else {
        return Ok(build_audit_page_response(Vec::<AuditLogEntry>::new(), None));
    };

    let page = read_audit_entries_page(path, limit, cursor).await?;
    let entries = page.entries.into_iter().map(AuditLogEntry::from).collect();

    Ok(build_audit_page_response(entries, page.next_cursor))
}

/// `GET /api/v1/audit/violations?limit=N&cursor=BYTE_OFFSET`
///
/// Returns recent guardrail violations. When more older entries are available,
/// the response includes an `x-next-cursor` header for the next page.
async fn audit_violations(
    State(state): State<Arc<RestApiState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<axum::response::Response, ApiError> {
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50)
        .min(500);
    let cursor = parse_audit_cursor(&params)?;

    let Some(path) = audit_log_path(&state) else {
        return Ok(build_audit_page_response(
            Vec::<ViolationEntry>::new(),
            None,
        ));
    };

    let page = read_audit_violations_page(path, limit, cursor).await?;
    let violations = page
        .entries
        .into_iter()
        .filter_map(ViolationEntry::from_audit_entry)
        .collect();

    Ok(build_audit_page_response(violations, page.next_cursor))
}

/// `GET /api/v1/audit/stats`
///
/// Returns aggregated audit statistics for the dashboard summary bar.
async fn audit_stats(State(state): State<Arc<RestApiState>>) -> Result<Json<AuditStats>, ApiError> {
    let Some(path) = audit_log_path(&state) else {
        return Ok(Json(AuditStats {
            events_today: 0,
            violations_today: 0,
            block_rate_percent: 0.0,
            total_events: 0,
        }));
    };

    let stats = read_audit_stats(path, state.audit_stats_cache.clone()).await?;
    Ok(Json(stats))
}

fn audit_log_path(state: &RestApiState) -> Option<PathBuf> {
    let path = state.audit_log_path.as_ref()?;
    if path.exists() {
        Some(path.clone())
    } else {
        None
    }
}

fn parse_audit_cursor(
    params: &std::collections::HashMap<String, String>,
) -> Result<Option<u64>, ApiError> {
    params
        .get("cursor")
        .map(|cursor| {
            cursor
                .parse::<u64>()
                .map_err(|_| ApiError::BadRequest("cursor must be an unsigned byte offset".into()))
        })
        .transpose()
}

fn build_audit_page_response<T: Serialize>(
    entries: Vec<T>,
    next_cursor: Option<u64>,
) -> axum::response::Response {
    let mut response = Json(entries).into_response();
    if let Some(cursor) = next_cursor {
        if let Ok(value) = HeaderValue::from_str(&cursor.to_string()) {
            response
                .headers_mut()
                .insert(AUDIT_NEXT_CURSOR_HEADER, value);
        }
    }
    response
}

async fn read_audit_entries_page(
    path: PathBuf,
    limit: usize,
    before: Option<u64>,
) -> Result<AuditPage, ApiError> {
    task::spawn_blocking(move || read_audit_entries_page_blocking(&path, limit, before))
        .await
        .map_err(|e| ApiError::Internal(format!("Audit log read task failed: {e}")))?
}

async fn read_audit_violations_page(
    path: PathBuf,
    limit: usize,
    before: Option<u64>,
) -> Result<AuditPage, ApiError> {
    task::spawn_blocking(move || read_audit_violations_page_blocking(&path, limit, before))
        .await
        .map_err(|e| ApiError::Internal(format!("Audit violation read task failed: {e}")))?
        .map_err(|e| ApiError::Internal(format!("Failed to read audit log: {e}")))
}

async fn read_audit_stats(
    path: PathBuf,
    cache: Arc<RwLock<Option<AuditStatsCacheEntry>>>,
) -> Result<AuditStats, ApiError> {
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to stat audit log: {e}")))?;
    let file_len = metadata.len();
    let modified = metadata
        .modified()
        .map_err(|e| ApiError::Internal(format!("Failed to stat audit log mtime: {e}")))?;

    if let Some(entry) = cache
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .filter(|entry| entry.file_len == file_len && entry.modified == modified)
    {
        return Ok(entry.stats.clone());
    }

    let modified_ns = system_time_unix_nanos(modified).unwrap_or(0);

    let stats =
        task::spawn_blocking(move || read_audit_stats_blocking(&path, file_len, modified_ns))
            .await
            .map_err(|e| ApiError::Internal(format!("Audit stats read task failed: {e}")))?
            .map_err(|e| ApiError::Internal(format!("Failed to read audit stats: {e}")))?;

    *cache
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(AuditStatsCacheEntry {
        file_len,
        modified,
        stats: stats.clone(),
    });

    Ok(stats)
}

fn read_audit_entries_page_blocking(
    path: &FsPath,
    limit: usize,
    before: Option<u64>,
) -> Result<AuditPage, ApiError> {
    if limit > 0 {
        return read_recent_audit_entries_page_blocking(path, limit, before)
            .map_err(|e| ApiError::Internal(format!("Failed to read audit log: {e}")));
    }

    let filter = AuditFilter {
        limit: 0,
        ..AuditFilter::all()
    };
    let mut entries = query_audit_log(path, &filter)
        .map_err(|e| ApiError::Internal(format!("Failed to read audit log: {e}")))?
        .entries;

    entries.sort_by_key(|entry| entry.timestamp);
    entries.reverse();

    if limit > 0 && entries.len() > limit {
        entries.truncate(limit);
    }

    Ok(AuditPage {
        entries,
        next_cursor: None,
    })
}

fn read_audit_stats_blocking(
    path: &FsPath,
    file_len: u64,
    modified_ns: u128,
) -> Result<AuditStats, String> {
    let index_path = audit_stats_index_path(path);
    match read_audit_stats_index_if_fresh(&index_path, file_len, modified_ns) {
        Ok(Some(stats)) => return Ok(stats),
        Ok(None) => {}
        Err(err) => warn!(error = %err, "Ignoring stale or invalid audit stats index"),
    }

    let stats = compute_audit_stats_blocking(path)?;
    if let Err(err) = write_audit_stats_index(&index_path, file_len, modified_ns, &stats) {
        warn!(error = %err, "Failed to write audit stats index");
    }

    Ok(stats)
}

fn compute_audit_stats_blocking(path: &FsPath) -> Result<AuditStats, String> {
    let today = Utc::now().date_naive();
    let total_events = count_audit_lines_blocking(path)?;
    let (events_today, violations_today, blocked_today) =
        read_today_audit_stats_reverse_blocking(path, today)?;

    let block_rate_percent = if events_today > 0 {
        (blocked_today as f64 / events_today as f64) * 100.0
    } else {
        0.0
    };

    Ok(AuditStats {
        events_today,
        violations_today,
        block_rate_percent,
        total_events,
    })
}

fn audit_stats_index_path(path: &FsPath) -> PathBuf {
    let mut index = path.as_os_str().to_owned();
    index.push(".stats.idx");
    PathBuf::from(index)
}

fn read_audit_stats_index_if_fresh(
    index_path: &FsPath,
    file_len: u64,
    modified_ns: u128,
) -> Result<Option<AuditStats>, String> {
    let Ok(file) = std::fs::File::open(index_path) else {
        return Ok(None);
    };
    let mut reader = BufReader::new(file);

    let mut len_header = String::new();
    reader
        .read_line(&mut len_header)
        .map_err(|e| format!("Failed to read audit stats index length: {e}"))?;
    let indexed_len = len_header
        .trim()
        .strip_prefix("audit_len=")
        .and_then(|value| value.parse::<u64>().ok());
    if indexed_len != Some(file_len) {
        return Ok(None);
    }

    let mut modified_header = String::new();
    reader
        .read_line(&mut modified_header)
        .map_err(|e| format!("Failed to read audit stats index modified time: {e}"))?;
    let indexed_modified = modified_header
        .trim()
        .strip_prefix("audit_modified_ns=")
        .and_then(|value| value.parse::<u128>().ok());
    if indexed_modified != Some(modified_ns) {
        return Ok(None);
    }

    let mut stats_json = String::new();
    reader
        .read_to_string(&mut stats_json)
        .map_err(|e| format!("Failed to read audit stats index body: {e}"))?;
    let stats = serde_json::from_str(stats_json.trim())
        .map_err(|e| format!("Failed to parse audit stats index body: {e}"))?;
    Ok(Some(stats))
}

fn write_audit_stats_index(
    index_path: &FsPath,
    file_len: u64,
    modified_ns: u128,
    stats: &AuditStats,
) -> Result<(), String> {
    let mut body = String::new();
    let _ = writeln!(body, "audit_len={file_len}");
    let _ = writeln!(body, "audit_modified_ns={modified_ns}");
    let stats_json = serde_json::to_string(stats)
        .map_err(|e| format!("Failed to serialize audit stats index: {e}"))?;
    body.push_str(&stats_json);
    body.push('\n');

    if let Some(parent) = index_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create audit stats index directory: {e}"))?;
    }
    std::fs::write(index_path, body).map_err(|e| format!("Failed to write audit stats index: {e}"))
}

fn count_audit_lines_blocking(path: &FsPath) -> Result<u64, String> {
    const BLOCK_SIZE: usize = 64 * 1024;

    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Failed to open audit log: {e}"))?;
    let mut buf = vec![0u8; BLOCK_SIZE];
    let mut lines = 0u64;
    let mut saw_non_empty_tail = false;

    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| format!("Failed to read audit log: {e}"))?;
        if read == 0 {
            break;
        }
        for byte in &buf[..read] {
            if *byte == b'\n' {
                lines += 1;
                saw_non_empty_tail = false;
            } else if !byte.is_ascii_whitespace() {
                saw_non_empty_tail = true;
            }
        }
    }

    if saw_non_empty_tail {
        lines += 1;
    }

    Ok(lines)
}

fn read_today_audit_stats_reverse_blocking(
    path: &FsPath,
    today: chrono::NaiveDate,
) -> Result<(u64, u64, u64), String> {
    const BLOCK_SIZE: u64 = 8192;

    let mut events_today = 0u64;
    let mut violations_today = 0u64;
    let mut blocked_today = 0u64;

    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Failed to open audit log: {e}"))?;
    let mut pos = file
        .metadata()
        .map_err(|e| format!("Failed to stat audit log: {e}"))?
        .len();
    if pos == 0 {
        return Ok((0, 0, 0));
    }

    let mut carry = Vec::new();
    loop {
        let read_len = BLOCK_SIZE.min(pos) as usize;
        pos -= read_len as u64;

        file.seek(SeekFrom::Start(pos))
            .map_err(|e| format!("Failed to seek audit log: {e}"))?;
        let mut chunk = vec![0u8; read_len];
        file.read_exact(&mut chunk)
            .map_err(|e| format!("Failed to read audit log: {e}"))?;

        chunk.extend_from_slice(&carry);
        let parse_start = if pos > 0 {
            if let Some(newline_idx) = chunk.iter().position(|byte| *byte == b'\n') {
                carry = chunk[..newline_idx].to_vec();
                newline_idx + 1
            } else {
                carry = chunk;
                continue;
            }
        } else {
            0
        };

        let content = String::from_utf8_lossy(&chunk[parse_start..]);
        let lines: Vec<&str> = content
            .split_inclusive('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();

        for line in lines.into_iter().rev() {
            let entry: AuditEntry = serde_json::from_str(line)
                .map_err(|e| format!("Failed to parse audit entry: {e}"))?;
            if entry.timestamp.date_naive() != today {
                return Ok((events_today, violations_today, blocked_today));
            }
            events_today += 1;
            if is_violation_entry(&entry) {
                violations_today += 1;
            }
            if matches!(entry.outcome, AuditOutcome::Denied) {
                blocked_today += 1;
            }
        }

        if pos == 0 {
            return Ok((events_today, violations_today, blocked_today));
        }
    }
}

fn read_audit_violations_page_blocking(
    path: &FsPath,
    limit: usize,
    before: Option<u64>,
) -> Result<AuditPage, String> {
    match read_audit_violations_page_from_index_blocking(path, limit, before) {
        Ok(page) => return Ok(page),
        Err(err) => warn!(error = %err, "Falling back to reverse scan for audit violations"),
    }

    read_recent_audit_entries_where_page_blocking(path, limit, before, is_violation_entry)
}

fn read_audit_violations_page_from_index_blocking(
    path: &FsPath,
    limit: usize,
    before: Option<u64>,
) -> Result<AuditPage, String> {
    if limit == 0 {
        return Ok(AuditPage {
            entries: vec![],
            next_cursor: None,
        });
    }

    let metadata = std::fs::metadata(path).map_err(|e| format!("Failed to stat audit log: {e}"))?;
    let file_len = metadata.len();
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(system_time_unix_nanos)
        .unwrap_or(0);
    if file_len == 0 {
        return Ok(AuditPage {
            entries: vec![],
            next_cursor: None,
        });
    }

    let index_path = audit_violation_index_path(path);
    let offsets = load_or_rebuild_violation_index(path, &index_path, file_len, modified_ns)?;
    if offsets.is_empty() {
        return Ok(AuditPage {
            entries: vec![],
            next_cursor: None,
        });
    }

    let before = before.unwrap_or(file_len).min(file_len);
    let upper = offsets.partition_point(|offset| *offset < before);
    if upper == 0 {
        return Ok(AuditPage {
            entries: vec![],
            next_cursor: None,
        });
    }

    let selected_offsets: Vec<u64> = offsets[..upper].iter().rev().take(limit).copied().collect();
    let has_more = selected_offsets
        .last()
        .is_some_and(|oldest| offsets[..upper].iter().any(|offset| offset < oldest));
    let mut entries = Vec::with_capacity(selected_offsets.len());
    for offset in &selected_offsets {
        if let Some(entry) = read_audit_entry_at_offset(path, *offset)? {
            if is_violation_entry(&entry) {
                entries.push(entry);
            }
        }
    }

    entries.sort_by_key(|entry| entry.timestamp);
    entries.reverse();

    Ok(AuditPage {
        entries,
        next_cursor: has_more.then(|| selected_offsets.last().copied()).flatten(),
    })
}

fn audit_violation_index_path(path: &FsPath) -> PathBuf {
    let mut index = path.as_os_str().to_owned();
    index.push(".violations.idx");
    PathBuf::from(index)
}

fn load_or_rebuild_violation_index(
    path: &FsPath,
    index_path: &FsPath,
    file_len: u64,
    modified_ns: u128,
) -> Result<Vec<u64>, String> {
    if let Some(offsets) = read_violation_index_if_fresh(index_path, file_len, modified_ns)? {
        return Ok(offsets);
    }

    let offsets = rebuild_violation_index(path, index_path, file_len, modified_ns)?;
    Ok(offsets)
}

fn read_violation_index_if_fresh(
    index_path: &FsPath,
    file_len: u64,
    modified_ns: u128,
) -> Result<Option<Vec<u64>>, String> {
    let Ok(file) = std::fs::File::open(index_path) else {
        return Ok(None);
    };
    let mut reader = BufReader::new(file);
    let mut header = String::new();
    reader
        .read_line(&mut header)
        .map_err(|e| format!("Failed to read violation index header: {e}"))?;
    let indexed_len = header
        .trim()
        .strip_prefix("audit_len=")
        .and_then(|value| value.parse::<u64>().ok());
    if indexed_len != Some(file_len) {
        return Ok(None);
    }

    let mut modified_header = String::new();
    reader
        .read_line(&mut modified_header)
        .map_err(|e| format!("Failed to read violation index modified header: {e}"))?;
    let indexed_modified = modified_header
        .trim()
        .strip_prefix("audit_modified_ns=")
        .and_then(|value| value.parse::<u128>().ok());
    if indexed_modified != Some(modified_ns) {
        return Ok(None);
    }

    let mut offsets = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|e| format!("Failed to read violation index: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let offset = line
            .trim()
            .parse::<u64>()
            .map_err(|e| format!("Invalid violation index offset: {e}"))?;
        offsets.push(offset);
    }
    Ok(Some(offsets))
}

fn rebuild_violation_index(
    path: &FsPath,
    index_path: &FsPath,
    file_len: u64,
    modified_ns: u128,
) -> Result<Vec<u64>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Failed to open audit log: {e}"))?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut offset = 0u64;
    let mut offsets = Vec::new();

    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| format!("Failed to read audit log: {e}"))?;
        if read == 0 {
            break;
        }

        let trimmed = line.trim();
        if !trimmed.is_empty() {
            let entry: AuditEntry = serde_json::from_str(trimmed)
                .map_err(|e| format!("Failed to parse audit entry: {e}"))?;
            if is_violation_entry(&entry) {
                offsets.push(offset);
            }
        }
        offset += read as u64;
    }

    write_violation_index(index_path, file_len, modified_ns, &offsets)?;
    Ok(offsets)
}

fn write_violation_index(
    index_path: &FsPath,
    file_len: u64,
    modified_ns: u128,
    offsets: &[u64],
) -> Result<(), String> {
    let mut body = String::new();
    let _ = writeln!(body, "audit_len={file_len}");
    let _ = writeln!(body, "audit_modified_ns={modified_ns}");
    for offset in offsets {
        let _ = writeln!(body, "{offset}");
    }

    if let Some(parent) = index_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create violation index directory: {e}"))?;
    }
    std::fs::write(index_path, body).map_err(|e| format!("Failed to write violation index: {e}"))
}

fn read_audit_entry_at_offset(path: &FsPath, offset: u64) -> Result<Option<AuditEntry>, String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Failed to open audit log: {e}"))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("Failed to seek audit log: {e}"))?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .map_err(|e| format!("Failed to read audit log: {e}"))?;
    if read == 0 || line.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(line.trim())
        .map(Some)
        .map_err(|e| format!("Failed to parse audit entry: {e}"))
}

fn system_time_unix_nanos(time: SystemTime) -> Option<u128> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}

fn read_recent_audit_entries_page_blocking(
    path: &FsPath,
    limit: usize,
    before: Option<u64>,
) -> Result<AuditPage, String> {
    read_recent_audit_entries_where_page_blocking(path, limit, before, |_| true)
}

fn read_recent_audit_entries_where_page_blocking(
    path: &FsPath,
    limit: usize,
    before: Option<u64>,
    mut matches_entry: impl FnMut(&AuditEntry) -> bool,
) -> Result<AuditPage, String> {
    const BLOCK_SIZE: u64 = 8192;

    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Failed to open audit log: {e}"))?;
    let file_len = file
        .metadata()
        .map_err(|e| format!("Failed to stat audit log: {e}"))?
        .len();
    let mut pos = before.unwrap_or(file_len).min(file_len);
    if limit == 0 || pos == 0 {
        return Ok(AuditPage {
            entries: vec![],
            next_cursor: None,
        });
    }

    let mut carry = Vec::new();
    let mut selected: Vec<(u64, AuditEntry)> = Vec::new();
    loop {
        let read_len = BLOCK_SIZE.min(pos) as usize;
        pos -= read_len as u64;

        file.seek(SeekFrom::Start(pos))
            .map_err(|e| format!("Failed to seek audit log: {e}"))?;
        let mut chunk = vec![0u8; read_len];
        file.read_exact(&mut chunk)
            .map_err(|e| format!("Failed to read audit log: {e}"))?;

        chunk.extend_from_slice(&carry);
        let parse_start = if pos > 0 {
            if let Some(newline_idx) = chunk.iter().position(|byte| *byte == b'\n') {
                carry = chunk[..newline_idx].to_vec();
                newline_idx + 1
            } else {
                carry = chunk;
                continue;
            }
        } else {
            0
        };

        let content = String::from_utf8_lossy(&chunk[parse_start..]);
        let mut offset = pos + parse_start as u64;
        let parsed: Vec<(u64, AuditEntry)> = content
            .split_inclusive('\n')
            .enumerate()
            .filter_map(|(idx, segment)| {
                let segment_offset = offset;
                offset += segment.len() as u64;

                if idx == 0 && parse_start > 0 && segment.is_empty() {
                    return None;
                }

                let line = segment.trim();
                if line.is_empty() {
                    return None;
                }

                serde_json::from_str(line)
                    .ok()
                    .map(|entry| (segment_offset, entry))
            })
            .collect();

        let remaining = limit.saturating_sub(selected.len());
        selected.extend(
            parsed
                .into_iter()
                .rev()
                .filter(|(_, entry)| matches_entry(entry))
                .take(remaining),
        );

        if selected.len() >= limit || pos == 0 {
            let next_cursor = selected
                .last()
                .and_then(|(offset, _)| (*offset > 0).then_some(*offset));
            let mut entries: Vec<AuditEntry> =
                selected.into_iter().map(|(_, entry)| entry).collect();
            entries.sort_by_key(|entry| entry.timestamp);
            entries.reverse();
            return Ok(AuditPage {
                entries,
                next_cursor,
            });
        }
    }
}

impl From<AuditEntry> for AuditLogEntry {
    fn from(entry: AuditEntry) -> Self {
        let outcome = match entry.outcome {
            AuditOutcome::Success => "success",
            AuditOutcome::Denied => "denied",
            AuditOutcome::Error => "error",
        };

        Self {
            timestamp: entry.timestamp.to_rfc3339(),
            session_id: entry.session_id.to_string(),
            action: entry.action,
            skill_name: entry.skill_name,
            details: entry.details,
            outcome: outcome.to_string(),
        }
    }
}

impl ViolationEntry {
    fn from_audit_entry(entry: AuditEntry) -> Option<Self> {
        if !is_violation_entry(&entry) {
            return None;
        }

        let severity = if matches!(entry.outcome, AuditOutcome::Denied) {
            "block"
        } else {
            "warn"
        };
        let rule = entry
            .details
            .get("rule")
            .or_else(|| entry.details.get("rule_name"))
            .and_then(|value| value.as_str())
            .unwrap_or(&entry.action)
            .to_string();
        let message = entry
            .details
            .get("message")
            .or_else(|| entry.details.get("reason"))
            .and_then(|value| value.as_str())
            .unwrap_or(&entry.action)
            .to_string();

        Some(Self {
            timestamp: entry.timestamp.to_rfc3339(),
            session_id: entry.session_id.to_string(),
            rule,
            severity: severity.to_string(),
            message,
        })
    }
}

fn is_violation_entry(entry: &AuditEntry) -> bool {
    let action = entry.action.to_ascii_lowercase();
    matches!(entry.outcome, AuditOutcome::Denied)
        || action.contains("guardrail")
        || action.contains("violation")
        || action.contains("policy")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use argentor_agent::{AgentRunner, LlmProvider, ModelConfig};
    use argentor_security::{AuditLog, PermissionSet};
    use argentor_session::FileSessionStore;
    use argentor_skills::SkillRegistry;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    /// Create a test `RestApiState` backed by a temp directory.
    async fn test_state(tmp: &tempfile::TempDir) -> Arc<RestApiState> {
        let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
        let sessions: Arc<dyn SessionStore> = Arc::new(
            FileSessionStore::new(tmp.path().join("sessions"))
                .await
                .unwrap(),
        );
        let skills = Arc::new(SkillRegistry::new());
        let permissions = PermissionSet::new();
        let config = ModelConfig {
            provider: LlmProvider::Claude,
            model_id: "test".to_string(),
            api_key: "test".to_string(),
            api_base_url: Some("http://127.0.0.1:1".to_string()),
            temperature: 0.7,
            max_tokens: 100,
            max_turns: 3,
            max_context_tokens: 200_000,
            fallback_models: vec![],
            retry_policy: None,
        };
        let agent = Arc::new(AgentRunner::new(config, skills.clone(), permissions, audit));
        let connections = ConnectionManager::new();
        let router = Arc::new(MessageRouter::new(
            agent,
            sessions.clone(),
            connections.clone(),
        ));

        Arc::new(RestApiState {
            router,
            connections,
            sessions,
            skills,
            started_at: Utc::now(),
            audit_log_path: Some(tmp.path().join("audit").join("audit.jsonl")),
            audit_stats_cache: Arc::new(RwLock::new(None)),
        })
    }

    #[tokio::test]
    async fn test_list_sessions_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let app = api_router(state);

        let req = Request::builder()
            .uri("/api/v1/sessions")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let sessions: Vec<SessionSummary> = serde_json::from_slice(&body).unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_get_session_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let app = api_router(state);

        let fake_id = Uuid::new_v4();
        let req = Request::builder()
            .uri(format!("/api/v1/sessions/{fake_id}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_session_create_get_delete_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;

        // Create a session via the store directly
        let session = argentor_session::Session::new();
        let session_id = session.id;
        state.sessions.create(&session).await.unwrap();

        // GET the session
        let app = api_router(state.clone());
        let req = Request::builder()
            .uri(format!("/api/v1/sessions/{session_id}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail: SessionDetail = serde_json::from_slice(&body).unwrap();
        assert_eq!(detail.session_id, session_id);
        assert_eq!(detail.message_count, 0);

        // DELETE the session
        let app = api_router(state.clone());
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/sessions/{session_id}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let del: DeleteSessionResponse = serde_json::from_slice(&body).unwrap();
        assert!(del.deleted);
        assert_eq!(del.session_id, session_id);

        // Verify it is gone
        let app = api_router(state.clone());
        let req = Request::builder()
            .uri(format!("/api/v1/sessions/{session_id}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_list_skills_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let app = api_router(state);

        let req = Request::builder()
            .uri("/api/v1/skills")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let skills: Vec<SkillSummary> = serde_json::from_slice(&body).unwrap();
        assert!(skills.is_empty());
    }

    #[tokio::test]
    async fn test_get_skill_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let app = api_router(state);

        let req = Request::builder()
            .uri("/api/v1/skills/nonexistent")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_agent_status() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let app = api_router(state);

        let req = Request::builder()
            .uri("/api/v1/agent/status")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: AgentStatus = serde_json::from_slice(&body).unwrap();
        assert!(status.ready);
        assert_eq!(status.skills_loaded, 0);
    }

    #[tokio::test]
    async fn test_connections_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let app = api_router(state);

        let req = Request::builder()
            .uri("/api/v1/connections")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let info: ConnectionsInfo = serde_json::from_slice(&body).unwrap();
        assert_eq!(info.count, 0);
        assert!(info.session_ids.is_empty());
    }

    #[tokio::test]
    async fn test_metrics() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let app = api_router(state);

        let req = Request::builder()
            .uri("/api/v1/metrics")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let metrics: MetricsResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(metrics.active_connections, 0);
        assert_eq!(metrics.active_sessions, 0);
        assert!(metrics.uptime_seconds >= 0);
        assert_eq!(metrics.skills_registered, 0);
    }

    #[tokio::test]
    async fn test_audit_endpoints_read_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let audit_path = state.audit_log_path.clone().unwrap();
        std::fs::create_dir_all(audit_path.parent().unwrap()).unwrap();

        let session_id = Uuid::new_v4();
        let success = AuditEntry {
            timestamp: Utc::now(),
            session_id,
            action: "tool_call".to_string(),
            skill_name: Some("lookup".to_string()),
            details: serde_json::json!({"ok": true}),
            outcome: AuditOutcome::Success,
            correlation_id: None,
        };
        let denied = AuditEntry {
            timestamp: Utc::now(),
            session_id,
            action: "guardrail_blocked".to_string(),
            skill_name: None,
            details: serde_json::json!({
                "rule": "pii_detection",
                "message": "PII blocked"
            }),
            outcome: AuditOutcome::Denied,
            correlation_id: None,
        };
        let jsonl = format!(
            "{}\n{}\n",
            serde_json::to_string(&success).unwrap(),
            serde_json::to_string(&denied).unwrap()
        );
        std::fs::write(&audit_path, &jsonl).unwrap();

        let app = api_router(state.clone());
        let req = Request::builder()
            .uri("/api/v1/audit/logs?limit=10")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let logs: Vec<AuditLogEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(logs.len(), 2);
        assert!(logs.iter().any(|entry| entry.outcome == "denied"));

        let app = api_router(state.clone());
        let req = Request::builder()
            .uri("/api/v1/audit/violations?limit=10")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let violations: Vec<ViolationEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "pii_detection");
        assert_eq!(violations[0].severity, "block");

        let app = api_router(state.clone());
        let req = Request::builder()
            .uri("/api/v1/audit/stats")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let stats: AuditStats = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.total_events, 2);
        assert_eq!(stats.events_today, 2);
        assert_eq!(stats.violations_today, 1);
        assert_eq!(stats.block_rate_percent, 50.0);
        assert!(state.audit_stats_cache.read().unwrap().as_ref().is_some());

        let extra = AuditEntry {
            timestamp: Utc::now(),
            session_id,
            action: "tool_call".to_string(),
            skill_name: Some("lookup".to_string()),
            details: serde_json::json!({"extra": true}),
            outcome: AuditOutcome::Success,
            correlation_id: None,
        };
        std::fs::write(
            &audit_path,
            format!("{jsonl}{}\n", serde_json::to_string(&extra).unwrap()),
        )
        .unwrap();

        let app = api_router(state);
        let req = Request::builder()
            .uri("/api/v1/audit/stats")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let stats: AuditStats = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.total_events, 3);
        assert_eq!(stats.events_today, 3);
        assert_eq!(stats.violations_today, 1);
    }

    #[tokio::test]
    async fn test_audit_prometheus_export() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let audit_path = state.audit_log_path.clone().unwrap();
        std::fs::create_dir_all(audit_path.parent().unwrap()).unwrap();

        let session_id = Uuid::new_v4();
        let success = AuditEntry {
            timestamp: Utc::now(),
            session_id,
            action: "tool_call".to_string(),
            skill_name: Some("lookup".to_string()),
            details: serde_json::json!({"ok": true}),
            outcome: AuditOutcome::Success,
            correlation_id: None,
        };
        let denied = AuditEntry {
            timestamp: Utc::now(),
            session_id,
            action: "guardrail_blocked".to_string(),
            skill_name: None,
            details: serde_json::json!({"rule": "pii_detection"}),
            outcome: AuditOutcome::Denied,
            correlation_id: None,
        };
        std::fs::write(
            &audit_path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&success).unwrap(),
                serde_json::to_string(&denied).unwrap()
            ),
        )
        .unwrap();

        let metrics = audit_prometheus_export(&state).await;
        assert!(metrics.contains("argentor_audit_configured 1"));
        assert!(metrics.contains("argentor_audit_log_bytes "));
        assert!(metrics.contains("argentor_audit_events_today 2"));
        assert!(metrics.contains("argentor_audit_violations_today 1"));
        assert!(metrics.contains("argentor_audit_block_rate_percent 50.000000"));
        assert!(metrics.contains("argentor_audit_events_total 2"));
    }

    #[tokio::test]
    async fn test_audit_stats_persisted_index_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let audit_path = state.audit_log_path.clone().unwrap();
        std::fs::create_dir_all(audit_path.parent().unwrap()).unwrap();

        let session_id = Uuid::new_v4();
        let denied = AuditEntry {
            timestamp: Utc::now(),
            session_id,
            action: "policy_denied".to_string(),
            skill_name: None,
            details: serde_json::json!({"rule": "stats_index"}),
            outcome: AuditOutcome::Denied,
            correlation_id: None,
        };
        let success = AuditEntry {
            timestamp: Utc::now(),
            session_id,
            action: "tool_call".to_string(),
            skill_name: None,
            details: serde_json::json!({"ok": true}),
            outcome: AuditOutcome::Success,
            correlation_id: None,
        };
        std::fs::write(
            &audit_path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&denied).unwrap(),
                serde_json::to_string(&success).unwrap()
            ),
        )
        .unwrap();

        let metadata = std::fs::metadata(&audit_path).unwrap();
        let file_len = metadata.len();
        let modified_ns = system_time_unix_nanos(metadata.modified().unwrap()).unwrap();
        let stats = read_audit_stats_blocking(&audit_path, file_len, modified_ns).unwrap();
        assert_eq!(stats.total_events, 2);
        assert_eq!(stats.violations_today, 1);

        let index_path = audit_stats_index_path(&audit_path);
        assert!(index_path.exists(), "stats index should be written");

        std::fs::remove_file(&audit_path).unwrap();
        let cached = read_audit_stats_blocking(&audit_path, file_len, modified_ns).unwrap();
        assert_eq!(cached.total_events, 2);
        assert_eq!(cached.violations_today, 1);
    }

    #[tokio::test]
    async fn test_audit_stats_rebuilds_stale_index() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let audit_path = state.audit_log_path.clone().unwrap();
        std::fs::create_dir_all(audit_path.parent().unwrap()).unwrap();

        let session_id = Uuid::new_v4();
        let success = AuditEntry {
            timestamp: Utc::now(),
            session_id,
            action: "tool_call".to_string(),
            skill_name: None,
            details: serde_json::json!({"ok": true}),
            outcome: AuditOutcome::Success,
            correlation_id: None,
        };
        std::fs::write(
            &audit_path,
            format!("{}\n", serde_json::to_string(&success).unwrap()),
        )
        .unwrap();
        let metadata = std::fs::metadata(&audit_path).unwrap();
        let stats = read_audit_stats_blocking(
            &audit_path,
            metadata.len(),
            system_time_unix_nanos(metadata.modified().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(stats.total_events, 1);
        assert_eq!(stats.violations_today, 0);

        let denied = AuditEntry {
            timestamp: Utc::now() + chrono::Duration::seconds(1),
            session_id,
            action: "policy_denied".to_string(),
            skill_name: None,
            details: serde_json::json!({"rule": "stats_stale_rebuild"}),
            outcome: AuditOutcome::Denied,
            correlation_id: None,
        };
        std::fs::write(
            &audit_path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&success).unwrap(),
                serde_json::to_string(&denied).unwrap()
            ),
        )
        .unwrap();
        let metadata = std::fs::metadata(&audit_path).unwrap();
        let stats = read_audit_stats_blocking(
            &audit_path,
            metadata.len(),
            system_time_unix_nanos(metadata.modified().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(stats.total_events, 2);
        assert_eq!(stats.violations_today, 1);
        assert_eq!(stats.block_rate_percent, 50.0);
    }

    #[tokio::test]
    async fn test_audit_prometheus_export_unconfigured() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let mut state = Arc::try_unwrap(state).ok().unwrap();
        state.audit_log_path = None;

        let metrics = audit_prometheus_export(&state).await;
        assert!(metrics.contains("argentor_audit_configured 0"));
        assert!(!metrics.contains("argentor_audit_events_total"));
    }

    #[tokio::test]
    async fn test_audit_logs_cursor_paginates() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let audit_path = state.audit_log_path.clone().unwrap();
        std::fs::create_dir_all(audit_path.parent().unwrap()).unwrap();

        let session_id = Uuid::new_v4();
        let base = Utc::now();
        let mut lines = Vec::new();
        for i in 0..3 {
            let entry = AuditEntry {
                timestamp: base + chrono::Duration::seconds(i),
                session_id,
                action: format!("event_{i}"),
                skill_name: None,
                details: serde_json::json!({ "i": i }),
                outcome: AuditOutcome::Success,
                correlation_id: None,
            };
            lines.push(serde_json::to_string(&entry).unwrap());
        }
        std::fs::write(&audit_path, format!("{}\n", lines.join("\n"))).unwrap();

        let app = api_router(state.clone());
        let req = Request::builder()
            .uri("/api/v1/audit/logs?limit=2")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cursor = resp
            .headers()
            .get(AUDIT_NEXT_CURSOR_HEADER)
            .expect("first page should include cursor")
            .to_str()
            .unwrap()
            .to_string();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let logs: Vec<AuditLogEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            logs.iter()
                .map(|entry| entry.action.as_str())
                .collect::<Vec<_>>(),
            vec!["event_2", "event_1"]
        );

        let app = api_router(state);
        let req = Request::builder()
            .uri(format!("/api/v1/audit/logs?limit=2&cursor={cursor}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get(AUDIT_NEXT_CURSOR_HEADER).is_none());
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let logs: Vec<AuditLogEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "event_0");
    }

    #[tokio::test]
    async fn test_audit_logs_cursor_paginates_across_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let audit_path = state.audit_log_path.clone().unwrap();
        std::fs::create_dir_all(audit_path.parent().unwrap()).unwrap();

        let session_id = Uuid::new_v4();
        let base = Utc::now();
        let payload = "x".repeat(512);
        let mut lines = Vec::new();
        for i in 0..200 {
            let entry = AuditEntry {
                timestamp: base + chrono::Duration::seconds(i),
                session_id,
                action: format!("event_{i}"),
                skill_name: None,
                details: serde_json::json!({ "i": i, "payload": payload }),
                outcome: AuditOutcome::Success,
                correlation_id: None,
            };
            lines.push(serde_json::to_string(&entry).unwrap());
        }
        std::fs::write(&audit_path, format!("{}\n", lines.join("\n"))).unwrap();

        let app = api_router(state.clone());
        let req = Request::builder()
            .uri("/api/v1/audit/logs?limit=50")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cursor = resp
            .headers()
            .get(AUDIT_NEXT_CURSOR_HEADER)
            .expect("first page should include cursor across multiple blocks")
            .to_str()
            .unwrap()
            .to_string();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let logs: Vec<AuditLogEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(logs.len(), 50);
        assert_eq!(logs[0].action, "event_199");
        assert_eq!(logs[49].action, "event_150");

        let app = api_router(state);
        let req = Request::builder()
            .uri(format!("/api/v1/audit/logs?limit=50&cursor={cursor}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let logs: Vec<AuditLogEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(logs.len(), 50);
        assert_eq!(logs[0].action, "event_149");
        assert_eq!(logs[49].action, "event_100");
    }

    #[tokio::test]
    async fn test_audit_logs_reject_invalid_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let app = api_router(state);

        let req = Request::builder()
            .uri("/api/v1/audit/logs?cursor=not-a-number")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_audit_violations_scans_back_until_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let audit_path = state.audit_log_path.clone().unwrap();
        std::fs::create_dir_all(audit_path.parent().unwrap()).unwrap();

        let session_id = Uuid::new_v4();
        let denied = AuditEntry {
            timestamp: Utc::now(),
            session_id,
            action: "policy_denied".to_string(),
            skill_name: None,
            details: serde_json::json!({
                "rule": "network_policy",
                "message": "Outbound host blocked"
            }),
            outcome: AuditOutcome::Denied,
            correlation_id: None,
        };

        let mut lines = vec![serde_json::to_string(&denied).unwrap()];
        for i in 0..1500 {
            let success = AuditEntry {
                timestamp: Utc::now(),
                session_id,
                action: "tool_call".to_string(),
                skill_name: Some("lookup".to_string()),
                details: serde_json::json!({ "i": i }),
                outcome: AuditOutcome::Success,
                correlation_id: None,
            };
            lines.push(serde_json::to_string(&success).unwrap());
        }
        std::fs::write(&audit_path, format!("{}\n", lines.join("\n"))).unwrap();

        let app = api_router(state);
        let req = Request::builder()
            .uri("/api/v1/audit/violations?limit=1")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let violations: Vec<ViolationEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "network_policy");
    }

    #[tokio::test]
    async fn test_audit_violations_uses_index_and_paginates() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let audit_path = state.audit_log_path.clone().unwrap();
        std::fs::create_dir_all(audit_path.parent().unwrap()).unwrap();

        let session_id = Uuid::new_v4();
        let base = Utc::now();
        let mut lines = Vec::new();
        for i in 0..5 {
            let denied = i == 0 || i == 2 || i == 4;
            let entry = AuditEntry {
                timestamp: base + chrono::Duration::seconds(i),
                session_id,
                action: if denied {
                    "policy_denied".to_string()
                } else {
                    "tool_call".to_string()
                },
                skill_name: None,
                details: serde_json::json!({
                    "rule": format!("rule_{i}"),
                    "message": format!("message_{i}")
                }),
                outcome: if denied {
                    AuditOutcome::Denied
                } else {
                    AuditOutcome::Success
                },
                correlation_id: None,
            };
            lines.push(serde_json::to_string(&entry).unwrap());
        }
        std::fs::write(&audit_path, format!("{}\n", lines.join("\n"))).unwrap();

        let app = api_router(state.clone());
        let req = Request::builder()
            .uri("/api/v1/audit/violations?limit=2")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cursor = resp
            .headers()
            .get(AUDIT_NEXT_CURSOR_HEADER)
            .expect("first violation page should include cursor")
            .to_str()
            .unwrap()
            .to_string();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let violations: Vec<ViolationEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            violations
                .iter()
                .map(|entry| entry.rule.as_str())
                .collect::<Vec<_>>(),
            vec!["rule_4", "rule_2"]
        );
        assert!(audit_violation_index_path(&audit_path).exists());

        let app = api_router(state);
        let req = Request::builder()
            .uri(format!("/api/v1/audit/violations?limit=2&cursor={cursor}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get(AUDIT_NEXT_CURSOR_HEADER).is_none());
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let violations: Vec<ViolationEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "rule_0");
    }

    #[tokio::test]
    async fn test_audit_violations_rebuilds_stale_index() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let audit_path = state.audit_log_path.clone().unwrap();
        std::fs::create_dir_all(audit_path.parent().unwrap()).unwrap();

        let session_id = Uuid::new_v4();
        let success = AuditEntry {
            timestamp: Utc::now(),
            session_id,
            action: "tool_call".to_string(),
            skill_name: None,
            details: serde_json::json!({ "ok": true }),
            outcome: AuditOutcome::Success,
            correlation_id: None,
        };
        std::fs::write(
            &audit_path,
            format!("{}\n", serde_json::to_string(&success).unwrap()),
        )
        .unwrap();

        let app = api_router(state.clone());
        let req = Request::builder()
            .uri("/api/v1/audit/violations?limit=10")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let violations: Vec<ViolationEntry> = serde_json::from_slice(&body).unwrap();
        assert!(violations.is_empty());

        let denied = AuditEntry {
            timestamp: Utc::now() + chrono::Duration::seconds(1),
            session_id,
            action: "policy_denied".to_string(),
            skill_name: None,
            details: serde_json::json!({
                "rule": "stale_rebuild",
                "message": "rebuilt"
            }),
            outcome: AuditOutcome::Denied,
            correlation_id: None,
        };
        std::fs::write(
            &audit_path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&success).unwrap(),
                serde_json::to_string(&denied).unwrap()
            ),
        )
        .unwrap();

        let app = api_router(state);
        let req = Request::builder()
            .uri("/api/v1/audit/violations?limit=10")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let violations: Vec<ViolationEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "stale_rebuild");
    }

    #[tokio::test]
    async fn test_agent_chat_empty_message() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp).await;
        let app = api_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/agent/chat")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&ChatRequest {
                    message: "   ".to_string(),
                    session_id: None,
                })
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
