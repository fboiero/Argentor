//! Audit log query and filtering for CLI and API consumption.
//!
//! Provides structured querying of the append-only JSONL audit log,
//! with filters by session, time range, action type, and outcome.

use crate::audit::{AuditEntry, AuditOutcome};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

/// Filter criteria for querying audit log entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditFilter {
    /// Filter by session ID.
    pub session_id: Option<Uuid>,
    /// Filter by action type (e.g., "tool_call", "agent_response").
    pub action: Option<String>,
    /// Filter by skill name.
    pub skill_name: Option<String>,
    /// Filter by outcome.
    pub outcome: Option<String>,
    /// Only entries after this timestamp.
    pub after: Option<DateTime<Utc>>,
    /// Only entries before this timestamp.
    pub before: Option<DateTime<Utc>>,
    /// Maximum number of entries to return (0 = all).
    pub limit: usize,
}

impl AuditFilter {
    /// Create a filter that matches everything.
    pub fn all() -> Self {
        Self::default()
    }

    /// Filter by session.
    pub fn for_session(session_id: Uuid) -> Self {
        Self {
            session_id: Some(session_id),
            ..Self::default()
        }
    }

    /// Check if an entry matches this filter.
    pub fn matches(&self, entry: &AuditEntry) -> bool {
        if let Some(sid) = &self.session_id {
            if &entry.session_id != sid {
                return false;
            }
        }

        if let Some(action) = &self.action {
            if &entry.action != action {
                return false;
            }
        }

        if let Some(skill) = &self.skill_name {
            match &entry.skill_name {
                Some(s) if s == skill => {}
                _ => return false,
            }
        }

        if let Some(outcome_str) = &self.outcome {
            if !matches!(
                (&entry.outcome, outcome_str.as_str()),
                (AuditOutcome::Success, "success")
                    | (AuditOutcome::Denied, "denied")
                    | (AuditOutcome::Error, "error")
            ) {
                return false;
            }
        }

        if let Some(after) = &self.after {
            if entry.timestamp < *after {
                return false;
            }
        }

        if let Some(before) = &self.before {
            if entry.timestamp > *before {
                return false;
            }
        }

        true
    }
}

/// Query result with entries and summary statistics.
#[derive(Debug, Clone, Serialize)]
pub struct AuditQueryResult {
    /// Matching entries.
    pub entries: Vec<AuditEntry>,
    /// Total entries scanned.
    pub total_scanned: usize,
    /// Total entries matching the filter.
    pub total_matched: usize,
    /// Summary statistics.
    pub stats: AuditStats,
}

/// Summary statistics from a query.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AuditStats {
    /// Count of successful actions.
    pub success_count: usize,
    /// Count of denied actions.
    pub denied_count: usize,
    /// Count of errors.
    pub error_count: usize,
    /// Unique sessions seen.
    pub unique_sessions: usize,
    /// Unique skills invoked.
    pub unique_skills: usize,
}

/// Query the audit log file with the given filter.
///
/// Reads the JSONL file line by line, applying the filter and collecting stats.
pub fn query_audit_log(log_path: &Path, filter: &AuditFilter) -> Result<AuditQueryResult, String> {
    let content =
        std::fs::read_to_string(log_path).map_err(|e| format!("Failed to read audit log: {e}"))?;

    let mut entries = Vec::new();
    let mut total_scanned = 0;
    let mut total_matched = 0;
    let mut sessions = std::collections::HashSet::new();
    let mut skills = std::collections::HashSet::new();
    let mut success_count = 0;
    let mut denied_count = 0;
    let mut error_count = 0;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: AuditEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue, // Skip malformed lines
        };

        total_scanned += 1;

        // Collect global stats
        sessions.insert(entry.session_id);
        if let Some(skill) = &entry.skill_name {
            skills.insert(skill.clone());
        }
        match &entry.outcome {
            AuditOutcome::Success => success_count += 1,
            AuditOutcome::Denied => denied_count += 1,
            AuditOutcome::Error => error_count += 1,
        }

        if filter.matches(&entry) {
            total_matched += 1;
            if filter.limit == 0 || entries.len() < filter.limit {
                entries.push(entry);
            }
        }
    }

    Ok(AuditQueryResult {
        entries,
        total_scanned,
        total_matched,
        stats: AuditStats {
            success_count,
            denied_count,
            error_count,
            unique_sessions: sessions.len(),
            unique_skills: skills.len(),
        },
    })
}

/// Format an audit entry as a human-readable line.
pub fn format_entry(entry: &AuditEntry) -> String {
    let outcome = match &entry.outcome {
        AuditOutcome::Success => "OK",
        AuditOutcome::Denied => "DENIED",
        AuditOutcome::Error => "ERROR",
    };

    let skill = entry.skill_name.as_deref().unwrap_or("-");

    format!(
        "[{}] {outcome} session={} action={} skill={skill} {}",
        entry.timestamp.format("%Y-%m-%d %H:%M:%S"),
        &entry.session_id.to_string()[..8],
        entry.action,
        entry.details,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_entry(action: &str, skill: Option<&str>, outcome: AuditOutcome) -> AuditEntry {
        AuditEntry {
            timestamp: Utc::now(),
            session_id: Uuid::new_v4(),
            action: action.into(),
            skill_name: skill.map(String::from),
            details: serde_json::json!({}),
            outcome,
            correlation_id: None,
        }
    }

    #[test]
    fn filter_all_matches_everything() {
        let filter = AuditFilter::all();
        let entry = make_entry("tool_call", Some("echo"), AuditOutcome::Success);
        assert!(filter.matches(&entry));
    }

    #[test]
    fn filter_by_action() {
        let filter = AuditFilter {
            action: Some("tool_call".into()),
            ..Default::default()
        };

        let match_entry = make_entry("tool_call", None, AuditOutcome::Success);
        let no_match = make_entry("agent_response", None, AuditOutcome::Success);

        assert!(filter.matches(&match_entry));
        assert!(!filter.matches(&no_match));
    }

    #[test]
    fn filter_by_session() {
        let sid = Uuid::new_v4();
        let filter = AuditFilter::for_session(sid);

        let mut entry = make_entry("test", None, AuditOutcome::Success);
        entry.session_id = sid;
        assert!(filter.matches(&entry));

        let other = make_entry("test", None, AuditOutcome::Success);
        assert!(!filter.matches(&other));
    }

    #[test]
    fn filter_by_outcome() {
        let filter = AuditFilter {
            outcome: Some("error".into()),
            ..Default::default()
        };

        let err_entry = make_entry("test", None, AuditOutcome::Error);
        let ok_entry = make_entry("test", None, AuditOutcome::Success);

        assert!(filter.matches(&err_entry));
        assert!(!filter.matches(&ok_entry));
    }

    #[test]
    fn filter_by_skill_name() {
        let filter = AuditFilter {
            skill_name: Some("echo".into()),
            ..Default::default()
        };

        let match_entry = make_entry("tool_call", Some("echo"), AuditOutcome::Success);
        let no_match = make_entry("tool_call", Some("time"), AuditOutcome::Success);
        let no_skill = make_entry("tool_call", None, AuditOutcome::Success);

        assert!(filter.matches(&match_entry));
        assert!(!filter.matches(&no_match));
        assert!(!filter.matches(&no_skill));
    }

    #[test]
    fn filter_by_time_range() {
        let now = Utc::now();
        let filter = AuditFilter {
            after: Some(now - chrono::Duration::hours(1)),
            before: Some(now + chrono::Duration::hours(1)),
            ..Default::default()
        };

        let entry = make_entry("test", None, AuditOutcome::Success);
        assert!(filter.matches(&entry));
    }

    #[test]
    fn query_from_jsonl_file() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("audit.jsonl");

        let e1 = make_entry("tool_call", Some("echo"), AuditOutcome::Success);
        let e2 = make_entry("tool_call", Some("time"), AuditOutcome::Error);
        let e3 = make_entry("agent_response", None, AuditOutcome::Success);

        let mut content = String::new();
        content.push_str(&serde_json::to_string(&e1).unwrap());
        content.push('\n');
        content.push_str(&serde_json::to_string(&e2).unwrap());
        content.push('\n');
        content.push_str(&serde_json::to_string(&e3).unwrap());
        content.push('\n');

        std::fs::write(&log_path, &content).unwrap();

        let result = query_audit_log(&log_path, &AuditFilter::all()).unwrap();
        assert_eq!(result.total_scanned, 3);
        assert_eq!(result.total_matched, 3);
        assert_eq!(result.entries.len(), 3);
        assert_eq!(result.stats.success_count, 2);
        assert_eq!(result.stats.error_count, 1);
        assert_eq!(result.stats.unique_skills, 2);
    }

    #[test]
    fn query_with_limit() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("audit.jsonl");

        let mut content = String::new();
        for _ in 0..10 {
            let e = make_entry("tool_call", None, AuditOutcome::Success);
            content.push_str(&serde_json::to_string(&e).unwrap());
            content.push('\n');
        }

        std::fs::write(&log_path, &content).unwrap();

        let filter = AuditFilter {
            limit: 3,
            ..Default::default()
        };
        let result = query_audit_log(&log_path, &filter).unwrap();
        assert_eq!(result.entries.len(), 3);
        assert_eq!(result.total_matched, 10);
    }

    #[test]
    fn format_entry_output() {
        let entry = make_entry("tool_call", Some("echo"), AuditOutcome::Success);
        let formatted = format_entry(&entry);
        assert!(formatted.contains("OK"));
        assert!(formatted.contains("tool_call"));
        assert!(formatted.contains("echo"));
    }
}
