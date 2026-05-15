// SPDX-License-Identifier: AGPL-3.0-only
//! `CacheLayer` — wraps any `LlmBackend` with transparent response caching.
//!
//! Before forwarding a prompt to the LLM the layer computes a SHA-256 cache
//! key over `(provider_name, system_prompt, messages, tool_descriptors)`. On
//! a cache hit the stored response is returned immediately; on a miss the
//! underlying backend is called and the response is stored for future hits.
//!
//! # Configuration
//!
//! ```no_run
//! use argentor_agent::response_cache_layer::{CacheConfig, CacheLayer};
//!
//! let config = CacheConfig {
//!     enabled: true,
//!     ttl_secs: 3600,
//!     max_entries: 1000,
//!     cache_tools: false,
//! };
//! ```
//!
//! Tool-calling responses are excluded by default (`cache_tools: false`) because
//! they are non-deterministic: the same prompt can trigger different tool results
//! depending on external state.

use crate::backends::LlmBackend;
use crate::llm::LlmResponse;
use crate::stream::StreamEvent;
use argentor_core::{ArgentorResult, Message};
use argentor_skills::SkillDescriptor;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// CacheConfig
// ---------------------------------------------------------------------------

/// Configuration for [`CacheLayer`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Whether caching is active. When `false` the layer is a pass-through.
    pub enabled: bool,
    /// How long (in seconds) a cached response is considered fresh.
    pub ttl_secs: u64,
    /// Maximum number of entries to keep in memory (LRU eviction when full).
    pub max_entries: usize,
    /// Whether to cache responses that contain tool calls.
    ///
    /// Defaults to `false` — tool-calling responses are typically non-deterministic
    /// because the tool results depend on external state.
    pub cache_tools: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ttl_secs: 3600,
            max_entries: 1000,
            cache_tools: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal entry types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CacheEntry {
    response: LlmResponse,
    inserted_at: Instant,
    /// LRU sequence counter — higher = more recently used.
    last_used: u64,
}

// ---------------------------------------------------------------------------
// Inner (behind the RwLock)
// ---------------------------------------------------------------------------

struct Inner {
    entries: HashMap<String, CacheEntry>,
    lru_heap: BinaryHeap<Reverse<(u64, String)>>,
    /// Monotonically increasing counter for LRU tracking.
    tick: u64,
    capacity: usize,
    ttl: Duration,
}

impl Inner {
    fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity),
            lru_heap: BinaryHeap::with_capacity(capacity),
            tick: 0,
            capacity,
            ttl,
        }
    }

    fn get(&mut self, key: &str) -> Option<LlmResponse> {
        let entry = self.entries.get_mut(key)?;

        if entry.inserted_at.elapsed() > self.ttl {
            self.entries.remove(key);
            return None;
        }

        self.tick += 1;
        entry.last_used = self.tick;
        let response = entry.response.clone();
        self.lru_heap.push(Reverse((self.tick, key.to_string())));
        self.compact_heap_if_needed();
        Some(response)
    }

    fn put(&mut self, key: String, response: LlmResponse) {
        if self.capacity == 0 {
            return;
        }

        // Evict LRU entry if at capacity and key is new.
        if !self.entries.contains_key(&key) && self.entries.len() >= self.capacity {
            self.evict_lru();
        }

        self.tick += 1;
        self.entries.insert(
            key.clone(),
            CacheEntry {
                response,
                inserted_at: Instant::now(),
                last_used: self.tick,
            },
        );
        self.lru_heap.push(Reverse((self.tick, key)));
        self.compact_heap_if_needed();
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn evict_lru(&mut self) {
        while let Some(Reverse((last_used, key))) = self.lru_heap.pop() {
            let is_current = self
                .entries
                .get(&key)
                .is_some_and(|entry| entry.last_used == last_used);
            if is_current {
                self.entries.remove(&key);
                debug!(key = %key, "CacheLayer: evicted LRU entry");
                return;
            }
        }
    }

    fn compact_heap_if_needed(&mut self) {
        if self.entries.is_empty() || self.lru_heap.len() <= self.entries.len().saturating_mul(4) {
            return;
        }

        self.lru_heap = self
            .entries
            .iter()
            .map(|(key, entry)| Reverse((entry.last_used, key.clone())))
            .collect();
    }
}

// ---------------------------------------------------------------------------
// CacheLayer
// ---------------------------------------------------------------------------

/// A transparent caching wrapper around any [`LlmBackend`].
///
/// The layer is cheaply cloneable (inner state behind `Arc<RwLock>`).
pub struct CacheLayer {
    backend: Box<dyn LlmBackend>,
    inner: Arc<RwLock<Inner>>,
    config: CacheConfig,
    hits: Arc<std::sync::atomic::AtomicU64>,
    misses: Arc<std::sync::atomic::AtomicU64>,
}

impl CacheLayer {
    /// Wrap `backend` with caching according to `config`.
    pub fn new(backend: Box<dyn LlmBackend>, config: CacheConfig) -> Self {
        let ttl = Duration::from_secs(config.ttl_secs);
        let capacity = config.max_entries;
        Self {
            backend,
            inner: Arc::new(RwLock::new(Inner::new(capacity, ttl))),
            config,
            hits: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            misses: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Compute a SHA-256 cache key from the backend, prompt, messages, and tool names.
    fn compute_key(
        provider_name: &str,
        system_prompt: Option<&str>,
        messages: &[Message],
        tool_names: &[String],
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"argentor-cache-v1\0");
        hasher.update(provider_name.as_bytes());
        hasher.update(b"\0");
        if let Some(prompt) = system_prompt {
            hasher.update(prompt.as_bytes());
        }
        hasher.update(b"\0messages\0");
        for msg in messages {
            hasher.update(format!("{:?}", msg.role).as_bytes());
            hasher.update(b"\0");
            hasher.update(msg.content.as_bytes());
            hasher.update(b"\0");
        }
        hasher.update(b"\0tools\0");
        for name in tool_names {
            hasher.update(name.as_bytes());
            hasher.update(b"\0");
        }
        format!("{:x}", hasher.finalize())
    }

    /// Return current cache statistics.
    pub fn stats(&self) -> CacheLayerStats {
        let hits = self.hits.load(std::sync::atomic::Ordering::Relaxed);
        let misses = self.misses.load(std::sync::atomic::Ordering::Relaxed);
        let total = hits + misses;
        let hit_rate = if total > 0 {
            (hits as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        let size = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        CacheLayerStats {
            hits,
            misses,
            hit_rate_percent: hit_rate,
            size,
            capacity: self.config.max_entries,
            ttl_secs: self.config.ttl_secs,
        }
    }
}

/// Statistics reported by [`CacheLayer::stats`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheLayerStats {
    /// Number of cache hits (LLM call avoided).
    pub hits: u64,
    /// Number of cache misses (LLM was called).
    pub misses: u64,
    /// Hit rate as a percentage (0–100).
    pub hit_rate_percent: f64,
    /// Current number of entries in cache.
    pub size: usize,
    /// Maximum capacity before LRU eviction triggers.
    pub capacity: usize,
    /// Entry TTL in seconds.
    pub ttl_secs: u64,
}

// ---------------------------------------------------------------------------
// LlmBackend impl
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl LlmBackend for CacheLayer {
    async fn chat(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        if !self.config.enabled {
            return self.backend.chat(system_prompt, messages, tools).await;
        }

        let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();

        // Build key from backend identity, prompt, messages, and tool names.
        let key = Self::compute_key(
            self.backend.provider_name(),
            system_prompt,
            messages,
            &tool_names,
        );

        // Check cache.
        {
            let mut inner = self
                .inner
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(cached) = inner.get(&key) {
                self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                debug!(key = %&key[..8], "CacheLayer: cache hit — skipping LLM call");
                return Ok(cached);
            }
        }

        // Cache miss — call the backend.
        self.misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let response = self.backend.chat(system_prompt, messages, tools).await?;

        // Store in cache only if the response does not contain tool calls,
        // unless the caller explicitly opted into tool-call response caching.
        let has_tool_use = matches!(
            &response,
            LlmResponse::ToolUse { tool_calls, .. } if !tool_calls.is_empty()
        );
        if !has_tool_use || self.config.cache_tools {
            let mut inner = self
                .inner
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.put(key.clone(), response.clone());
            info!(key = %&key[..8], "CacheLayer: stored response in cache");
        }

        Ok(response)
    }

    fn provider_name(&self) -> &str {
        self.backend.provider_name()
    }

    async fn chat_stream(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        tools: &[SkillDescriptor],
    ) -> ArgentorResult<(
        mpsc::Receiver<StreamEvent>,
        JoinHandle<ArgentorResult<LlmResponse>>,
    )> {
        self.backend
            .chat_stream(system_prompt, messages, tools)
            .await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::backends::LlmBackend;
    use crate::llm::LlmResponse;
    use argentor_core::Message;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ---------------------------------------------------------------------------
    // Mock backend — counts calls and returns a fixed response.
    // ---------------------------------------------------------------------------

    struct MockBackend {
        call_count: Arc<AtomicU32>,
        model: String,
        response: LlmResponse,
    }

    impl MockBackend {
        fn new(model: &str) -> (Self, Arc<AtomicU32>) {
            Self::with_response(model, LlmResponse::Done("mocked response".to_string()))
        }

        fn with_response(model: &str, response: LlmResponse) -> (Self, Arc<AtomicU32>) {
            let counter = Arc::new(AtomicU32::new(0));
            (
                Self {
                    call_count: counter.clone(),
                    model: model.to_string(),
                    response,
                },
                counter,
            )
        }
    }

    #[async_trait::async_trait]
    impl LlmBackend for MockBackend {
        async fn chat(
            &self,
            _system_prompt: Option<&str>,
            _messages: &[Message],
            _tools: &[SkillDescriptor],
        ) -> ArgentorResult<LlmResponse> {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            Ok(self.response.clone())
        }

        fn provider_name(&self) -> &str {
            &self.model
        }

        async fn chat_stream(
            &self,
            _system_prompt: Option<&str>,
            _messages: &[Message],
            _tools: &[SkillDescriptor],
        ) -> ArgentorResult<(
            mpsc::Receiver<StreamEvent>,
            JoinHandle<ArgentorResult<LlmResponse>>,
        )> {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            let (_tx, rx) = mpsc::channel(1);
            let handle =
                tokio::spawn(async { Ok(LlmResponse::Done("mocked response".to_string())) });
            Ok((rx, handle))
        }
    }

    fn msgs() -> Vec<Message> {
        vec![Message::user("hello world", uuid::Uuid::new_v4())]
    }

    fn tool_descriptor(name: &str) -> SkillDescriptor {
        SkillDescriptor {
            name: name.to_string(),
            description: "test tool".to_string(),
            parameters_schema: serde_json::json!({ "type": "object" }),
            required_capabilities: vec![],
            requires_approval: false,
        }
    }

    fn config(enabled: bool) -> CacheConfig {
        CacheConfig {
            enabled,
            ttl_secs: 60,
            max_entries: 10,
            cache_tools: false,
        }
    }

    // 1. Cache disabled — always calls backend.
    #[tokio::test]
    async fn test_cache_disabled_passes_through() {
        let (backend, counter) = MockBackend::new("test");
        let layer = CacheLayer::new(Box::new(backend), config(false));

        for _ in 0..3 {
            layer.chat(Some("sys"), &msgs(), &[]).await.unwrap();
        }

        assert_eq!(counter.load(Ordering::Relaxed), 3);
    }

    // 2. Cache hit skips LLM call.
    #[tokio::test]
    async fn test_cache_hit_skips_llm() {
        let (backend, counter) = MockBackend::new("test");
        let layer = CacheLayer::new(Box::new(backend), config(true));
        let messages = msgs();

        // First call — cache miss.
        layer.chat(Some("sys"), &messages, &[]).await.unwrap();
        assert_eq!(counter.load(Ordering::Relaxed), 1);

        // Second call — cache hit, backend NOT called again.
        layer.chat(Some("sys"), &messages, &[]).await.unwrap();
        assert_eq!(counter.load(Ordering::Relaxed), 1);

        let stats = layer.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    // 3. Different messages produce different cache keys.
    #[tokio::test]
    async fn test_different_messages_different_keys() {
        let (backend, counter) = MockBackend::new("test");
        let layer = CacheLayer::new(Box::new(backend), config(true));

        let m1 = vec![Message::user("hello", uuid::Uuid::new_v4())];
        let m2 = vec![Message::user("goodbye", uuid::Uuid::new_v4())];

        layer.chat(Some("sys"), &m1, &[]).await.unwrap();
        layer.chat(Some("sys"), &m2, &[]).await.unwrap();

        // Both are cache misses — two backend calls.
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    // 4. TTL expiry causes cache miss after expiration.
    #[tokio::test]
    async fn test_ttl_expiry() {
        let (backend, counter) = MockBackend::new("test");
        let cfg = CacheConfig {
            enabled: true,
            ttl_secs: 0, // effectively instant expiry
            max_entries: 10,
            cache_tools: false,
        };
        let layer = CacheLayer::new(Box::new(backend), cfg);
        let messages = msgs();

        layer.chat(Some("sys"), &messages, &[]).await.unwrap();
        // Sleep briefly so the entry expires.
        tokio::time::sleep(Duration::from_millis(5)).await;
        layer.chat(Some("sys"), &messages, &[]).await.unwrap();

        // Both calls should have hit the backend.
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    // 5. LRU eviction when at capacity.
    #[tokio::test]
    async fn test_lru_eviction() {
        let (backend, _counter) = MockBackend::new("test");
        let cfg = CacheConfig {
            enabled: true,
            ttl_secs: 3600,
            max_entries: 3,
            cache_tools: false,
        };
        let layer = CacheLayer::new(Box::new(backend), cfg);

        // Fill the cache.
        for i in 0..3u64 {
            let m = vec![Message::user(format!("msg-{i}"), uuid::Uuid::new_v4())];
            layer.chat(Some("sys"), &m, &[]).await.unwrap();
        }
        assert_eq!(layer.stats().size, 3);

        // One more entry — should evict the LRU.
        let m_extra = vec![Message::user("extra", uuid::Uuid::new_v4())];
        layer.chat(Some("sys"), &m_extra, &[]).await.unwrap();
        assert_eq!(layer.stats().size, 3);
    }

    // 6. Text responses are cacheable even when tools are available.
    #[tokio::test]
    async fn test_text_response_with_available_tools_is_cached() {
        let (backend, counter) = MockBackend::new("test");
        let layer = CacheLayer::new(Box::new(backend), config(true));
        let messages = msgs();
        let tools = vec![tool_descriptor("lookup")];

        layer.chat(Some("sys"), &messages, &tools).await.unwrap();
        layer.chat(Some("sys"), &messages, &tools).await.unwrap();

        assert_eq!(counter.load(Ordering::Relaxed), 1);
        assert_eq!(layer.stats().hits, 1);
    }

    // 7. ToolUse responses are skipped unless explicitly enabled.
    #[tokio::test]
    async fn test_tool_use_response_skips_cache_by_default() {
        let response = LlmResponse::ToolUse {
            content: Some("calling tool".to_string()),
            tool_calls: vec![argentor_core::ToolCall {
                id: "call-1".to_string(),
                name: "lookup".to_string(),
                arguments: serde_json::json!({}),
            }],
        };
        let (backend, counter) = MockBackend::with_response("test", response);
        let layer = CacheLayer::new(Box::new(backend), config(true));
        let messages = msgs();
        let tools = vec![tool_descriptor("lookup")];

        layer.chat(Some("sys"), &messages, &tools).await.unwrap();
        layer.chat(Some("sys"), &messages, &tools).await.unwrap();

        assert_eq!(counter.load(Ordering::Relaxed), 2);
        assert_eq!(layer.stats().size, 0);
    }

    // 8. Zero capacity is a valid "track misses but store nothing" config.
    #[tokio::test]
    async fn test_zero_capacity_does_not_store_entries() {
        let (backend, counter) = MockBackend::new("test");
        let cfg = CacheConfig {
            enabled: true,
            ttl_secs: 3600,
            max_entries: 0,
            cache_tools: false,
        };
        let layer = CacheLayer::new(Box::new(backend), cfg);
        let messages = msgs();

        layer.chat(Some("sys"), &messages, &[]).await.unwrap();
        layer.chat(Some("sys"), &messages, &[]).await.unwrap();

        assert_eq!(counter.load(Ordering::Relaxed), 2);
        assert_eq!(layer.stats().size, 0);
    }

    // 9. Stats serializable.
    #[tokio::test]
    async fn test_stats_serializable() {
        let (backend, _) = MockBackend::new("test");
        let layer = CacheLayer::new(Box::new(backend), config(true));
        let stats = layer.stats();
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("hit_rate_percent"));
    }

    // 10. provider_name delegates to inner backend.
    #[test]
    fn test_provider_name_delegates() {
        let (backend, _) = MockBackend::new("my-model");
        let layer = CacheLayer::new(Box::new(backend), config(false));
        assert_eq!(layer.provider_name(), "my-model");
    }
}
