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
//!
//! If [`AuditRotationConfig::retention_days`] is set, rotated files older than
//! that many days are removed during startup and after each rotation. The active
//! file is never removed by retention cleanup.
//!
//! If [`AuditRotationConfig::compress_rotated`] is enabled, rotated files are
//! compressed with Zstandard and stored as `audit.jsonl.N.zst`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
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
    /// Delete rotated files older than this many days. `None` disables age-based cleanup.
    pub retention_days: Option<u64>,
    /// Compress rotated audit logs with Zstandard (`.zst`). Default: false.
    pub compress_rotated: bool,
}

impl Default for AuditRotationConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: 10 * 1024 * 1024, // 10 MiB
            max_rotated_files: 5,
            retention_days: None,
            compress_rotated: false,
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
        Self::with_file_rotation(log_dir.join("audit.jsonl"), rotation)
    }

    /// Create a new `AuditLog` writing to a specific file path.
    ///
    /// This keeps the same rotation and retention semantics as [`Self::with_rotation`],
    /// but lets operators configure an explicit `audit.jsonl` location.
    pub fn with_file_rotation(log_file: PathBuf, rotation: AuditRotationConfig) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<AuditEntry>();

        tokio::spawn(async move {
            if let Some(parent) = log_file.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            cleanup_expired_rotated_logs(&log_file, rotation.retention_days).await;

            while let Some(entry) = rx.recv().await {
                let Ok(line) = serde_json::to_string(&entry) else {
                    continue;
                };
                let line = format!("{line}\n");

                // Rotate if needed before writing.
                if should_rotate(&log_file, rotation.max_size_bytes).await {
                    rotate_logs(
                        &log_file,
                        rotation.max_rotated_files,
                        rotation.compress_rotated,
                    )
                    .await;
                    cleanup_expired_rotated_logs(&log_file, rotation.retention_days).await;
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

/// Build the path for a compressed rotated log file: `<base>.N.zst`.
fn compressed_rotated_path(base: &Path, n: u32) -> PathBuf {
    let mut p = base.as_os_str().to_owned();
    p.push(format!(".{n}.zst"));
    PathBuf::from(p)
}

/// Shift rotated files and rename the active log to `.1`.
///
/// - Delete `base.max_rotated` if it exists.
/// - Rename `base.N` -> `base.(N+1)` for N = max_rotated-1 downto 1.
/// - Rename `base` -> `base.1`.
async fn rotate_logs(base: &Path, max_rotated: u32, compress_rotated: bool) {
    if max_rotated == 0 {
        // Rotation disabled -- just truncate the active file.
        let _ = tokio::fs::remove_file(base).await;
        return;
    }

    // Delete the oldest rotated file.
    let oldest = rotated_path(base, max_rotated);
    let _ = tokio::fs::remove_file(&oldest).await; // ignore error if it doesn't exist
    let oldest_compressed = compressed_rotated_path(base, max_rotated);
    let _ = tokio::fs::remove_file(&oldest_compressed).await;

    // Shift: .N-1 -> .N down to .1 -> .2
    for n in (1..max_rotated).rev() {
        let src = rotated_path(base, n);
        let dst = rotated_path(base, n + 1);
        let _ = tokio::fs::rename(&src, &dst).await;

        let src_compressed = compressed_rotated_path(base, n);
        let dst_compressed = compressed_rotated_path(base, n + 1);
        let _ = tokio::fs::rename(&src_compressed, &dst_compressed).await;
    }

    // Rename the active log to .1
    let first_rotated = rotated_path(base, 1);
    let _ = tokio::fs::rename(base, &first_rotated).await;
    if compress_rotated {
        compress_rotated_log(first_rotated).await;
    }
}

async fn compress_rotated_log(path: PathBuf) {
    let compressed = path.with_extension(format!(
        "{}.zst",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
    ));
    let source = path.clone();
    let compressed_for_task = compressed.clone();

    let compressed_ok = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let input = BufReader::new(File::open(&source)?);
        let output = BufWriter::new(File::create(&compressed_for_task)?);
        zstd::stream::copy_encode(input, output, 0)?;
        Ok(())
    })
    .await
    .ok()
    .and_then(Result::ok)
    .is_some();

    if compressed_ok {
        let _ = tokio::fs::remove_file(path).await;
    } else {
        let _ = tokio::fs::remove_file(compressed).await;
    }
}

/// Remove rotated logs that exceed the configured retention window.
async fn cleanup_expired_rotated_logs(base: &Path, retention_days: Option<u64>) {
    let Some(retention_days) = retention_days else {
        return;
    };
    let Some(parent) = base.parent() else {
        return;
    };
    let Some(base_name) = base.file_name().and_then(|name| name.to_str()) else {
        return;
    };

    let max_age = Duration::from_secs(retention_days.saturating_mul(24 * 60 * 60));
    let Ok(mut entries) = tokio::fs::read_dir(parent).await else {
        return;
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with(&format!("{base_name}.")) {
            continue;
        }

        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if is_older_than(modified, max_age) {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

fn is_older_than(modified: SystemTime, max_age: Duration) -> bool {
    match SystemTime::now().duration_since(modified) {
        Ok(age) => age > max_age,
        Err(_) => false,
    }
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
            retention_days: None,
            compress_rotated: false,
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

        rotate_logs(&base, 3, false).await;

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

    #[tokio::test]
    async fn test_file_rotation_writes_to_explicit_path() {
        let dir = TempDir::new().unwrap();
        let log_file = dir.path().join("custom-audit.jsonl");
        let log = AuditLog::with_file_rotation(log_file.clone(), AuditRotationConfig::default());
        log.log(make_entry());

        sleep(Duration::from_millis(100)).await;

        assert!(log_file.exists(), "explicit audit file should be created");
        assert!(!dir.path().join("audit.jsonl").exists());
    }

    #[tokio::test]
    async fn test_retention_cleanup_removes_old_rotated_files() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().join("audit.jsonl");
        let old_rotated = rotated_path(&base, 1);
        let active = base.clone();
        tokio::fs::write(&old_rotated, b"old\n").await.unwrap();
        tokio::fs::write(&active, b"active\n").await.unwrap();

        cleanup_expired_rotated_logs(&base, Some(0)).await;

        assert!(
            !old_rotated.exists(),
            "expired rotated audit file should be removed"
        );
        assert!(active.exists(), "active audit file should not be removed");
    }

    #[tokio::test]
    async fn test_rotate_logs_compresses_rotated_file() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().join("audit.jsonl");
        tokio::fs::write(&base, b"active\n").await.unwrap();

        rotate_logs(&base, 3, true).await;

        let compressed = compressed_rotated_path(&base, 1);
        assert!(
            compressed.exists(),
            "compressed rotated audit file should exist"
        );
        assert!(
            !rotated_path(&base, 1).exists(),
            "plain rotated file should be removed after compression"
        );

        let compressed_bytes = tokio::fs::read(&compressed).await.unwrap();
        let decoded = zstd::decode_all(compressed_bytes.as_slice()).unwrap();
        assert_eq!(decoded, b"active\n");
    }

    #[tokio::test]
    async fn test_rotate_logs_shifts_compressed_files() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().join("audit.jsonl");
        tokio::fs::write(&base, b"active\n").await.unwrap();
        tokio::fs::write(compressed_rotated_path(&base, 1), b"zst1\n")
            .await
            .unwrap();

        rotate_logs(&base, 3, true).await;

        assert!(compressed_rotated_path(&base, 1).exists());
        assert!(compressed_rotated_path(&base, 2).exists());
    }
}
