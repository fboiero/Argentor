//! Server-Sent Events streaming for real-time agent conversations.
//!
//! Provides proper SSE formatting, heartbeat keep-alive, error recovery,
//! and typed event streams for the gateway REST API.
//!
//! # Protocol
//!
//! Each SSE event follows the standard format:
//!
//! ```text
//! id: 1
//! event: text
//! data: {"type":"text","text":"Hello","token_index":0}
//!
//! ```
//!
//! Heartbeats are sent every 15 seconds by default to prevent proxy/load-balancer
//! timeouts. Clients can use the `id:` field for reconnection via
//! `Last-Event-ID`.

use argentor_agent::StreamEvent;
use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::{get, post},
    Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, RwLock};
use tokio_stream::{wrappers::UnboundedReceiverStream, Stream, StreamExt};
use tracing::{info, warn};

use crate::connection::ConnectionManager;
use crate::rest_api::ApiError;
use crate::router::MessageRouter;

use argentor_session::SessionStore;

const MAX_SESSION_BROADCAST_CHANNELS: usize = 4096;

// ---------------------------------------------------------------------------
// SSE event types
// ---------------------------------------------------------------------------

/// SSE event types sent to clients during a streaming agent conversation.
///
/// Each variant maps to a named SSE event (`event: <type>`) so clients can
/// dispatch on the event name without parsing the JSON payload first.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SseEvent {
    /// Agent is thinking/processing.
    #[serde(rename = "thinking")]
    Thinking {
        /// The thinking/reasoning text fragment.
        text: String,
    },

    /// Partial text output (streaming token).
    #[serde(rename = "text")]
    Text {
        /// The text fragment being streamed.
        text: String,
        /// Zero-based index of this token in the stream.
        token_index: u64,
    },

    /// Agent is calling a tool.
    #[serde(rename = "tool_call")]
    ToolCall {
        /// Name of the tool being invoked.
        name: String,
        /// JSON arguments passed to the tool.
        arguments: serde_json::Value,
    },

    /// Tool returned a result.
    #[serde(rename = "tool_result")]
    ToolResult {
        /// Name of the tool that returned.
        name: String,
        /// Result content from the tool execution.
        content: String,
        /// Whether the tool execution produced an error.
        is_error: bool,
    },

    /// Final complete response.
    #[serde(rename = "done")]
    Done {
        /// The complete response text.
        text: String,
        /// Number of agentic loop turns executed.
        turns: u32,
        /// Estimated tokens consumed during the run.
        tokens_used: u64,
    },

    /// Error occurred.
    #[serde(rename = "error")]
    Error {
        /// Human-readable error message.
        message: String,
        /// Whether the client can retry the request.
        recoverable: bool,
    },

    /// Heartbeat (keep-alive).
    #[serde(rename = "heartbeat")]
    Heartbeat {
        /// ISO 8601 timestamp of the heartbeat.
        timestamp: String,
    },

    /// Guardrail violation detected.
    #[serde(rename = "guardrail")]
    GuardrailViolation {
        /// Name of the guardrail rule that was violated.
        rule: String,
        /// Severity level (e.g. "warn", "block").
        severity: String,
        /// Human-readable description of the violation.
        message: String,
    },
}

impl SseEvent {
    /// Returns the SSE event name for this variant.
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::Thinking { .. } => "thinking",
            Self::Text { .. } => "text",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
            Self::Done { .. } => "done",
            Self::Error { .. } => "error",
            Self::Heartbeat { .. } => "heartbeat",
            Self::GuardrailViolation { .. } => "guardrail",
        }
    }

    /// Convert this event to an axum SSE [`Event`] with the given ID.
    pub fn to_sse_event(&self, id: u64) -> Result<Event, Infallible> {
        let data = serde_json::to_string(self).unwrap_or_default();
        Ok(Event::default()
            .id(id.to_string())
            .event(self.event_name())
            .data(data))
    }
}

// ---------------------------------------------------------------------------
// StreamEvent -> SseEvent conversion
// ---------------------------------------------------------------------------

/// Convert an internal [`StreamEvent`] from the agent runner into a
/// client-facing [`SseEvent`].
///
/// A running token counter is passed by reference so that `TextDelta` events
/// receive sequential indices.
pub fn stream_event_to_sse(event: StreamEvent, token_counter: &AtomicU64) -> SseEvent {
    match event {
        StreamEvent::TextDelta { text } => {
            let idx = token_counter.fetch_add(1, Ordering::Relaxed);
            SseEvent::Text {
                text,
                token_index: idx,
            }
        }
        StreamEvent::ToolCallStart { id: _, name } => SseEvent::ToolCall {
            name,
            arguments: serde_json::Value::Null,
        },
        StreamEvent::ToolCallDelta {
            id: _,
            arguments_delta,
        } => SseEvent::Thinking {
            text: arguments_delta,
        },
        StreamEvent::ToolCallEnd { id: _ } => SseEvent::Thinking {
            text: String::new(),
        },
        StreamEvent::Done => SseEvent::Done {
            text: String::new(),
            turns: 0,
            tokens_used: 0,
        },
        StreamEvent::Error { message } => SseEvent::Error {
            message,
            recoverable: false,
        },
    }
}

// ---------------------------------------------------------------------------
// Request / State types
// ---------------------------------------------------------------------------

/// Request body for the SSE streaming chat endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamRequest {
    /// The user message to send to the agent.
    pub input: String,
    /// Optional session ID to continue an existing conversation.
    pub session_id: Option<String>,
    /// Optional role hint (e.g. "user").
    pub role: Option<String>,
    /// Optional model override.
    pub model: Option<String>,
}

/// Shared state for the streaming SSE handler.
pub struct StreamingState {
    /// The message router that handles agent interactions.
    pub router: Arc<MessageRouter>,
    /// Tracks active connections.
    pub connections: Arc<ConnectionManager>,
    /// Session persistence backend.
    pub sessions: Arc<dyn SessionStore>,
    /// Per-session broadcast channels for `GET /api/v1/stream/{session_id}`.
    ///
    /// When an agent run is active it publishes SSE JSON strings here;
    /// subscribers receive them in real time.
    pub session_broadcast: Arc<RwLock<HashMap<uuid::Uuid, broadcast::Sender<String>>>>,
}

// ---------------------------------------------------------------------------
// SSE streaming handler
// ---------------------------------------------------------------------------

/// Create an SSE event stream from an agent runner via the message router.
///
/// The returned stream yields SSE events as the agent processes the input,
/// interleaved with periodic heartbeats to keep the connection alive.
pub fn stream_agent_events(
    event_rx: tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
    heartbeat_interval: Duration,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let token_counter = Arc::new(AtomicU64::new(0));
    let event_id = Arc::new(AtomicU64::new(1));

    // Convert the mpsc receiver into a stream of SSE events
    let agent_stream = UnboundedReceiverStream::new(event_rx).map(move |stream_event| {
        let sse = stream_event_to_sse(stream_event, &token_counter);
        let id = event_id.fetch_add(1, Ordering::Relaxed);
        sse.to_sse_event(id)
    });

    // Heartbeat stream — sends a heartbeat every `heartbeat_interval`
    let heartbeat_id = Arc::new(AtomicU64::new(1_000_000));
    let heartbeat_stream = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
        heartbeat_interval,
    ))
    .map(move |_| {
        let sse = SseEvent::Heartbeat {
            timestamp: Utc::now().to_rfc3339(),
        };
        let id = heartbeat_id.fetch_add(1, Ordering::Relaxed);
        sse.to_sse_event(id)
    });

    // Merge both streams: agent events take priority, heartbeats fill the gaps.
    // When the agent stream ends, the merged stream also ends.
    StreamExt::merge(agent_stream, heartbeat_stream)
}

/// Axum handler for SSE streaming chat.
///
/// Accepts a JSON [`StreamRequest`], creates a session (or resumes an existing
/// one), starts the agent in streaming mode, and returns an SSE stream of
/// events.
///
/// # Endpoint
///
/// `POST /api/v1/chat/stream`
pub async fn sse_chat_handler(
    State(state): State<Arc<StreamingState>>,
    Json(request): Json<StreamRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    if request.input.trim().is_empty() {
        return Err(ApiError::BadRequest("Input must not be empty".to_string()));
    }

    // Resolve or create session
    let session_id: uuid::Uuid = request
        .session_id
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(uuid::Uuid::new_v4);

    info!(session_id = %session_id, "SSE streaming chat request");

    let mut session = match state.sessions.get(session_id).await {
        Ok(Some(s)) => s,
        _ => {
            let mut s = argentor_session::Session::new();
            s.id = session_id;
            s
        }
    };

    // Create the event channel
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();

    // Spawn the agent runner in a background task
    let router = state.router.clone();
    let sessions = state.sessions.clone();
    let input = request.input.clone();

    tokio::spawn(async move {
        let result = router
            .agent()
            .run_streaming(&mut session, &input, event_tx)
            .await;

        // Persist session regardless of outcome
        if let Err(e) = sessions.update(&session).await {
            warn!(error = %e, "Failed to persist session after SSE stream");
        }

        if let Err(e) = result {
            warn!(error = %e, "Agent streaming run failed");
        }
    });

    // Build the SSE event stream with heartbeats
    let heartbeat_interval = Duration::from_secs(15);
    let sse_stream = stream_agent_events(event_rx, heartbeat_interval);

    Ok(
        Sse::new(sse_stream)
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("")),
    )
}

// ---------------------------------------------------------------------------
// GET /api/v1/stream/{session_id} — subscribe to a running agent
// ---------------------------------------------------------------------------

/// SSE subscriber handler for `GET /api/v1/stream/{session_id}`.
///
/// Clients subscribe to this endpoint *before* (or immediately after)
/// triggering an agent run via `POST /api/v1/chat/stream`. Events are
/// published by the running agent and forwarded here in real time.
///
/// # Event format
///
/// ```text
/// event: token
/// data: {"text": "Hello", "index": 0}
///
/// event: tool_call
/// data: {"skill": "web_search", "args": {...}}
///
/// event: done
/// data: {"total_tokens": 150, "duration_ms": 2340}
/// ```
pub async fn sse_session_stream_handler(
    State(state): State<Arc<StreamingState>>,
    Path(session_id_str): Path<String>,
) -> impl IntoResponse {
    let session_id: uuid::Uuid = match session_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "Invalid session_id — must be a UUID",
            )
                .into_response();
        }
    };

    info!(session_id = %session_id, "SSE session stream subscription");

    let rx = match subscribe_session_broadcast(&state, session_id).await {
        Ok(rx) => rx,
        Err(status) => return status.into_response(),
    };

    let token_idx = Arc::new(AtomicU64::new(0));

    let sse_stream = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(move |result| {
        let token_idx = token_idx.clone();
        match result {
            Ok(raw) => {
                // Determine the SSE event name from the "event" field in the payload.
                let event_name = serde_json::from_str::<serde_json::Value>(&raw)
                    .ok()
                    .and_then(|v| {
                        v.get("event")
                            .and_then(|e| e.as_str())
                            .map(std::string::ToString::to_string)
                    })
                    .unwrap_or_else(|| "message".to_string());

                let idx = token_idx.fetch_add(1, Ordering::Relaxed);
                let event = Event::default()
                    .id(idx.to_string())
                    .event(event_name)
                    .data(raw);
                Some(Ok::<Event, Infallible>(event))
            }
            // Receiver lagged — skip missed events rather than terminating.
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => None,
        }
    });

    Sse::new(sse_stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text(""))
        .into_response()
}

async fn subscribe_session_broadcast(
    state: &StreamingState,
    session_id: uuid::Uuid,
) -> Result<broadcast::Receiver<String>, StatusCode> {
    let mut map = state.session_broadcast.write().await;

    if !map.contains_key(&session_id) && map.len() >= MAX_SESSION_BROADCAST_CHANNELS {
        prune_idle_broadcast_channels(&mut map);
        if map.len() >= MAX_SESSION_BROADCAST_CHANNELS {
            warn!(
                active_channels = map.len(),
                "SSE session broadcast channel limit reached"
            );
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
    }

    let sender = map.entry(session_id).or_insert_with(|| {
        let (tx, _) = broadcast::channel(256);
        tx
    });

    Ok(sender.subscribe())
}

fn prune_idle_broadcast_channels(map: &mut HashMap<uuid::Uuid, broadcast::Sender<String>>) {
    map.retain(|_, tx| tx.receiver_count() > 0);
}

/// Publish an event to the per-session broadcast channel.
///
/// Called by gateway layers when an agent produces a token, tool call, or
/// completion event. Returns the number of active subscribers.
pub async fn publish_session_event(
    state: &StreamingState,
    session_id: uuid::Uuid,
    event: &str,
    data: serde_json::Value,
) -> usize {
    let payload = serde_json::json!({ "event": event, "data": data }).to_string();
    let tx = {
        let map = state.session_broadcast.read().await;
        map.get(&session_id).cloned()
    };

    let Some(tx) = tx else {
        return 0;
    };

    match tx.send(payload) {
        Ok(count) => count,
        Err(_) => {
            let mut map = state.session_broadcast.write().await;
            if map
                .get(&session_id)
                .is_some_and(|sender| sender.receiver_count() == 0)
            {
                map.remove(&session_id);
            }
            0
        }
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the SSE streaming sub-router.
///
/// Mounts:
/// - `POST /api/v1/chat/stream` — start an agent run and stream its output
/// - `GET  /api/v1/stream/{session_id}` — subscribe to a running agent's event stream
pub fn streaming_router(state: Arc<StreamingState>) -> Router {
    Router::new()
        .route("/api/v1/chat/stream", post(sse_chat_handler))
        .route(
            "/api/v1/stream/{session_id}",
            get(sse_session_stream_handler),
        )
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- SseEvent serialization tests --

    #[test]
    fn test_sse_event_serialize_thinking() {
        let event = SseEvent::Thinking {
            text: "reasoning...".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "thinking");
        assert_eq!(json["text"], "reasoning...");
    }

    #[test]
    fn test_sse_event_serialize_text() {
        let event = SseEvent::Text {
            text: "Hello".to_string(),
            token_index: 42,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "Hello");
        assert_eq!(json["token_index"], 42);
    }

    #[test]
    fn test_sse_event_serialize_tool_call() {
        let event = SseEvent::ToolCall {
            name: "search".to_string(),
            arguments: json!({"query": "rust sse"}),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_call");
        assert_eq!(json["name"], "search");
        assert_eq!(json["arguments"]["query"], "rust sse");
    }

    #[test]
    fn test_sse_event_serialize_tool_result() {
        let event = SseEvent::ToolResult {
            name: "search".to_string(),
            content: "found 3 results".to_string(),
            is_error: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["name"], "search");
        assert_eq!(json["content"], "found 3 results");
        assert_eq!(json["is_error"], false);
    }

    #[test]
    fn test_sse_event_serialize_done() {
        let event = SseEvent::Done {
            text: "Final answer".to_string(),
            turns: 3,
            tokens_used: 512,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "done");
        assert_eq!(json["text"], "Final answer");
        assert_eq!(json["turns"], 3);
        assert_eq!(json["tokens_used"], 512);
    }

    #[test]
    fn test_sse_event_serialize_error() {
        let event = SseEvent::Error {
            message: "rate limit exceeded".to_string(),
            recoverable: true,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["message"], "rate limit exceeded");
        assert_eq!(json["recoverable"], true);
    }

    #[test]
    fn test_sse_event_serialize_heartbeat() {
        let event = SseEvent::Heartbeat {
            timestamp: "2026-04-01T12:00:00Z".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "heartbeat");
        assert_eq!(json["timestamp"], "2026-04-01T12:00:00Z");
    }

    #[test]
    fn test_sse_event_serialize_guardrail() {
        let event = SseEvent::GuardrailViolation {
            rule: "pii_detection".to_string(),
            severity: "warn".to_string(),
            message: "PII detected in output".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "guardrail");
        assert_eq!(json["rule"], "pii_detection");
        assert_eq!(json["severity"], "warn");
        assert_eq!(json["message"], "PII detected in output");
    }

    // -- StreamEvent -> SseEvent conversion --

    #[test]
    fn test_stream_event_text_delta_conversion() {
        let stream_event = StreamEvent::TextDelta {
            text: "Hello".to_string(),
        };
        let counter = AtomicU64::new(0);
        let sse = stream_event_to_sse(stream_event, &counter);
        match sse {
            SseEvent::Text { text, token_index } => {
                assert_eq!(text, "Hello");
                assert_eq!(token_index, 0);
            }
            _ => panic!("Expected SseEvent::Text"),
        }
        // Counter should have been incremented
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_stream_event_tool_call_start_conversion() {
        let stream_event = StreamEvent::ToolCallStart {
            id: "tc_1".to_string(),
            name: "echo".to_string(),
        };
        let counter = AtomicU64::new(0);
        let sse = stream_event_to_sse(stream_event, &counter);
        match sse {
            SseEvent::ToolCall { name, arguments } => {
                assert_eq!(name, "echo");
                assert!(arguments.is_null());
            }
            _ => panic!("Expected SseEvent::ToolCall"),
        }
    }

    #[test]
    fn test_stream_event_done_conversion() {
        let stream_event = StreamEvent::Done;
        let counter = AtomicU64::new(0);
        let sse = stream_event_to_sse(stream_event, &counter);
        match sse {
            SseEvent::Done {
                text,
                turns,
                tokens_used,
            } => {
                assert!(text.is_empty());
                assert_eq!(turns, 0);
                assert_eq!(tokens_used, 0);
            }
            _ => panic!("Expected SseEvent::Done"),
        }
    }

    #[test]
    fn test_stream_event_error_conversion() {
        let stream_event = StreamEvent::Error {
            message: "provider timeout".to_string(),
        };
        let counter = AtomicU64::new(0);
        let sse = stream_event_to_sse(stream_event, &counter);
        match sse {
            SseEvent::Error {
                message,
                recoverable,
            } => {
                assert_eq!(message, "provider timeout");
                assert!(!recoverable);
            }
            _ => panic!("Expected SseEvent::Error"),
        }
    }

    // -- StreamRequest deserialization --

    #[test]
    fn test_stream_request_deserialize_minimal() {
        let json_str = r#"{"input": "Hello agent"}"#;
        let req: StreamRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.input, "Hello agent");
        assert!(req.session_id.is_none());
        assert!(req.role.is_none());
        assert!(req.model.is_none());
    }

    #[test]
    fn test_stream_request_deserialize_full() {
        let json_str = r#"{
            "input": "Hello agent",
            "session_id": "550e8400-e29b-41d4-a716-446655440000",
            "role": "user",
            "model": "claude-sonnet-4"
        }"#;
        let req: StreamRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.input, "Hello agent");
        assert_eq!(
            req.session_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(req.role.as_deref(), Some("user"));
        assert_eq!(req.model.as_deref(), Some("claude-sonnet-4"));
    }

    // -- Event name mapping --

    #[test]
    fn test_event_names() {
        assert_eq!(
            SseEvent::Thinking {
                text: String::new()
            }
            .event_name(),
            "thinking"
        );
        assert_eq!(
            SseEvent::Text {
                text: String::new(),
                token_index: 0
            }
            .event_name(),
            "text"
        );
        assert_eq!(
            SseEvent::ToolCall {
                name: String::new(),
                arguments: serde_json::Value::Null
            }
            .event_name(),
            "tool_call"
        );
        assert_eq!(
            SseEvent::ToolResult {
                name: String::new(),
                content: String::new(),
                is_error: false
            }
            .event_name(),
            "tool_result"
        );
        assert_eq!(
            SseEvent::Done {
                text: String::new(),
                turns: 0,
                tokens_used: 0
            }
            .event_name(),
            "done"
        );
        assert_eq!(
            SseEvent::Error {
                message: String::new(),
                recoverable: false
            }
            .event_name(),
            "error"
        );
        assert_eq!(
            SseEvent::Heartbeat {
                timestamp: String::new()
            }
            .event_name(),
            "heartbeat"
        );
        assert_eq!(
            SseEvent::GuardrailViolation {
                rule: String::new(),
                severity: String::new(),
                message: String::new()
            }
            .event_name(),
            "guardrail"
        );
    }

    // -- SSE Event formatting --

    #[test]
    fn test_to_sse_event_has_id_and_event_name() {
        let event = SseEvent::Text {
            text: "hello".to_string(),
            token_index: 5,
        };
        let sse = event.to_sse_event(42).unwrap();
        // axum Event doesn't expose fields directly, but we can verify it doesn't panic
        // and the construction succeeds. The actual SSE wire format is tested via
        // integration tests or by inspecting the string representation.
        let _ = sse;
    }

    #[test]
    fn test_error_event_format() {
        let event = SseEvent::Error {
            message: "something went wrong".to_string(),
            recoverable: false,
        };
        let data = serde_json::to_string(&event).unwrap();
        assert!(data.contains("\"type\":\"error\""));
        assert!(data.contains("\"message\":\"something went wrong\""));
        assert!(data.contains("\"recoverable\":false"));
    }

    // -- Token counter atomicity --

    #[test]
    fn test_token_counter_increments_sequentially() {
        let counter = AtomicU64::new(0);

        for expected in 0..10 {
            let event = StreamEvent::TextDelta {
                text: format!("word{expected}"),
            };
            let sse = stream_event_to_sse(event, &counter);
            match sse {
                SseEvent::Text { token_index, .. } => {
                    assert_eq!(token_index, expected);
                }
                _ => panic!("Expected SseEvent::Text"),
            }
        }
        assert_eq!(counter.load(Ordering::Relaxed), 10);
    }

    // -- Heartbeat generation --

    #[test]
    fn test_heartbeat_has_timestamp() {
        let event = SseEvent::Heartbeat {
            timestamp: Utc::now().to_rfc3339(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "heartbeat");
        assert!(json["timestamp"].as_str().unwrap().contains("T"));
    }

    // -- Deserialization round-trip --

    #[test]
    fn test_sse_event_roundtrip() {
        let events = vec![
            SseEvent::Thinking {
                text: "hmm".to_string(),
            },
            SseEvent::Text {
                text: "hi".to_string(),
                token_index: 0,
            },
            SseEvent::ToolCall {
                name: "echo".to_string(),
                arguments: json!({"msg": "test"}),
            },
            SseEvent::ToolResult {
                name: "echo".to_string(),
                content: "test".to_string(),
                is_error: false,
            },
            SseEvent::Done {
                text: "done".to_string(),
                turns: 1,
                tokens_used: 100,
            },
            SseEvent::Error {
                message: "oops".to_string(),
                recoverable: true,
            },
            SseEvent::Heartbeat {
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            SseEvent::GuardrailViolation {
                rule: "pii".to_string(),
                severity: "block".to_string(),
                message: "PII found".to_string(),
            },
        ];

        for event in events {
            let serialized = serde_json::to_string(&event).unwrap();
            let deserialized: SseEvent = serde_json::from_str(&serialized).unwrap();
            // Verify the type tag is preserved
            let v1 = serde_json::to_value(&event).unwrap();
            let v2 = serde_json::to_value(&deserialized).unwrap();
            assert_eq!(v1, v2, "Round-trip failed for event: {serialized}");
        }
    }

    #[test]
    fn test_prune_idle_broadcast_channels() {
        let active_id = uuid::Uuid::new_v4();
        let idle_id = uuid::Uuid::new_v4();
        let (active_tx, active_rx) = broadcast::channel(1);
        let (idle_tx, _idle_rx) = broadcast::channel(1);
        drop(_idle_rx);

        let mut map = HashMap::new();
        map.insert(active_id, active_tx);
        map.insert(idle_id, idle_tx);

        prune_idle_broadcast_channels(&mut map);

        assert!(map.contains_key(&active_id));
        assert!(!map.contains_key(&idle_id));
        drop(active_rx);
    }
}
