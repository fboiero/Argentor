//! Batch processor for grouping and executing multiple LLM requests.
//!
//! Improves throughput by collecting requests and processing them
//! in configurable batches with concurrency limits.
//!
//! # Main types
//!
//! - [`BatchProcessor`] — Collects and processes batches of requests.
//! - [`BatchRequest`] — A single request in a batch.
//! - [`BatchResult`] — Result of processing a batch.
//! - [`BatchConfig`] — Configuration for batch processing.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// ---------------------------------------------------------------------------
// BatchConfig
// ---------------------------------------------------------------------------

/// Configuration for batch processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Maximum number of requests in a single batch.
    pub max_batch_size: usize,
    /// Maximum concurrency for processing batches.
    pub max_concurrency: usize,
    /// Timeout per request in milliseconds.
    pub timeout_ms: u64,
    /// Whether to continue processing after a failure.
    pub continue_on_error: bool,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 10,
            max_concurrency: 5,
            timeout_ms: 30_000,
            continue_on_error: true,
        }
    }
}

impl BatchConfig {
    /// Create a new config with the given batch size.
    pub fn new(max_batch_size: usize) -> Self {
        Self {
            max_batch_size,
            ..Default::default()
        }
    }

    /// Set max concurrency.
    pub fn with_concurrency(mut self, max: usize) -> Self {
        self.max_concurrency = max;
        self
    }

    /// Set timeout.
    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }
}

// ---------------------------------------------------------------------------
// RequestStatus
// ---------------------------------------------------------------------------

/// Status of a batch request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    /// Waiting to be processed.
    Pending,
    /// Currently being processed.
    Processing,
    /// Completed successfully.
    Completed,
    /// Failed with an error.
    Failed,
    /// Skipped (e.g., due to a prior failure).
    Skipped,
}

// ---------------------------------------------------------------------------
// BatchRequest
// ---------------------------------------------------------------------------

/// A single request in a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRequest {
    /// Unique request ID within the batch.
    pub id: String,
    /// The prompt/input for this request.
    pub input: String,
    /// Optional model override for this request.
    pub model: Option<String>,
    /// Priority (higher = processed first).
    pub priority: u32,
    /// Status of this request.
    pub status: RequestStatus,
    /// Optional metadata.
    pub metadata: HashMap<String, String>,
}

impl BatchRequest {
    /// Create a new batch request.
    pub fn new(id: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            input: input.into(),
            model: None,
            priority: 0,
            status: RequestStatus::Pending,
            metadata: HashMap::new(),
        }
    }

    /// Set the model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the priority.
    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    /// Add metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

// ---------------------------------------------------------------------------
// RequestResult
// ---------------------------------------------------------------------------

/// Result of processing a single request in a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestResult {
    /// The request ID.
    pub request_id: String,
    /// The output (if successful).
    pub output: Option<String>,
    /// Error message (if failed).
    pub error: Option<String>,
    /// Processing time in milliseconds.
    pub duration_ms: u64,
    /// Whether the request succeeded.
    pub success: bool,
    /// Estimated token usage.
    pub token_estimate: u64,
}

// ---------------------------------------------------------------------------
// BatchResult
// ---------------------------------------------------------------------------

/// Result of processing an entire batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResult {
    /// Batch identifier.
    pub batch_id: String,
    /// Individual request results.
    pub results: Vec<RequestResult>,
    /// Total processing time in milliseconds.
    pub total_duration_ms: u64,
    /// Number of successful requests.
    pub succeeded: usize,
    /// Number of failed requests.
    pub failed: usize,
    /// Number of skipped requests.
    pub skipped: usize,
    /// Total estimated tokens used.
    pub total_tokens: u64,
}

impl BatchResult {
    /// Success rate as a percentage.
    pub fn success_rate(&self) -> f64 {
        let total = self.succeeded + self.failed + self.skipped;
        if total == 0 {
            return 100.0;
        }
        (self.succeeded as f64 / total as f64) * 100.0
    }
}

// ---------------------------------------------------------------------------
// BatchProcessor
// ---------------------------------------------------------------------------

/// Collects and processes batches of LLM requests.
///
/// Clone is cheap (inner state is behind `Arc<RwLock>`).
#[derive(Debug, Clone)]
pub struct BatchProcessor {
    config: BatchConfig,
    inner: Arc<RwLock<BatchInner>>,
}

#[derive(Debug)]
struct BatchInner {
    pending: Vec<BatchRequest>,
    completed_batches: Vec<BatchResult>,
    batch_counter: u64,
    total_requests: u64,
    total_completed: u64,
    total_failed: u64,
}

impl BatchProcessor {
    /// Create a new batch processor.
    pub fn new(config: BatchConfig) -> Self {
        Self {
            config,
            inner: Arc::new(RwLock::new(BatchInner {
                pending: Vec::new(),
                completed_batches: Vec::new(),
                batch_counter: 0,
                total_requests: 0,
                total_completed: 0,
                total_failed: 0,
            })),
        }
    }

    /// Add a request to the pending queue.
    pub fn enqueue(&self, request: BatchRequest) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.total_requests += 1;
        inner.pending.push(request);
    }

    /// Get the number of pending requests.
    pub fn pending_count(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .len()
    }

    /// Take the next batch of requests from the queue (up to max_batch_size).
    /// Requests are sorted by priority (highest first).
    pub fn take_batch(&self) -> Vec<BatchRequest> {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Sort by priority (descending)
        inner
            .pending
            .sort_by_key(|entry| std::cmp::Reverse(entry.priority));

        let batch_size = self.config.max_batch_size.min(inner.pending.len());
        let batch: Vec<BatchRequest> = inner.pending.drain(..batch_size).collect();
        batch
    }

    /// Record a completed batch result.
    pub fn record_batch(&self, result: BatchResult) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.total_completed += result.succeeded as u64;
        inner.total_failed += result.failed as u64;
        inner.completed_batches.push(result);
    }

    /// Process a batch synchronously using a closure.
    /// The closure receives each request and returns an output or error.
    pub fn process_batch<F>(&self, processor: F) -> BatchResult
    where
        F: Fn(&BatchRequest) -> Result<(String, u64), String>,
    {
        let batch = self.take_batch();
        let batch_id = {
            let mut inner = self
                .inner
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.batch_counter += 1;
            format!("batch-{}", inner.batch_counter)
        };

        let start = std::time::Instant::now();
        let mut results = Vec::with_capacity(batch.len());
        let mut succeeded = 0;
        let mut failed = 0;
        let mut total_tokens = 0u64;

        for request in &batch {
            let req_start = std::time::Instant::now();
            match processor(request) {
                Ok((output, tokens)) => {
                    succeeded += 1;
                    total_tokens += tokens;
                    results.push(RequestResult {
                        request_id: request.id.clone(),
                        output: Some(output),
                        error: None,
                        duration_ms: req_start.elapsed().as_millis() as u64,
                        success: true,
                        token_estimate: tokens,
                    });
                }
                Err(e) => {
                    failed += 1;
                    results.push(RequestResult {
                        request_id: request.id.clone(),
                        output: None,
                        error: Some(e),
                        duration_ms: req_start.elapsed().as_millis() as u64,
                        success: false,
                        token_estimate: 0,
                    });
                    if !self.config.continue_on_error {
                        break;
                    }
                }
            }
        }

        let result = BatchResult {
            batch_id,
            results,
            total_duration_ms: start.elapsed().as_millis() as u64,
            succeeded,
            failed,
            skipped: batch.len() - succeeded - failed,
            total_tokens,
        };

        self.record_batch(result.clone());
        result
    }

    /// Get processor statistics.
    pub fn stats(&self) -> BatchProcessorStats {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        BatchProcessorStats {
            pending_requests: inner.pending.len(),
            completed_batches: inner.completed_batches.len(),
            total_requests: inner.total_requests,
            total_completed: inner.total_completed,
            total_failed: inner.total_failed,
            max_batch_size: self.config.max_batch_size,
            max_concurrency: self.config.max_concurrency,
        }
    }
}

impl Default for BatchProcessor {
    fn default() -> Self {
        Self::new(BatchConfig::default())
    }
}

/// Statistics for the batch processor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchProcessorStats {
    /// Number of pending requests.
    pub pending_requests: usize,
    /// Number of completed batches.
    pub completed_batches: usize,
    /// Total requests ever enqueued.
    pub total_requests: u64,
    /// Total requests successfully completed.
    pub total_completed: u64,
    /// Total requests that failed.
    pub total_failed: u64,
    /// Configured max batch size.
    pub max_batch_size: usize,
    /// Configured max concurrency.
    pub max_concurrency: usize,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn processor() -> BatchProcessor {
        BatchProcessor::new(BatchConfig::new(5))
    }

    // 1. New processor is empty
    #[test]
    fn test_new_processor() {
        let p = processor();
        assert_eq!(p.pending_count(), 0);
    }

    // 2. Enqueue requests
    #[test]
    fn test_enqueue() {
        let p = processor();
        p.enqueue(BatchRequest::new("1", "hello"));
        p.enqueue(BatchRequest::new("2", "world"));
        assert_eq!(p.pending_count(), 2);
    }

    // 3. Take batch respects size limit
    #[test]
    fn test_take_batch_limit() {
        let p = BatchProcessor::new(BatchConfig::new(3));
        for i in 0..10 {
            p.enqueue(BatchRequest::new(format!("{i}"), format!("req-{i}")));
        }
        let batch = p.take_batch();
        assert_eq!(batch.len(), 3);
        assert_eq!(p.pending_count(), 7);
    }

    // 4. Take batch sorts by priority
    #[test]
    fn test_priority_sorting() {
        let p = processor();
        p.enqueue(BatchRequest::new("low", "low").with_priority(1));
        p.enqueue(BatchRequest::new("high", "high").with_priority(10));
        p.enqueue(BatchRequest::new("mid", "mid").with_priority(5));

        let batch = p.take_batch();
        assert_eq!(batch[0].id, "high");
        assert_eq!(batch[1].id, "mid");
        assert_eq!(batch[2].id, "low");
    }

    // 5. Process batch success
    #[test]
    fn test_process_batch_success() {
        let p = processor();
        p.enqueue(BatchRequest::new("1", "hello"));
        p.enqueue(BatchRequest::new("2", "world"));

        let result = p.process_batch(|req| Ok((format!("echo: {}", req.input), 10)));
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 0);
        assert_eq!(result.total_tokens, 20);
    }

    // 6. Process batch with failures
    #[test]
    fn test_process_batch_failures() {
        let p = processor();
        p.enqueue(BatchRequest::new("1", "ok"));
        p.enqueue(BatchRequest::new("2", "fail"));

        let result = p.process_batch(|req| {
            if req.input == "fail" {
                Err("intentional".to_string())
            } else {
                Ok(("ok".to_string(), 10))
            }
        });
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 1);
    }

    // 7. Continue on error
    #[test]
    fn test_continue_on_error() {
        let p = BatchProcessor::new(BatchConfig {
            continue_on_error: true,
            ..BatchConfig::new(5)
        });
        p.enqueue(BatchRequest::new("1", "fail"));
        p.enqueue(BatchRequest::new("2", "ok"));

        let result = p.process_batch(|req| {
            if req.input == "fail" {
                Err("error".to_string())
            } else {
                Ok(("ok".to_string(), 10))
            }
        });
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.results.len(), 2);
    }

    // 8. Stop on error
    #[test]
    fn test_stop_on_error() {
        let p = BatchProcessor::new(BatchConfig {
            continue_on_error: false,
            ..BatchConfig::new(5)
        });
        p.enqueue(BatchRequest::new("1", "fail"));
        p.enqueue(BatchRequest::new("2", "ok"));

        let result = p.process_batch(|req| {
            if req.input == "fail" {
                Err("error".to_string())
            } else {
                Ok(("ok".to_string(), 10))
            }
        });
        assert_eq!(result.results.len(), 1); // stopped after first failure
    }

    // 9. Empty batch
    #[test]
    fn test_empty_batch() {
        let p = processor();
        let result = p.process_batch(|_| Ok(("ok".to_string(), 10)));
        assert_eq!(result.succeeded, 0);
        assert!(result.results.is_empty());
    }

    // 10. Batch result success rate
    #[test]
    fn test_success_rate() {
        let result = BatchResult {
            batch_id: "test".to_string(),
            results: Vec::new(),
            total_duration_ms: 0,
            succeeded: 8,
            failed: 2,
            skipped: 0,
            total_tokens: 0,
        };
        assert!((result.success_rate() - 80.0).abs() < 0.01);
    }

    // 11. Stats tracking
    #[test]
    fn test_stats() {
        let p = processor();
        p.enqueue(BatchRequest::new("1", "a"));
        p.enqueue(BatchRequest::new("2", "b"));
        p.process_batch(|_| Ok(("ok".to_string(), 10)));

        let stats = p.stats();
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.total_completed, 2);
        assert_eq!(stats.completed_batches, 1);
    }

    // 12. Request with model
    #[test]
    fn test_request_with_model() {
        let req = BatchRequest::new("1", "hello").with_model("gpt-4");
        assert_eq!(req.model.unwrap(), "gpt-4");
    }

    // 13. Request with metadata
    #[test]
    fn test_request_metadata() {
        let req = BatchRequest::new("1", "hello").with_metadata("user", "alice");
        assert_eq!(req.metadata.get("user").unwrap(), "alice");
    }

    // 14. BatchConfig defaults
    #[test]
    fn test_config_defaults() {
        let c = BatchConfig::default();
        assert_eq!(c.max_batch_size, 10);
        assert_eq!(c.max_concurrency, 5);
        assert!(c.continue_on_error);
    }

    // 15. Config builder
    #[test]
    fn test_config_builder() {
        let c = BatchConfig::new(20)
            .with_concurrency(10)
            .with_timeout_ms(5000);
        assert_eq!(c.max_batch_size, 20);
        assert_eq!(c.max_concurrency, 10);
        assert_eq!(c.timeout_ms, 5000);
    }

    // 16. BatchResult serializable
    #[test]
    fn test_result_serializable() {
        let p = processor();
        p.enqueue(BatchRequest::new("1", "a"));
        let result = p.process_batch(|_| Ok(("ok".to_string(), 10)));
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"succeeded\":1"));
    }

    // 17. RequestResult serializable
    #[test]
    fn test_request_result_serializable() {
        let rr = RequestResult {
            request_id: "1".to_string(),
            output: Some("ok".to_string()),
            error: None,
            duration_ms: 42,
            success: true,
            token_estimate: 100,
        };
        let json = serde_json::to_string(&rr).unwrap();
        assert!(json.contains("\"request_id\":\"1\""));
    }

    // 18. Default processor
    #[test]
    fn test_default() {
        let p = BatchProcessor::default();
        assert_eq!(p.pending_count(), 0);
    }

    // 19. Clone shares state
    #[test]
    fn test_clone_shares() {
        let p1 = processor();
        let p2 = p1.clone();
        p1.enqueue(BatchRequest::new("1", "hello"));
        assert_eq!(p2.pending_count(), 1);
    }

    // 20. Batch IDs are sequential
    #[test]
    fn test_batch_ids() {
        let p = processor();
        p.enqueue(BatchRequest::new("1", "a"));
        let r1 = p.process_batch(|_| Ok(("ok".to_string(), 10)));

        p.enqueue(BatchRequest::new("2", "b"));
        let r2 = p.process_batch(|_| Ok(("ok".to_string(), 10)));

        assert_eq!(r1.batch_id, "batch-1");
        assert_eq!(r2.batch_id, "batch-2");
    }
}
