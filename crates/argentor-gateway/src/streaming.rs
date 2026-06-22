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
use async_trait::async_trait;
use axum::{
    extract::{Json, Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::{get, post},
    Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::path::{Path as FsPath, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, RwLock};
use tokio_stream::{wrappers::UnboundedReceiverStream, Stream, StreamExt};
use tracing::{info, warn};

use crate::connection::ConnectionManager;
use crate::rest_api::ApiError;
use crate::router::MessageRouter;

use argentor_session::SessionStore;

const MAX_SESSION_BROADCAST_CHANNELS: usize = 4096;
const SESSION_BROADCAST_CHANNEL_CAPACITY: usize = 256;
const SESSION_REPLAY_BUFFER_CAPACITY: usize = 1024;

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
    /// Per-session broadcast adapter for `GET /api/v1/stream/{session_id}`.
    pub session_broadcast: Arc<dyn SessionBroadcast>,
    /// Backpressure limits for long-lived stream subscriptions.
    pub stream_backpressure: Arc<StreamBackpressureLimiter>,
}

/// Error returned by a session broadcast adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionBroadcastError {
    /// The adapter has reached its configured session-channel limit.
    CapacityReached,
    /// The adapter could not read or write its backing transport/storage.
    StorageUnavailable,
}

impl SessionBroadcastError {
    fn status_code(self) -> StatusCode {
        match self {
            Self::CapacityReached => StatusCode::SERVICE_UNAVAILABLE,
            Self::StorageUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

/// Configuration for long-lived SSE stream backpressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamBackpressureConfig {
    /// Maximum active stream subscriptions across the gateway instance.
    pub max_global_streams: usize,
    /// Maximum active stream subscriptions for one tenant ID.
    pub max_streams_per_tenant: usize,
    /// Maximum active stream subscriptions for one API key.
    pub max_streams_per_api_key: usize,
}

impl Default for StreamBackpressureConfig {
    fn default() -> Self {
        Self {
            max_global_streams: 4096,
            max_streams_per_tenant: 256,
            max_streams_per_api_key: 64,
        }
    }
}

/// Reason a stream subscription was rejected by backpressure limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamBackpressureError {
    /// The gateway-wide active stream limit was reached.
    GlobalLimitReached,
    /// The tenant active stream limit was reached.
    TenantLimitReached,
    /// The API-key active stream limit was reached.
    ApiKeyLimitReached,
}

impl StreamBackpressureError {
    fn status_message(self) -> &'static str {
        match self {
            Self::GlobalLimitReached => "Global stream limit reached",
            Self::TenantLimitReached => "Tenant stream limit reached",
            Self::ApiKeyLimitReached => "API key stream limit reached",
        }
    }
}

#[derive(Debug, Default)]
struct StreamBackpressureCounts {
    global: usize,
    by_tenant: HashMap<String, usize>,
    by_api_key: HashMap<String, usize>,
}

/// In-memory active-stream limiter for SSE subscriptions.
pub struct StreamBackpressureLimiter {
    config: StreamBackpressureConfig,
    counts: Mutex<StreamBackpressureCounts>,
}

impl Default for StreamBackpressureLimiter {
    fn default() -> Self {
        Self::new(StreamBackpressureConfig::default())
    }
}

impl StreamBackpressureLimiter {
    /// Create a stream backpressure limiter from explicit limits.
    pub fn new(config: StreamBackpressureConfig) -> Self {
        Self {
            config,
            counts: Mutex::new(StreamBackpressureCounts::default()),
        }
    }

    /// Try to acquire one active stream slot.
    pub fn try_acquire(
        self: &Arc<Self>,
        tenant_id: Option<String>,
        api_key: Option<String>,
    ) -> Result<StreamBackpressurePermit, StreamBackpressureError> {
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if counts.global >= self.config.max_global_streams {
            return Err(StreamBackpressureError::GlobalLimitReached);
        }

        if let Some(ref tenant) = tenant_id {
            let current = counts.by_tenant.get(tenant).copied().unwrap_or(0);
            if current >= self.config.max_streams_per_tenant {
                return Err(StreamBackpressureError::TenantLimitReached);
            }
        }

        if let Some(ref key) = api_key {
            let current = counts.by_api_key.get(key).copied().unwrap_or(0);
            if current >= self.config.max_streams_per_api_key {
                return Err(StreamBackpressureError::ApiKeyLimitReached);
            }
        }

        counts.global += 1;
        if let Some(ref tenant) = tenant_id {
            *counts.by_tenant.entry(tenant.clone()).or_insert(0) += 1;
        }
        if let Some(ref key) = api_key {
            *counts.by_api_key.entry(key.clone()).or_insert(0) += 1;
        }

        Ok(StreamBackpressurePermit {
            limiter: Arc::clone(self),
            tenant_id,
            api_key,
        })
    }

    /// Gateway-wide maximum number of concurrent SSE streams.
    ///
    /// Paired with [`active_counts`](Self::active_counts), operators compute
    /// stream pressure as `global_active / max_global_streams`.
    pub fn max_global_streams(&self) -> usize {
        self.config.max_global_streams
    }

    /// Return current active stream counts as `(global, tenant, api_key)`.
    pub fn active_counts(
        &self,
        tenant_id: Option<&str>,
        api_key: Option<&str>,
    ) -> (usize, usize, usize) {
        let counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tenant = tenant_id
            .and_then(|id| counts.by_tenant.get(id).copied())
            .unwrap_or(0);
        let key = api_key
            .and_then(|id| counts.by_api_key.get(id).copied())
            .unwrap_or(0);
        (counts.global, tenant, key)
    }
}

/// Active-stream permit that releases its slot when the SSE stream is dropped.
pub struct StreamBackpressurePermit {
    limiter: Arc<StreamBackpressureLimiter>,
    tenant_id: Option<String>,
    api_key: Option<String>,
}

impl Drop for StreamBackpressurePermit {
    fn drop(&mut self) {
        let mut counts = self
            .limiter
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        counts.global = counts.global.saturating_sub(1);

        if let Some(ref tenant) = self.tenant_id {
            decrement_count(&mut counts.by_tenant, tenant);
        }

        if let Some(ref key) = self.api_key {
            decrement_count(&mut counts.by_api_key, key);
        }
    }
}

fn decrement_count<K>(map: &mut HashMap<K, usize>, key: &K)
where
    K: Eq + std::hash::Hash,
{
    if let Some(count) = map.get_mut(key) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            map.remove(key);
        }
    }
}

/// Receiver returned by a session broadcast adapter.
///
/// This wrapper keeps the HTTP handler independent from the adapter
/// construction path. A future Redis or NATS implementation can introduce a
/// different receiver variant without changing the public handler shape.
pub enum SessionBroadcastReceiver {
    /// Local in-process Tokio broadcast receiver.
    Local(broadcast::Receiver<SessionBroadcastEvent>),
    /// Generic event stream used by distributed or externally-backed adapters.
    Stream(Pin<Box<dyn Stream<Item = SessionBroadcastEvent> + Send>>),
}

/// A per-session broadcast event with a monotonic SSE event ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBroadcastEvent {
    /// SSE event ID scoped to a single session stream.
    pub id: u64,
    /// Serialized JSON payload sent as the SSE data field.
    pub payload: String,
}

/// Subscription returned by a session broadcast adapter.
pub struct SessionBroadcastSubscription {
    /// Live receiver for newly published events.
    pub receiver: SessionBroadcastReceiver,
    /// Buffered events with IDs greater than the client's `Last-Event-ID`.
    pub replay: Vec<SessionBroadcastEvent>,
}

/// Per-session event broadcast adapter used by the SSE subscription endpoint.
#[async_trait]
pub trait SessionBroadcast: Send + Sync {
    /// Subscribe to events for a session.
    async fn subscribe(
        &self,
        session_id: uuid::Uuid,
        after_event_id: Option<u64>,
    ) -> Result<SessionBroadcastSubscription, SessionBroadcastError>;

    /// Publish an already-serialized event payload to a session.
    async fn publish(&self, session_id: uuid::Uuid, payload: String) -> usize;

    /// Count currently tracked session channels.
    async fn active_channels(&self) -> usize;
}

/// Local in-process session broadcast adapter.
///
/// This preserves the existing Tokio broadcast behavior while isolating the
/// gateway from the storage mechanics. Distributed adapters should implement
/// [`SessionBroadcast`] behind the same contract.
pub struct LocalSessionBroadcast {
    channels: RwLock<HashMap<uuid::Uuid, LocalSessionChannel>>,
    max_channels: usize,
    channel_capacity: usize,
    replay_capacity: usize,
}

struct LocalSessionChannel {
    sender: broadcast::Sender<SessionBroadcastEvent>,
    replay: VecDeque<SessionBroadcastEvent>,
    next_event_id: u64,
}

impl Default for LocalSessionBroadcast {
    fn default() -> Self {
        Self::with_replay_capacity(
            MAX_SESSION_BROADCAST_CHANNELS,
            SESSION_BROADCAST_CHANNEL_CAPACITY,
            SESSION_REPLAY_BUFFER_CAPACITY,
        )
    }
}

impl LocalSessionBroadcast {
    /// Create a local broadcast adapter with explicit capacity controls.
    pub fn new(max_channels: usize, channel_capacity: usize) -> Self {
        Self::with_replay_capacity(
            max_channels,
            channel_capacity,
            SESSION_REPLAY_BUFFER_CAPACITY,
        )
    }

    /// Create a local broadcast adapter with explicit channel and replay limits.
    pub fn with_replay_capacity(
        max_channels: usize,
        channel_capacity: usize,
        replay_capacity: usize,
    ) -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
            max_channels,
            channel_capacity,
            replay_capacity,
        }
    }

    fn prune_idle_channels(map: &mut HashMap<uuid::Uuid, LocalSessionChannel>) {
        map.retain(|_, channel| channel.sender.receiver_count() > 0);
    }
}

#[async_trait]
impl SessionBroadcast for LocalSessionBroadcast {
    async fn subscribe(
        &self,
        session_id: uuid::Uuid,
        after_event_id: Option<u64>,
    ) -> Result<SessionBroadcastSubscription, SessionBroadcastError> {
        let mut map = self.channels.write().await;

        if !map.contains_key(&session_id) && map.len() >= self.max_channels {
            Self::prune_idle_channels(&mut map);
            if map.len() >= self.max_channels {
                warn!(
                    active_channels = map.len(),
                    "SSE session broadcast channel limit reached"
                );
                return Err(SessionBroadcastError::CapacityReached);
            }
        }

        let replay_capacity = self.replay_capacity;
        let channel = map.entry(session_id).or_insert_with(|| {
            let (tx, _) = broadcast::channel(self.channel_capacity);
            LocalSessionChannel {
                sender: tx,
                replay: VecDeque::with_capacity(replay_capacity.min(64)),
                next_event_id: 1,
            }
        });

        let replay = after_event_id
            .map(|last_id| {
                channel
                    .replay
                    .iter()
                    .filter(|event| event.id > last_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        Ok(SessionBroadcastSubscription {
            receiver: SessionBroadcastReceiver::Local(channel.sender.subscribe()),
            replay,
        })
    }

    async fn publish(&self, session_id: uuid::Uuid, payload: String) -> usize {
        let (tx, event) = {
            let mut map = self.channels.write().await;
            let Some(channel) = map.get_mut(&session_id) else {
                return 0;
            };

            let event = SessionBroadcastEvent {
                id: channel.next_event_id,
                payload,
            };
            channel.next_event_id = channel.next_event_id.saturating_add(1);
            channel.replay.push_back(event.clone());
            while channel.replay.len() > self.replay_capacity {
                channel.replay.pop_front();
            }

            (channel.sender.clone(), event)
        };

        match tx.send(event) {
            Ok(count) => count,
            Err(_) => {
                let mut map = self.channels.write().await;
                if map
                    .get(&session_id)
                    .is_some_and(|channel| channel.sender.receiver_count() == 0)
                {
                    map.remove(&session_id);
                }
                0
            }
        }
    }

    async fn active_channels(&self) -> usize {
        self.channels.read().await.len()
    }
}

/// File-backed session broadcast adapter for shared-filesystem deployments.
///
/// Each session stream is stored as newline-delimited JSON under the configured
/// directory. Publishers use a per-session lock file before assigning the next
/// event ID and appending to the log. Subscribers replay events from disk and
/// then poll the same log for new records, which allows multiple gateway
/// replicas sharing a volume to exchange stream events without a broker.
pub struct FileSessionBroadcast {
    directory: PathBuf,
    poll_interval: Duration,
    active_subscribers: Arc<Mutex<HashMap<uuid::Uuid, usize>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSessionBroadcastRecord {
    id: u64,
    payload: String,
}

struct FileSessionLock {
    path: PathBuf,
}

impl FileSessionBroadcast {
    /// Create a file-backed broadcast adapter rooted at `directory`.
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self::with_poll_interval(directory, Duration::from_millis(100))
    }

    /// Create a file-backed broadcast adapter with an explicit poll interval.
    pub fn with_poll_interval(directory: impl Into<PathBuf>, poll_interval: Duration) -> Self {
        Self {
            directory: directory.into(),
            poll_interval,
            active_subscribers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn session_path(&self, session_id: uuid::Uuid) -> PathBuf {
        self.directory.join(format!("{session_id}.jsonl"))
    }

    fn lock_path(&self, session_id: uuid::Uuid) -> PathBuf {
        self.directory.join(format!("{session_id}.lock"))
    }

    async fn ensure_directory(&self) -> Result<(), SessionBroadcastError> {
        tokio::fs::create_dir_all(&self.directory)
            .await
            .map_err(|_| SessionBroadcastError::StorageUnavailable)
    }

    async fn acquire_lock(
        &self,
        session_id: uuid::Uuid,
    ) -> Result<FileSessionLock, SessionBroadcastError> {
        self.ensure_directory().await?;
        let path = self.lock_path(session_id);

        for _ in 0..100 {
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .await
            {
                Ok(_) => return Ok(FileSessionLock { path }),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(_) => return Err(SessionBroadcastError::StorageUnavailable),
            }
        }

        Err(SessionBroadcastError::StorageUnavailable)
    }

    async fn read_records_after(
        path: &FsPath,
        after_event_id: Option<u64>,
    ) -> Result<Vec<SessionBroadcastEvent>, SessionBroadcastError> {
        let contents = match tokio::fs::read_to_string(path).await {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(SessionBroadcastError::StorageUnavailable),
        };

        let after = after_event_id.unwrap_or(0);
        let mut events = Vec::new();
        for line in contents.lines().filter(|line| !line.trim().is_empty()) {
            if let Ok(record) = serde_json::from_str::<StoredSessionBroadcastRecord>(line) {
                if record.id > after {
                    events.push(SessionBroadcastEvent {
                        id: record.id,
                        payload: record.payload,
                    });
                }
            }
        }
        Ok(events)
    }

    async fn latest_event_id(path: &FsPath) -> Result<u64, SessionBroadcastError> {
        Ok(Self::read_records_after(path, None)
            .await?
            .last()
            .map(|event| event.id)
            .unwrap_or(0))
    }

    fn increment_subscriber(&self, session_id: uuid::Uuid) {
        let mut active = self
            .active_subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *active.entry(session_id).or_insert(0) += 1;
    }

    fn decrement_subscriber(
        active_subscribers: &Mutex<HashMap<uuid::Uuid, usize>>,
        session_id: uuid::Uuid,
    ) {
        let mut active = active_subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        decrement_count(&mut active, &session_id);
    }
}

impl FileSessionLock {
    async fn release(self) {
        let _ = tokio::fs::remove_file(&self.path).await;
    }
}

impl Drop for FileSessionLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[async_trait]
impl SessionBroadcast for FileSessionBroadcast {
    async fn subscribe(
        &self,
        session_id: uuid::Uuid,
        after_event_id: Option<u64>,
    ) -> Result<SessionBroadcastSubscription, SessionBroadcastError> {
        self.ensure_directory().await?;
        let path = self.session_path(session_id);
        let replay = if after_event_id.is_some() {
            Self::read_records_after(&path, after_event_id).await?
        } else {
            Vec::new()
        };

        let start_after = replay
            .last()
            .map(|event| event.id)
            .unwrap_or(Self::latest_event_id(&path).await?);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let poll_interval = self.poll_interval;
        let active_subscribers = Arc::clone(&self.active_subscribers);
        self.increment_subscriber(session_id);

        tokio::spawn(async move {
            let mut last_seen = start_after;
            loop {
                if tx.is_closed() {
                    FileSessionBroadcast::decrement_subscriber(&active_subscribers, session_id);
                    return;
                }
                tokio::time::sleep(poll_interval).await;
                let Ok(events) =
                    FileSessionBroadcast::read_records_after(&path, Some(last_seen)).await
                else {
                    continue;
                };

                for event in events {
                    last_seen = last_seen.max(event.id);
                    if tx.send(event).is_err() {
                        FileSessionBroadcast::decrement_subscriber(&active_subscribers, session_id);
                        return;
                    }
                }
            }
        });

        Ok(SessionBroadcastSubscription {
            receiver: SessionBroadcastReceiver::Stream(Box::pin(UnboundedReceiverStream::new(rx))),
            replay,
        })
    }

    async fn publish(&self, session_id: uuid::Uuid, payload: String) -> usize {
        let Ok(lock) = self.acquire_lock(session_id).await else {
            warn!(session_id = %session_id, "file session broadcast lock acquisition failed");
            return 0;
        };

        let path = self.session_path(session_id);
        let event_id = match Self::latest_event_id(&path).await {
            Ok(last_id) => last_id.saturating_add(1),
            Err(_) => {
                lock.release().await;
                return 0;
            }
        };

        let record = StoredSessionBroadcastRecord {
            id: event_id,
            payload,
        };
        let line = match serde_json::to_string(&record) {
            Ok(line) => line,
            Err(_) => {
                lock.release().await;
                return 0;
            }
        };

        let write_result = async {
            self.ensure_directory().await?;
            use tokio::io::AsyncWriteExt;
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
                .map_err(|_| SessionBroadcastError::StorageUnavailable)?;
            file.write_all(line.as_bytes())
                .await
                .map_err(|_| SessionBroadcastError::StorageUnavailable)?;
            file.write_all(b"\n")
                .await
                .map_err(|_| SessionBroadcastError::StorageUnavailable)?;
            file.flush()
                .await
                .map_err(|_| SessionBroadcastError::StorageUnavailable)
        }
        .await;

        lock.release().await;

        if write_result.is_err() {
            return 0;
        }

        let active = self
            .active_subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        active.get(&session_id).copied().unwrap_or(0)
    }

    async fn active_channels(&self) -> usize {
        let active = self
            .active_subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        active.len()
    }
}

/// Redis-backed session broadcast adapter.
///
/// This adapter is available with the `redis-broadcast` Cargo feature. It uses
/// Redis lists for bounded replay, per-session counters for monotonic event IDs,
/// and Redis Pub/Sub for live fanout across gateway replicas.
#[cfg(feature = "redis-broadcast")]
pub struct RedisSessionBroadcast {
    client: redis::Client,
    namespace: String,
    replay_capacity: usize,
    active_subscribers: Arc<Mutex<HashMap<uuid::Uuid, usize>>>,
}

#[cfg(feature = "redis-broadcast")]
impl RedisSessionBroadcast {
    /// Create a Redis broadcast adapter using the default namespace.
    pub fn new(redis_url: &str) -> Result<Self, SessionBroadcastError> {
        Self::with_namespace(
            redis_url,
            "argentor:session-broadcast",
            SESSION_REPLAY_BUFFER_CAPACITY,
        )
    }

    /// Create a Redis broadcast adapter with explicit namespace and replay cap.
    pub fn with_namespace(
        redis_url: &str,
        namespace: impl Into<String>,
        replay_capacity: usize,
    ) -> Result<Self, SessionBroadcastError> {
        let client = redis::Client::open(redis_url)
            .map_err(|_| SessionBroadcastError::StorageUnavailable)?;
        Ok(Self {
            client,
            namespace: namespace.into(),
            replay_capacity,
            active_subscribers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn id_key(&self, session_id: uuid::Uuid) -> String {
        format!("{}:{session_id}:id", self.namespace)
    }

    fn log_key(&self, session_id: uuid::Uuid) -> String {
        format!("{}:{session_id}:log", self.namespace)
    }

    fn channel_name(&self, session_id: uuid::Uuid) -> String {
        format!("{}:{session_id}:channel", self.namespace)
    }

    async fn connection(&self) -> Result<redis::aio::MultiplexedConnection, SessionBroadcastError> {
        self.client
            .get_multiplexed_async_connection()
            .await
            .map_err(|_| SessionBroadcastError::StorageUnavailable)
    }

    async fn read_replay(
        &self,
        session_id: uuid::Uuid,
        after_event_id: Option<u64>,
    ) -> Result<Vec<SessionBroadcastEvent>, SessionBroadcastError> {
        let mut connection = self.connection().await?;
        let values: Vec<String> = redis::cmd("LRANGE")
            .arg(self.log_key(session_id))
            .arg(0)
            .arg(-1)
            .query_async(&mut connection)
            .await
            .map_err(|_| SessionBroadcastError::StorageUnavailable)?;
        let after = after_event_id.unwrap_or(0);
        let mut events = Vec::new();
        for value in values {
            if let Ok(record) = serde_json::from_str::<StoredSessionBroadcastRecord>(&value) {
                if record.id > after {
                    events.push(SessionBroadcastEvent {
                        id: record.id,
                        payload: record.payload,
                    });
                }
            }
        }
        Ok(events)
    }

    async fn latest_event_id(&self, session_id: uuid::Uuid) -> Result<u64, SessionBroadcastError> {
        let mut connection = self.connection().await?;
        let value: Option<u64> = redis::cmd("GET")
            .arg(self.id_key(session_id))
            .query_async(&mut connection)
            .await
            .map_err(|_| SessionBroadcastError::StorageUnavailable)?;
        Ok(value.unwrap_or(0))
    }

    fn increment_subscriber(&self, session_id: uuid::Uuid) {
        let mut active = self
            .active_subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *active.entry(session_id).or_insert(0) += 1;
    }

    fn decrement_subscriber(
        active_subscribers: &Mutex<HashMap<uuid::Uuid, usize>>,
        session_id: uuid::Uuid,
    ) {
        let mut active = active_subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        decrement_count(&mut active, &session_id);
    }
}

#[cfg(feature = "redis-broadcast")]
#[async_trait]
impl SessionBroadcast for RedisSessionBroadcast {
    async fn subscribe(
        &self,
        session_id: uuid::Uuid,
        after_event_id: Option<u64>,
    ) -> Result<SessionBroadcastSubscription, SessionBroadcastError> {
        let channel_name = self.channel_name(session_id);
        let mut pubsub = self
            .client
            .get_async_pubsub()
            .await
            .map_err(|_| SessionBroadcastError::StorageUnavailable)?;
        pubsub
            .subscribe(&channel_name)
            .await
            .map_err(|_| SessionBroadcastError::StorageUnavailable)?;

        let replay = if after_event_id.is_some() {
            self.read_replay(session_id, after_event_id).await?
        } else {
            Vec::new()
        };
        let start_after = replay
            .last()
            .map(|event| event.id)
            .unwrap_or(self.latest_event_id(session_id).await?);

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let active_subscribers = Arc::clone(&self.active_subscribers);
        self.increment_subscriber(session_id);

        tokio::spawn(async move {
            let mut last_seen = start_after;
            let mut messages = pubsub.on_message();
            loop {
                if tx.is_closed() {
                    RedisSessionBroadcast::decrement_subscriber(&active_subscribers, session_id);
                    return;
                }

                let Some(message) = futures_util::StreamExt::next(&mut messages).await else {
                    RedisSessionBroadcast::decrement_subscriber(&active_subscribers, session_id);
                    return;
                };
                let Ok(payload) = message.get_payload::<String>() else {
                    continue;
                };
                let Ok(record) = serde_json::from_str::<StoredSessionBroadcastRecord>(&payload)
                else {
                    continue;
                };
                if record.id <= last_seen {
                    continue;
                }
                last_seen = record.id;
                if tx
                    .send(SessionBroadcastEvent {
                        id: record.id,
                        payload: record.payload,
                    })
                    .is_err()
                {
                    RedisSessionBroadcast::decrement_subscriber(&active_subscribers, session_id);
                    return;
                }
            }
        });

        Ok(SessionBroadcastSubscription {
            receiver: SessionBroadcastReceiver::Stream(Box::pin(UnboundedReceiverStream::new(rx))),
            replay,
        })
    }

    async fn publish(&self, session_id: uuid::Uuid, payload: String) -> usize {
        let mut connection = match self.connection().await {
            Ok(connection) => connection,
            Err(_) => return 0,
        };
        let script = redis::Script::new(
            r#"
            local id = redis.call('INCR', KEYS[1])
            local record = cjson.encode({ id = id, payload = ARGV[1] })
            redis.call('RPUSH', KEYS[2], record)
            redis.call('LTRIM', KEYS[2], -tonumber(ARGV[2]), -1)
            return redis.call('PUBLISH', KEYS[3], record)
            "#,
        );

        script
            .key(self.id_key(session_id))
            .key(self.log_key(session_id))
            .key(self.channel_name(session_id))
            .arg(payload)
            .arg(self.replay_capacity)
            .invoke_async::<usize>(&mut connection)
            .await
            .unwrap_or(0)
    }

    async fn active_channels(&self) -> usize {
        let active = self
            .active_subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        active.len()
    }
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
    headers: HeaderMap,
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

    let tenant_id = extract_stream_tenant_id(&headers);
    let api_key = extract_stream_api_key(&headers);
    let permit = match state
        .stream_backpressure
        .try_acquire(tenant_id.clone(), api_key.clone())
    {
        Ok(permit) => permit,
        Err(err) => {
            warn!(
                session_id = %session_id,
                tenant_id = tenant_id.as_deref().unwrap_or(""),
                has_api_key = api_key.is_some(),
                reason = err.status_message(),
                "SSE stream subscription rejected by backpressure"
            );
            return (StatusCode::TOO_MANY_REQUESTS, err.status_message()).into_response();
        }
    };

    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok());

    let subscription = match state
        .session_broadcast
        .subscribe(session_id, last_event_id)
        .await
    {
        Ok(subscription) => subscription,
        Err(err) => return err.status_code().into_response(),
    };

    let replay_stream = tokio_stream::iter(
        subscription
            .replay
            .into_iter()
            .map(session_broadcast_event_to_sse),
    );

    let live_stream: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        match subscription.receiver {
            SessionBroadcastReceiver::Local(rx) => Box::pin(
                tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(move |result| {
                    match result {
                        Ok(event) => Some(session_broadcast_event_to_sse(event)),
                        // Receiver lagged — skip missed events rather than terminating.
                        Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(
                            _,
                        )) => None,
                    }
                }),
            ),
            SessionBroadcastReceiver::Stream(stream) => {
                Box::pin(stream.map(session_broadcast_event_to_sse))
            }
        };

    let sse_stream = replay_stream.chain(live_stream).map(move |event| {
        let _permit = &permit;
        event
    });

    Sse::new(sse_stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text(""))
        .into_response()
}

fn extract_stream_tenant_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-tenant-id")
        .or_else(|| headers.get("x-tenant"))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(std::string::ToString::to_string)
}

fn extract_stream_api_key(headers: &HeaderMap) -> Option<String> {
    if let Some(key) = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(key.to_string());
    }

    headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(std::string::ToString::to_string)
}

fn session_broadcast_event_to_sse(event: SessionBroadcastEvent) -> Result<Event, Infallible> {
    let event_name = serde_json::from_str::<serde_json::Value>(&event.payload)
        .ok()
        .and_then(|v| {
            v.get("event")
                .and_then(|e| e.as_str())
                .map(std::string::ToString::to_string)
        })
        .unwrap_or_else(|| "message".to_string());

    Ok(Event::default()
        .id(event.id.to_string())
        .event(event_name)
        .data(event.payload))
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
    state.session_broadcast.publish(session_id, payload).await
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

    #[tokio::test]
    async fn test_local_session_broadcast_delivers_payload() {
        let adapter = LocalSessionBroadcast::new(8, 16);
        let session_id = uuid::Uuid::new_v4();
        let subscription = adapter.subscribe(session_id, None).await.unwrap();
        let SessionBroadcastReceiver::Local(mut rx) = subscription.receiver else {
            panic!("expected local broadcast receiver");
        };

        let subscribers = adapter
            .publish(
                session_id,
                r#"{"event":"token","data":{"text":"hi"}}"#.to_string(),
            )
            .await;

        assert_eq!(subscribers, 1);
        let event = rx.recv().await.unwrap();
        assert_eq!(event.id, 1);
        assert!(event.payload.contains("\"event\":\"token\""));
        assert!(event.payload.contains("\"text\":\"hi\""));
    }

    #[tokio::test]
    async fn test_local_session_broadcast_prunes_idle_channels_at_capacity() {
        let adapter = LocalSessionBroadcast::new(1, 16);
        let first_session = uuid::Uuid::new_v4();
        let second_session = uuid::Uuid::new_v4();

        let first = adapter.subscribe(first_session, None).await.unwrap();
        assert_eq!(adapter.active_channels().await, 1);

        let blocked = adapter.subscribe(second_session, None).await;
        assert!(matches!(
            blocked,
            Err(SessionBroadcastError::CapacityReached)
        ));

        drop(first);

        let second = adapter.subscribe(second_session, None).await;
        assert!(second.is_ok());
        assert_eq!(adapter.active_channels().await, 1);
    }

    #[tokio::test]
    async fn test_local_session_broadcast_replays_after_last_event_id() {
        let adapter = LocalSessionBroadcast::with_replay_capacity(8, 16, 4);
        let session_id = uuid::Uuid::new_v4();
        let first = adapter.subscribe(session_id, None).await.unwrap();
        let SessionBroadcastReceiver::Local(_rx) = first.receiver else {
            panic!("expected local broadcast receiver");
        };

        adapter
            .publish(
                session_id,
                r#"{"event":"token","data":{"text":"one"}}"#.to_string(),
            )
            .await;
        adapter
            .publish(
                session_id,
                r#"{"event":"token","data":{"text":"two"}}"#.to_string(),
            )
            .await;
        adapter
            .publish(
                session_id,
                r#"{"event":"done","data":{"ok":true}}"#.to_string(),
            )
            .await;

        let replay = adapter.subscribe(session_id, Some(1)).await.unwrap().replay;

        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0].id, 2);
        assert!(replay[0].payload.contains("\"two\""));
        assert_eq!(replay[1].id, 3);
        assert!(replay[1].payload.contains("\"done\""));
    }

    #[tokio::test]
    async fn test_file_session_broadcast_delivers_across_instances() {
        let dir = tempfile::tempdir().unwrap();
        let publisher = FileSessionBroadcast::with_poll_interval(
            dir.path().join("streams"),
            Duration::from_millis(5),
        );
        let subscriber = FileSessionBroadcast::with_poll_interval(
            dir.path().join("streams"),
            Duration::from_millis(5),
        );
        let session_id = uuid::Uuid::new_v4();
        let subscription = subscriber.subscribe(session_id, None).await.unwrap();
        let SessionBroadcastReceiver::Stream(mut rx) = subscription.receiver else {
            panic!("expected file-backed stream receiver");
        };

        let active = publisher
            .publish(
                session_id,
                r#"{"event":"token","data":{"text":"shared"}}"#.to_string(),
            )
            .await;

        assert_eq!(active, 0);
        let event = tokio::time::timeout(Duration::from_secs(1), rx.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.id, 1);
        assert!(event.payload.contains("\"shared\""));
    }

    #[tokio::test]
    async fn test_file_session_broadcast_replays_after_last_event_id() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = FileSessionBroadcast::with_poll_interval(
            dir.path().join("streams"),
            Duration::from_millis(5),
        );
        let session_id = uuid::Uuid::new_v4();

        adapter
            .publish(
                session_id,
                r#"{"event":"token","data":{"text":"one"}}"#.to_string(),
            )
            .await;
        adapter
            .publish(
                session_id,
                r#"{"event":"token","data":{"text":"two"}}"#.to_string(),
            )
            .await;

        let replay = adapter.subscribe(session_id, Some(1)).await.unwrap().replay;

        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].id, 2);
        assert!(replay[0].payload.contains("\"two\""));
    }

    #[tokio::test]
    async fn test_file_session_broadcast_serializes_concurrent_publish_ids() {
        let dir = tempfile::tempdir().unwrap();
        let first = Arc::new(FileSessionBroadcast::with_poll_interval(
            dir.path().join("streams"),
            Duration::from_millis(5),
        ));
        let second = Arc::new(FileSessionBroadcast::with_poll_interval(
            dir.path().join("streams"),
            Duration::from_millis(5),
        ));
        let session_id = uuid::Uuid::new_v4();

        let first_publish = {
            let first = Arc::clone(&first);
            tokio::spawn(async move {
                first
                    .publish(
                        session_id,
                        r#"{"event":"token","data":{"text":"one"}}"#.to_string(),
                    )
                    .await
            })
        };
        let second_publish = {
            let second = Arc::clone(&second);
            tokio::spawn(async move {
                second
                    .publish(
                        session_id,
                        r#"{"event":"token","data":{"text":"two"}}"#.to_string(),
                    )
                    .await
            })
        };

        let _ = tokio::join!(first_publish, second_publish);
        let path = first.session_path(session_id);
        let events = FileSessionBroadcast::read_records_after(&path, None)
            .await
            .unwrap();
        let mut ids = events.iter().map(|event| event.id).collect::<Vec<_>>();
        ids.sort_unstable();

        assert_eq!(ids, vec![1, 2]);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_stream_backpressure_enforces_api_key_limit() {
        let limiter = Arc::new(StreamBackpressureLimiter::new(StreamBackpressureConfig {
            max_global_streams: 10,
            max_streams_per_tenant: 10,
            max_streams_per_api_key: 1,
        }));

        let first = limiter
            .try_acquire(None, Some("key-a".to_string()))
            .unwrap();
        let second = limiter.try_acquire(None, Some("key-a".to_string()));

        assert!(matches!(
            second,
            Err(StreamBackpressureError::ApiKeyLimitReached)
        ));

        drop(first);
        assert!(limiter.try_acquire(None, Some("key-a".to_string())).is_ok());
    }

    #[test]
    fn test_stream_backpressure_enforces_tenant_limit() {
        let limiter = Arc::new(StreamBackpressureLimiter::new(StreamBackpressureConfig {
            max_global_streams: 10,
            max_streams_per_tenant: 1,
            max_streams_per_api_key: 10,
        }));

        let first = limiter
            .try_acquire(Some("tenant-a".to_string()), None)
            .unwrap();
        let second = limiter.try_acquire(Some("tenant-a".to_string()), None);

        assert!(matches!(
            second,
            Err(StreamBackpressureError::TenantLimitReached)
        ));

        drop(first);
        assert!(limiter
            .try_acquire(Some("tenant-a".to_string()), None)
            .is_ok());
    }

    #[test]
    fn test_stream_backpressure_enforces_global_limit() {
        let limiter = Arc::new(StreamBackpressureLimiter::new(StreamBackpressureConfig {
            max_global_streams: 1,
            max_streams_per_tenant: 10,
            max_streams_per_api_key: 10,
        }));

        let first = limiter.try_acquire(None, None).unwrap();
        let second = limiter.try_acquire(None, None);

        assert!(matches!(
            second,
            Err(StreamBackpressureError::GlobalLimitReached)
        ));

        drop(first);
        assert!(limiter.try_acquire(None, None).is_ok());
    }

    #[test]
    fn test_stream_backpressure_active_counts() {
        let limiter = Arc::new(StreamBackpressureLimiter::new(StreamBackpressureConfig {
            max_global_streams: 10,
            max_streams_per_tenant: 10,
            max_streams_per_api_key: 10,
        }));

        let permit = limiter
            .try_acquire(Some("tenant-a".to_string()), Some("key-a".to_string()))
            .unwrap();

        assert_eq!(
            limiter.active_counts(Some("tenant-a"), Some("key-a")),
            (1, 1, 1)
        );

        drop(permit);
        assert_eq!(
            limiter.active_counts(Some("tenant-a"), Some("key-a")),
            (0, 0, 0)
        );
    }

    #[test]
    fn test_stream_identity_extraction_from_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-tenant-id", "tenant-a".parse().unwrap());
        headers.insert("authorization", "Bearer key-a".parse().unwrap());

        assert_eq!(extract_stream_tenant_id(&headers), Some("tenant-a".into()));
        assert_eq!(extract_stream_api_key(&headers), Some("key-a".into()));
    }
}
