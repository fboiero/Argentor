//! Audit log export to SIEM formats (Splunk HEC, Elasticsearch, CEF, JSON-LD, CSV, Syslog).
//!
//! Provides [`AuditExporter`] for converting [`AuditEntry`] records into industry-standard
//! SIEM ingestion formats, plus configuration and REST-ready handler types.
//!
//! # Supported formats
//!
//! - **Splunk HEC** — HTTP Event Collector JSON (`/services/collector/event`)
//! - **Elasticsearch** — Bulk API NDJSON (`_bulk`)
//! - **CEF** — Common Event Format (ArcSight / QRadar)
//! - **JSON-LD** — Linked Data with schema.org Action vocabulary
//! - **CSV** — Comma-separated values with headers
//! - **Syslog** — RFC 5424 structured data

use crate::audit::{AuditEntry, AuditOutcome};
use crate::audit_query::{query_audit_log, AuditFilter};
use chrono::{Datelike, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// ExportFormat
// ---------------------------------------------------------------------------

/// Supported SIEM export formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// Splunk HTTP Event Collector JSON.
    Splunk,
    /// Elasticsearch Bulk API NDJSON.
    Elasticsearch,
    /// Common Event Format (ArcSight / QRadar).
    Cef,
    /// JSON-LD with schema.org Action vocabulary.
    JsonLd,
    /// Comma-separated values with headers.
    Csv,
    /// RFC 5424 structured syslog.
    Syslog,
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Splunk => write!(f, "splunk"),
            Self::Elasticsearch => write!(f, "elasticsearch"),
            Self::Cef => write!(f, "cef"),
            Self::JsonLd => write!(f, "jsonld"),
            Self::Csv => write!(f, "csv"),
            Self::Syslog => write!(f, "syslog"),
        }
    }
}

// ---------------------------------------------------------------------------
// ExportConfig
// ---------------------------------------------------------------------------

/// Configuration for audit log exports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportConfig {
    /// Target format.
    pub format: ExportFormat,
    /// Hostname used in generated events (default: `"argentor"`).
    pub hostname: String,
    /// Elasticsearch / Splunk index prefix (default: `"argentor-audit"`).
    pub index_prefix: String,
    /// Whether to include extra metadata fields.
    pub include_metadata: bool,
    /// strftime-compatible date format string (default: `"%Y-%m-%dT%H:%M:%S%.3fZ"`).
    pub date_format: String,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            format: ExportFormat::Splunk,
            hostname: "argentor".into(),
            index_prefix: "argentor-audit".into(),
            include_metadata: true,
            date_format: "%Y-%m-%dT%H:%M:%S%.3fZ".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// SplunkHecEvent
// ---------------------------------------------------------------------------

/// Splunk HTTP Event Collector event envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplunkHecEvent {
    /// Unix epoch seconds (with fractional part).
    pub time: f64,
    /// Originating host.
    pub host: String,
    /// Event source identifier.
    pub source: String,
    /// Source type for Splunk indexing.
    pub sourcetype: String,
    /// Target Splunk index.
    pub index: String,
    /// Payload (the serialised audit entry).
    pub event: serde_json::Value,
}

// ---------------------------------------------------------------------------
// ElasticsearchBulkAction
// ---------------------------------------------------------------------------

/// Elasticsearch bulk-API action line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElasticsearchBulkAction {
    /// The index operation metadata.
    pub index: ElasticsearchBulkIndex,
}

/// Inner `"index"` metadata for the bulk action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElasticsearchBulkIndex {
    /// Target Elasticsearch index name.
    pub _index: String,
    /// Document identifier.
    pub _id: String,
}

// ---------------------------------------------------------------------------
// CefEvent (helper)
// ---------------------------------------------------------------------------

/// Parsed representation of a CEF line -- useful for tests and validation.
#[derive(Debug, Clone)]
pub struct CefEvent {
    /// CEF version (always 0).
    pub version: u8,
    /// Vendor name in the CEF header.
    pub device_vendor: String,
    /// Product name in the CEF header.
    pub device_product: String,
    /// Product version string.
    pub device_version: String,
    /// Unique event type identifier.
    pub signature_id: String,
    /// Human-readable event name.
    pub name: String,
    /// Numeric severity (0-10).
    pub severity: u8,
    /// CEF extension key=value pairs.
    pub extension: String,
}

// ---------------------------------------------------------------------------
// Severity mapping
// ---------------------------------------------------------------------------

/// Map [`AuditOutcome`] to a CEF / syslog numeric severity.
///
/// - `Success` → 1  (informational)
/// - `Error`   → 5  (warning)
/// - `Denied`  → 7  (high)
pub fn outcome_severity(outcome: &AuditOutcome) -> u8 {
    match outcome {
        AuditOutcome::Success => 1,
        AuditOutcome::Error => 5,
        AuditOutcome::Denied => 7,
    }
}

/// Map [`AuditOutcome`] to a syslog facility + severity PRI value (RFC 5424).
/// We use facility=16 (local0) and the CEF severity mapped to syslog severity.
fn syslog_pri(outcome: &AuditOutcome) -> u8 {
    let facility: u8 = 16; // local0
    let severity = match outcome {
        AuditOutcome::Success => 6, // informational
        AuditOutcome::Error => 4,   // warning
        AuditOutcome::Denied => 2,  // critical
    };
    facility * 8 + severity
}

/// Outcome as a lowercase string.
fn outcome_str(outcome: &AuditOutcome) -> &'static str {
    match outcome {
        AuditOutcome::Success => "success",
        AuditOutcome::Denied => "denied",
        AuditOutcome::Error => "error",
    }
}

// ---------------------------------------------------------------------------
// AuditExporter
// ---------------------------------------------------------------------------

/// Stateless converter from [`AuditEntry`] slices to SIEM-format strings.
pub struct AuditExporter;

impl AuditExporter {
    // -- Splunk HEC --------------------------------------------------------

    /// Convert entries to Splunk HTTP Event Collector JSON objects.
    ///
    /// Each returned string is a self-contained JSON object suitable for
    /// `POST /services/collector/event`.
    pub fn export_splunk_hec(entries: &[AuditEntry], config: &ExportConfig) -> Vec<String> {
        entries
            .iter()
            .map(|e| {
                let hec = SplunkHecEvent {
                    time: e.timestamp.timestamp() as f64
                        + (e.timestamp.timestamp_subsec_millis() as f64 / 1000.0),
                    host: config.hostname.clone(),
                    source: "argentor".into(),
                    sourcetype: "argentor:audit".into(),
                    index: config.index_prefix.clone(),
                    event: serde_json::to_value(e).unwrap_or(serde_json::Value::Null),
                };
                serde_json::to_string(&hec).unwrap_or_default()
            })
            .collect()
    }

    // -- Elasticsearch bulk ------------------------------------------------

    /// Convert entries to Elasticsearch bulk API NDJSON.
    ///
    /// Returns pairs of lines: action-metadata followed by the document body.
    /// The index name is `{index_prefix}-{YYYY.MM}`.
    pub fn export_elasticsearch(entries: &[AuditEntry], config: &ExportConfig) -> Vec<String> {
        let mut lines = Vec::with_capacity(entries.len() * 2);
        for entry in entries {
            let index_name = format!(
                "{}-{}.{:02}",
                config.index_prefix,
                entry.timestamp.year(),
                entry.timestamp.month(),
            );
            let action = ElasticsearchBulkAction {
                index: ElasticsearchBulkIndex {
                    _index: index_name,
                    _id: format!(
                        "{}-{}",
                        entry.session_id,
                        entry.timestamp.timestamp_nanos_opt().unwrap_or(0)
                    ),
                },
            };
            lines.push(serde_json::to_string(&action).unwrap_or_default());
            lines.push(serde_json::to_string(entry).unwrap_or_default());
        }
        lines
    }

    // -- CEF ---------------------------------------------------------------

    /// Convert entries to Common Event Format lines.
    ///
    /// Format: `CEF:0|Argentor|AgentRuntime|1.0|{action}|{action}|{severity}|src=... outcome=... msg=...`
    pub fn export_cef(entries: &[AuditEntry], config: &ExportConfig) -> Vec<String> {
        entries
            .iter()
            .map(|e| {
                let severity = outcome_severity(&e.outcome);
                let details = e
                    .details
                    .to_string()
                    .replace('|', "\\|")
                    .replace('\\', "\\\\");
                let skill = e.skill_name.as_deref().unwrap_or("-");
                format!(
                    "CEF:0|Argentor|AgentRuntime|1.0|{action}|{action}|{severity}|\
                     src={session_id} outcome={outcome} skill={skill} \
                     dhost={host} msg={details}",
                    action = e.action,
                    severity = severity,
                    session_id = e.session_id,
                    outcome = outcome_str(&e.outcome),
                    skill = skill,
                    host = config.hostname,
                    details = details,
                )
            })
            .collect()
    }

    // -- JSON-LD -----------------------------------------------------------

    /// Convert entries to JSON-LD using schema.org Action vocabulary.
    pub fn export_json_ld(entries: &[AuditEntry], config: &ExportConfig) -> Vec<String> {
        entries
            .iter()
            .map(|e| {
                let action_status = match &e.outcome {
                    AuditOutcome::Success => "CompletedActionStatus",
                    AuditOutcome::Denied => "FailedActionStatus",
                    AuditOutcome::Error => "FailedActionStatus",
                };
                let mut obj = serde_json::json!({
                    "@context": "https://schema.org",
                    "@type": "Action",
                    "name": e.action,
                    "actionStatus": format!("https://schema.org/{action_status}"),
                    "startTime": e.timestamp.format(&config.date_format).to_string(),
                    "agent": {
                        "@type": "SoftwareApplication",
                        "name": "Argentor",
                        "identifier": e.session_id.to_string(),
                    },
                    "result": {
                        "@type": "Thing",
                        "name": outcome_str(&e.outcome),
                        "description": e.details,
                    },
                });
                if let Some(skill) = &e.skill_name {
                    obj["instrument"] = serde_json::json!({
                        "@type": "SoftwareApplication",
                        "name": skill,
                    });
                }
                serde_json::to_string(&obj).unwrap_or_default()
            })
            .collect()
    }

    // -- CSV ---------------------------------------------------------------

    /// Export entries as a single CSV string with headers.
    ///
    /// Columns: `timestamp,session_id,action,skill_name,outcome,details`
    pub fn export_csv(entries: &[AuditEntry], config: &ExportConfig) -> String {
        let mut buf = String::from("timestamp,session_id,action,skill_name,outcome,details\n");
        for e in entries {
            let ts = e.timestamp.format(&config.date_format).to_string();
            let skill = e.skill_name.as_deref().unwrap_or("");
            let details = e.details.to_string().replace('"', "\"\"");
            buf.push_str(&format!(
                "{ts},{session_id},{action},{skill},{outcome},\"{details}\"\n",
                ts = ts,
                session_id = e.session_id,
                action = e.action,
                skill = skill,
                outcome = outcome_str(&e.outcome),
                details = details,
            ));
        }
        buf
    }

    // -- Syslog (RFC 5424) -------------------------------------------------

    /// Convert entries to RFC 5424 syslog messages.
    ///
    /// `<PRI>1 TIMESTAMP HOSTNAME argentor - - [audit session_id="..." action="..." outcome="..."] MSG`
    pub fn export_syslog(entries: &[AuditEntry], config: &ExportConfig) -> Vec<String> {
        entries
            .iter()
            .map(|e| {
                let pri = syslog_pri(&e.outcome);
                let ts = e.timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
                let skill = e.skill_name.as_deref().unwrap_or("-");
                format!(
                    "<{pri}>1 {ts} {host} argentor - - \
                     [audit session_id=\"{sid}\" action=\"{action}\" \
                     skill=\"{skill}\" outcome=\"{outcome}\"] {msg}",
                    pri = pri,
                    ts = ts,
                    host = config.hostname,
                    sid = e.session_id,
                    action = e.action,
                    skill = skill,
                    outcome = outcome_str(&e.outcome),
                    msg = e.details,
                )
            })
            .collect()
    }

    // -- Dispatch by config ------------------------------------------------

    /// Export using the format specified in `config`.
    ///
    /// Returns individual formatted strings (one per entry for most formats,
    /// two per entry for Elasticsearch, or a single CSV blob wrapped in a Vec).
    pub fn export(entries: &[AuditEntry], config: &ExportConfig) -> Vec<String> {
        match config.format {
            ExportFormat::Splunk => Self::export_splunk_hec(entries, config),
            ExportFormat::Elasticsearch => Self::export_elasticsearch(entries, config),
            ExportFormat::Cef => Self::export_cef(entries, config),
            ExportFormat::JsonLd => Self::export_json_ld(entries, config),
            ExportFormat::Csv => vec![Self::export_csv(entries, config)],
            ExportFormat::Syslog => Self::export_syslog(entries, config),
        }
    }
}

// ---------------------------------------------------------------------------
// AuditExportState
// ---------------------------------------------------------------------------

/// Shared state for the audit export endpoints.
#[derive(Debug, Clone)]
pub struct AuditExportState {
    /// Directory containing `audit.jsonl`.
    pub log_dir: PathBuf,
    /// Export configuration.
    pub config: ExportConfig,
}

// ---------------------------------------------------------------------------
// REST-ready handler types (framework-agnostic)
// ---------------------------------------------------------------------------

/// Query parameters accepted by the export endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExportQuery {
    /// Target format (splunk | elasticsearch | cef | jsonld | csv | syslog).
    pub format: Option<String>,
    /// ISO-8601 start time.
    pub from: Option<String>,
    /// ISO-8601 end time.
    pub to: Option<String>,
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Filter by action.
    pub action: Option<String>,
    /// Maximum entries.
    pub limit: Option<usize>,
}

/// Response body for the export endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct ExportResponse {
    /// The format used.
    pub format: String,
    /// Number of entries exported.
    pub count: usize,
    /// The formatted data lines.
    pub data: Vec<String>,
}

/// Response body for the config endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct ExportConfigResponse {
    /// The current export configuration.
    pub config: ExportConfig,
}

/// Build an [`AuditFilter`] from an [`ExportQuery`].
pub fn build_filter_from_query(query: &ExportQuery) -> AuditFilter {
    use chrono::DateTime;

    let mut filter = AuditFilter::all();

    if let Some(ref sid) = query.session_id {
        if let Ok(uuid) = sid.parse() {
            filter.session_id = Some(uuid);
        }
    }
    if let Some(ref action) = query.action {
        filter.action = Some(action.clone());
    }
    if let Some(ref from) = query.from {
        if let Ok(dt) = DateTime::parse_from_rfc3339(from) {
            filter.after = Some(dt.with_timezone(&Utc));
        }
    }
    if let Some(ref to) = query.to {
        if let Ok(dt) = DateTime::parse_from_rfc3339(to) {
            filter.before = Some(dt.with_timezone(&Utc));
        }
    }
    if let Some(limit) = query.limit {
        filter.limit = limit;
    }

    filter
}

/// Parse an [`ExportFormat`] from a string, defaulting to the config value.
pub fn parse_format(s: Option<&str>, default: ExportFormat) -> ExportFormat {
    match s {
        Some("splunk") => ExportFormat::Splunk,
        Some("elasticsearch") | Some("elastic") | Some("elk") => ExportFormat::Elasticsearch,
        Some("cef") => ExportFormat::Cef,
        Some("jsonld") | Some("json-ld") => ExportFormat::JsonLd,
        Some("csv") => ExportFormat::Csv,
        Some("syslog") => ExportFormat::Syslog,
        _ => default,
    }
}

/// Handle `GET /api/v1/audit/export` — export filtered audit data.
///
/// This is framework-agnostic: call it from an axum handler, passing the
/// deserialized query params and shared state.
pub fn handle_export(
    state: &AuditExportState,
    query: &ExportQuery,
) -> Result<ExportResponse, String> {
    let filter = build_filter_from_query(query);
    let log_path = state.log_dir.join("audit.jsonl");
    let result = query_audit_log(&log_path, &filter)?;

    let format = parse_format(query.format.as_deref(), state.config.format);
    let mut cfg = state.config.clone();
    cfg.format = format;

    let data = AuditExporter::export(&result.entries, &cfg);
    Ok(ExportResponse {
        format: format.to_string(),
        count: result.entries.len(),
        data,
    })
}

/// Handle `GET /api/v1/audit/export/config` — return current config.
pub fn handle_export_config(state: &AuditExportState) -> ExportConfigResponse {
    ExportConfigResponse {
        config: state.config.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::audit::{AuditEntry, AuditOutcome};
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    fn default_config() -> ExportConfig {
        ExportConfig::default()
    }

    fn make_entry(action: &str, skill: Option<&str>, outcome: AuditOutcome) -> AuditEntry {
        AuditEntry {
            timestamp: Utc.with_ymd_and_hms(2026, 4, 1, 12, 0, 0).unwrap(),
            session_id: Uuid::parse_str("a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8").unwrap(),
            action: action.into(),
            skill_name: skill.map(String::from),
            details: serde_json::json!({"key": "value"}),
            outcome,
            correlation_id: None,
        }
    }

    fn sample_entries() -> Vec<AuditEntry> {
        vec![
            make_entry("tool_call", Some("echo"), AuditOutcome::Success),
            make_entry("permission_check", None, AuditOutcome::Denied),
            make_entry("agent_response", Some("time"), AuditOutcome::Error),
        ]
    }

    // -- Splunk HEC --------------------------------------------------------

    #[test]
    fn splunk_hec_produces_valid_json() {
        let entries = sample_entries();
        let cfg = default_config();
        let lines = AuditExporter::export_splunk_hec(&entries, &cfg);
        assert_eq!(lines.len(), 3);
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(v.get("time").is_some());
            assert_eq!(v["source"], "argentor");
            assert_eq!(v["sourcetype"], "argentor:audit");
        }
    }

    #[test]
    fn splunk_hec_host_from_config() {
        let entries = vec![make_entry("test", None, AuditOutcome::Success)];
        let mut cfg = default_config();
        cfg.hostname = "myhost".into();
        let lines = AuditExporter::export_splunk_hec(&entries, &cfg);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(v["host"], "myhost");
    }

    #[test]
    fn splunk_hec_index_from_config() {
        let entries = vec![make_entry("test", None, AuditOutcome::Success)];
        let mut cfg = default_config();
        cfg.index_prefix = "custom-index".into();
        let lines = AuditExporter::export_splunk_hec(&entries, &cfg);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(v["index"], "custom-index");
    }

    #[test]
    fn splunk_hec_time_is_epoch() {
        let entries = vec![make_entry("test", None, AuditOutcome::Success)];
        let cfg = default_config();
        let lines = AuditExporter::export_splunk_hec(&entries, &cfg);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        let t = v["time"].as_f64().unwrap();
        // 2026-04-01T12:00:00Z as epoch
        assert!(t > 1_700_000_000.0);
    }

    // -- Elasticsearch bulk ------------------------------------------------

    #[test]
    fn elasticsearch_bulk_produces_pairs() {
        let entries = sample_entries();
        let cfg = default_config();
        let lines = AuditExporter::export_elasticsearch(&entries, &cfg);
        // 3 entries => 6 lines (action + doc each)
        assert_eq!(lines.len(), 6);
    }

    #[test]
    fn elasticsearch_bulk_action_has_index() {
        let entries = vec![make_entry("test", None, AuditOutcome::Success)];
        let cfg = default_config();
        let lines = AuditExporter::export_elasticsearch(&entries, &cfg);
        let action: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        let idx = action["index"]["_index"].as_str().unwrap();
        assert!(idx.starts_with("argentor-audit-2026.04"));
    }

    #[test]
    fn elasticsearch_bulk_doc_is_valid_entry() {
        let entries = vec![make_entry("tool_call", Some("echo"), AuditOutcome::Success)];
        let cfg = default_config();
        let lines = AuditExporter::export_elasticsearch(&entries, &cfg);
        let doc: AuditEntry = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(doc.action, "tool_call");
    }

    // -- CEF ---------------------------------------------------------------

    #[test]
    fn cef_format_starts_with_header() {
        let entries = sample_entries();
        let cfg = default_config();
        let lines = AuditExporter::export_cef(&entries, &cfg);
        assert_eq!(lines.len(), 3);
        for line in &lines {
            assert!(line.starts_with("CEF:0|Argentor|AgentRuntime|1.0|"));
        }
    }

    #[test]
    fn cef_severity_mapping() {
        assert_eq!(outcome_severity(&AuditOutcome::Success), 1);
        assert_eq!(outcome_severity(&AuditOutcome::Error), 5);
        assert_eq!(outcome_severity(&AuditOutcome::Denied), 7);
    }

    #[test]
    fn cef_contains_session_and_outcome() {
        let entries = vec![make_entry("tool_call", None, AuditOutcome::Denied)];
        let cfg = default_config();
        let lines = AuditExporter::export_cef(&entries, &cfg);
        assert!(lines[0].contains("outcome=denied"));
        assert!(lines[0].contains("src=a1a2a3a4"));
    }

    #[test]
    fn cef_contains_skill() {
        let entries = vec![make_entry("tool_call", Some("echo"), AuditOutcome::Success)];
        let cfg = default_config();
        let lines = AuditExporter::export_cef(&entries, &cfg);
        assert!(lines[0].contains("skill=echo"));
    }

    // -- JSON-LD -----------------------------------------------------------

    #[test]
    fn json_ld_has_context_and_type() {
        let entries = vec![make_entry("test", None, AuditOutcome::Success)];
        let cfg = default_config();
        let lines = AuditExporter::export_json_ld(&entries, &cfg);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(v["@context"], "https://schema.org");
        assert_eq!(v["@type"], "Action");
    }

    #[test]
    fn json_ld_action_status_success() {
        let entries = vec![make_entry("test", None, AuditOutcome::Success)];
        let cfg = default_config();
        let lines = AuditExporter::export_json_ld(&entries, &cfg);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(
            v["actionStatus"],
            "https://schema.org/CompletedActionStatus"
        );
    }

    #[test]
    fn json_ld_action_status_denied() {
        let entries = vec![make_entry("test", None, AuditOutcome::Denied)];
        let cfg = default_config();
        let lines = AuditExporter::export_json_ld(&entries, &cfg);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(v["actionStatus"], "https://schema.org/FailedActionStatus");
    }

    #[test]
    fn json_ld_includes_instrument_when_skill_present() {
        let entries = vec![make_entry("test", Some("echo"), AuditOutcome::Success)];
        let cfg = default_config();
        let lines = AuditExporter::export_json_ld(&entries, &cfg);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(v["instrument"]["name"], "echo");
    }

    #[test]
    fn json_ld_no_instrument_when_no_skill() {
        let entries = vec![make_entry("test", None, AuditOutcome::Success)];
        let cfg = default_config();
        let lines = AuditExporter::export_json_ld(&entries, &cfg);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(v.get("instrument").is_none());
    }

    // -- CSV ---------------------------------------------------------------

    #[test]
    fn csv_has_header_row() {
        let entries = sample_entries();
        let cfg = default_config();
        let csv = AuditExporter::export_csv(&entries, &cfg);
        let first_line = csv.lines().next().unwrap();
        assert_eq!(
            first_line,
            "timestamp,session_id,action,skill_name,outcome,details"
        );
    }

    #[test]
    fn csv_row_count_matches_entries() {
        let entries = sample_entries();
        let cfg = default_config();
        let csv = AuditExporter::export_csv(&entries, &cfg);
        // header + 3 data rows + trailing newline
        let non_empty_lines: Vec<_> = csv.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(non_empty_lines.len(), 4); // 1 header + 3 data
    }

    #[test]
    fn csv_contains_entry_data() {
        let entries = vec![make_entry("tool_call", Some("echo"), AuditOutcome::Success)];
        let cfg = default_config();
        let csv = AuditExporter::export_csv(&entries, &cfg);
        assert!(csv.contains("tool_call"));
        assert!(csv.contains("echo"));
        assert!(csv.contains("success"));
    }

    // -- Syslog (RFC 5424) -------------------------------------------------

    #[test]
    fn syslog_format_structure() {
        let entries = vec![make_entry("test", None, AuditOutcome::Success)];
        let cfg = default_config();
        let lines = AuditExporter::export_syslog(&entries, &cfg);
        assert_eq!(lines.len(), 1);
        // RFC 5424: <PRI>VERSION TIMESTAMP HOSTNAME APP-NAME ...
        assert!(lines[0].starts_with('<'));
        assert!(lines[0].contains("argentor"));
        assert!(lines[0].contains("[audit "));
    }

    #[test]
    fn syslog_pri_values() {
        // local0 (facility=16) => base = 128
        // informational (6) => 134
        assert_eq!(syslog_pri(&AuditOutcome::Success), 134);
        // warning (4) => 132
        assert_eq!(syslog_pri(&AuditOutcome::Error), 132);
        // critical (2) => 130
        assert_eq!(syslog_pri(&AuditOutcome::Denied), 130);
    }

    #[test]
    fn syslog_contains_structured_data() {
        let entries = vec![make_entry("tool_call", Some("echo"), AuditOutcome::Success)];
        let cfg = default_config();
        let lines = AuditExporter::export_syslog(&entries, &cfg);
        assert!(lines[0].contains("session_id=\"a1a2a3a4"));
        assert!(lines[0].contains("action=\"tool_call\""));
        assert!(lines[0].contains("outcome=\"success\""));
        assert!(lines[0].contains("skill=\"echo\""));
    }

    // -- ExportConfig defaults ---------------------------------------------

    #[test]
    fn export_config_defaults() {
        let cfg = ExportConfig::default();
        assert_eq!(cfg.hostname, "argentor");
        assert_eq!(cfg.index_prefix, "argentor-audit");
        assert!(cfg.include_metadata);
        assert_eq!(cfg.format, ExportFormat::Splunk);
    }

    // -- Dispatch ----------------------------------------------------------

    #[test]
    fn export_dispatch_splunk() {
        let entries = sample_entries();
        let mut cfg = default_config();
        cfg.format = ExportFormat::Splunk;
        let lines = AuditExporter::export(&entries, &cfg);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn export_dispatch_csv_wraps_in_vec() {
        let entries = sample_entries();
        let mut cfg = default_config();
        cfg.format = ExportFormat::Csv;
        let lines = AuditExporter::export(&entries, &cfg);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("timestamp,session_id"));
    }

    // -- Timestamp formatting ----------------------------------------------

    #[test]
    fn timestamp_formatting_in_csv() {
        let entries = vec![make_entry("test", None, AuditOutcome::Success)];
        let cfg = default_config();
        let csv = AuditExporter::export_csv(&entries, &cfg);
        // Default format: %Y-%m-%dT%H:%M:%S%.3fZ
        assert!(csv.contains("2026-04-01T12:00:00.000Z"));
    }

    // -- parse_format ------------------------------------------------------

    #[test]
    fn parse_format_known_values() {
        assert_eq!(
            parse_format(Some("splunk"), ExportFormat::Csv),
            ExportFormat::Splunk
        );
        assert_eq!(
            parse_format(Some("elk"), ExportFormat::Csv),
            ExportFormat::Elasticsearch
        );
        assert_eq!(
            parse_format(Some("elastic"), ExportFormat::Csv),
            ExportFormat::Elasticsearch
        );
        assert_eq!(
            parse_format(Some("cef"), ExportFormat::Csv),
            ExportFormat::Cef
        );
        assert_eq!(
            parse_format(Some("jsonld"), ExportFormat::Csv),
            ExportFormat::JsonLd
        );
        assert_eq!(
            parse_format(Some("json-ld"), ExportFormat::Csv),
            ExportFormat::JsonLd
        );
        assert_eq!(
            parse_format(Some("csv"), ExportFormat::Splunk),
            ExportFormat::Csv
        );
        assert_eq!(
            parse_format(Some("syslog"), ExportFormat::Csv),
            ExportFormat::Syslog
        );
    }

    #[test]
    fn parse_format_unknown_returns_default() {
        assert_eq!(
            parse_format(Some("unknown"), ExportFormat::Cef),
            ExportFormat::Cef
        );
        assert_eq!(
            parse_format(None, ExportFormat::Syslog),
            ExportFormat::Syslog
        );
    }

    // -- Filter integration ------------------------------------------------

    #[test]
    fn build_filter_from_query_all_fields() {
        let query = ExportQuery {
            format: Some("splunk".into()),
            from: Some("2026-01-01T00:00:00Z".into()),
            to: Some("2026-12-31T23:59:59Z".into()),
            session_id: Some("a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8".into()),
            action: Some("tool_call".into()),
            limit: Some(10),
        };
        let filter = build_filter_from_query(&query);
        assert!(filter.session_id.is_some());
        assert_eq!(filter.action.as_deref(), Some("tool_call"));
        assert!(filter.after.is_some());
        assert!(filter.before.is_some());
        assert_eq!(filter.limit, 10);
    }

    #[test]
    fn build_filter_from_empty_query() {
        let query = ExportQuery::default();
        let filter = build_filter_from_query(&query);
        assert!(filter.session_id.is_none());
        assert!(filter.action.is_none());
        assert!(filter.after.is_none());
        assert!(filter.before.is_none());
        assert_eq!(filter.limit, 0);
    }

    // -- handle_export integration -----------------------------------------

    #[test]
    fn handle_export_reads_log_file() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("audit.jsonl");

        let entry = make_entry("tool_call", Some("echo"), AuditOutcome::Success);
        let line = serde_json::to_string(&entry).unwrap();
        std::fs::write(&log_path, format!("{line}\n")).unwrap();

        let state = AuditExportState {
            log_dir: dir.path().to_path_buf(),
            config: ExportConfig {
                format: ExportFormat::Csv,
                ..Default::default()
            },
        };

        let query = ExportQuery {
            format: Some("csv".into()),
            ..Default::default()
        };

        let resp = handle_export(&state, &query).unwrap();
        assert_eq!(resp.count, 1);
        assert_eq!(resp.format, "csv");
        assert!(!resp.data.is_empty());
    }

    #[test]
    fn handle_export_config_returns_current() {
        let state = AuditExportState {
            log_dir: PathBuf::from("/tmp"),
            config: ExportConfig {
                hostname: "testhost".into(),
                ..Default::default()
            },
        };
        let resp = handle_export_config(&state);
        assert_eq!(resp.config.hostname, "testhost");
    }
}
