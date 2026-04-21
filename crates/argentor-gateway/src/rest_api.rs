//! REST API endpoints for managing agents, sessions, skills, and connections.
//!
//! Provides a comprehensive JSON API mounted under `/api/v1/` that complements
//! the existing WebSocket and health endpoints.

use crate::connection::ConnectionManager;
use crate::router::{InboundMessage, MessageRouter};
use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use argentor_session::SessionStore;
use argentor_skills::SkillRegistry;

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
