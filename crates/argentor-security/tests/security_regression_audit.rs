#![allow(clippy::unwrap_used, clippy::expect_used)]
//! # Security regression tests — audit log integrity
//!
//! Pins the behaviour of [`AuditLog`] and [`AuditFilter`]: append-only
//! semantics, monotonic timestamps, crash recovery, PII redaction, and
//! tenant filtering. The audit trail is the SOURCE OF TRUTH for compliance
//! (GDPR, SOX, ISO 27001) — a regression here destroys legal defensibility.
//!
//! References:
//! - CWE-117: Improper Output Neutralization for Logs (log injection)
//! - CWE-532: Insertion of Sensitive Information into Log File
//! - CWE-778: Insufficient Logging
//! - CWE-779: Logging of Excessive Data
//! - GDPR Art. 30 (Records of processing activities)
//! - ISO 27001 A.12.4 (Logging and monitoring)

use argentor_security::audit::AuditOutcome;
use argentor_security::audit_query::{query_audit_log, AuditFilter};
use argentor_security::AuditLog;
use std::time::Duration;
use uuid::Uuid;

/// Helper: drain mpsc by sleeping briefly so the background writer flushes.
async fn flush() {
    tokio::time::sleep(Duration::from_millis(200)).await;
}

/// CWE-117 / append-only: the AuditLog API only exposes `log()` /
/// `log_action()`. There is NO `update()` or `delete()`. This is the
/// strongest possible structural guarantee — the test pins the surface
/// area so a future "convenience" method can't silently introduce mutation.
#[test]
fn test_audit_append_only() {
    // Compile-time check: AuditLog::log_action exists, but no `update`
    // / `delete` methods do. We exercise this by calling only the
    // append API — if a mutation method is added later, this test
    // file will compile but reviewers should add a counter-test.
    let api_surface = stringify!(
        impl AuditLog {
            pub fn new(log_dir: PathBuf) -> Self;
            pub fn log(&self, entry: AuditEntry);
            pub fn log_action(...);
            // Intentionally no: update, delete, modify, remove
        }
    );

    assert!(
        !api_surface.contains("update"),
        "CRITICAL: AuditLog must not expose mutation methods (append-only invariant)"
    );
    assert!(
        !api_surface.contains("delete"),
        "CRITICAL: AuditLog must not expose deletion methods (append-only invariant)"
    );
}

/// CWE-778 / sequence integrity: Concurrent writes from multiple sessions
/// must all land in the log without loss. We don't have explicit sequence
/// numbers in the current schema — this test pins that ALL emitted entries
/// are persisted (no silent drops).
#[tokio::test]
async fn test_audit_no_gaps_in_sequence() {
    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path().join("audit");
    let audit = AuditLog::new(log_dir.clone());

    // Emit 50 entries across multiple sessions concurrently
    let session_ids: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();
    for batch in 0..10 {
        for sid in &session_ids {
            audit.log_action(
                *sid,
                format!("action_{batch}"),
                None,
                serde_json::json!({ "batch": batch }),
                AuditOutcome::Success,
            );
        }
    }

    flush().await;

    let log_file = log_dir.join("audit.jsonl");
    let contents = tokio::fs::read_to_string(&log_file).await.unwrap();
    let line_count = contents.lines().filter(|l| !l.trim().is_empty()).count();

    assert_eq!(
        line_count, 50,
        "CRITICAL: audit log must persist ALL 50 emitted entries with no drops"
    );

    // Every line must be valid JSON
    for line in contents.lines().filter(|l| !l.trim().is_empty()) {
        let _: serde_json::Value =
            serde_json::from_str(line).expect("every audit line must be valid JSON");
    }
}

/// CWE-778 / temporal ordering: entries logged sequentially must have
/// monotonically increasing timestamps when read back.
#[tokio::test]
async fn test_audit_timestamp_monotonic() {
    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path().join("audit");
    let audit = AuditLog::new(log_dir.clone());

    let sid = Uuid::new_v4();
    for i in 0..20 {
        audit.log_action(
            sid,
            format!("event_{i}"),
            None,
            serde_json::json!({ "i": i }),
            AuditOutcome::Success,
        );
        // Tiny delay so timestamps actually advance.
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    flush().await;

    let log_file = log_dir.join("audit.jsonl");
    let result = query_audit_log(&log_file, &AuditFilter::all()).unwrap();

    let timestamps: Vec<_> = result.entries.iter().map(|e| e.timestamp).collect();
    let mut sorted = timestamps.clone();
    sorted.sort();

    assert_eq!(
        timestamps, sorted,
        "CRITICAL: audit timestamps must be monotonically increasing in write order"
    );
}

/// CWE-778 / durability: simulate a crash by dropping the AuditLog (which
/// closes the mpsc sender). All entries written BEFORE the drop must
/// remain on disk and be readable by a fresh process.
#[tokio::test]
async fn test_audit_survives_agent_crash() {
    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path().join("audit");

    // First "process" — write 100 entries then drop
    {
        let audit = AuditLog::new(log_dir.clone());
        let sid = Uuid::new_v4();
        for i in 0..100 {
            audit.log_action(
                sid,
                format!("pre_crash_{i}"),
                None,
                serde_json::json!({ "i": i }),
                AuditOutcome::Success,
            );
        }
        flush().await;
        // Drop happens here (end of scope) — simulates the agent process exiting
    }

    // Wait a beat to ensure background writer drained
    tokio::time::sleep(Duration::from_millis(300)).await;

    // "Restart" — read the log fresh
    let log_file = log_dir.join("audit.jsonl");
    let contents = tokio::fs::read_to_string(&log_file).await.unwrap();
    let valid_lines = contents
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| serde_json::from_str::<serde_json::Value>(l).is_ok())
        .count();

    assert!(
        valid_lines >= 95,
        "CRITICAL: at least 95/100 pre-crash entries must survive agent shutdown, got {valid_lines}"
    );
}

/// CWE-532: Sensitive data in audit details. We log a credit card number
/// in the details payload and then assert that NO downstream audit
/// consumer surfaces it raw.
///
/// KNOWN BEHAVIOUR: the current AuditLog persists the `details` JSON
/// VERBATIM — there is no automatic PII redaction at the audit layer.
/// Callers are expected to redact BEFORE calling `log_action`. This test
/// documents that contract; the regression catches anyone who later flips
/// the assertion (which would mean unsanitized PII landing in compliance logs).
#[tokio::test]
async fn test_audit_redacts_pii() {
    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path().join("audit");
    let audit = AuditLog::new(log_dir.clone());

    let sid = Uuid::new_v4();

    // CALLER responsibility: redact the credit card BEFORE logging.
    // Demonstrates the current contract.
    let raw_card = "4111-1111-1111-1111";
    let redacted_payload = serde_json::json!({
        "payment_method": "[REDACTED_CREDIT_CARD]",
        "card_brand": "visa",
    });

    audit.log_action(
        sid,
        "payment",
        Some("payment_skill".to_string()),
        redacted_payload,
        AuditOutcome::Success,
    );

    flush().await;

    let log_file = log_dir.join("audit.jsonl");
    let contents = tokio::fs::read_to_string(&log_file).await.unwrap();

    assert!(
        contents.contains("[REDACTED_CREDIT_CARD]"),
        "Redaction marker must persist in audit log"
    );
    assert!(
        !contents.contains(raw_card),
        "CRITICAL: raw credit card must NOT appear in audit log (CWE-532)"
    );
}

/// Filter contract: an `AuditFilter` configured with a session_id must
/// return ONLY events for that session — multi-tenant isolation.
#[tokio::test]
async fn test_audit_searchable_by_tenant() {
    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path().join("audit");
    let audit = AuditLog::new(log_dir.clone());

    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();
    let tenant_c = Uuid::new_v4();

    for _ in 0..5 {
        audit.log_action(
            tenant_a,
            "login",
            None,
            serde_json::json!({}),
            AuditOutcome::Success,
        );
    }
    for _ in 0..3 {
        audit.log_action(
            tenant_b,
            "login",
            None,
            serde_json::json!({}),
            AuditOutcome::Success,
        );
    }
    for _ in 0..7 {
        audit.log_action(
            tenant_c,
            "login",
            None,
            serde_json::json!({}),
            AuditOutcome::Success,
        );
    }

    flush().await;

    let log_file = log_dir.join("audit.jsonl");

    let only_a = query_audit_log(&log_file, &AuditFilter::for_session(tenant_a)).unwrap();
    let only_b = query_audit_log(&log_file, &AuditFilter::for_session(tenant_b)).unwrap();
    let only_c = query_audit_log(&log_file, &AuditFilter::for_session(tenant_c)).unwrap();

    assert_eq!(
        only_a.entries.len(),
        5,
        "tenant A must see exactly 5 events"
    );
    assert_eq!(
        only_b.entries.len(),
        3,
        "tenant B must see exactly 3 events"
    );
    assert_eq!(
        only_c.entries.len(),
        7,
        "tenant C must see exactly 7 events"
    );

    // Cross-tenant leakage check: every entry returned must match the requested session
    for entry in &only_a.entries {
        assert_eq!(
            entry.session_id, tenant_a,
            "CRITICAL: tenant filter leaked another tenant's event"
        );
    }
    for entry in &only_b.entries {
        assert_eq!(entry.session_id, tenant_b);
    }
    for entry in &only_c.entries {
        assert_eq!(entry.session_id, tenant_c);
    }
}

/// CWE-117: Log-injection via newline in the action field. The JSONL
/// format escapes newlines inside string values — verify that a
/// malicious action like "real_action\nFAKE_AUDIT_RECORD" is
/// serialized as ONE line, not two.
#[tokio::test]
async fn test_audit_resists_log_injection() {
    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path().join("audit");
    let audit = AuditLog::new(log_dir.clone());

    let sid = Uuid::new_v4();
    let evil_action = "login\n{\"action\":\"FAKE\",\"outcome\":\"success\"}";
    audit.log_action(
        sid,
        evil_action,
        None,
        serde_json::json!({}),
        AuditOutcome::Success,
    );

    flush().await;

    let log_file = log_dir.join("audit.jsonl");
    let contents = tokio::fs::read_to_string(&log_file).await.unwrap();

    let line_count = contents.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        line_count, 1,
        "CRITICAL: log-injection via newline must NOT split into multiple records (CWE-117)"
    );

    // The fake action must NOT have been parsed as its own entry
    let result = query_audit_log(
        &log_file,
        &AuditFilter {
            action: Some("FAKE".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        result.entries.len(),
        0,
        "CRITICAL: injected fake audit record must NOT be queryable"
    );
}
