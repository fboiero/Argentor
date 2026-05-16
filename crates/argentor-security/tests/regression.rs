#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Regression tests for argentor-security: AuditLog, TLS config validation,
//! PermissionSet, Sanitizer, RateLimiter.

use argentor_security::audit::{AuditEntry, AuditOutcome};
use argentor_security::{AuditLog, Capability, PermissionSet, RateLimiter, Sanitizer, TlsConfig};
use uuid::Uuid;

// --- AuditLog ---

#[tokio::test]
async fn test_audit_log_writes_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path().join("audit");
    let audit = AuditLog::new(log_dir.clone());

    let session_id = Uuid::new_v4();
    audit.log_action(
        session_id,
        "test_action",
        Some("test_skill".to_string()),
        serde_json::json!({"key": "value"}),
        AuditOutcome::Success,
    );

    // Give the background task time to write
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let log_file = log_dir.join("audit.jsonl");
    let contents = tokio::fs::read_to_string(&log_file).await.unwrap();
    assert!(!contents.is_empty());
    assert!(contents.contains("test_action"));
    assert!(contents.contains("test_skill"));
    assert!(contents.contains(&session_id.to_string()));
}

#[tokio::test]
async fn test_audit_log_multiple_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path().join("audit");
    let audit = AuditLog::new(log_dir.clone());

    let session_id = Uuid::new_v4();
    for i in 0..5 {
        audit.log_action(
            session_id,
            format!("action_{i}"),
            None,
            serde_json::json!({"index": i}),
            AuditOutcome::Success,
        );
    }

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let log_file = log_dir.join("audit.jsonl");
    let contents = tokio::fs::read_to_string(&log_file).await.unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 5);

    // Each line should be valid JSON
    for line in &lines {
        let entry: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(entry.get("timestamp").is_some());
        assert!(entry.get("session_id").is_some());
        assert!(entry.get("action").is_some());
    }
}

#[tokio::test]
async fn test_audit_log_outcomes() {
    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path().join("audit");
    let audit = AuditLog::new(log_dir.clone());

    let sid = Uuid::new_v4();
    audit.log_action(
        sid,
        "success",
        None,
        serde_json::json!({}),
        AuditOutcome::Success,
    );
    audit.log_action(
        sid,
        "denied",
        None,
        serde_json::json!({}),
        AuditOutcome::Denied,
    );
    audit.log_action(
        sid,
        "error",
        None,
        serde_json::json!({}),
        AuditOutcome::Error,
    );

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let log_file = log_dir.join("audit.jsonl");
    let contents = tokio::fs::read_to_string(&log_file).await.unwrap();
    assert!(contents.contains("\"outcome\":\"success\""));
    assert!(contents.contains("\"outcome\":\"denied\""));
    assert!(contents.contains("\"outcome\":\"error\""));
}

#[test]
fn test_audit_entry_serialization() {
    let entry = AuditEntry {
        timestamp: chrono::Utc::now(),
        session_id: Uuid::new_v4(),
        action: "tool_call".to_string(),
        skill_name: Some("shell".to_string()),
        details: serde_json::json!({"cmd": "ls"}),
        outcome: AuditOutcome::Success,
        correlation_id: None,
    };

    let json = serde_json::to_string(&entry).unwrap();
    assert!(json.contains("tool_call"));
    assert!(json.contains("shell"));
    assert!(json.contains("\"outcome\":\"success\""));
}

// --- TLS Config Validation ---

#[tokio::test]
async fn test_tls_config_disabled_passes() {
    let config = TlsConfig {
        enabled: false,
        cert_path: String::new(),
        key_path: String::new(),
        client_ca_path: String::new(),
    };
    assert!(argentor_security::tls::validate_tls_config(&config)
        .await
        .is_ok());
}

#[tokio::test]
async fn test_tls_config_enabled_empty_paths_fails() {
    let config = TlsConfig {
        enabled: true,
        cert_path: String::new(),
        key_path: String::new(),
        client_ca_path: String::new(),
    };
    let result = argentor_security::tls::validate_tls_config(&config).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("cert_path"));
}

#[tokio::test]
async fn test_tls_config_enabled_missing_cert_fails() {
    let config = TlsConfig {
        enabled: true,
        cert_path: "/nonexistent/cert.pem".to_string(),
        key_path: "/nonexistent/key.pem".to_string(),
        client_ca_path: String::new(),
    };
    let result = argentor_security::tls::validate_tls_config(&config).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not found"));
}

#[tokio::test]
async fn test_tls_config_missing_client_ca_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let cert_path = tmp.path().join("cert.pem");
    let key_path = tmp.path().join("key.pem");
    tokio::fs::write(&cert_path, "fake cert").await.unwrap();
    tokio::fs::write(&key_path, "fake key").await.unwrap();

    let config = TlsConfig {
        enabled: true,
        cert_path: cert_path.to_str().unwrap().to_string(),
        key_path: key_path.to_str().unwrap().to_string(),
        client_ca_path: "/nonexistent/ca.pem".to_string(),
    };
    let result = argentor_security::tls::validate_tls_config(&config).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Client CA"));
}

// --- PermissionSet comprehensive ---

#[test]
fn test_permission_set_grant_revoke() {
    let mut perms = PermissionSet::new();
    assert!(perms.is_empty());

    let cap = Capability::FileRead {
        allowed_paths: vec!["/tmp".to_string()],
    };
    perms.grant(cap.clone());
    assert!(!perms.is_empty());
    assert!(perms.has(&cap));

    perms.revoke(&cap);
    assert!(perms.is_empty());
    assert!(!perms.has(&cap));
}

#[test]
fn test_permission_set_file_write() {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::FileWrite {
        allowed_paths: vec!["/home/user".to_string()],
    });

    assert!(perms.check_file_write("/home/user/doc.txt"));
    assert!(!perms.check_file_write("/etc/passwd"));
}

#[test]
fn test_permission_set_shell_exec() {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::ShellExec {
        allowed_commands: vec!["ls".to_string(), "echo".to_string()],
    });

    assert!(perms.check_shell("ls -la"));
    assert!(perms.check_shell("echo hello"));
    assert!(!perms.check_shell("rm -rf /"));
}

#[test]
fn test_permission_set_multiple_capabilities() {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::FileRead {
        allowed_paths: vec!["/tmp".to_string()],
    });
    perms.grant(Capability::NetworkAccess {
        allowed_hosts: vec!["api.example.com".to_string()],
    });
    perms.grant(Capability::ShellExec {
        allowed_commands: vec!["echo".to_string()],
    });

    assert!(perms.check_file_read("/tmp/test.txt"));
    assert!(perms.check_network("api.example.com"));
    assert!(perms.check_shell("echo hi"));

    // Cross-type checks should fail
    assert!(!perms.check_file_write("/tmp/test.txt"));
    assert!(!perms.check_network("evil.com"));
    assert!(!perms.check_shell("rm -rf /"));
}

#[test]
fn test_permission_set_iter() {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::DatabaseQuery);
    perms.grant(Capability::BrowserAccess {
        allowed_domains: vec!["example.com".to_string()],
    });

    let caps: Vec<_> = perms.iter().collect();
    assert_eq!(caps.len(), 2);
}

#[test]
fn test_capability_serialization() {
    let cap = Capability::NetworkAccess {
        allowed_hosts: vec!["*.example.com".to_string()],
    };
    let json = serde_json::to_string(&cap).unwrap();
    let deserialized: Capability = serde_json::from_str(&json).unwrap();
    assert_eq!(cap, deserialized);
}

#[test]
fn test_permission_set_serialization() {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::FileRead {
        allowed_paths: vec!["/tmp".to_string()],
    });

    let json = serde_json::to_string(&perms).unwrap();
    let deserialized: PermissionSet = serde_json::from_str(&json).unwrap();
    assert!(deserialized.check_file_read("/tmp/file"));
}

// --- Sanitizer comprehensive ---

#[test]
fn test_sanitizer_allows_newlines_tabs() {
    let s = Sanitizer::default();
    let result = s.sanitize("line1\nline2\ttab");
    assert_eq!(result.into_string().unwrap(), "line1\nline2\ttab");
}

#[test]
fn test_sanitizer_strips_null_bytes() {
    let s = Sanitizer::default();
    let input = "before\x00after";
    let result = s.sanitize(input);
    assert_eq!(result.into_string().unwrap(), "beforeafter");
}

#[test]
fn test_sanitizer_strips_ansi_escapes() {
    let s = Sanitizer::default();
    let input = "normal\x1b[31mred\x1b[0mnormal";
    let result = s.sanitize(input);
    let clean = result.into_string().unwrap();
    assert!(!clean.contains('\x1b'));
    assert!(clean.contains("normal"));
}

#[test]
fn test_sanitizer_rejects_only_control_chars() {
    let s = Sanitizer::default();
    let result = s.sanitize("\x00\x01\x02\x03");
    assert!(result.is_rejected());
}

#[test]
fn test_sanitizer_empty_input_allowed() {
    let s = Sanitizer::default();
    let result = s.sanitize("");
    assert_eq!(result.into_string().unwrap(), "");
}

#[test]
fn test_sanitizer_unicode_preserved() {
    let s = Sanitizer::default();
    let input = "Héllo wörld 日本語 🦀";
    let result = s.sanitize(input);
    assert_eq!(result.into_string().unwrap(), input);
}

#[test]
fn test_sanitizer_custom_max_length() {
    let s = Sanitizer::new(5);
    assert!(s.sanitize("12345").into_string().is_some());
    assert!(s.sanitize("123456").is_rejected());
}

#[test]
fn test_sanitizer_header_limits_length() {
    let s = Sanitizer::default();
    let long_header = "x".repeat(2000);
    let clean = s.sanitize_header(&long_header);
    assert_eq!(clean.len(), 1000);
}

// --- RateLimiter comprehensive ---

#[tokio::test]
async fn test_rate_limiter_independent_sessions() {
    let limiter = RateLimiter::new(2.0, 0.1);
    let session1 = Uuid::new_v4();
    let session2 = Uuid::new_v4();

    // Drain session1's tokens
    assert!(limiter.check(session1).await);
    assert!(limiter.check(session1).await);
    assert!(!limiter.check(session1).await);

    // Session2 should still have tokens
    assert!(limiter.check(session2).await);
    assert!(limiter.check(session2).await);
}

#[tokio::test]
async fn test_rate_limiter_refill() {
    let limiter = RateLimiter::new(1.0, 100.0); // 100 tokens/sec refill = fast
    let session = Uuid::new_v4();

    // Drain the bucket
    assert!(limiter.check(session).await);
    assert!(!limiter.check(session).await);

    // Wait for refill
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Should have tokens again
    assert!(limiter.check(session).await);
}

#[tokio::test]
async fn test_rate_limiter_cleanup() {
    let limiter = RateLimiter::new(10.0, 1.0);
    let session = Uuid::new_v4();

    limiter.check(session).await;

    // Cleanup with 0 duration should remove the bucket
    limiter.cleanup(std::time::Duration::from_secs(0)).await;

    // After cleanup, a new check creates a fresh bucket with full tokens
    assert!(limiter.check(session).await);
}
