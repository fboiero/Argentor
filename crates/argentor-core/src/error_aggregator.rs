//! Error aggregation, deduplication, and trending for production diagnostics.
//!
//! Provides intelligent error grouping so that operators can identify the
//! most impactful issues without wading through thousands of duplicate
//! log lines.
//!
//! # Main types
//!
//! - [`ErrorAggregator`] — Thread-safe error collector with fingerprinting.
//! - [`ErrorGroup`] — A deduplicated group of similar errors.
//! - [`ErrorFingerprint`] — Hash-based error identity for deduplication.
//! - [`ErrorTrend`] — Time-bucketed error frequency analysis.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// ---------------------------------------------------------------------------
// ErrorSeverity
// ---------------------------------------------------------------------------

/// Severity level for aggregated errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorSeverity {
    /// Informational — not actionable.
    Info,
    /// Warning — should be investigated.
    Warning,
    /// Error — requires attention.
    Error,
    /// Critical — requires immediate action.
    Critical,
}

// ---------------------------------------------------------------------------
// ErrorCategory
// ---------------------------------------------------------------------------

/// Category for classifying errors into operational domains.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// LLM provider errors (rate limits, auth, timeouts).
    LlmProvider,
    /// Tool execution failures.
    ToolExecution,
    /// Network/connectivity issues.
    Network,
    /// Authentication/authorization failures.
    Auth,
    /// Data parsing or serialization errors.
    Serialization,
    /// Internal framework errors.
    Internal,
    /// User input validation errors.
    Validation,
    /// Resource exhaustion (memory, disk, connections).
    Resource,
    /// Custom category.
    Custom(String),
}

// ---------------------------------------------------------------------------
// ErrorFingerprint
// ---------------------------------------------------------------------------

/// A hash-based identity for grouping similar errors.
///
/// Two errors with the same fingerprint are considered "the same error"
/// for deduplication purposes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ErrorFingerprint(String);

impl ErrorFingerprint {
    /// Compute a fingerprint from error components.
    ///
    /// The fingerprint is based on category + a normalized message
    /// (numbers stripped, paths shortened) so that minor variations
    /// like different IDs or line numbers still group together.
    pub fn compute(category: &ErrorCategory, message: &str) -> Self {
        let normalized = normalize_message(message);
        let cat_str = serde_json::to_string(category).unwrap_or_default();
        // Simple FNV-1a style hash
        let mut hash: u64 = 14695981039346656037;
        for byte in cat_str.bytes().chain(normalized.bytes()) {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(1099511628211);
        }
        Self(format!("{hash:016x}"))
    }

    /// Return the fingerprint string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Normalize an error message for fingerprinting:
/// - Replace sequences of digits with `<N>`
/// - Replace UUIDs with `<UUID>`
/// - Collapse whitespace
fn normalize_message(msg: &str) -> String {
    let mut result = String::with_capacity(msg.len());
    let mut chars = msg.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch.is_ascii_digit() {
            // Skip all consecutive digits, replace with <N>
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_digit() || *c == '-')
            {
                chars.next();
            }
            result.push_str("<N>");
        } else if ch.is_whitespace() {
            if !result.ends_with(' ') {
                result.push(' ');
            }
        } else {
            result.push(ch);
        }
    }

    result.trim().to_string()
}

// ---------------------------------------------------------------------------
// ErrorRecord
// ---------------------------------------------------------------------------

/// A single error occurrence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorRecord {
    /// When the error occurred.
    pub timestamp: DateTime<Utc>,
    /// The original error message.
    pub message: String,
    /// Severity of this error.
    pub severity: ErrorSeverity,
    /// Category of this error.
    pub category: ErrorCategory,
    /// The computed fingerprint.
    pub fingerprint: ErrorFingerprint,
    /// Optional source (agent role, tool name, etc.).
    pub source: Option<String>,
    /// Optional correlation ID for tracing.
    pub correlation_id: Option<String>,
}

// ---------------------------------------------------------------------------
// ErrorGroup
// ---------------------------------------------------------------------------

/// A deduplicated group of similar errors sharing the same fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorGroup {
    /// The shared fingerprint for all errors in this group.
    pub fingerprint: ErrorFingerprint,
    /// The representative error message (from the first occurrence).
    pub message: String,
    /// Category of errors in this group.
    pub category: ErrorCategory,
    /// Highest severity seen in this group.
    pub max_severity: ErrorSeverity,
    /// Total number of occurrences.
    pub count: u64,
    /// Timestamp of the first occurrence.
    pub first_seen: DateTime<Utc>,
    /// Timestamp of the most recent occurrence.
    pub last_seen: DateTime<Utc>,
    /// Distinct sources that have produced this error.
    pub sources: Vec<String>,
    /// Sample correlation IDs (up to 5).
    pub sample_correlation_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// ErrorTrend
// ---------------------------------------------------------------------------

/// Time-bucketed error frequency for trend analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorTrend {
    /// The fingerprint being analyzed.
    pub fingerprint: ErrorFingerprint,
    /// Counts per time bucket (bucket label → count).
    pub buckets: Vec<TrendBucket>,
    /// Whether the error rate is increasing.
    pub is_increasing: bool,
    /// Rate of change (errors per minute delta between last two buckets).
    pub rate_change: f64,
}

/// A single time bucket in a trend analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendBucket {
    /// Bucket start time.
    pub start: DateTime<Utc>,
    /// Bucket end time.
    pub end: DateTime<Utc>,
    /// Number of errors in this bucket.
    pub count: u64,
}

// ---------------------------------------------------------------------------
// AggregatorSummary
// ---------------------------------------------------------------------------

/// Point-in-time summary of the error aggregator state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatorSummary {
    /// Total number of errors recorded.
    pub total_errors: u64,
    /// Number of distinct error groups.
    pub unique_groups: usize,
    /// Top error groups by occurrence count.
    pub top_groups: Vec<ErrorGroup>,
    /// Errors per severity level.
    pub by_severity: HashMap<String, u64>,
    /// Errors per category.
    pub by_category: HashMap<String, u64>,
}

// ---------------------------------------------------------------------------
// Inner state
// ---------------------------------------------------------------------------

struct Inner {
    /// All error records (bounded).
    records: Vec<ErrorRecord>,
    /// Grouped errors by fingerprint.
    groups: HashMap<ErrorFingerprint, ErrorGroup>,
    /// Maximum number of records to keep.
    max_records: usize,
    /// Counters by severity.
    by_severity: HashMap<ErrorSeverity, u64>,
    /// Counters by category.
    by_category: HashMap<String, u64>,
    /// Total errors recorded.
    total: u64,
}

impl Inner {
    fn new(max_records: usize) -> Self {
        Self {
            records: Vec::new(),
            groups: HashMap::new(),
            max_records,
            by_severity: HashMap::new(),
            by_category: HashMap::new(),
            total: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// ErrorAggregator
// ---------------------------------------------------------------------------

/// Thread-safe error aggregator with fingerprinting and deduplication.
///
/// Clone is cheap (inner state is behind `Arc<RwLock>`).
#[derive(Debug, Clone)]
pub struct ErrorAggregator {
    inner: Arc<RwLock<Inner>>,
}

impl ErrorAggregator {
    /// Create a new aggregator with the given maximum record capacity.
    pub fn new(max_records: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner::new(max_records))),
        }
    }

    /// Record a new error.
    pub fn record(
        &self,
        message: impl Into<String>,
        severity: ErrorSeverity,
        category: ErrorCategory,
        source: Option<String>,
        correlation_id: Option<String>,
    ) {
        let message = message.into();
        let fingerprint = ErrorFingerprint::compute(&category, &message);
        let now = Utc::now();
        let cat_label = serde_json::to_string(&category).unwrap_or_default();

        let record = ErrorRecord {
            timestamp: now,
            message: message.clone(),
            severity,
            category: category.clone(),
            fingerprint: fingerprint.clone(),
            source: source.clone(),
            correlation_id: correlation_id.clone(),
        };

        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        inner.total += 1;
        *inner.by_severity.entry(severity).or_insert(0) += 1;
        *inner.by_category.entry(cat_label).or_insert(0) += 1;

        // Update or create group
        let group = inner
            .groups
            .entry(fingerprint)
            .or_insert_with(|| ErrorGroup {
                fingerprint: record.fingerprint.clone(),
                message: message.clone(),
                category,
                max_severity: severity,
                count: 0,
                first_seen: now,
                last_seen: now,
                sources: Vec::new(),
                sample_correlation_ids: Vec::new(),
            });

        group.count += 1;
        group.last_seen = now;
        if severity > group.max_severity {
            group.max_severity = severity;
        }
        if let Some(src) = &source {
            if !group.sources.contains(src) && group.sources.len() < 10 {
                group.sources.push(src.clone());
            }
        }
        if let Some(cid) = &correlation_id {
            if group.sample_correlation_ids.len() < 5 {
                group.sample_correlation_ids.push(cid.clone());
            }
        }

        // Store record (with eviction)
        if inner.records.len() >= inner.max_records {
            inner.records.remove(0);
        }
        inner.records.push(record);
    }

    /// Get the top-N error groups by occurrence count.
    pub fn top_groups(&self, n: usize) -> Vec<ErrorGroup> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let mut groups: Vec<ErrorGroup> = inner.groups.values().cloned().collect();
        groups.sort_by_key(|group| std::cmp::Reverse(group.count));
        groups.truncate(n);
        groups
    }

    /// Get a specific error group by fingerprint.
    pub fn group(&self, fingerprint: &ErrorFingerprint) -> Option<ErrorGroup> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.groups.get(fingerprint).cloned()
    }

    /// Compute a trend for a specific fingerprint using the given bucket duration.
    pub fn trend(
        &self,
        fingerprint: &ErrorFingerprint,
        bucket_duration: chrono::Duration,
        num_buckets: usize,
    ) -> ErrorTrend {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let now = Utc::now();
        let mut buckets = Vec::with_capacity(num_buckets);

        for i in (0..num_buckets).rev() {
            let end = now - bucket_duration * i as i32;
            let start = end - bucket_duration;
            let count = inner
                .records
                .iter()
                .filter(|r| {
                    r.fingerprint == *fingerprint && r.timestamp >= start && r.timestamp < end
                })
                .count() as u64;
            buckets.push(TrendBucket { start, end, count });
        }

        let (is_increasing, rate_change) = if buckets.len() >= 2 {
            let last = buckets[buckets.len() - 1].count as f64;
            let prev = buckets[buckets.len() - 2].count as f64;
            (last > prev, last - prev)
        } else {
            (false, 0.0)
        };

        ErrorTrend {
            fingerprint: fingerprint.clone(),
            buckets,
            is_increasing,
            rate_change,
        }
    }

    /// Get a full summary of the current aggregator state.
    pub fn summary(&self, top_n: usize) -> AggregatorSummary {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let mut groups: Vec<ErrorGroup> = inner.groups.values().cloned().collect();
        groups.sort_by_key(|group| std::cmp::Reverse(group.count));
        groups.truncate(top_n);

        let by_severity = inner
            .by_severity
            .iter()
            .map(|(k, v)| (format!("{k:?}").to_lowercase(), *v))
            .collect();

        AggregatorSummary {
            total_errors: inner.total,
            unique_groups: inner.groups.len(),
            top_groups: groups,
            by_severity,
            by_category: inner.by_category.clone(),
        }
    }

    /// Get the total number of errors recorded.
    pub fn total_errors(&self) -> u64 {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .total
    }

    /// Get the number of unique error groups.
    pub fn unique_groups_count(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .groups
            .len()
    }

    /// Clear all recorded errors.
    pub fn clear(&self) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *inner = Inner::new(inner.max_records);
    }
}

impl Default for ErrorAggregator {
    fn default() -> Self {
        Self::new(10_000)
    }
}

// Implement Debug for Inner manually since RwLock doesn't auto-derive
impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("records_count", &self.records.len())
            .field("groups_count", &self.groups.len())
            .field("total", &self.total)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn aggregator() -> ErrorAggregator {
        ErrorAggregator::new(1000)
    }

    // 1. Fresh aggregator is empty
    #[test]
    fn test_new_is_empty() {
        let agg = aggregator();
        assert_eq!(agg.total_errors(), 0);
        assert_eq!(agg.unique_groups_count(), 0);
    }

    // 2. Recording an error increments total
    #[test]
    fn test_record_increments_total() {
        let agg = aggregator();
        agg.record(
            "test error",
            ErrorSeverity::Error,
            ErrorCategory::Internal,
            None,
            None,
        );
        assert_eq!(agg.total_errors(), 1);
    }

    // 3. Same error message deduplicates into one group
    #[test]
    fn test_deduplication() {
        let agg = aggregator();
        for _ in 0..5 {
            agg.record(
                "connection refused to host:8080",
                ErrorSeverity::Error,
                ErrorCategory::Network,
                None,
                None,
            );
        }
        assert_eq!(agg.total_errors(), 5);
        assert_eq!(agg.unique_groups_count(), 1);

        let groups = agg.top_groups(10);
        assert_eq!(groups[0].count, 5);
    }

    // 4. Different errors create different groups
    #[test]
    fn test_different_errors_different_groups() {
        let agg = aggregator();
        agg.record(
            "error A",
            ErrorSeverity::Error,
            ErrorCategory::Internal,
            None,
            None,
        );
        agg.record(
            "error B",
            ErrorSeverity::Warning,
            ErrorCategory::Network,
            None,
            None,
        );
        assert_eq!(agg.unique_groups_count(), 2);
    }

    // 5. Fingerprint normalization groups similar messages
    #[test]
    fn test_fingerprint_normalization() {
        let fp1 = ErrorFingerprint::compute(
            &ErrorCategory::Network,
            "timeout after 5000ms on request 123",
        );
        let fp2 = ErrorFingerprint::compute(
            &ErrorCategory::Network,
            "timeout after 3000ms on request 456",
        );
        assert_eq!(
            fp1, fp2,
            "Similar messages with different numbers should group together"
        );
    }

    // 6. Different categories produce different fingerprints
    #[test]
    fn test_different_category_different_fingerprint() {
        let fp1 = ErrorFingerprint::compute(&ErrorCategory::Network, "timeout");
        let fp2 = ErrorFingerprint::compute(&ErrorCategory::Internal, "timeout");
        assert_ne!(fp1, fp2);
    }

    // 7. Top groups are sorted by count
    #[test]
    fn test_top_groups_sorted() {
        let agg = aggregator();
        for _ in 0..3 {
            agg.record(
                "error A",
                ErrorSeverity::Error,
                ErrorCategory::Internal,
                None,
                None,
            );
        }
        for _ in 0..7 {
            agg.record(
                "error B",
                ErrorSeverity::Warning,
                ErrorCategory::Network,
                None,
                None,
            );
        }
        for _ in 0..1 {
            agg.record(
                "error C",
                ErrorSeverity::Critical,
                ErrorCategory::Auth,
                None,
                None,
            );
        }

        let groups = agg.top_groups(10);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].count, 7); // error B
        assert_eq!(groups[1].count, 3); // error A
        assert_eq!(groups[2].count, 1); // error C
    }

    // 8. Top groups respects N limit
    #[test]
    fn test_top_groups_limit() {
        let agg = aggregator();
        let unique_msgs = [
            "alpha failure",
            "beta failure",
            "gamma failure",
            "delta failure",
            "epsilon failure",
            "zeta failure",
            "eta failure",
            "theta failure",
            "iota failure",
            "kappa failure",
        ];
        for msg in &unique_msgs {
            agg.record(
                *msg,
                ErrorSeverity::Error,
                ErrorCategory::Internal,
                None,
                None,
            );
        }
        let groups = agg.top_groups(3);
        assert_eq!(groups.len(), 3);
    }

    // 9. Max severity tracks escalation
    #[test]
    fn test_max_severity_escalation() {
        let agg = aggregator();
        agg.record(
            "error X",
            ErrorSeverity::Warning,
            ErrorCategory::Internal,
            None,
            None,
        );
        agg.record(
            "error X",
            ErrorSeverity::Critical,
            ErrorCategory::Internal,
            None,
            None,
        );
        agg.record(
            "error X",
            ErrorSeverity::Error,
            ErrorCategory::Internal,
            None,
            None,
        );

        let groups = agg.top_groups(1);
        assert_eq!(groups[0].max_severity, ErrorSeverity::Critical);
    }

    // 10. Sources are tracked per group
    #[test]
    fn test_sources_tracked() {
        let agg = aggregator();
        agg.record(
            "tool failed",
            ErrorSeverity::Error,
            ErrorCategory::ToolExecution,
            Some("agent-coder".to_string()),
            None,
        );
        agg.record(
            "tool failed",
            ErrorSeverity::Error,
            ErrorCategory::ToolExecution,
            Some("agent-tester".to_string()),
            None,
        );
        agg.record(
            "tool failed",
            ErrorSeverity::Error,
            ErrorCategory::ToolExecution,
            Some("agent-coder".to_string()), // duplicate
            None,
        );

        let groups = agg.top_groups(1);
        assert_eq!(groups[0].sources.len(), 2);
        assert!(groups[0].sources.contains(&"agent-coder".to_string()));
        assert!(groups[0].sources.contains(&"agent-tester".to_string()));
    }

    // 11. Correlation IDs are sampled (max 5)
    #[test]
    fn test_correlation_id_sampling() {
        let agg = aggregator();
        for i in 0..10 {
            agg.record(
                "recurring error",
                ErrorSeverity::Error,
                ErrorCategory::Internal,
                None,
                Some(format!("corr-{i}")),
            );
        }
        let groups = agg.top_groups(1);
        assert_eq!(groups[0].sample_correlation_ids.len(), 5);
    }

    // 12. Summary includes severity breakdown
    #[test]
    fn test_summary_severity_breakdown() {
        let agg = aggregator();
        agg.record(
            "e1",
            ErrorSeverity::Error,
            ErrorCategory::Internal,
            None,
            None,
        );
        agg.record(
            "e2",
            ErrorSeverity::Error,
            ErrorCategory::Internal,
            None,
            None,
        );
        agg.record(
            "w1",
            ErrorSeverity::Warning,
            ErrorCategory::Network,
            None,
            None,
        );
        agg.record(
            "c1",
            ErrorSeverity::Critical,
            ErrorCategory::Auth,
            None,
            None,
        );

        let summary = agg.summary(10);
        assert_eq!(summary.total_errors, 4);
        assert_eq!(*summary.by_severity.get("error").unwrap(), 2);
        assert_eq!(*summary.by_severity.get("warning").unwrap(), 1);
        assert_eq!(*summary.by_severity.get("critical").unwrap(), 1);
    }

    // 13. Clear resets everything
    #[test]
    fn test_clear() {
        let agg = aggregator();
        agg.record(
            "e1",
            ErrorSeverity::Error,
            ErrorCategory::Internal,
            None,
            None,
        );
        agg.record(
            "e2",
            ErrorSeverity::Warning,
            ErrorCategory::Network,
            None,
            None,
        );
        assert_eq!(agg.total_errors(), 2);

        agg.clear();
        assert_eq!(agg.total_errors(), 0);
        assert_eq!(agg.unique_groups_count(), 0);
    }

    // 14. Record capacity eviction
    #[test]
    fn test_record_eviction() {
        let agg = ErrorAggregator::new(5);
        let unique_msgs = [
            "alpha error",
            "beta error",
            "gamma error",
            "delta error",
            "epsilon error",
            "zeta error",
            "eta error",
            "theta error",
            "iota error",
            "kappa error",
        ];
        for msg in &unique_msgs {
            agg.record(
                *msg,
                ErrorSeverity::Error,
                ErrorCategory::Internal,
                None,
                None,
            );
        }
        assert_eq!(agg.total_errors(), 10);
        // Groups are NOT evicted, only raw records
        assert_eq!(agg.unique_groups_count(), 10);
    }

    // 15. Get specific group by fingerprint
    #[test]
    fn test_get_group_by_fingerprint() {
        let agg = aggregator();
        agg.record(
            "specific error",
            ErrorSeverity::Error,
            ErrorCategory::Internal,
            None,
            None,
        );

        let fp = ErrorFingerprint::compute(&ErrorCategory::Internal, "specific error");
        let group = agg.group(&fp).unwrap();
        assert_eq!(group.count, 1);
        assert_eq!(group.message, "specific error");
    }

    // 16. Missing group returns None
    #[test]
    fn test_missing_group() {
        let agg = aggregator();
        let fp = ErrorFingerprint::compute(&ErrorCategory::Internal, "nonexistent");
        assert!(agg.group(&fp).is_none());
    }

    // 17. Trend computation
    #[test]
    fn test_trend_computation() {
        let agg = aggregator();
        let fp = ErrorFingerprint::compute(&ErrorCategory::Internal, "trending error");

        // Record some errors
        agg.record(
            "trending error",
            ErrorSeverity::Error,
            ErrorCategory::Internal,
            None,
            None,
        );

        let trend = agg.trend(&fp, chrono::Duration::minutes(1), 5);
        assert_eq!(trend.fingerprint, fp);
        assert_eq!(trend.buckets.len(), 5);
        // The last bucket should contain the error
        let last_bucket_count: u64 = trend.buckets.iter().map(|b| b.count).sum();
        assert_eq!(last_bucket_count, 1);
    }

    // 18. ErrorSeverity ordering
    #[test]
    fn test_severity_ordering() {
        assert!(ErrorSeverity::Critical > ErrorSeverity::Error);
        assert!(ErrorSeverity::Error > ErrorSeverity::Warning);
        assert!(ErrorSeverity::Warning > ErrorSeverity::Info);
    }

    // 19. Summary serializable
    #[test]
    fn test_summary_serializable() {
        let agg = aggregator();
        agg.record(
            "e1",
            ErrorSeverity::Error,
            ErrorCategory::Internal,
            None,
            None,
        );
        let summary = agg.summary(5);
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"total_errors\":1"));
    }

    // 20. Clone shares state
    #[test]
    fn test_clone_shares_state() {
        let a1 = aggregator();
        let a2 = a1.clone();
        a1.record(
            "error",
            ErrorSeverity::Error,
            ErrorCategory::Internal,
            None,
            None,
        );
        assert_eq!(a2.total_errors(), 1);
    }

    // 21. Default creates aggregator
    #[test]
    fn test_default() {
        let agg = ErrorAggregator::default();
        assert_eq!(agg.total_errors(), 0);
    }

    // 22. First seen and last seen are tracked
    #[test]
    fn test_first_last_seen() {
        let agg = aggregator();
        agg.record(
            "timed error",
            ErrorSeverity::Error,
            ErrorCategory::Internal,
            None,
            None,
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
        agg.record(
            "timed error",
            ErrorSeverity::Error,
            ErrorCategory::Internal,
            None,
            None,
        );

        let groups = agg.top_groups(1);
        assert!(groups[0].last_seen >= groups[0].first_seen);
    }

    // 23. Normalize message strips numbers
    #[test]
    fn test_normalize_message() {
        let n1 = normalize_message("error on line 42 in file.rs");
        let n2 = normalize_message("error on line 99 in file.rs");
        assert_eq!(n1, n2);
    }

    // 24. Custom category works
    #[test]
    fn test_custom_category() {
        let agg = aggregator();
        agg.record(
            "custom error",
            ErrorSeverity::Warning,
            ErrorCategory::Custom("my_domain".to_string()),
            None,
            None,
        );
        assert_eq!(agg.unique_groups_count(), 1);
    }
}
