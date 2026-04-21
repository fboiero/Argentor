//! In-memory LRU response cache for LLM calls with TTL expiration.
//!
//! Caches LLM responses keyed by a hash of the prompt messages and model
//! configuration, avoiding redundant API calls for identical requests.
//!
//! # Main types
//!
//! - [`ResponseCache`] — Thread-safe LRU cache with TTL and metrics.
//! - [`CacheKey`] — Hash-based key derived from prompt content.
//! - [`CacheEntry`] — Cached response with metadata.
//! - [`CacheStats`] — Hit/miss statistics.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// CacheKey
// ---------------------------------------------------------------------------

/// A hash-based cache key derived from prompt content and model configuration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CacheKey(String);

impl CacheKey {
    /// Compute a cache key from the given components.
    ///
    /// Uses FNV-1a hashing over the model name and message contents.
    pub fn compute(model: &str, messages: &[CacheMessage]) -> Self {
        let mut hash: u64 = 14695981039346656037;
        for byte in model.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(1099511628211);
        }
        for msg in messages {
            for byte in msg.role.bytes() {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(1099511628211);
            }
            for byte in msg.content.bytes() {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(1099511628211);
            }
        }
        Self(format!("{hash:016x}"))
    }

    /// Return the key string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Simplified message representation for cache key computation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMessage {
    /// Role of the message sender (e.g. "user", "system").
    pub role: String,
    /// Content of the message.
    pub content: String,
}

impl CacheMessage {
    /// Create a new cache message.
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// CacheEntry
// ---------------------------------------------------------------------------

/// A cached LLM response with metadata.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// The cached response text.
    pub response: String,
    /// When this entry was created.
    pub created_at: Instant,
    /// How many times this entry has been served.
    pub hit_count: u64,
    /// Estimated token count of the cached response.
    pub token_estimate: u64,
    /// The model that produced this response.
    pub model: String,
}

// ---------------------------------------------------------------------------
// CacheStats
// ---------------------------------------------------------------------------

/// Cache performance statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    /// Total number of cache lookups.
    pub total_lookups: u64,
    /// Number of cache hits.
    pub hits: u64,
    /// Number of cache misses.
    pub misses: u64,
    /// Number of entries evicted (LRU or TTL).
    pub evictions: u64,
    /// Current number of entries in the cache.
    pub size: usize,
    /// Maximum capacity of the cache.
    pub capacity: usize,
    /// Hit rate as a percentage.
    pub hit_rate_percent: f64,
    /// Estimated tokens saved by cache hits.
    pub tokens_saved: u64,
}

// ---------------------------------------------------------------------------
// LRU node (doubly-linked via indices)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct LruNode {
    key: CacheKey,
    entry: CacheEntry,
    /// Index of the previous node in the access order (None = head).
    prev: Option<usize>,
    /// Index of the next node in the access order (None = tail).
    next: Option<usize>,
}

// ---------------------------------------------------------------------------
// Result of a get() call — carries data out of the write lock
// ---------------------------------------------------------------------------

enum LruGetResult {
    Hit {
        response: String,
        token_estimate: u64,
    },
    Miss,
    Expired,
}

// ---------------------------------------------------------------------------
// Inner state (no stats — they live on AtomicU64 fields outside the lock)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Inner {
    /// Nodes stored in a Vec for cache-friendly access.
    nodes: Vec<Option<LruNode>>,
    /// Map from CacheKey to node index.
    index: HashMap<CacheKey, usize>,
    /// Free list of reusable indices.
    free_list: Vec<usize>,
    /// Index of the most recently accessed node.
    head: Option<usize>,
    /// Index of the least recently accessed node.
    tail: Option<usize>,
    /// Maximum number of entries.
    capacity: usize,
    /// TTL for cache entries.
    ttl: Duration,
}

impl Inner {
    fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            nodes: Vec::with_capacity(capacity),
            index: HashMap::with_capacity(capacity),
            free_list: Vec::new(),
            head: None,
            tail: None,
            capacity,
            ttl,
        }
    }

    fn len(&self) -> usize {
        self.index.len()
    }

    /// Detach a node from the linked list.
    fn detach(&mut self, idx: usize) {
        if let Some(node) = &self.nodes[idx] {
            let prev = node.prev;
            let next = node.next;

            if let Some(p) = prev {
                if let Some(pnode) = &mut self.nodes[p] {
                    pnode.next = next;
                }
            } else {
                self.head = next;
            }

            if let Some(n) = next {
                if let Some(nnode) = &mut self.nodes[n] {
                    nnode.prev = prev;
                }
            } else {
                self.tail = prev;
            }

            if let Some(node) = &mut self.nodes[idx] {
                node.prev = None;
                node.next = None;
            }
        }
    }

    /// Push a node to the front (most recently used).
    fn push_front(&mut self, idx: usize) {
        if let Some(node) = &mut self.nodes[idx] {
            node.prev = None;
            node.next = self.head;
        }

        if let Some(old_head) = self.head {
            if let Some(node) = &mut self.nodes[old_head] {
                node.prev = Some(idx);
            }
        }

        self.head = Some(idx);

        if self.tail.is_none() {
            self.tail = Some(idx);
        }
    }

    /// Remove the least recently used entry. Returns `true` if an entry was evicted.
    fn evict_lru(&mut self) -> bool {
        if let Some(tail_idx) = self.tail {
            self.detach(tail_idx);
            if let Some(node) = self.nodes[tail_idx].take() {
                self.index.remove(&node.key);
                self.free_list.push(tail_idx);
                return true;
            }
        }
        false
    }

    /// Allocate or reuse a node index.
    fn alloc_index(&mut self) -> usize {
        if let Some(idx) = self.free_list.pop() {
            idx
        } else {
            let idx = self.nodes.len();
            self.nodes.push(None);
            idx
        }
    }

    /// Get a cached response, moving it to front if found and not expired.
    ///
    /// Returns a `LruGetResult` so that stats counters (which live outside the
    /// lock as `AtomicU64`s) can be updated *after* the lock is released.
    fn get(&mut self, key: &CacheKey) -> LruGetResult {
        let idx = match self.index.get(key) {
            Some(&i) => i,
            None => return LruGetResult::Miss,
        };

        // Check TTL
        let expired = self.nodes[idx]
            .as_ref()
            .is_some_and(|n| n.entry.created_at.elapsed() > self.ttl);

        if expired {
            self.detach(idx);
            if let Some(node) = self.nodes[idx].take() {
                self.index.remove(&node.key);
                self.free_list.push(idx);
            }
            return LruGetResult::Expired;
        }

        // Move to front
        self.detach(idx);
        self.push_front(idx);

        // Update hit count and return data
        if let Some(node) = &mut self.nodes[idx] {
            node.entry.hit_count += 1;
            let token_estimate = node.entry.token_estimate;
            return LruGetResult::Hit {
                response: node.entry.response.clone(),
                token_estimate,
            };
        }

        LruGetResult::Miss
    }

    /// Insert a response into the cache. Returns `true` if an LRU eviction occurred.
    fn put(&mut self, key: CacheKey, response: String, model: String, token_estimate: u64) -> bool {
        let mut evicted = false;

        // If already present, update and move to front
        if let Some(&idx) = self.index.get(&key) {
            self.detach(idx);
            if let Some(node) = &mut self.nodes[idx] {
                node.entry.response = response;
                node.entry.created_at = Instant::now();
                node.entry.model = model;
                node.entry.token_estimate = token_estimate;
            }
            self.push_front(idx);
            return false;
        }

        // Evict if at capacity
        if self.len() >= self.capacity {
            evicted = self.evict_lru();
        }

        let idx = self.alloc_index();
        let node = LruNode {
            key: key.clone(),
            entry: CacheEntry {
                response,
                created_at: Instant::now(),
                hit_count: 0,
                token_estimate,
                model,
            },
            prev: None,
            next: None,
        };

        if idx < self.nodes.len() {
            self.nodes[idx] = Some(node);
        } else {
            self.nodes.push(Some(node));
        }

        self.index.insert(key, idx);
        self.push_front(idx);
        evicted
    }
}

// ---------------------------------------------------------------------------
// ResponseCache
// ---------------------------------------------------------------------------

/// Thread-safe in-memory LRU cache for LLM responses.
///
/// Clone is cheap — inner state is behind `Arc<RwLock>` and all stats
/// counters are `Arc<AtomicU64>`, so `stats()` is wait-free (no lock held).
#[derive(Debug, Clone)]
pub struct ResponseCache {
    inner: Arc<RwLock<Inner>>,
    // Stats live outside the lock so stats() is wait-free.
    total_lookups: Arc<AtomicU64>,
    hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
    evictions: Arc<AtomicU64>,
    tokens_saved: Arc<AtomicU64>,
}

impl ResponseCache {
    /// Create a new cache with the given capacity and TTL.
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner::new(capacity, ttl))),
            total_lookups: Arc::new(AtomicU64::new(0)),
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            evictions: Arc::new(AtomicU64::new(0)),
            tokens_saved: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Look up a cached response for the given key.
    ///
    /// Acquires a write lock (LRU promotion on hit is unavoidable) but updates
    /// all stats counters *after* the lock is released via `AtomicU64`.
    pub fn get(&self, key: &CacheKey) -> Option<String> {
        self.total_lookups.fetch_add(1, Ordering::Relaxed);

        let result = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key);

        match result {
            LruGetResult::Hit {
                response,
                token_estimate,
            } => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                self.tokens_saved
                    .fetch_add(token_estimate, Ordering::Relaxed);
                Some(response)
            }
            LruGetResult::Miss => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
            LruGetResult::Expired => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                self.evictions.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Store a response in the cache.
    pub fn put(
        &self,
        key: CacheKey,
        response: impl Into<String>,
        model: impl Into<String>,
        token_estimate: u64,
    ) {
        let evicted = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .put(key, response.into(), model.into(), token_estimate);

        if evicted {
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Get cache performance statistics.
    ///
    /// All counters are read from `AtomicU64` (wait-free).
    /// Only `size` and `capacity` require a short read lock.
    pub fn stats(&self) -> CacheStats {
        let total_lookups = self.total_lookups.load(Ordering::Relaxed);
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let evictions = self.evictions.load(Ordering::Relaxed);
        let tokens_saved = self.tokens_saved.load(Ordering::Relaxed);

        let hit_rate = if total_lookups > 0 {
            (hits as f64 / total_lookups as f64) * 100.0
        } else {
            0.0
        };

        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        CacheStats {
            total_lookups,
            hits,
            misses,
            evictions,
            size: inner.len(),
            capacity: inner.capacity,
            hit_rate_percent: hit_rate,
            tokens_saved,
        }
    }

    /// Clear all cached entries and reset all stats counters.
    pub fn clear(&self) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let capacity = inner.capacity;
        let ttl = inner.ttl;
        *inner = Inner::new(capacity, ttl);
        drop(inner);

        self.total_lookups.store(0, Ordering::Relaxed);
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
        self.tokens_saved.store(0, Ordering::Relaxed);
    }

    /// Get the current number of entries.
    pub fn len(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn cache() -> ResponseCache {
        ResponseCache::new(10, Duration::from_secs(60))
    }

    fn msgs(content: &str) -> Vec<CacheMessage> {
        vec![CacheMessage::new("user", content)]
    }

    // 1. New cache is empty
    #[test]
    fn test_new_cache_empty() {
        let c = cache();
        assert_eq!(c.len(), 0);
        assert!(c.is_empty());
    }

    // 2. Put and get
    #[test]
    fn test_put_get() {
        let c = cache();
        let key = CacheKey::compute("gpt-4", &msgs("hello"));
        c.put(key.clone(), "world", "gpt-4", 10);
        assert_eq!(c.get(&key).unwrap(), "world");
    }

    // 3. Cache miss returns None
    #[test]
    fn test_miss() {
        let c = cache();
        let key = CacheKey::compute("gpt-4", &msgs("nonexistent"));
        assert!(c.get(&key).is_none());
    }

    // 4. Different keys produce different entries
    #[test]
    fn test_different_keys() {
        let c = cache();
        let k1 = CacheKey::compute("gpt-4", &msgs("hello"));
        let k2 = CacheKey::compute("gpt-4", &msgs("goodbye"));
        c.put(k1.clone(), "response1", "gpt-4", 10);
        c.put(k2.clone(), "response2", "gpt-4", 10);
        assert_eq!(c.get(&k1).unwrap(), "response1");
        assert_eq!(c.get(&k2).unwrap(), "response2");
        assert_eq!(c.len(), 2);
    }

    // 5. Same key with different model produces different key
    #[test]
    fn test_model_affects_key() {
        let k1 = CacheKey::compute("gpt-4", &msgs("hello"));
        let k2 = CacheKey::compute("claude-3", &msgs("hello"));
        assert_ne!(k1, k2);
    }

    // 6. LRU eviction when at capacity
    #[test]
    fn test_lru_eviction() {
        let c = ResponseCache::new(3, Duration::from_secs(60));
        for i in 0..4 {
            let key = CacheKey::compute("m", &msgs(&format!("msg-{i}")));
            c.put(key, format!("resp-{i}"), "m", 10);
        }
        assert_eq!(c.len(), 3);
        // First entry (msg-0) should be evicted
        let k0 = CacheKey::compute("m", &msgs("msg-0"));
        assert!(c.get(&k0).is_none());
        // Last entry should still be present
        let k3 = CacheKey::compute("m", &msgs("msg-3"));
        assert!(c.get(&k3).is_some());
    }

    // 7. Accessing an entry moves it to front (prevents eviction)
    #[test]
    fn test_access_prevents_eviction() {
        let c = ResponseCache::new(3, Duration::from_secs(60));
        let k0 = CacheKey::compute("m", &msgs("msg-0"));
        let k1 = CacheKey::compute("m", &msgs("msg-1"));
        let k2 = CacheKey::compute("m", &msgs("msg-2"));

        c.put(k0.clone(), "r0", "m", 10);
        c.put(k1.clone(), "r1", "m", 10);
        c.put(k2.clone(), "r2", "m", 10);

        // Access k0, making it recently used
        c.get(&k0);

        // Insert new entry, should evict k1 (LRU)
        let k3 = CacheKey::compute("m", &msgs("msg-3"));
        c.put(k3, "r3", "m", 10);

        assert!(
            c.get(&k0).is_some(),
            "k0 was accessed recently, should survive"
        );
        assert!(c.get(&k1).is_none(), "k1 should be evicted");
    }

    // 8. TTL expiration
    #[test]
    fn test_ttl_expiration() {
        let c = ResponseCache::new(10, Duration::from_millis(50));
        let key = CacheKey::compute("m", &msgs("hello"));
        c.put(key.clone(), "world", "m", 10);
        assert!(c.get(&key).is_some());

        std::thread::sleep(Duration::from_millis(60));
        assert!(c.get(&key).is_none(), "Entry should be expired");
    }

    // 9. Stats tracking — hits
    #[test]
    fn test_stats_hits() {
        let c = cache();
        let key = CacheKey::compute("m", &msgs("hello"));
        c.put(key.clone(), "world", "m", 50);
        c.get(&key);
        c.get(&key);

        let stats = c.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.total_lookups, 2);
        assert_eq!(stats.tokens_saved, 100); // 50 * 2 hits
    }

    // 10. Stats tracking — misses
    #[test]
    fn test_stats_misses() {
        let c = cache();
        let key = CacheKey::compute("m", &msgs("nope"));
        c.get(&key);
        c.get(&key);

        let stats = c.stats();
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.hits, 0);
    }

    // 11. Hit rate calculation
    #[test]
    fn test_hit_rate() {
        let c = cache();
        let key = CacheKey::compute("m", &msgs("hello"));
        c.put(key.clone(), "world", "m", 10);
        c.get(&key); // hit
        let miss_key = CacheKey::compute("m", &msgs("miss"));
        c.get(&miss_key); // miss

        let stats = c.stats();
        assert!((stats.hit_rate_percent - 50.0).abs() < 0.01);
    }

    // 12. Stats on empty cache
    #[test]
    fn test_stats_empty() {
        let c = cache();
        let stats = c.stats();
        assert_eq!(stats.total_lookups, 0);
        assert_eq!(stats.hit_rate_percent, 0.0);
    }

    // 13. Eviction stats
    #[test]
    fn test_eviction_stats() {
        let c = ResponseCache::new(2, Duration::from_secs(60));
        for i in 0..5 {
            let key = CacheKey::compute("m", &msgs(&format!("msg-{i}")));
            c.put(key, format!("r-{i}"), "m", 10);
        }
        let stats = c.stats();
        assert_eq!(stats.evictions, 3); // 5 puts - 2 capacity = 3 evictions
    }

    // 14. Update existing entry
    #[test]
    fn test_update_existing() {
        let c = cache();
        let key = CacheKey::compute("m", &msgs("hello"));
        c.put(key.clone(), "first", "m", 10);
        c.put(key.clone(), "second", "m", 20);
        assert_eq!(c.get(&key).unwrap(), "second");
        assert_eq!(c.len(), 1);
    }

    // 15. Clear removes all entries and resets stats
    #[test]
    fn test_clear() {
        let c = cache();
        for i in 0..5 {
            let key = CacheKey::compute("m", &msgs(&format!("msg-{i}")));
            c.put(key, format!("r-{i}"), "m", 10);
        }
        assert_eq!(c.len(), 5);
        c.clear();
        assert_eq!(c.len(), 0);
        assert!(c.is_empty());
        let stats = c.stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
        assert_eq!(stats.total_lookups, 0);
    }

    // 16. CacheKey determinism
    #[test]
    fn test_cache_key_deterministic() {
        let k1 = CacheKey::compute("gpt-4", &msgs("hello world"));
        let k2 = CacheKey::compute("gpt-4", &msgs("hello world"));
        assert_eq!(k1, k2);
    }

    // 17. CacheKey with multiple messages
    #[test]
    fn test_cache_key_multi_message() {
        let msgs1 = vec![
            CacheMessage::new("system", "You are helpful"),
            CacheMessage::new("user", "Hello"),
        ];
        let msgs2 = vec![
            CacheMessage::new("system", "You are helpful"),
            CacheMessage::new("user", "Goodbye"),
        ];
        let k1 = CacheKey::compute("m", &msgs1);
        let k2 = CacheKey::compute("m", &msgs2);
        assert_ne!(k1, k2);
    }

    // 18. Clone shares state
    #[test]
    fn test_clone_shares_state() {
        let c1 = cache();
        let c2 = c1.clone();
        let key = CacheKey::compute("m", &msgs("hello"));
        c1.put(key.clone(), "world", "m", 10);
        assert_eq!(c2.get(&key).unwrap(), "world");
    }

    // 19. Stats serializable
    #[test]
    fn test_stats_serializable() {
        let c = cache();
        let key = CacheKey::compute("m", &msgs("hello"));
        c.put(key.clone(), "world", "m", 10);
        c.get(&key);

        let stats = c.stats();
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("\"hits\":1"));
    }

    // 20. Large capacity cache works
    #[test]
    fn test_large_capacity() {
        let c = ResponseCache::new(1000, Duration::from_secs(60));
        for i in 0..500 {
            let key = CacheKey::compute("m", &msgs(&format!("msg-{i}")));
            c.put(key, format!("r-{i}"), "m", 10);
        }
        assert_eq!(c.len(), 500);
    }

    // 21. Empty messages produce valid key
    #[test]
    fn test_empty_messages_key() {
        let key = CacheKey::compute("m", &[]);
        assert!(!key.as_str().is_empty());
    }
}
