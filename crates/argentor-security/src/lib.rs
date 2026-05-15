//! Security primitives for the Argentor framework.
//!
//! Provides capabilities, permissions, rate limiting, audit logging,
//! input sanitization, and TLS configuration used throughout the system.
//!
//! # Main types
//!
//! - [`Capability`] — A fine-grained permission token (file, network, shell, etc.).
//! - [`PermissionSet`] — A collection of granted capabilities.
//! - [`RateLimiter`] — Token-bucket rate limiter for request throttling.
//! - [`AuditLog`] — Append-only audit trail persisted to disk.
//! - [`Sanitizer`] — Input sanitization utilities.
//! - [`TlsConfig`] — TLS and mutual-TLS configuration.
//!
//! # Security boundaries (IMPORTANT)
//!
//! This crate provides **policy primitives**: it answers "is this path allowed?",
//! "is this IP allowed?", etc., given a [`PermissionSet`]. It does NOT:
//!
//! - Resolve DNS. If a skill accepts hostnames, it must resolve and re-check the
//!   resolved IP at request time (see `HttpFetchSkill` in `argentor-builtins`).
//!   **DNS rebinding** protection is a skill-level concern, not a PermissionSet one.
//!   See GitHub issue #8 for the canonical reference.
//!
//! - Follow filesystem symlinks. [`PermissionSet`] validates paths logically;
//!   skills that read/write files are responsible for canonicalizing (e.g.
//!   `std::fs::canonicalize`) and re-checking the resolved path to prevent
//!   symlink escape.
//!
//! - Decode transport-layer encodings (URL percent-encoding, base64). Callers
//!   that receive user-controlled encoded input must decode before passing to
//!   [`PermissionSet`]. Tracked gaps: GitHub issues #4 (URL-encoded traversal),
//!   #5 (Unicode/UTF-8 overlong).
//!
//! In short: `argentor-security` is the last-line-of-defense policy engine.
//! Skills are the first-line-of-defense input validators.

/// Alert rule engine for production monitoring.
pub mod alert_engine;
/// Audit logging module.
pub mod audit;
/// Audit log export to SIEM formats (Splunk, Elasticsearch, CEF, JSON-LD, CSV, Syslog).
pub mod audit_export;
/// Audit log query and filtering.
pub mod audit_query;
/// Capability and permission definitions.
pub mod capability;
/// Encrypted at-rest storage (AES-256-GCM).
pub mod encrypted_store;
/// Production observability: metrics collection and Prometheus export.
pub mod observability;
/// Token-bucket rate limiting.
pub mod rate_limit;
/// Role-Based Access Control (RBAC).
pub mod rbac;
/// Input sanitization utilities.
pub mod sanitizer;
/// SLA compliance tracking with uptime, availability, and incident windows.
pub mod sla_tracker;
/// Per-tenant rate limiting and quota enforcement for multi-tenant SaaS.
pub mod tenant_limits;
/// TLS and mutual-TLS configuration.
pub mod tls;

pub use alert_engine::{
    Alert, AlertCondition, AlertEngine, AlertEngineStats, AlertRule, AlertSeverity,
};
pub use audit::{AuditLog, AuditRotationConfig};
pub use audit_export::{
    AuditExportState, AuditExporter, ExportConfig, ExportFormat, ExportQuery, ExportResponse,
};
pub use audit_query::{query_audit_log, AuditFilter, AuditQueryResult};
pub use capability::{is_private_ip, Capability, PermissionSet, ShellCheckResult};
pub use encrypted_store::{decrypt_value, derive_key, encrypt_value, EncryptedStore, KEY_SIZE};
pub use observability::{
    AgentMetricsCollector, AgentMetricsSummary, MetricEvent, MetricsSummary, SecurityEventType,
    ToolMetricsSummary,
};
pub use rate_limit::RateLimiter;
pub use rbac::{check_permission, Permission, PolicyBinding, RbacDecision, RbacPolicy, Role};
pub use sanitizer::Sanitizer;
pub use sla_tracker::{Incident, SlaComplianceReport, SlaDefinition, SlaStatus, SlaTracker};
pub use tls::TlsConfig;
