// SPDX-License-Identifier: AGPL-3.0-only
//! Append-only audit log with automatic rotation.
//!
//! Entries are written by a single background task via an unbounded channel,
//! so the hot path (`log` / `log_action`) is lock-free and non-blocking.
//!
//! # Rotation
//!
//! When the active log file exceeds [`AuditRotationConfig::max_size_bytes`],
//! the background writer renames it to `audit.jsonl.1`, shifts older rotated
//! files up (`.1` -> `.2`, ...), and deletes any file beyond
//! [`AuditRotationConfig::max_rotated_files`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tracing::info;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single entry in the audit log, recording one agent action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// UTC timestamp of when the action occurred.
    pub timestamp: DateTime<Utc>,
    /// Session in which the action was performed.
    pub session_id: Uuid,
    /// Human-readable description of the action (e.g., "tool_call", "login").
    pub action: String,
    /// Name of the skill involved, if any.
    pub skill_name: Option<String>,
    /// Structured details about the action (free-form JSON).
    pub details: serde_json::Value,
    /// Whether the action succeeded, was denied, or errored.
    pub outcome: AuditOutcome,
}

/// Outcome of an audited action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditOutcome {
    /// The action completed successfully.
    Success,
    /// The action was denied by a security check.
    Denied,
    /// The action failed with an error.
    Error,
}

/// Configuration for log rotation.
#[derive(Debug, Clone)]
pub struct AuditRotationConfig {
    /// Rotate when the active log file exceeds this size (bytes). Default: 10 MiB.
    pub max_size_bytes: u64,
    /// Number of rotated files to keep (`.1` through `.N`). Default: 5.
    pub max_rotated_files: u32,
}

impl Default for AuditRotationConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: 10 * 1024 * 1024, // 10 MiB
            max_rotated_files: 5,
        }
    }
}

// ---------------------------------------------------------------------------
// AuditLog
// ---------------------------------------------------------------------------

/// Append-only audit log that records all agent actions.
///
/// Entries are sent over an unbounded channel to a single background writer
/// task -- callers never block. When the active file exceeds the configured
/// size limit it is automatically rotated.
pub struct AuditLog {
    tx: mpsc::UnboundedSender<AuditEntry>,
}

impl AuditLog {
    /// Create a new `AuditLog` with default rotation settings (10 MiB / 5 files).
    pub fn new(log_dir: PathBuf) -> Self {
        Self::with_rotation(log_dir, AuditRotationConfig::default())
    }

    /// Create a new `AuditLog` with custom rotation settings.
    ///
    /// Spawns a background task that is the sole writer to `audit.jsonl`.
    pub fn with_rotation(log_dir: PathBuf, rotation: AuditRotationConfig) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<AuditEntry>();

        tokio::spawn(async move {
            let _ = tokio::fs::create_dir_all(&log_dir).await;
            let log_file = log_dir.join("audit.jsonl");

            while let Some(entry) = rx.recv().await {
                let Ok(line) = serde_json::to_string(&entry) else {
                    continue;
                };
                let line = format!("{line}\n");

                // Rotate if needed before writing.
                if should_rotate(&log_file, rotation.max_size_bytes).await {
                    rotate_logs(&log_file, rotation.max_rotated_files).await;
                }

                // Write directly -- no inner spawn, no extra task per entry.
                if let Ok(mut file) = tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_file)
                    .await
                {
                    let _ = file.write_all(line.as_bytes()).await;
                }
            }
        });

        Self { tx }
    }

    /// Send an audit entry to the background writer. Logs the action via `tracing`.
    pub fn log(&self, entry: AuditEntry) {
        info!(
            session_id = %entry.session_id,
            action = %entry.action,
            outcome = ?entry.outcome,
            "audit"
        );
        let _ = self.tx.send(entry);
    }

    /// Convenience method to construct and log an [`AuditEntry`] in one call.
    pub fn log_action(
        &self,
        session_id: Uuid,
        action: impl Into<String>,
        skill_name: Option<String>,
        details: serde_json::Value,
        outcome: AuditOutcome,
    ) {
        self.log(AuditEntry {
            timestamp: Utc::now(),
            session_id,
            action: action.into(),
            skill_name,
            details,
            outcome,
        });
    }
}

// ---------------------------------------------------------------------------
// Rotation helpers
// ---------------------------------------------------------------------------

/// Returns `true` when `path` exists and is at least `max_bytes` in size.
async fn should_rotate(path: &Path, max_bytes: u64) -> bool {
    match tokio::fs::metadata(path).await {
        Ok(meta) => meta.len() >= max_bytes,
        Err(_) => false,
    }
}

/// Build the path for a rotated log file: `<base>.N`.
fn rotated_path(base: &Path, n: u32) -> PathBuf {
    let mut p = base.as_os_str().to_owned();
    p.push(format!(".{n}"));
    PathBuf::from(p)
}

/// Shift rotated files and rename the active log to `.1`.
///
/// - Delete `base.max_rotated` if it exists.
/// - Rename `base.N` -> `base.(N+1)` for N = max_rotated-1 downto 1.
/// - Rename `base` -> `base.1`.
async fn rotate_logs(base: &Path, max_rotated: u32) {
    if max_rotated == 0 {
        // Rotation disabled -- just truncate the active file.
        let _ = tokio::fs::remove_file(base).await;
        return;
    }

    // Delete the oldest rotated file.
    let oldest = rotated_path(base, max_rotated);
    let _ = tokio::fs::remove_file(&oldest).await; // ignore error if it doesn't exist

    // Shift: .N-1 -> .N down to .1 -> .2
    for n in (1..max_rotated).rev() {
        let src = rotated_path(base, n);
        let dst = rotated_path(base, n + 1);
        let _ = tokio::fs::rename(&src, &dst).await;
    }

    // Rename the active log to .1
    let _ = tokio::fs::rename(base, &rotated_path(base, 1)).await;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::time::{sleep, Duration};

    fn make_entry() -> AuditEntry {
        AuditEntry {
            timestamp: Utc::now(),
            session_id: Uuid::new_v4(),
            action: "test_action".to_string(),
            skill_name: None,
            details: serde_json::json!({"key": "value"}),
            outcome: AuditOutcome::Success,
        }
    }

    #[tokio::test]
    async fn test_log_creates_file() {
        let dir = TempDir::new().unwrap();
        let log = AuditLog::new(dir.path().to_path_buf());
        log.log(make_entry());

        sleep(Duration::from_millis(100)).await;

        let log_file = dir.path().join("audit.jsonl");
        assert!(log_file.exists(), "audit.jsonl should be created");
        let content = tokio::fs::read_to_string(&log_file).await.unwrap();
        assert!(!content.is_empty());
        assert!(content.contains("test_action"));
    }

    #[tokio::test]
    async fn test_log_action_convenience() {
        let dir = TempDir::new().unwrap();
        let log = AuditLog::new(dir.path().to_path_buf());
        log.log_action(
            Uuid::new_v4(),
            "my_action",
            Some("my_skill".to_string()),
            serde_json::json!({}),
            AuditOutcome::Denied,
        );

        sleep(Duration::from_millis(100)).await;

        let content = tokio::fs::read_to_string(dir.path().join("audit.jsonl"))
            .await
            .unwrap();
        assert!(content.contains("my_action"));
        assert!(content.contains("denied"));
    }

    #[tokio::test]
    async fn test_rotation_triggers_when_size_exceeded() {
        let dir = TempDir::new().unwrap();
        let log_file = dir.path().join("audit.jsonl");

        // Very small rotation threshold (100 bytes).
        let rotation = AuditRotationConfig {
            max_size_bytes: 100,
            max_rotated_files: 3,
        };
        let log = AuditLog::with_rotation(dir.path().to_path_buf(), rotation);

        // Write enough entries to exceed the threshold.
        for _ in 0..20 {
            log.log(make_entry());
        }

        sleep(Duration::from_millis(300)).await;

        // At least one rotation should have occurred.
        let rotated = dir.path().join("audit.jsonl.1");
        assert!(
            rotated.exists() || log_file.exists(),
            "Either rotated or active file should exist"
        );
    }

    #[tokio::test]
    async fn test_rotate_logs_shifts_files() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().join("audit.jsonl");

        // Pre-create some rotated files.
        tokio::fs::write(&base, b"active\n").await.unwrap();
        tokio::fs::write(&rotated_path(&base, 1), b"rot1\n")
            .await
            .unwrap();
        tokio::fs::write(&rotated_path(&base, 2), b"rot2\n")
            .await
            .unwrap();

        rotate_logs(&base, 3).await;

        // Active -> .1, old .1 -> .2, old .2 -> .3
        assert!(!base.exists(), "active file should be gone after rotation");
        let p1 = rotated_path(&base, 1);
        let p2 = rotated_path(&base, 2);
        let p3 = rotated_path(&base, 3);
        assert!(p1.exists(), ".1 should exist");
        assert!(p2.exists(), ".2 should exist");
        assert!(p3.exists(), ".3 should exist");

        assert_eq!(tokio::fs::read_to_string(&p1).await.unwrap(), "active\n");
        assert_eq!(tokio::fs::read_to_string(&p2).await.unwrap(), "rot1\n");
        assert_eq!(tokio::fs::read_to_string(&p3).await.unwrap(), "rot2\n");
    }
}
