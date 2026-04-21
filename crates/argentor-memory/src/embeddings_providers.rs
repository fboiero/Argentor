//! Multiple embedding provider backends implementing [`EmbeddingProvider`].
//!
//! Includes API-backed providers ([`OpenAiEmbeddingProvider`],
//! [`CohereEmbeddingProvider`], [`VoyageEmbeddingProvider`]) that call their
//! respective HTTP APIs to compute real embeddings.
//!
//! # Feature flag: `http-embeddings`
//!
//! The actual HTTP calls require the **`http-embeddings`** Cargo feature, which
//! pulls in `reqwest`. When the feature is **disabled** (the default), calling
//! `embed()` on any API-backed provider returns a descriptive error suggesting
//! the user either enable the feature or use [`LocalEmbedding`].
//!
//! ```toml
//! # Enable real HTTP embedding calls:
//! argentor-memory = { version = "0.1", features = ["http-embeddings"] }
//! ```
//!
//! For embeddings that work out of the box without external dependencies, use
//! the [`LocalEmbedding`] provider (deterministic, hash-based, zero API keys).
//!
//! Also provides fully functional utilities:
//! [`CachedEmbeddingProvider`], [`BatchEmbeddingProvider`],
//! [`EmbeddingProviderFactory`], and [`EmbeddingConfig`].

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use argentor_core::{ArgentorError, ArgentorResult};

use crate::embedding::{EmbeddingProvider, LocalEmbedding};

// ---------------------------------------------------------------------------
// FNV-1a hash (same algorithm as embedding.rs, re-implemented here to avoid
// depending on a private function).
// ---------------------------------------------------------------------------

fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 14695981039346656037;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

// ===========================================================================
// API request/response structures
// ===========================================================================

/// OpenAI embeddings API request body.
#[derive(Debug, Serialize)]
pub struct OpenAiEmbeddingRequest {
    /// Model identifier (e.g., `"text-embedding-3-small"`).
    pub model: String,
    /// Texts to embed.
    pub input: Vec<String>,
}

/// A single embedding object from the OpenAI response.
#[derive(Debug, Deserialize)]
pub struct OpenAiEmbeddingObject {
    /// The embedding vector.
    pub embedding: Vec<f32>,
    /// Position of this embedding in the input batch.
    pub index: usize,
}

/// OpenAI embeddings API response body.
#[derive(Debug, Deserialize)]
pub struct OpenAiEmbeddingResponse {
    /// Embedding results, one per input text.
    pub data: Vec<OpenAiEmbeddingObject>,
    /// Model that produced the embeddings.
    pub model: String,
}

/// Cohere embed API request body (v2).
#[derive(Debug, Serialize)]
pub struct CohereEmbedRequest {
    /// Model identifier (e.g., `"embed-english-v3.0"`).
    pub model: String,
    /// Texts to embed.
    pub texts: Vec<String>,
    /// Input type hint (e.g., `"search_document"`, `"search_query"`).
    pub input_type: String,
    /// Which embedding types to return.
    pub embedding_types: Vec<String>,
}

/// Cohere embed API v2 response body.
#[derive(Debug, Deserialize)]
pub struct CohereEmbedResponse {
    /// Embedding vectors keyed by type. We request `"float"`.
    pub embeddings: CohereEmbeddingsMap,
}

/// Container for different embedding type outputs from Cohere.
#[derive(Debug, Deserialize)]
pub struct CohereEmbeddingsMap {
    /// Float embeddings, one vector per input text.
    #[serde(default)]
    pub float: Vec<Vec<f32>>,
}

/// Voyage AI embeddings API request body.
#[derive(Debug, Serialize)]
pub struct VoyageEmbeddingRequest {
    /// Model identifier.
    pub model: String,
    /// Texts to embed.
    pub input: Vec<String>,
}

/// A single embedding object from the Voyage response.
#[derive(Debug, Deserialize)]
pub struct VoyageEmbeddingObject {
    /// The embedding vector.
    pub embedding: Vec<f32>,
    /// Position of this embedding in the input batch.
    pub index: usize,
}

/// Voyage AI embeddings API response body.
#[derive(Debug, Deserialize)]
pub struct VoyageEmbeddingResponse {
    /// Embedding results, one per input text.
    pub data: Vec<VoyageEmbeddingObject>,
}

// ===========================================================================
// Response parsing helpers (testable without HTTP)
// ===========================================================================

/// Parse an OpenAI embedding response JSON into a single embedding vector.
///
/// Expects the standard OpenAI response shape with `data[0].embedding`.
/// Returns `Err` if the response is missing required fields.
pub fn parse_openai_embedding_response(json: &serde_json::Value) -> ArgentorResult<Vec<f32>> {
    let response: OpenAiEmbeddingResponse = serde_json::from_value(json.clone())
        .map_err(|e| ArgentorError::Agent(format!("Failed to parse OpenAI response: {e}")))?;
    response
        .data
        .into_iter()
        .next()
        .map(|obj| obj.embedding)
        .ok_or_else(|| {
            ArgentorError::Agent("OpenAI response contains no embedding data".to_string())
        })
}

/// Parse a Cohere v2 embed response JSON into a single embedding vector.
///
/// Expects the v2 shape: `embeddings.float[0]`.
/// Returns `Err` if the response is missing required fields.
pub fn parse_cohere_embedding_response(json: &serde_json::Value) -> ArgentorResult<Vec<f32>> {
    let response: CohereEmbedResponse = serde_json::from_value(json.clone())
        .map_err(|e| ArgentorError::Agent(format!("Failed to parse Cohere response: {e}")))?;
    response.embeddings.float.into_iter().next().ok_or_else(|| {
        ArgentorError::Agent("Cohere response contains no float embeddings".to_string())
    })
}

/// Parse a Voyage AI embedding response JSON into a single embedding vector.
///
/// Expects the standard Voyage shape: `data[0].embedding`.
/// Returns `Err` if the response is missing required fields.
pub fn parse_voyage_embedding_response(json: &serde_json::Value) -> ArgentorResult<Vec<f32>> {
    let response: VoyageEmbeddingResponse = serde_json::from_value(json.clone())
        .map_err(|e| ArgentorError::Agent(format!("Failed to parse Voyage response: {e}")))?;
    response
        .data
        .into_iter()
        .next()
        .map(|obj| obj.embedding)
        .ok_or_else(|| {
            ArgentorError::Agent("Voyage response contains no embedding data".to_string())
        })
}

// ===========================================================================
// 1. OpenAiEmbeddingProvider
// ===========================================================================

/// Embedding provider backed by the OpenAI embeddings API.
///
/// Stores the API key, model, and dimension configuration. Calling [`embed()`]
/// performs a real HTTP request when the `http-embeddings` feature is enabled.
/// Without the feature, it returns an error. For local/offline embeddings, use
/// [`LocalEmbedding`].
pub struct OpenAiEmbeddingProvider {
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    api_key: String,
    model: String,
    dimensions: usize,
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    base_url: String,
}

impl OpenAiEmbeddingProvider {
    /// Create a new OpenAI embedding provider.
    ///
    /// `model` defaults to `"text-embedding-3-small"` when `None`.
    pub fn new(api_key: impl Into<String>, model: Option<String>) -> Self {
        let model = model.unwrap_or_else(|| "text-embedding-3-small".to_string());
        let dimensions = Self::default_dimensions(&model);
        Self {
            api_key: api_key.into(),
            model,
            dimensions,
            base_url: "https://api.openai.com/v1/embeddings".to_string(),
        }
    }

    /// Create with a custom base URL (e.g. Azure OpenAI endpoint).
    pub fn with_base_url(
        api_key: impl Into<String>,
        model: Option<String>,
        base_url: impl Into<String>,
    ) -> Self {
        let model = model.unwrap_or_else(|| "text-embedding-3-small".to_string());
        let dimensions = Self::default_dimensions(&model);
        Self {
            api_key: api_key.into(),
            model,
            dimensions,
            base_url: base_url.into(),
        }
    }

    /// Override the output dimension count.
    pub fn with_dimensions(mut self, dimensions: usize) -> Self {
        self.dimensions = dimensions;
        self
    }

    fn default_dimensions(model: &str) -> usize {
        match model {
            "text-embedding-3-large" => 3072,
            "text-embedding-3-small" => 1536,
            "text-embedding-ada-002" => 1536,
            _ => 1536,
        }
    }

    /// Returns the model name this provider is configured with.
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    #[cfg(feature = "http-embeddings")]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        let client = reqwest::Client::new();
        let response = client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&serde_json::json!({
                "model": self.model,
                "input": text,
            }))
            .send()
            .await
            .map_err(|e| ArgentorError::Http(format!("OpenAI embedding request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ArgentorError::Http(format!(
                "OpenAI API error {status}: {body}"
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            ArgentorError::Http(format!("Failed to read OpenAI response body: {e}"))
        })?;

        parse_openai_embedding_response(&json)
    }

    #[cfg(not(feature = "http-embeddings"))]
    async fn embed(&self, _text: &str) -> ArgentorResult<Vec<f32>> {
        Err(ArgentorError::Http(
            "HTTP embeddings not enabled. Enable the 'http-embeddings' feature flag \
             or use LocalEmbedding for offline embeddings."
                .to_string(),
        ))
    }

    fn dimension(&self) -> usize {
        self.dimensions
    }
}

// ===========================================================================
// 2. CohereEmbeddingProvider
// ===========================================================================

/// Embedding provider backed by the Cohere embed API (v2).
///
/// Stores the API key, model, and dimension configuration. Calling [`embed()`]
/// performs a real HTTP request when the `http-embeddings` feature is enabled.
/// Without the feature, it returns an error. For local/offline embeddings, use
/// [`LocalEmbedding`].
pub struct CohereEmbeddingProvider {
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    api_key: String,
    model: String,
    dimensions: usize,
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    input_type: String,
}

impl CohereEmbeddingProvider {
    /// Create a new Cohere embedding provider.
    ///
    /// `model` defaults to `"embed-english-v3.0"`.
    pub fn new(api_key: impl Into<String>, model: Option<String>) -> Self {
        let model = model.unwrap_or_else(|| "embed-english-v3.0".to_string());
        let dimensions = Self::default_dimensions(&model);
        Self {
            api_key: api_key.into(),
            model,
            dimensions,
            input_type: "search_document".to_string(),
        }
    }

    /// Set the input type (`"search_document"` for indexing, `"search_query"` for querying).
    pub fn with_input_type(mut self, input_type: impl Into<String>) -> Self {
        self.input_type = input_type.into();
        self
    }

    /// Override the output dimension count.
    pub fn with_dimensions(mut self, dimensions: usize) -> Self {
        self.dimensions = dimensions;
        self
    }

    fn default_dimensions(model: &str) -> usize {
        match model {
            "embed-english-v3.0" | "embed-multilingual-v3.0" => 1024,
            "embed-english-light-v3.0" | "embed-multilingual-light-v3.0" => 384,
            _ => 1024,
        }
    }

    /// Returns the model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the current input type.
    pub fn input_type(&self) -> &str {
        &self.input_type
    }
}

#[async_trait]
impl EmbeddingProvider for CohereEmbeddingProvider {
    #[cfg(feature = "http-embeddings")]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        let client = reqwest::Client::new();
        let response = client
            .post("https://api.cohere.com/v2/embed")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&serde_json::json!({
                "model": self.model,
                "texts": [text],
                "input_type": self.input_type,
                "embedding_types": ["float"],
            }))
            .send()
            .await
            .map_err(|e| ArgentorError::Http(format!("Cohere embedding request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ArgentorError::Http(format!(
                "Cohere API error {status}: {body}"
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            ArgentorError::Http(format!("Failed to read Cohere response body: {e}"))
        })?;

        parse_cohere_embedding_response(&json)
    }

    #[cfg(not(feature = "http-embeddings"))]
    async fn embed(&self, _text: &str) -> ArgentorResult<Vec<f32>> {
        Err(ArgentorError::Http(
            "HTTP embeddings not enabled. Enable the 'http-embeddings' feature flag \
             or use LocalEmbedding for offline embeddings."
                .to_string(),
        ))
    }

    fn dimension(&self) -> usize {
        self.dimensions
    }
}

// ===========================================================================
// 3. VoyageEmbeddingProvider
// ===========================================================================

/// Embedding provider backed by the Voyage AI embeddings API.
///
/// Stores the API key, model, and dimension configuration. Calling [`embed()`]
/// performs a real HTTP request when the `http-embeddings` feature is enabled.
/// Without the feature, it returns an error. For local/offline embeddings, use
/// [`LocalEmbedding`].
pub struct VoyageEmbeddingProvider {
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    api_key: String,
    model: String,
    dimensions: usize,
}

impl VoyageEmbeddingProvider {
    /// Create a new Voyage embedding provider.
    ///
    /// `model` defaults to `"voyage-2"`.
    pub fn new(api_key: impl Into<String>, model: Option<String>) -> Self {
        let model = model.unwrap_or_else(|| "voyage-2".to_string());
        let dimensions = Self::default_dimensions(&model);
        Self {
            api_key: api_key.into(),
            model,
            dimensions,
        }
    }

    /// Override the output dimension count.
    pub fn with_dimensions(mut self, dimensions: usize) -> Self {
        self.dimensions = dimensions;
        self
    }

    fn default_dimensions(model: &str) -> usize {
        match model {
            "voyage-2" | "voyage-large-2" => 1024,
            "voyage-lite-02-instruct" => 1024,
            "voyage-3" => 1024,
            "voyage-code-2" => 1536,
            _ => 1024,
        }
    }

    /// Returns the model name.
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[async_trait]
impl EmbeddingProvider for VoyageEmbeddingProvider {
    #[cfg(feature = "http-embeddings")]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        let client = reqwest::Client::new();
        let response = client
            .post("https://api.voyageai.com/v1/embeddings")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&serde_json::json!({
                "model": self.model,
                "input": [text],
            }))
            .send()
            .await
            .map_err(|e| ArgentorError::Http(format!("Voyage embedding request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ArgentorError::Http(format!(
                "Voyage API error {status}: {body}"
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            ArgentorError::Http(format!("Failed to read Voyage response body: {e}"))
        })?;

        parse_voyage_embedding_response(&json)
    }

    #[cfg(not(feature = "http-embeddings"))]
    async fn embed(&self, _text: &str) -> ArgentorResult<Vec<f32>> {
        Err(ArgentorError::Http(
            "HTTP embeddings not enabled. Enable the 'http-embeddings' feature flag \
             or use LocalEmbedding for offline embeddings."
                .to_string(),
        ))
    }

    fn dimension(&self) -> usize {
        self.dimensions
    }
}

// ===========================================================================
// 4. CachedEmbeddingProvider
// ===========================================================================

/// Statistics about cache usage.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of cache hits.
    pub hits: u64,
    /// Number of cache misses.
    pub misses: u64,
    /// Current number of entries in the cache.
    pub size: usize,
}

/// Wraps any [`EmbeddingProvider`] with a thread-safe in-memory LRU-ish cache.
///
/// Embeddings are cached by FNV-1a hash of the input text. When the cache
/// exceeds `max_cache_size`, the oldest entry (by insertion order) is evicted.
pub struct CachedEmbeddingProvider {
    inner: Arc<dyn EmbeddingProvider>,
    cache: Arc<RwLock<HashMap<u64, Vec<f32>>>>,
    max_cache_size: usize,
    stats: Arc<RwLock<CacheStats>>,
}

impl CachedEmbeddingProvider {
    /// Wrap an existing provider with caching.
    pub fn new(inner: Arc<dyn EmbeddingProvider>, max_cache_size: usize) -> Self {
        Self {
            inner,
            cache: Arc::new(RwLock::new(HashMap::new())),
            max_cache_size,
            stats: Arc::new(RwLock::new(CacheStats::default())),
        }
    }

    /// Returns current cache statistics.
    pub async fn cache_stats(&self) -> CacheStats {
        self.stats.read().await.clone()
    }

    /// Clears the cache and resets statistics.
    pub async fn clear(&self) {
        self.cache.write().await.clear();
        let mut stats = self.stats.write().await;
        stats.size = 0;
    }

    fn text_hash(text: &str) -> u64 {
        fnv1a_hash(text.as_bytes())
    }
}

#[async_trait]
impl EmbeddingProvider for CachedEmbeddingProvider {
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        let key = Self::text_hash(text);

        // Check cache (read lock).
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&key) {
                let mut stats = self.stats.write().await;
                stats.hits += 1;
                return Ok(cached.clone());
            }
        }

        // Cache miss — compute embedding.
        let embedding = self.inner.embed(text).await?;

        // Insert into cache (write lock).
        {
            let mut cache = self.cache.write().await;

            // Evict if at capacity.
            if cache.len() >= self.max_cache_size {
                // Remove an arbitrary entry (HashMap iteration order is random,
                // which acts as a simple eviction strategy).
                if let Some(&evict_key) = cache.keys().next() {
                    cache.remove(&evict_key);
                }
            }

            cache.insert(key, embedding.clone());

            let mut stats = self.stats.write().await;
            stats.misses += 1;
            stats.size = cache.len();
        }

        Ok(embedding)
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }
}

// ===========================================================================
// 5. BatchEmbeddingProvider
// ===========================================================================

/// Wraps any [`EmbeddingProvider`] to expose a convenience batch method.
///
/// Delegates to the inner provider's `embed_batch` (which by default calls
/// `embed` sequentially). Providers that support native batching can override
/// `embed_batch` on the trait for better performance.
pub struct BatchEmbeddingProvider {
    inner: Arc<dyn EmbeddingProvider>,
}

impl BatchEmbeddingProvider {
    /// Wrap an existing provider for batch operations.
    pub fn new(inner: Arc<dyn EmbeddingProvider>) -> Self {
        Self { inner }
    }

    /// Embed multiple texts, returning one vector per input.
    pub async fn embed_batch(&self, texts: &[&str]) -> ArgentorResult<Vec<Vec<f32>>> {
        self.inner.embed_batch(texts).await
    }
}

#[async_trait]
impl EmbeddingProvider for BatchEmbeddingProvider {
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        self.inner.embed(text).await
    }

    async fn embed_batch(&self, texts: &[&str]) -> ArgentorResult<Vec<Vec<f32>>> {
        self.inner.embed_batch(texts).await
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }
}

// ===========================================================================
// 6. EmbeddingProviderFactory
// ===========================================================================

/// Factory that creates [`EmbeddingProvider`] instances by name.
pub struct EmbeddingProviderFactory;

impl EmbeddingProviderFactory {
    /// Create an embedding provider from its string name.
    ///
    /// Supported names: `"openai"`, `"cohere"`, `"voyage"`, `"local"`.
    pub fn create(
        provider_name: &str,
        api_key: impl Into<String>,
        model: Option<String>,
    ) -> ArgentorResult<Box<dyn EmbeddingProvider>> {
        let api_key = api_key.into();
        match provider_name {
            "openai" => Ok(Box::new(OpenAiEmbeddingProvider::new(api_key, model))),
            "cohere" => Ok(Box::new(CohereEmbeddingProvider::new(api_key, model))),
            "voyage" => Ok(Box::new(VoyageEmbeddingProvider::new(api_key, model))),
            "local" => {
                let dim = model
                    .as_deref()
                    .and_then(|m| m.parse::<usize>().ok())
                    .unwrap_or(256);
                Ok(Box::new(LocalEmbedding::new(dim)))
            }
            other => Err(ArgentorError::Config(format!(
                "Unknown embedding provider: {other}"
            ))),
        }
    }

    /// List all supported provider names.
    pub fn available_providers() -> &'static [&'static str] {
        &["openai", "cohere", "voyage", "local"]
    }
}

// ===========================================================================
// 7. EmbeddingConfig
// ===========================================================================

/// Serializable configuration for constructing an embedding provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Provider name (`"openai"`, `"cohere"`, `"voyage"`, `"local"`).
    pub provider: String,
    /// API key (ignored for `"local"`).
    #[serde(default)]
    pub api_key: String,
    /// Model name override.
    #[serde(default)]
    pub model: Option<String>,
    /// Override for output dimensions.
    #[serde(default)]
    pub dimensions: Option<usize>,
    /// Custom API base URL (e.g. Azure OpenAI).
    #[serde(default)]
    pub base_url: Option<String>,
    /// If set, wraps the provider with a [`CachedEmbeddingProvider`].
    #[serde(default)]
    pub cache_size: Option<usize>,
}

impl EmbeddingConfig {
    /// Build an [`EmbeddingProvider`] from this configuration.
    ///
    /// Returns an `Arc`-wrapped provider, optionally wrapped in a cache layer.
    pub fn build(&self) -> ArgentorResult<Arc<dyn EmbeddingProvider>> {
        let mut provider: Box<dyn EmbeddingProvider> = match self.provider.as_str() {
            "openai" => {
                let mut p = if let Some(ref url) = self.base_url {
                    OpenAiEmbeddingProvider::with_base_url(&self.api_key, self.model.clone(), url)
                } else {
                    OpenAiEmbeddingProvider::new(&self.api_key, self.model.clone())
                };
                if let Some(dim) = self.dimensions {
                    p = p.with_dimensions(dim);
                }
                Box::new(p)
            }
            "cohere" => {
                let mut p = CohereEmbeddingProvider::new(&self.api_key, self.model.clone());
                if let Some(dim) = self.dimensions {
                    p = p.with_dimensions(dim);
                }
                Box::new(p)
            }
            "voyage" => {
                let mut p = VoyageEmbeddingProvider::new(&self.api_key, self.model.clone());
                if let Some(dim) = self.dimensions {
                    p = p.with_dimensions(dim);
                }
                Box::new(p)
            }
            "local" => {
                let dim = self.dimensions.unwrap_or(256);
                Box::new(LocalEmbedding::new(dim))
            }
            other => {
                return Err(ArgentorError::Config(format!(
                    "Unknown embedding provider: {other}"
                )));
            }
        };

        // Wrap with dimensions override if the provider itself doesn't support
        // it natively (already handled above for each provider).
        let _ = &mut provider; // suppress unused-mut if branch is empty

        let arc: Arc<dyn EmbeddingProvider> = Arc::from(provider);

        // Optionally wrap with caching.
        if let Some(cache_size) = self.cache_size {
            Ok(Arc::new(CachedEmbeddingProvider::new(arc, cache_size)))
        } else {
            Ok(arc)
        }
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: "local".to_string(),
            api_key: String::new(),
            model: None,
            dimensions: None,
            base_url: None,
            cache_size: None,
        }
    }
}

// ===========================================================================
// Shared helpers for new providers (stub + payload builders)
// ===========================================================================

/// Build a deterministic stub embedding from the input text.
///
/// Used by all new providers when the `http-embeddings` feature is disabled,
/// so tests and offline usage still get a usable L2-normalized vector.
/// Also exercised by unit tests even when the HTTP feature is on.
#[cfg_attr(all(feature = "http-embeddings", not(test)), allow(dead_code))]
fn stub_embedding(text: &str, dimensions: usize) -> Vec<f32> {
    let dim = dimensions.max(1);
    let mut v = vec![0.0f32; dim];
    for (i, b) in text.bytes().enumerate() {
        v[i % dim] += (b as f32) / 255.0;
    }
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

// ===========================================================================
// 8. JinaEmbeddingProvider
// ===========================================================================

/// Embedding provider backed by the Jina AI embeddings API.
///
/// Default model: `jina-embeddings-v3` (1024 dims). Also supports multimodal
/// models such as `jina-clip-v2`. Calling [`embed()`] performs a real HTTP
/// request when the `http-embeddings` feature is enabled. Without the feature,
/// returns a deterministic stub vector (useful for offline tests).
pub struct JinaEmbeddingProvider {
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    api_key: String,
    model: String,
    dimensions: usize,
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    base_url: String,
}

impl JinaEmbeddingProvider {
    /// Create a new Jina provider with the default model (`jina-embeddings-v3`).
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_model(api_key, "jina-embeddings-v3", 1024)
    }

    /// Create with an explicit model and dimension override.
    pub fn with_model(
        api_key: impl Into<String>,
        model: impl Into<String>,
        dimensions: usize,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            dimensions,
            base_url: "https://api.jina.ai/v1/embeddings".to_string(),
        }
    }

    /// Override the API base URL.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Returns the configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Build the request payload for the Jina embeddings API.
    pub fn build_payload(&self, texts: &[String]) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "input": texts,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for JinaEmbeddingProvider {
    #[cfg(feature = "http-embeddings")]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        let client = reqwest::Client::new();
        let payload = self.build_payload(&[text.to_string()]);
        let response = client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgentorError::Http(format!("Jina embedding request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ArgentorError::Http(format!(
                "Jina API error {status}: {body}"
            )));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ArgentorError::Http(format!("Failed to read Jina response body: {e}")))?;

        // Jina follows the OpenAI-compatible `data[].embedding` shape.
        parse_openai_embedding_response(&json)
    }

    #[cfg(not(feature = "http-embeddings"))]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        Ok(stub_embedding(text, self.dimensions))
    }

    fn dimension(&self) -> usize {
        self.dimensions
    }
}

// ===========================================================================
// 9. MistralEmbedProvider
// ===========================================================================

/// Embedding provider backed by the Mistral AI embeddings API.
///
/// Default model: `mistral-embed` (1024 dims). Mistral's embedding endpoint
/// follows an OpenAI-compatible request/response shape.
pub struct MistralEmbedProvider {
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    api_key: String,
    model: String,
    dimensions: usize,
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    base_url: String,
}

impl MistralEmbedProvider {
    /// Create a new Mistral provider with the default model (`mistral-embed`).
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_model(api_key, "mistral-embed", 1024)
    }

    /// Create with an explicit model and dimension override.
    pub fn with_model(
        api_key: impl Into<String>,
        model: impl Into<String>,
        dimensions: usize,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            dimensions,
            base_url: "https://api.mistral.ai/v1/embeddings".to_string(),
        }
    }

    /// Override the API base URL.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Returns the configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Build the request payload for the Mistral embeddings API.
    pub fn build_payload(&self, texts: &[String]) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "input": texts,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for MistralEmbedProvider {
    #[cfg(feature = "http-embeddings")]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        let client = reqwest::Client::new();
        let payload = self.build_payload(&[text.to_string()]);
        let response = client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgentorError::Http(format!("Mistral embedding request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ArgentorError::Http(format!(
                "Mistral API error {status}: {body}"
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            ArgentorError::Http(format!("Failed to read Mistral response body: {e}"))
        })?;

        parse_openai_embedding_response(&json)
    }

    #[cfg(not(feature = "http-embeddings"))]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        Ok(stub_embedding(text, self.dimensions))
    }

    fn dimension(&self) -> usize {
        self.dimensions
    }
}

// ===========================================================================
// 10. NomicEmbedProvider
// ===========================================================================

/// Embedding provider backed by the Nomic Atlas embeddings API.
///
/// Default model: `nomic-embed-text-v1.5` (768 dims). The Nomic endpoint
/// accepts an array of texts under the `texts` key and responds with
/// `{ "embeddings": [ [...], ... ] }`.
pub struct NomicEmbedProvider {
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    api_key: String,
    model: String,
    dimensions: usize,
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    base_url: String,
    task_type: String,
}

impl NomicEmbedProvider {
    /// Create a new Nomic provider with the default model (`nomic-embed-text-v1.5`).
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_model(api_key, "nomic-embed-text-v1.5", 768)
    }

    /// Create with an explicit model and dimension override.
    pub fn with_model(
        api_key: impl Into<String>,
        model: impl Into<String>,
        dimensions: usize,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            dimensions,
            base_url: "https://api-atlas.nomic.ai/v1/embedding/text".to_string(),
            task_type: "search_document".to_string(),
        }
    }

    /// Set the `task_type` (`search_document`, `search_query`, `clustering`, `classification`).
    pub fn with_task_type(mut self, task_type: impl Into<String>) -> Self {
        self.task_type = task_type.into();
        self
    }

    /// Returns the configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the current task type.
    pub fn task_type(&self) -> &str {
        &self.task_type
    }

    /// Build the request payload for the Nomic embeddings API.
    pub fn build_payload(&self, texts: &[String]) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "texts": texts,
            "task_type": self.task_type,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for NomicEmbedProvider {
    #[cfg(feature = "http-embeddings")]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        let client = reqwest::Client::new();
        let payload = self.build_payload(&[text.to_string()]);
        let response = client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgentorError::Http(format!("Nomic embedding request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ArgentorError::Http(format!(
                "Nomic API error {status}: {body}"
            )));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ArgentorError::Http(format!("Failed to read Nomic response body: {e}")))?;

        // Nomic response shape: { "embeddings": [[...]] }
        let embeddings = json
            .get("embeddings")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                ArgentorError::Agent("Nomic response missing 'embeddings' array".to_string())
            })?;
        let first = embeddings.first().ok_or_else(|| {
            ArgentorError::Agent("Nomic response contains no embedding vectors".to_string())
        })?;
        let vec: Vec<f32> = serde_json::from_value(first.clone()).map_err(|e| {
            ArgentorError::Agent(format!("Failed to parse Nomic embedding vector: {e}"))
        })?;
        Ok(vec)
    }

    #[cfg(not(feature = "http-embeddings"))]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        Ok(stub_embedding(text, self.dimensions))
    }

    fn dimension(&self) -> usize {
        self.dimensions
    }
}

// ===========================================================================
// 11. SentenceTransformersProvider (via Hugging Face Inference API)
// ===========================================================================

/// Embedding provider backed by the Hugging Face Inference API for
/// `sentence-transformers/*` models.
///
/// Default model: `sentence-transformers/all-MiniLM-L6-v2` (384 dims).
/// Also supports `all-mpnet-base-v2` (768) and `multi-qa-mpnet-base-dot-v1` (768).
pub struct SentenceTransformersProvider {
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    api_key: String,
    model: String,
    dimensions: usize,
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    base_url: String,
}

impl SentenceTransformersProvider {
    /// Create a new provider with the default model (`all-MiniLM-L6-v2`, 384 dims).
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_model(api_key, "sentence-transformers/all-MiniLM-L6-v2", 384)
    }

    /// Create with an explicit model and dimension override.
    pub fn with_model(
        api_key: impl Into<String>,
        model: impl Into<String>,
        dimensions: usize,
    ) -> Self {
        let model = model.into();
        let base_url =
            format!("https://api-inference.huggingface.co/pipeline/feature-extraction/{model}");
        Self {
            api_key: api_key.into(),
            model,
            dimensions,
            base_url,
        }
    }

    /// Override the API base URL (useful for self-hosted HF inference endpoints).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Returns the configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Default dimension for well-known sentence-transformer models.
    pub fn default_dimensions(model: &str) -> usize {
        match model {
            "sentence-transformers/all-MiniLM-L6-v2" => 384,
            "sentence-transformers/all-mpnet-base-v2"
            | "sentence-transformers/multi-qa-mpnet-base-dot-v1" => 768,
            _ => 384,
        }
    }

    /// Build the request payload for the HF Inference API.
    pub fn build_payload(&self, texts: &[String]) -> serde_json::Value {
        serde_json::json!({
            "inputs": texts,
            "options": { "wait_for_model": true },
        })
    }
}

#[async_trait]
impl EmbeddingProvider for SentenceTransformersProvider {
    #[cfg(feature = "http-embeddings")]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        let client = reqwest::Client::new();
        let payload = self.build_payload(&[text.to_string()]);
        let response = client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                ArgentorError::Http(format!("HuggingFace embedding request failed: {e}"))
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ArgentorError::Http(format!(
                "HuggingFace API error {status}: {body}"
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            ArgentorError::Http(format!("Failed to read HuggingFace response body: {e}"))
        })?;

        // HF feature-extraction returns either `[[f32; D]]` (batch) or `[f32; D]` (single).
        match &json {
            serde_json::Value::Array(arr)
                if arr.first().is_some_and(serde_json::Value::is_array) =>
            {
                let first = arr.first().cloned().ok_or_else(|| {
                    ArgentorError::Agent("HuggingFace response empty".to_string())
                })?;
                serde_json::from_value(first)
                    .map_err(|e| ArgentorError::Agent(format!("Failed to parse HF vector: {e}")))
            }
            serde_json::Value::Array(_) => serde_json::from_value(json)
                .map_err(|e| ArgentorError::Agent(format!("Failed to parse HF vector: {e}"))),
            _ => Err(ArgentorError::Agent(
                "HuggingFace response is not an array".to_string(),
            )),
        }
    }

    #[cfg(not(feature = "http-embeddings"))]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        Ok(stub_embedding(text, self.dimensions))
    }

    fn dimension(&self) -> usize {
        self.dimensions
    }
}

// ===========================================================================
// 12. TogetherEmbedProvider
// ===========================================================================

/// Embedding provider backed by the Together AI embeddings API.
///
/// Default model: `togethercomputer/m2-bert-80M-32k-retrieval` (768 dims).
/// Together uses an OpenAI-compatible request/response shape.
pub struct TogetherEmbedProvider {
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    api_key: String,
    model: String,
    dimensions: usize,
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    base_url: String,
}

impl TogetherEmbedProvider {
    /// Create a new Together provider with the default BERT retrieval model.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_model(api_key, "togethercomputer/m2-bert-80M-32k-retrieval", 768)
    }

    /// Create with an explicit model and dimension override.
    pub fn with_model(
        api_key: impl Into<String>,
        model: impl Into<String>,
        dimensions: usize,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            dimensions,
            base_url: "https://api.together.xyz/v1/embeddings".to_string(),
        }
    }

    /// Override the API base URL.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Returns the configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Build the request payload for the Together API.
    pub fn build_payload(&self, texts: &[String]) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "input": texts,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for TogetherEmbedProvider {
    #[cfg(feature = "http-embeddings")]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        let client = reqwest::Client::new();
        let payload = self.build_payload(&[text.to_string()]);
        let response = client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgentorError::Http(format!("Together embedding request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ArgentorError::Http(format!(
                "Together API error {status}: {body}"
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            ArgentorError::Http(format!("Failed to read Together response body: {e}"))
        })?;

        parse_openai_embedding_response(&json)
    }

    #[cfg(not(feature = "http-embeddings"))]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        Ok(stub_embedding(text, self.dimensions))
    }

    fn dimension(&self) -> usize {
        self.dimensions
    }
}

// ===========================================================================
// 13. CohereEmbedV4Provider (newer v4 embed endpoint)
// ===========================================================================

/// Embedding provider backed by the Cohere v2 embed API (labeled "v4" here
/// to disambiguate from the existing [`CohereEmbeddingProvider`] and to
/// mirror the naming in higher-level integrations).
///
/// Differs from [`CohereEmbeddingProvider`] in that it exposes explicit
/// `input_type` helpers (`for_search_document`, `for_search_query`) and
/// supports `embed-english-v3.0` / `embed-multilingual-v3.0` at 1024 dims.
pub struct CohereEmbedV4Provider {
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    api_key: String,
    model: String,
    dimensions: usize,
    input_type: String,
    #[cfg_attr(not(feature = "http-embeddings"), allow(dead_code))]
    base_url: String,
}

impl CohereEmbedV4Provider {
    /// Create a new v4 provider with the default model (`embed-english-v3.0`).
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_model(api_key, "embed-english-v3.0", 1024)
    }

    /// Create with an explicit model and dimension override.
    pub fn with_model(
        api_key: impl Into<String>,
        model: impl Into<String>,
        dimensions: usize,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            dimensions,
            input_type: "search_document".to_string(),
            base_url: "https://api.cohere.com/v2/embed".to_string(),
        }
    }

    /// Configure this provider for indexing documents (`search_document`).
    pub fn for_search_document(mut self) -> Self {
        self.input_type = "search_document".to_string();
        self
    }

    /// Configure this provider for querying (`search_query`).
    pub fn for_search_query(mut self) -> Self {
        self.input_type = "search_query".to_string();
        self
    }

    /// Set an arbitrary `input_type` string.
    pub fn with_input_type(mut self, input_type: impl Into<String>) -> Self {
        self.input_type = input_type.into();
        self
    }

    /// Override the API base URL.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Returns the configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the current input type.
    pub fn input_type(&self) -> &str {
        &self.input_type
    }

    /// Build the request payload for the Cohere v2 embed endpoint.
    pub fn build_payload(&self, texts: &[String]) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "texts": texts,
            "input_type": self.input_type,
            "embedding_types": ["float"],
        })
    }
}

#[async_trait]
impl EmbeddingProvider for CohereEmbedV4Provider {
    #[cfg(feature = "http-embeddings")]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        let client = reqwest::Client::new();
        let payload = self.build_payload(&[text.to_string()]);
        let response = client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgentorError::Http(format!("Cohere v4 embedding request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ArgentorError::Http(format!(
                "Cohere v4 API error {status}: {body}"
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            ArgentorError::Http(format!("Failed to read Cohere v4 response body: {e}"))
        })?;

        parse_cohere_embedding_response(&json)
    }

    #[cfg(not(feature = "http-embeddings"))]
    async fn embed(&self, text: &str) -> ArgentorResult<Vec<f32>> {
        Ok(stub_embedding(text, self.dimensions))
    }

    fn dimension(&self) -> usize {
        self.dimensions
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // -- Provider creation tests ------------------------------------------

    #[test]
    fn test_openai_provider_default_model() {
        let p = OpenAiEmbeddingProvider::new("sk-test", None);
        assert_eq!(p.model(), "text-embedding-3-small");
        assert_eq!(p.dimension(), 1536);
    }

    #[test]
    fn test_openai_provider_large_model() {
        let p = OpenAiEmbeddingProvider::new("sk-test", Some("text-embedding-3-large".into()));
        assert_eq!(p.dimension(), 3072);
    }

    #[test]
    fn test_openai_provider_custom_dimensions() {
        let p = OpenAiEmbeddingProvider::new("sk-test", None).with_dimensions(512);
        assert_eq!(p.dimension(), 512);
    }

    #[test]
    fn test_openai_provider_custom_base_url() {
        let p = OpenAiEmbeddingProvider::with_base_url(
            "sk-test",
            None,
            "https://my-azure.openai.azure.com/openai/deployments/embed",
        );
        assert_eq!(p.dimension(), 1536);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_openai_provider_returns_feature_error() {
        let p = OpenAiEmbeddingProvider::new("sk-test", None);
        let err = p.embed("hello").await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("HTTP embeddings not enabled"), "got: {msg}");
    }

    #[test]
    fn test_cohere_provider_default() {
        let p = CohereEmbeddingProvider::new("key", None);
        assert_eq!(p.model(), "embed-english-v3.0");
        assert_eq!(p.dimension(), 1024);
        assert_eq!(p.input_type(), "search_document");
    }

    #[test]
    fn test_cohere_provider_query_input_type() {
        let p = CohereEmbeddingProvider::new("key", None).with_input_type("search_query");
        assert_eq!(p.input_type(), "search_query");
    }

    #[test]
    fn test_cohere_provider_light_model() {
        let p = CohereEmbeddingProvider::new("key", Some("embed-english-light-v3.0".into()));
        assert_eq!(p.dimension(), 384);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_cohere_provider_returns_feature_error() {
        let p = CohereEmbeddingProvider::new("key", None);
        let err = p.embed("hello").await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("HTTP embeddings not enabled"), "got: {msg}");
    }

    #[test]
    fn test_voyage_provider_default() {
        let p = VoyageEmbeddingProvider::new("key", None);
        assert_eq!(p.model(), "voyage-2");
        assert_eq!(p.dimension(), 1024);
    }

    #[test]
    fn test_voyage_provider_code_model() {
        let p = VoyageEmbeddingProvider::new("key", Some("voyage-code-2".into()));
        assert_eq!(p.dimension(), 1536);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_voyage_provider_returns_feature_error() {
        let p = VoyageEmbeddingProvider::new("key", None);
        let err = p.embed("hello").await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("HTTP embeddings not enabled"), "got: {msg}");
    }

    // -- Response parsing tests -------------------------------------------

    #[test]
    fn test_parse_openai_embedding_response_valid() {
        let json = serde_json::json!({
            "data": [
                {
                    "embedding": [0.1, 0.2, 0.3, 0.4],
                    "index": 0
                }
            ],
            "model": "text-embedding-3-small"
        });
        let result = parse_openai_embedding_response(&json).unwrap();
        assert_eq!(result, vec![0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn test_parse_openai_embedding_response_empty_data() {
        let json = serde_json::json!({
            "data": [],
            "model": "text-embedding-3-small"
        });
        let err = parse_openai_embedding_response(&json).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("no embedding data"), "got: {msg}");
    }

    #[test]
    fn test_parse_openai_embedding_response_invalid_shape() {
        let json = serde_json::json!({ "error": "bad request" });
        let err = parse_openai_embedding_response(&json).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Failed to parse"), "got: {msg}");
    }

    #[test]
    fn test_parse_openai_embedding_response_multiple_picks_first() {
        let json = serde_json::json!({
            "data": [
                { "embedding": [1.0, 2.0], "index": 0 },
                { "embedding": [3.0, 4.0], "index": 1 }
            ],
            "model": "text-embedding-3-small"
        });
        let result = parse_openai_embedding_response(&json).unwrap();
        assert_eq!(result, vec![1.0, 2.0]);
    }

    #[test]
    fn test_parse_cohere_embedding_response_valid() {
        let json = serde_json::json!({
            "embeddings": {
                "float": [
                    [0.5, 0.6, 0.7]
                ]
            }
        });
        let result = parse_cohere_embedding_response(&json).unwrap();
        assert_eq!(result, vec![0.5, 0.6, 0.7]);
    }

    #[test]
    fn test_parse_cohere_embedding_response_empty_float() {
        let json = serde_json::json!({
            "embeddings": {
                "float": []
            }
        });
        let err = parse_cohere_embedding_response(&json).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("no float embeddings"), "got: {msg}");
    }

    #[test]
    fn test_parse_cohere_embedding_response_invalid_shape() {
        let json = serde_json::json!({ "message": "unauthorized" });
        let err = parse_cohere_embedding_response(&json).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Failed to parse"), "got: {msg}");
    }

    #[test]
    fn test_parse_cohere_embedding_response_missing_float_key() {
        // If "float" key is absent, serde default gives empty vec.
        let json = serde_json::json!({
            "embeddings": {}
        });
        let err = parse_cohere_embedding_response(&json).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("no float embeddings"), "got: {msg}");
    }

    #[test]
    fn test_parse_voyage_embedding_response_valid() {
        let json = serde_json::json!({
            "data": [
                {
                    "embedding": [0.9, 0.8, 0.7, 0.6, 0.5],
                    "index": 0
                }
            ]
        });
        let result = parse_voyage_embedding_response(&json).unwrap();
        assert_eq!(result, vec![0.9, 0.8, 0.7, 0.6, 0.5]);
    }

    #[test]
    fn test_parse_voyage_embedding_response_empty_data() {
        let json = serde_json::json!({ "data": [] });
        let err = parse_voyage_embedding_response(&json).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("no embedding data"), "got: {msg}");
    }

    #[test]
    fn test_parse_voyage_embedding_response_invalid_shape() {
        let json = serde_json::json!({ "error": "invalid key" });
        let err = parse_voyage_embedding_response(&json).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Failed to parse"), "got: {msg}");
    }

    // -- CachedEmbeddingProvider tests ------------------------------------

    #[tokio::test]
    async fn test_cache_hit() {
        let local = Arc::new(LocalEmbedding::new(64));
        let cached = CachedEmbeddingProvider::new(local, 100);

        let v1 = cached.embed("hello world").await.unwrap();
        let v2 = cached.embed("hello world").await.unwrap();
        assert_eq!(v1, v2);

        let stats = cached.cache_stats().await;
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.size, 1);
    }

    #[tokio::test]
    async fn test_cache_miss_different_texts() {
        let local = Arc::new(LocalEmbedding::new(64));
        let cached = CachedEmbeddingProvider::new(local, 100);

        let _ = cached.embed("alpha").await.unwrap();
        let _ = cached.embed("bravo").await.unwrap();

        let stats = cached.cache_stats().await;
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.size, 2);
    }

    #[tokio::test]
    async fn test_cache_eviction() {
        let local = Arc::new(LocalEmbedding::new(64));
        let cached = CachedEmbeddingProvider::new(local, 2);

        let _ = cached.embed("one").await.unwrap();
        let _ = cached.embed("two").await.unwrap();
        let _ = cached.embed("three").await.unwrap();

        let stats = cached.cache_stats().await;
        // After eviction, cache should still have at most max_cache_size entries.
        assert!(stats.size <= 2, "size={} should be <= 2", stats.size);
        assert_eq!(stats.misses, 3);
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let local = Arc::new(LocalEmbedding::new(64));
        let cached = CachedEmbeddingProvider::new(local, 100);

        let _ = cached.embed("text").await.unwrap();
        cached.clear().await;

        let stats = cached.cache_stats().await;
        assert_eq!(stats.size, 0);
    }

    #[tokio::test]
    async fn test_cache_dimension_delegates() {
        let local = Arc::new(LocalEmbedding::new(128));
        let cached = CachedEmbeddingProvider::new(local, 10);
        assert_eq!(cached.dimension(), 128);
    }

    // -- BatchEmbeddingProvider tests -------------------------------------

    #[tokio::test]
    async fn test_batch_embed() {
        let local = Arc::new(LocalEmbedding::new(64));
        let batch = BatchEmbeddingProvider::new(local);

        let results = batch
            .embed_batch(&["hello", "world", "test"])
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        for v in &results {
            assert_eq!(v.len(), 64);
        }
    }

    #[tokio::test]
    async fn test_batch_single_embed_delegates() {
        let local = Arc::new(LocalEmbedding::new(64));
        let batch = BatchEmbeddingProvider::new(local);

        let v = batch.embed("hello").await.unwrap();
        assert_eq!(v.len(), 64);
    }

    #[tokio::test]
    async fn test_batch_empty() {
        let local = Arc::new(LocalEmbedding::new(64));
        let batch = BatchEmbeddingProvider::new(local);

        let results = batch.embed_batch(&[]).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_batch_dimension_delegates() {
        let local = Arc::new(LocalEmbedding::new(200));
        let batch = BatchEmbeddingProvider::new(local);
        assert_eq!(batch.dimension(), 200);
    }

    // -- Factory tests ----------------------------------------------------

    #[test]
    fn test_factory_create_local() {
        let p = EmbeddingProviderFactory::create("local", "", None).unwrap();
        assert_eq!(p.dimension(), 256);
    }

    #[test]
    fn test_factory_create_local_custom_dim() {
        let p = EmbeddingProviderFactory::create("local", "", Some("128".into())).unwrap();
        assert_eq!(p.dimension(), 128);
    }

    #[test]
    fn test_factory_create_openai() {
        let p = EmbeddingProviderFactory::create("openai", "sk-test", None).unwrap();
        assert_eq!(p.dimension(), 1536);
    }

    #[test]
    fn test_factory_create_cohere() {
        let p = EmbeddingProviderFactory::create("cohere", "key", None).unwrap();
        assert_eq!(p.dimension(), 1024);
    }

    #[test]
    fn test_factory_create_voyage() {
        let p = EmbeddingProviderFactory::create("voyage", "key", None).unwrap();
        assert_eq!(p.dimension(), 1024);
    }

    #[test]
    fn test_factory_unknown_provider() {
        let result = EmbeddingProviderFactory::create("unknown", "", None);
        assert!(result.is_err(), "Unknown provider should return Err");
    }

    #[test]
    fn test_factory_available_providers() {
        let names = EmbeddingProviderFactory::available_providers();
        assert!(names.contains(&"openai"));
        assert!(names.contains(&"cohere"));
        assert!(names.contains(&"voyage"));
        assert!(names.contains(&"local"));
    }

    // -- Config tests -----------------------------------------------------

    #[test]
    fn test_config_default() {
        let cfg = EmbeddingConfig::default();
        assert_eq!(cfg.provider, "local");
        assert!(cfg.api_key.is_empty());
        assert!(cfg.model.is_none());
        assert!(cfg.dimensions.is_none());
        assert!(cfg.base_url.is_none());
        assert!(cfg.cache_size.is_none());
    }

    #[test]
    fn test_config_serialize_deserialize() {
        let cfg = EmbeddingConfig {
            provider: "openai".to_string(),
            api_key: "sk-123".to_string(),
            model: Some("text-embedding-3-small".to_string()),
            dimensions: Some(1536),
            base_url: None,
            cache_size: Some(500),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: EmbeddingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.provider, "openai");
        assert_eq!(parsed.api_key, "sk-123");
        assert_eq!(parsed.dimensions, Some(1536));
        assert_eq!(parsed.cache_size, Some(500));
    }

    #[test]
    fn test_config_deserialize_minimal() {
        let json = r#"{"provider":"local"}"#;
        let cfg: EmbeddingConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.provider, "local");
        assert!(cfg.api_key.is_empty());
    }

    #[tokio::test]
    async fn test_config_build_local() {
        let cfg = EmbeddingConfig::default();
        let provider = cfg.build().unwrap();
        assert_eq!(provider.dimension(), 256);
        let v = provider.embed("test text").await.unwrap();
        assert_eq!(v.len(), 256);
    }

    #[tokio::test]
    async fn test_config_build_local_with_cache() {
        let cfg = EmbeddingConfig {
            provider: "local".to_string(),
            cache_size: Some(50),
            ..Default::default()
        };
        let provider = cfg.build().unwrap();
        // Dimension from local default.
        assert_eq!(provider.dimension(), 256);
        // Should work — cache wraps local.
        let v1 = provider.embed("cached text").await.unwrap();
        let v2 = provider.embed("cached text").await.unwrap();
        assert_eq!(v1, v2);
    }

    #[tokio::test]
    async fn test_config_build_local_custom_dimensions() {
        let cfg = EmbeddingConfig {
            provider: "local".to_string(),
            dimensions: Some(512),
            ..Default::default()
        };
        let provider = cfg.build().unwrap();
        assert_eq!(provider.dimension(), 512);
    }

    #[test]
    fn test_config_build_unknown_provider() {
        let cfg = EmbeddingConfig {
            provider: "imaginary".to_string(),
            ..Default::default()
        };
        assert!(cfg.build().is_err());
    }

    // -- Misc / edge cases ------------------------------------------------

    #[test]
    fn test_fnv_hash_deterministic() {
        let h1 = fnv1a_hash(b"hello world");
        let h2 = fnv1a_hash(b"hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_fnv_hash_different_inputs() {
        let h1 = fnv1a_hash(b"alpha");
        let h2 = fnv1a_hash(b"bravo");
        assert_ne!(h1, h2);
    }

    // =====================================================================
    // Stub helper tests
    // =====================================================================

    #[test]
    fn test_stub_embedding_length() {
        let v = stub_embedding("hello", 128);
        assert_eq!(v.len(), 128);
    }

    #[test]
    fn test_stub_embedding_deterministic() {
        let v1 = stub_embedding("same input", 64);
        let v2 = stub_embedding("same input", 64);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_stub_embedding_normalized() {
        let v = stub_embedding("the quick brown fox", 256);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01, "norm={norm}");
    }

    #[test]
    fn test_stub_embedding_different_inputs_differ() {
        let a = stub_embedding("alpha", 64);
        let b = stub_embedding("bravo", 64);
        assert_ne!(a, b);
    }

    #[test]
    fn test_stub_embedding_empty_text_zeroes() {
        let v = stub_embedding("", 32);
        assert_eq!(v.len(), 32);
        assert!(v.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn test_stub_embedding_zero_dimension_safe() {
        // Must not panic; helper clamps dimension to at least 1.
        let v = stub_embedding("hi", 0);
        assert_eq!(v.len(), 1);
    }

    // =====================================================================
    // JinaEmbeddingProvider tests
    // =====================================================================

    #[test]
    fn test_jina_default_construction() {
        let p = JinaEmbeddingProvider::new("jina-key");
        assert_eq!(p.model(), "jina-embeddings-v3");
        assert_eq!(p.dimension(), 1024);
    }

    #[test]
    fn test_jina_with_model_clip() {
        let p = JinaEmbeddingProvider::with_model("k", "jina-clip-v2", 768);
        assert_eq!(p.model(), "jina-clip-v2");
        assert_eq!(p.dimension(), 768);
    }

    #[test]
    fn test_jina_with_base_url() {
        let p = JinaEmbeddingProvider::new("k").with_base_url("https://custom.jina/v1");
        // Indirect check: construction succeeds and model unchanged.
        assert_eq!(p.model(), "jina-embeddings-v3");
    }

    #[test]
    fn test_jina_build_payload_shape() {
        let p = JinaEmbeddingProvider::new("k");
        let payload = p.build_payload(&["hello".to_string(), "world".to_string()]);
        assert_eq!(payload["model"], "jina-embeddings-v3");
        assert_eq!(payload["input"][0], "hello");
        assert_eq!(payload["input"][1], "world");
    }

    #[tokio::test]
    async fn test_jina_embed_length_matches_dimension() {
        let p = JinaEmbeddingProvider::new("k");
        #[cfg(not(feature = "http-embeddings"))]
        {
            let v = p.embed("hello jina").await.unwrap();
            assert_eq!(v.len(), 1024);
        }
        // When http-embeddings is enabled, we don't hit the real API in tests;
        // just confirm dimension() reports 1024.
        assert_eq!(p.dimension(), 1024);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_jina_stub_is_normalized() {
        let p = JinaEmbeddingProvider::new("k");
        let v = p.embed("some input").await.unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_jina_stub_deterministic() {
        let p = JinaEmbeddingProvider::new("k");
        let a = p.embed("consistent").await.unwrap();
        let b = p.embed("consistent").await.unwrap();
        assert_eq!(a, b);
    }

    // =====================================================================
    // MistralEmbedProvider tests
    // =====================================================================

    #[test]
    fn test_mistral_default_construction() {
        let p = MistralEmbedProvider::new("mistral-key");
        assert_eq!(p.model(), "mistral-embed");
        assert_eq!(p.dimension(), 1024);
    }

    #[test]
    fn test_mistral_with_model_and_dimensions() {
        let p = MistralEmbedProvider::with_model("k", "mistral-embed-large", 2048);
        assert_eq!(p.model(), "mistral-embed-large");
        assert_eq!(p.dimension(), 2048);
    }

    #[test]
    fn test_mistral_build_payload_shape() {
        let p = MistralEmbedProvider::new("k");
        let payload = p.build_payload(&["alpha".to_string()]);
        assert_eq!(payload["model"], "mistral-embed");
        assert_eq!(payload["input"][0], "alpha");
    }

    #[test]
    fn test_mistral_with_base_url() {
        let p = MistralEmbedProvider::new("k").with_base_url("https://custom.mistral/v1");
        assert_eq!(p.dimension(), 1024);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_mistral_embed_length() {
        let p = MistralEmbedProvider::new("k");
        let v = p.embed("hello mistral").await.unwrap();
        assert_eq!(v.len(), 1024);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_mistral_stub_normalized() {
        let p = MistralEmbedProvider::new("k");
        let v = p.embed("normalized?").await.unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    // =====================================================================
    // NomicEmbedProvider tests
    // =====================================================================

    #[test]
    fn test_nomic_default_construction() {
        let p = NomicEmbedProvider::new("nomic-key");
        assert_eq!(p.model(), "nomic-embed-text-v1.5");
        assert_eq!(p.dimension(), 768);
        assert_eq!(p.task_type(), "search_document");
    }

    #[test]
    fn test_nomic_with_task_type() {
        let p = NomicEmbedProvider::new("k").with_task_type("search_query");
        assert_eq!(p.task_type(), "search_query");
    }

    #[test]
    fn test_nomic_build_payload_shape() {
        let p = NomicEmbedProvider::new("k").with_task_type("clustering");
        let payload = p.build_payload(&["doc a".to_string(), "doc b".to_string()]);
        assert_eq!(payload["model"], "nomic-embed-text-v1.5");
        assert_eq!(payload["texts"][0], "doc a");
        assert_eq!(payload["texts"][1], "doc b");
        assert_eq!(payload["task_type"], "clustering");
    }

    #[test]
    fn test_nomic_with_model_custom_dims() {
        let p = NomicEmbedProvider::with_model("k", "custom-nomic", 512);
        assert_eq!(p.dimension(), 512);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_nomic_embed_length() {
        let p = NomicEmbedProvider::new("k");
        let v = p.embed("nomic test").await.unwrap();
        assert_eq!(v.len(), 768);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_nomic_embed_normalized() {
        let p = NomicEmbedProvider::new("k");
        let v = p.embed("some text").await.unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    // =====================================================================
    // SentenceTransformersProvider tests
    // =====================================================================

    #[test]
    fn test_sentence_transformers_default_construction() {
        let p = SentenceTransformersProvider::new("hf-key");
        assert_eq!(p.model(), "sentence-transformers/all-MiniLM-L6-v2");
        assert_eq!(p.dimension(), 384);
    }

    #[test]
    fn test_sentence_transformers_mpnet_dims() {
        let dims = SentenceTransformersProvider::default_dimensions(
            "sentence-transformers/all-mpnet-base-v2",
        );
        assert_eq!(dims, 768);
    }

    #[test]
    fn test_sentence_transformers_multi_qa_dims() {
        let dims = SentenceTransformersProvider::default_dimensions(
            "sentence-transformers/multi-qa-mpnet-base-dot-v1",
        );
        assert_eq!(dims, 768);
    }

    #[test]
    fn test_sentence_transformers_unknown_model_fallback() {
        let dims =
            SentenceTransformersProvider::default_dimensions("sentence-transformers/unknown");
        assert_eq!(dims, 384);
    }

    #[test]
    fn test_sentence_transformers_with_model() {
        let p = SentenceTransformersProvider::with_model(
            "k",
            "sentence-transformers/all-mpnet-base-v2",
            768,
        );
        assert_eq!(p.model(), "sentence-transformers/all-mpnet-base-v2");
        assert_eq!(p.dimension(), 768);
    }

    #[test]
    fn test_sentence_transformers_build_payload_shape() {
        let p = SentenceTransformersProvider::new("k");
        let payload = p.build_payload(&["hi".to_string()]);
        assert_eq!(payload["inputs"][0], "hi");
        assert_eq!(payload["options"]["wait_for_model"], true);
    }

    #[test]
    fn test_sentence_transformers_with_base_url() {
        let p =
            SentenceTransformersProvider::new("k").with_base_url("https://self-hosted.hf/embed");
        assert_eq!(p.dimension(), 384);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_sentence_transformers_embed_length() {
        let p = SentenceTransformersProvider::new("k");
        let v = p.embed("minilm test").await.unwrap();
        assert_eq!(v.len(), 384);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_sentence_transformers_embed_normalized() {
        let p = SentenceTransformersProvider::new("k");
        let v = p.embed("some input").await.unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    // =====================================================================
    // TogetherEmbedProvider tests
    // =====================================================================

    #[test]
    fn test_together_default_construction() {
        let p = TogetherEmbedProvider::new("together-key");
        assert_eq!(p.model(), "togethercomputer/m2-bert-80M-32k-retrieval");
        assert_eq!(p.dimension(), 768);
    }

    #[test]
    fn test_together_with_model() {
        let p = TogetherEmbedProvider::with_model("k", "togethercomputer/custom", 1024);
        assert_eq!(p.model(), "togethercomputer/custom");
        assert_eq!(p.dimension(), 1024);
    }

    #[test]
    fn test_together_build_payload_shape() {
        let p = TogetherEmbedProvider::new("k");
        let payload = p.build_payload(&["x".to_string(), "y".to_string()]);
        assert_eq!(
            payload["model"],
            "togethercomputer/m2-bert-80M-32k-retrieval"
        );
        assert_eq!(payload["input"][0], "x");
        assert_eq!(payload["input"][1], "y");
    }

    #[test]
    fn test_together_with_base_url() {
        let p = TogetherEmbedProvider::new("k").with_base_url("https://custom.together/v1");
        assert_eq!(p.dimension(), 768);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_together_embed_length() {
        let p = TogetherEmbedProvider::new("k");
        let v = p.embed("together test").await.unwrap();
        assert_eq!(v.len(), 768);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_together_embed_normalized() {
        let p = TogetherEmbedProvider::new("k");
        let v = p.embed("text").await.unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    // =====================================================================
    // CohereEmbedV4Provider tests
    // =====================================================================

    #[test]
    fn test_cohere_v4_default_construction() {
        let p = CohereEmbedV4Provider::new("cohere-key");
        assert_eq!(p.model(), "embed-english-v3.0");
        assert_eq!(p.dimension(), 1024);
        assert_eq!(p.input_type(), "search_document");
    }

    #[test]
    fn test_cohere_v4_multilingual_model() {
        let p = CohereEmbedV4Provider::with_model("k", "embed-multilingual-v3.0", 1024);
        assert_eq!(p.model(), "embed-multilingual-v3.0");
        assert_eq!(p.dimension(), 1024);
    }

    #[test]
    fn test_cohere_v4_for_search_document() {
        let p = CohereEmbedV4Provider::new("k").for_search_document();
        assert_eq!(p.input_type(), "search_document");
    }

    #[test]
    fn test_cohere_v4_for_search_query() {
        let p = CohereEmbedV4Provider::new("k").for_search_query();
        assert_eq!(p.input_type(), "search_query");
    }

    #[test]
    fn test_cohere_v4_with_input_type() {
        let p = CohereEmbedV4Provider::new("k").with_input_type("classification");
        assert_eq!(p.input_type(), "classification");
    }

    #[test]
    fn test_cohere_v4_build_payload_shape_document() {
        let p = CohereEmbedV4Provider::new("k").for_search_document();
        let payload = p.build_payload(&["doc".to_string()]);
        assert_eq!(payload["model"], "embed-english-v3.0");
        assert_eq!(payload["texts"][0], "doc");
        assert_eq!(payload["input_type"], "search_document");
        assert_eq!(payload["embedding_types"][0], "float");
    }

    #[test]
    fn test_cohere_v4_build_payload_shape_query() {
        let p = CohereEmbedV4Provider::new("k").for_search_query();
        let payload = p.build_payload(&["q".to_string()]);
        assert_eq!(payload["input_type"], "search_query");
    }

    #[test]
    fn test_cohere_v4_with_base_url() {
        let p = CohereEmbedV4Provider::new("k").with_base_url("https://custom.cohere/v2/embed");
        assert_eq!(p.dimension(), 1024);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_cohere_v4_embed_length() {
        let p = CohereEmbedV4Provider::new("k");
        let v = p.embed("cohere v4 test").await.unwrap();
        assert_eq!(v.len(), 1024);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_cohere_v4_embed_normalized() {
        let p = CohereEmbedV4Provider::new("k");
        let v = p.embed("x").await.unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    #[cfg(not(feature = "http-embeddings"))]
    #[tokio::test]
    async fn test_cohere_v4_embed_deterministic() {
        let p = CohereEmbedV4Provider::new("k");
        let a = p.embed("same").await.unwrap();
        let b = p.embed("same").await.unwrap();
        assert_eq!(a, b);
    }

    // =====================================================================
    // Cross-provider checks
    // =====================================================================

    #[test]
    fn test_all_new_providers_implement_embedding_provider_trait() {
        // Compile-time check — if these coerce into `Box<dyn EmbeddingProvider>`,
        // they correctly implement the trait.
        let _boxes: Vec<Box<dyn EmbeddingProvider>> = vec![
            Box::new(JinaEmbeddingProvider::new("k")),
            Box::new(MistralEmbedProvider::new("k")),
            Box::new(NomicEmbedProvider::new("k")),
            Box::new(SentenceTransformersProvider::new("k")),
            Box::new(TogetherEmbedProvider::new("k")),
            Box::new(CohereEmbedV4Provider::new("k")),
        ];
    }

    #[test]
    fn test_new_providers_have_expected_dimensions() {
        assert_eq!(JinaEmbeddingProvider::new("k").dimension(), 1024);
        assert_eq!(MistralEmbedProvider::new("k").dimension(), 1024);
        assert_eq!(NomicEmbedProvider::new("k").dimension(), 768);
        assert_eq!(SentenceTransformersProvider::new("k").dimension(), 384);
        assert_eq!(TogetherEmbedProvider::new("k").dimension(), 768);
        assert_eq!(CohereEmbedV4Provider::new("k").dimension(), 1024);
    }
}
