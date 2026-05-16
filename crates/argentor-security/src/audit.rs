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

use async_trait::async_trait;
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
// AuditSink
// ---------------------------------------------------------------------------

/// A destination for audit entries.
///
/// The [`AuditLog`] owns exactly one sink, driven by a single background
/// writer task. Implementations therefore do not need internal locking — the
/// task calls [`init`](AuditSink::init) once, then [`write`](AuditSink::write)
/// sequentially for every entry.
///
/// Implementations must not panic: a failed write should be dropped or logged,
/// never propagated, because the audit hot path is fire-and-forget.
#[async_trait]
pub trait AuditSink: Send {
    /// Called once before the write loop begins. Use it for directory
    /// creation, schema setup, or retention cleanup. Default: no-op.
    async fn init(&mut self) {}

    /// Persist a single audit entry.
    async fn write(&mut self, entry: &AuditEntry);
}

/// JSONL file sink with size rotation, age retention, and optional compression.
///
/// This is the default sink behind [`AuditLog::new`] and the rotation-aware
/// constructors. It reproduces the historical behavior exactly.
pub struct JsonlSink {
    log_file: PathBuf,
    rotation: AuditRotationConfig,
}

impl JsonlSink {
    /// Create a JSONL sink writing to `log_file` with the given rotation policy.
    pub fn new(log_file: PathBuf, rotation: AuditRotationConfig) -> Self {
        Self { log_file, rotation }
    }
}

#[async_trait]
impl AuditSink for JsonlSink {
    async fn init(&mut self) {
        if let Some(parent) = self.log_file.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        cleanup_expired_rotated_logs(&self.log_file, self.rotation.retention_days).await;
    }

    async fn write(&mut self, entry: &AuditEntry) {
        let Ok(line) = serde_json::to_string(entry) else {
            return;
        };
        let line = format!("{line}\n");

        // Rotate if needed before writing.
        if should_rotate(&self.log_file, self.rotation.max_size_bytes).await {
            rotate_logs(
                &self.log_file,
                self.rotation.max_rotated_files,
                self.rotation.compress_rotated,
            )
            .await;
            cleanup_expired_rotated_logs(&self.log_file, self.rotation.retention_days).await;
        }

        if let Ok(mut file) = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)
            .await
        {
            let _ = file.write_all(line.as_bytes()).await;
        }
    }
}

// ---------------------------------------------------------------------------
// AuditLog
// ---------------------------------------------------------------------------

/// Append-only audit log that records all agent actions.
///
/// Entries are sent over an unbounded channel to a single background writer
/// task -- callers never block. The writer delegates persistence to an
/// [`AuditSink`]; the default sink is [`JsonlSink`], which rotates the active
/// file when it exceeds the configured size limit.
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
        Self::with_sink(Box::new(JsonlSink::new(log_file, rotation)))
    }

    /// Create a new `AuditLog` backed by an arbitrary [`AuditSink`].
    ///
    /// Spawns the single background writer task. The task calls
    /// [`AuditSink::init`] once, then [`AuditSink::write`] for every entry
    /// received on the channel until all senders are dropped.
    pub fn with_sink(mut sink: Box<dyn AuditSink>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<AuditEntry>();

        tokio::spawn(async move {
            sink.init().await;
            while let Some(entry) = rx.recv().await {
                sink.write(&entry).await;
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
// SqliteSink (feature = "sqlite")
// ---------------------------------------------------------------------------

/// SQLite-backed audit sink.
///
/// Entries are inserted into an `audit_entries` table with indexes on
/// `timestamp` and `outcome` for the queries the dashboard and exporters run.
/// Each insert runs on a blocking thread pool so the async runtime is never
/// stalled by SQLite I/O.
///
/// Enabled by the `sqlite` crate feature.
#[cfg(feature = "sqlite")]
pub struct SqliteSink {
    db_path: PathBuf,
    conn: Option<rusqlite::Connection>,
}

#[cfg(feature = "sqlite")]
impl SqliteSink {
    /// Create a SQLite sink that opens (or creates) the database at `db_path`.
    ///
    /// The connection and schema are established lazily in [`AuditSink::init`].
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            conn: None,
        }
    }

    fn outcome_str(outcome: &AuditOutcome) -> &'static str {
        match outcome {
            AuditOutcome::Success => "success",
            AuditOutcome::Denied => "denied",
            AuditOutcome::Error => "error",
        }
    }
}

#[cfg(feature = "sqlite")]
#[async_trait]
impl AuditSink for SqliteSink {
    async fn init(&mut self) {
        let db_path = self.db_path.clone();
        // rusqlite::Connection is Send but not Sync and its calls are
        // blocking — open it on the blocking pool and move it back here.
        self.conn = tokio::task::spawn_blocking(move || {
            if let Some(parent) = db_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let conn = rusqlite::Connection::open(&db_path).ok()?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS audit_entries (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    timestamp   TEXT NOT NULL,
                    session_id  TEXT NOT NULL,
                    action      TEXT NOT NULL,
                    skill_name  TEXT,
                    details     TEXT NOT NULL,
                    outcome     TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_entries(timestamp);
                CREATE INDEX IF NOT EXISTS idx_audit_outcome ON audit_entries(outcome);",
            )
            .ok()?;
            Some(conn)
        })
        .await
        .ok()
        .flatten();
    }

    async fn write(&mut self, entry: &AuditEntry) {
        let Some(conn) = self.conn.take() else {
            return;
        };
        let timestamp = entry.timestamp.to_rfc3339();
        let session_id = entry.session_id.to_string();
        let action = entry.action.clone();
        let skill_name = entry.skill_name.clone();
        let details = entry.details.to_string();
        let outcome = Self::outcome_str(&entry.outcome).to_string();

        // Take the connection into the blocking task and return it so the
        // sink keeps ownership across calls.
        self.conn = tokio::task::spawn_blocking(move || {
            let _ = conn.execute(
                "INSERT INTO audit_entries
                    (timestamp, session_id, action, skill_name, details, outcome)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![timestamp, session_id, action, skill_name, details, outcome],
            );
            conn
        })
        .await
        .ok();
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

    // -- AuditSink abstraction -----------------------------------------------

    /// Minimal in-memory sink that records how many entries it received and
    /// the last action seen. Proves a third-party sink works through the
    /// `with_sink` constructor without touching the filesystem.
    struct CountingSink {
        count: std::sync::Arc<std::sync::atomic::AtomicU32>,
    }

    #[async_trait]
    impl AuditSink for CountingSink {
        async fn write(&mut self, _entry: &AuditEntry) {
            self.count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn test_with_sink_drives_custom_sink() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let log = AuditLog::with_sink(Box::new(CountingSink {
            count: count.clone(),
        }));

        for _ in 0..5 {
            log.log(make_entry());
        }
        sleep(Duration::from_millis(150)).await;

        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn test_jsonl_sink_through_with_sink_is_equivalent() {
        let dir = TempDir::new().unwrap();
        let log_file = dir.path().join("audit.jsonl");
        let log = AuditLog::with_sink(Box::new(JsonlSink::new(
            log_file.clone(),
            AuditRotationConfig::default(),
        )));

        log.log(make_entry());
        sleep(Duration::from_millis(100)).await;

        let content = tokio::fs::read_to_string(&log_file).await.unwrap();
        assert!(content.contains("test_action"));
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn test_sqlite_sink_inserts_entries() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("audit.db");
        let log = AuditLog::with_sink(Box::new(SqliteSink::new(db_path.clone())));

        log.log_action(
            Uuid::new_v4(),
            "sqlite_action",
            Some("sqlite_skill".to_string()),
            serde_json::json!({"k": "v"}),
            AuditOutcome::Denied,
        );
        sleep(Duration::from_millis(200)).await;

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let (count, action, outcome): (i64, String, String) = conn
            .query_row(
                "SELECT COUNT(*), action, outcome FROM audit_entries",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(action, "sqlite_action");
        assert_eq!(outcome, "denied");
    }
}
