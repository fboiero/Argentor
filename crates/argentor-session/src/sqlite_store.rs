//! SQLite-style persistence layer for sessions, usage tracking, and personas.
//!
//! Provides three complementary stores that persist state to the local
//! filesystem using a JSON-file-per-table layout with an index file for fast
//! lookups. All writes are **atomic** (write to a temp file, then rename) so
//! that crashes never leave half-written data on disk.
//!
//! # Stores
//!
//! - [`SqliteSessionStore`] — Implements [`SessionStore`] with an in-memory
//!   index backed by `{dir}/sessions/{uuid}.json` and `{dir}/sessions/index.json`.
//! - [`PersistentUsageStore`] — Append-only JSONL storage for per-tenant usage
//!   records at `{dir}/usage/{tenant_id}.jsonl`.
//! - [`PersistentPersonaStore`] — JSON files for agent persona configurations
//!   at `{dir}/personas/{tenant_id}_{role}.json`.
//!
//! # Directory layout
//!
//! ```text
//! <base_dir>/
//!   sessions/
//!     <uuid>.json
//!     index.json
//!   usage/
//!     <tenant_id>.jsonl
//!   personas/
//!     <tenant_id>_<role>.json
//! ```
//!
//! # Thread safety
//!
//! Every store wraps its in-memory state in `Arc<RwLock<…>>` so multiple
//! tasks can read concurrently while writes are serialised.

use crate::session::Session;
use crate::store::SessionStore;
use argentor_core::{ArgentorError, ArgentorResult};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers — atomic write
// ---------------------------------------------------------------------------

/// Write `data` to `target` atomically: first write to a temporary sibling
/// file, then rename over the target. This guarantees that a reader never
/// sees a partially written file.
async fn atomic_write(target: &Path, data: &[u8]) -> ArgentorResult<()> {
    let tmp = target.with_extension(format!("{}.tmp", Uuid::new_v4()));
    tokio::fs::write(&tmp, data).await?;
    tokio::fs::rename(&tmp, target).await?;
    Ok(())
}

struct IndexLock {
    path: PathBuf,
}

impl IndexLock {
    async fn acquire(base_dir: &Path) -> ArgentorResult<Self> {
        let path = base_dir.join("sessions").join("index.lock");
        for _ in 0..100 {
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .await
            {
                Ok(_) => return Ok(Self { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }

        Err(ArgentorError::Session(
            "Timed out waiting for session index lock".to_string(),
        ))
    }

    async fn release(self) -> ArgentorResult<()> {
        if self.path.exists() {
            tokio::fs::remove_file(&self.path).await?;
        }
        Ok(())
    }
}

impl Drop for IndexLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ===========================================================================
// SqliteSessionStore
// ===========================================================================

/// Metadata cached in the on-disk index for fast lookups without having to
/// deserialise every session file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionIndexEntry {
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    message_count: usize,
    #[serde(default)]
    metadata: HashMap<String, serde_json::Value>,
}

/// A session store that mirrors a relational-database layout on the local
/// filesystem. Thread-safe via an internal [`RwLock`] guarding the in-memory
/// index.
///
/// Implements [`SessionStore`] so it is a drop-in replacement for
/// [`FileSessionStore`](crate::store::FileSessionStore).
///
/// All writes are atomic (tmp + rename) to prevent corruption.
pub struct SqliteSessionStore {
    /// Root directory that contains `sessions/`.
    base_dir: PathBuf,
    /// In-memory cache of the session index.
    index: Arc<RwLock<HashMap<Uuid, SessionIndexEntry>>>,
}

impl SqliteSessionStore {
    /// Create a new [`SqliteSessionStore`].
    ///
    /// The required directory structure is created automatically if it does
    /// not exist yet. The on-disk index is loaded into memory at startup.
    pub async fn new(base_dir: PathBuf) -> ArgentorResult<Self> {
        let sessions_dir = base_dir.join("sessions");
        tokio::fs::create_dir_all(&sessions_dir).await?;

        let index = Self::load_index(&base_dir).await?;

        Ok(Self {
            base_dir,
            index: Arc::new(RwLock::new(index)),
        })
    }

    // -- path helpers -------------------------------------------------------

    fn sessions_dir(&self) -> PathBuf {
        self.base_dir.join("sessions")
    }

    fn session_path(&self, id: Uuid) -> PathBuf {
        self.sessions_dir().join(format!("{id}.json"))
    }

    fn index_path(&self) -> PathBuf {
        self.base_dir.join("sessions").join("index.json")
    }

    // -- index persistence --------------------------------------------------

    async fn load_index(base_dir: &Path) -> ArgentorResult<HashMap<Uuid, SessionIndexEntry>> {
        let path = base_dir.join("sessions").join("index.json");
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let data = tokio::fs::read_to_string(&path).await?;
        let index: HashMap<Uuid, SessionIndexEntry> = serde_json::from_str(&data)
            .map_err(|e| ArgentorError::Session(format!("Failed to parse session index: {e}")))?;
        Ok(index)
    }

    async fn persist_index(&self, index: &HashMap<Uuid, SessionIndexEntry>) -> ArgentorResult<()> {
        let json = serde_json::to_string_pretty(index)?;
        atomic_write(&self.index_path(), json.as_bytes()).await
    }

    async fn refresh_index(&self) -> ArgentorResult<()> {
        let latest = Self::load_index(&self.base_dir).await?;
        let mut index = self.index.write().await;
        *index = latest;
        Ok(())
    }

    // -- public helpers beyond SessionStore -----------------------------------

    /// Return session IDs whose metadata contains a key with the given
    /// string value (exact, case-sensitive comparison).
    pub async fn query_by_metadata(&self, key: &str, value: &str) -> ArgentorResult<Vec<Uuid>> {
        self.refresh_index().await?;
        let index = self.index.read().await;
        let ids = index
            .iter()
            .filter(|(_, entry)| {
                entry
                    .metadata
                    .get(key)
                    .and_then(|v| v.as_str())
                    .is_some_and(|v| v == value)
            })
            .map(|(id, _)| *id)
            .collect();
        Ok(ids)
    }

    /// Return the total number of sessions tracked in the index.
    pub async fn count(&self) -> ArgentorResult<usize> {
        self.refresh_index().await?;
        let index = self.index.read().await;
        Ok(index.len())
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn create(&self, session: &Session) -> ArgentorResult<()> {
        let lock = IndexLock::acquire(&self.base_dir).await?;
        let path = self.session_path(session.id);
        let json = serde_json::to_string_pretty(session)?;
        atomic_write(&path, json.as_bytes()).await?;

        let entry = SessionIndexEntry {
            created_at: session.created_at,
            updated_at: session.updated_at,
            message_count: session.messages.len(),
            metadata: session.metadata.clone(),
        };

        let mut index = Self::load_index(&self.base_dir).await?;
        index.insert(session.id, entry);
        self.persist_index(&index).await?;
        {
            let mut cached = self.index.write().await;
            *cached = index;
        }
        lock.release().await?;

        Ok(())
    }

    async fn get(&self, id: Uuid) -> ArgentorResult<Option<Session>> {
        self.refresh_index().await?;
        // Fast-path: check the index first to avoid a filesystem hit.
        {
            let index = self.index.read().await;
            if !index.contains_key(&id) {
                return Ok(None);
            }
        }

        let path = self.session_path(id);
        if !path.exists() {
            return Ok(None);
        }

        let data = tokio::fs::read_to_string(path).await?;
        let session: Session = serde_json::from_str(&data)
            .map_err(|e| ArgentorError::Session(format!("Failed to parse session: {e}")))?;
        Ok(Some(session))
    }

    async fn update(&self, session: &Session) -> ArgentorResult<()> {
        self.create(session).await
    }

    async fn delete(&self, id: Uuid) -> ArgentorResult<()> {
        let lock = IndexLock::acquire(&self.base_dir).await?;
        let path = self.session_path(id);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }

        let mut index = Self::load_index(&self.base_dir).await?;
        index.remove(&id);
        self.persist_index(&index).await?;
        {
            let mut cached = self.index.write().await;
            *cached = index;
        }
        lock.release().await?;

        Ok(())
    }

    async fn list(&self) -> ArgentorResult<Vec<Uuid>> {
        self.refresh_index().await?;
        let index = self.index.read().await;
        Ok(index.keys().copied().collect())
    }
}

// ===========================================================================
// PersistentUsageStore
// ===========================================================================

/// A single usage record for billing / observability purposes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageRecord {
    /// Tenant that owns this usage.
    pub tenant_id: String,
    /// Role of the agent that consumed the tokens.
    pub agent_role: String,
    /// Model identifier (e.g. `"gpt-4o"`, `"claude-opus-4-20250514"`).
    pub model: String,
    /// Number of input tokens consumed.
    pub tokens_in: u64,
    /// Number of output tokens produced.
    pub tokens_out: u64,
    /// Estimated cost in USD.
    pub cost_usd: f64,
    /// When this usage was recorded.
    pub timestamp: DateTime<Utc>,
}

/// Append-only JSONL store for per-tenant usage records.
///
/// Records are stored in `{base_dir}/usage/{tenant_id}.jsonl`, one JSON
/// object per line. An in-memory cache allows fast reads without hitting
/// the filesystem on every call.
pub struct PersistentUsageStore {
    base_dir: PathBuf,
    /// In-memory cache: tenant_id -> Vec<UsageRecord>.
    cache: Arc<RwLock<HashMap<String, Vec<UsageRecord>>>>,
}

impl PersistentUsageStore {
    /// Create a new [`PersistentUsageStore`], loading any existing records
    /// from disk into memory.
    pub async fn new(base_dir: PathBuf) -> ArgentorResult<Self> {
        let usage_dir = base_dir.join("usage");
        tokio::fs::create_dir_all(&usage_dir).await?;

        let store = Self {
            base_dir,
            cache: Arc::new(RwLock::new(HashMap::new())),
        };

        // Pre-load all existing tenants.
        let tenants = store.load_all_tenants_from_disk().await?;
        let mut cache = store.cache.write().await;
        for tid in tenants {
            let records = store.load_usage_from_disk(&tid).await?;
            cache.insert(tid, records);
        }
        drop(cache);

        Ok(store)
    }

    fn usage_dir(&self) -> PathBuf {
        self.base_dir.join("usage")
    }

    fn usage_path(&self, tenant_id: &str) -> PathBuf {
        self.usage_dir().join(format!("{tenant_id}.jsonl"))
    }

    /// Append one or more usage records for a tenant. Records are written
    /// atomically by appending to a tmp file then renaming.
    pub async fn save_usage(&self, tenant_id: &str, records: &[UsageRecord]) -> ArgentorResult<()> {
        if records.is_empty() {
            return Ok(());
        }

        let path = self.usage_path(tenant_id);

        // Build the lines to append.
        let mut lines = String::new();
        for r in records {
            let line = serde_json::to_string(r)?;
            lines.push_str(&line);
            lines.push('\n');
        }

        // Read existing content (if any) so we can write the full file atomically.
        let mut existing = String::new();
        if path.exists() {
            existing = tokio::fs::read_to_string(&path).await?;
        }
        existing.push_str(&lines);

        atomic_write(&path, existing.as_bytes()).await?;

        // Update cache.
        let mut cache = self.cache.write().await;
        let entry = cache.entry(tenant_id.to_string()).or_default();
        entry.extend(records.iter().cloned());

        Ok(())
    }

    /// Load all usage records for a tenant.
    pub async fn load_usage(&self, tenant_id: &str) -> ArgentorResult<Vec<UsageRecord>> {
        let cache = self.cache.read().await;
        Ok(cache.get(tenant_id).cloned().unwrap_or_default())
    }

    /// Return a list of all tenant IDs that have usage data.
    pub async fn load_all_tenants(&self) -> ArgentorResult<Vec<String>> {
        let cache = self.cache.read().await;
        Ok(cache.keys().cloned().collect())
    }

    // -- internal helpers ---------------------------------------------------

    async fn load_all_tenants_from_disk(&self) -> ArgentorResult<Vec<String>> {
        let dir = self.usage_dir();
        let mut tenants = Vec::new();
        if !dir.exists() {
            return Ok(tenants);
        }
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if let Some(name) = entry.file_name().to_str() {
                if let Some(stem) = name.strip_suffix(".jsonl") {
                    tenants.push(stem.to_string());
                }
            }
        }
        Ok(tenants)
    }

    async fn load_usage_from_disk(&self, tenant_id: &str) -> ArgentorResult<Vec<UsageRecord>> {
        let path = self.usage_path(tenant_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let data = tokio::fs::read_to_string(&path).await?;
        let records: Vec<UsageRecord> = data
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ArgentorError::Session(format!("Failed to parse usage records: {e}")))?;
        Ok(records)
    }
}

// ===========================================================================
// PersistentPersonaStore
// ===========================================================================

/// Configuration for an agent persona.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersonaConfig {
    /// The tenant this persona belongs to.
    pub tenant_id: String,
    /// The agent role (e.g. `"coder"`, `"reviewer"`, `"planner"`).
    pub agent_role: String,
    /// Arbitrary configuration (system prompt, temperature, model, etc.).
    pub config: serde_json::Value,
    /// When this persona was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Persistent store for agent persona configurations.
///
/// Each persona is stored as `{base_dir}/personas/{tenant_id}_{role}.json`.
/// An in-memory cache keyed by `(tenant_id, role)` provides fast reads.
pub struct PersistentPersonaStore {
    base_dir: PathBuf,
    /// In-memory cache: (tenant_id, role) -> PersonaConfig.
    cache: Arc<RwLock<HashMap<(String, String), PersonaConfig>>>,
}

impl PersistentPersonaStore {
    /// Create a new [`PersistentPersonaStore`], loading any existing personas
    /// from disk into memory.
    pub async fn new(base_dir: PathBuf) -> ArgentorResult<Self> {
        let personas_dir = base_dir.join("personas");
        tokio::fs::create_dir_all(&personas_dir).await?;

        let store = Self {
            base_dir,
            cache: Arc::new(RwLock::new(HashMap::new())),
        };

        // Pre-load all existing personas.
        let personas = store.load_all_from_disk().await?;
        let mut cache = store.cache.write().await;
        for p in personas {
            cache.insert((p.tenant_id.clone(), p.agent_role.clone()), p);
        }
        drop(cache);

        Ok(store)
    }

    fn personas_dir(&self) -> PathBuf {
        self.base_dir.join("personas")
    }

    fn persona_path(&self, tenant_id: &str, role: &str) -> PathBuf {
        self.personas_dir().join(format!("{tenant_id}_{role}.json"))
    }

    /// Save (create or overwrite) a persona configuration.
    pub async fn save_persona(
        &self,
        tenant_id: &str,
        role: &str,
        config: serde_json::Value,
    ) -> ArgentorResult<()> {
        let persona = PersonaConfig {
            tenant_id: tenant_id.to_string(),
            agent_role: role.to_string(),
            config,
            updated_at: Utc::now(),
        };

        let path = self.persona_path(tenant_id, role);
        let json = serde_json::to_string_pretty(&persona)?;
        atomic_write(&path, json.as_bytes()).await?;

        let mut cache = self.cache.write().await;
        cache.insert((tenant_id.to_string(), role.to_string()), persona);

        Ok(())
    }

    /// Load a specific persona configuration.
    pub async fn load_persona(
        &self,
        tenant_id: &str,
        role: &str,
    ) -> ArgentorResult<Option<PersonaConfig>> {
        let cache = self.cache.read().await;
        Ok(cache
            .get(&(tenant_id.to_string(), role.to_string()))
            .cloned())
    }

    /// Load all personas for a given tenant.
    pub async fn load_all(&self, tenant_id: &str) -> ArgentorResult<Vec<PersonaConfig>> {
        let cache = self.cache.read().await;
        let personas = cache
            .iter()
            .filter(|((tid, _), _)| tid == tenant_id)
            .map(|(_, p)| p.clone())
            .collect();
        Ok(personas)
    }

    /// Delete a specific persona. Returns `true` if the persona existed.
    pub async fn delete_persona(&self, tenant_id: &str, role: &str) -> ArgentorResult<bool> {
        let path = self.persona_path(tenant_id, role);
        let existed = path.exists();
        if existed {
            tokio::fs::remove_file(&path).await?;
        }

        let mut cache = self.cache.write().await;
        cache.remove(&(tenant_id.to_string(), role.to_string()));

        Ok(existed)
    }

    // -- internal helpers ---------------------------------------------------

    async fn load_all_from_disk(&self) -> ArgentorResult<Vec<PersonaConfig>> {
        let dir = self.personas_dir();
        let mut personas = Vec::new();
        if !dir.exists() {
            return Ok(personas);
        }
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(".json") && !name.ends_with(".tmp") {
                    let data = tokio::fs::read_to_string(entry.path()).await?;
                    let persona: PersonaConfig = serde_json::from_str(&data).map_err(|e| {
                        ArgentorError::Session(format!("Failed to parse persona {name}: {e}"))
                    })?;
                    personas.push(persona);
                }
            }
        }
        Ok(personas)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // SqliteSessionStore tests
    // -----------------------------------------------------------------------

    async fn temp_session_store() -> (TempDir, SqliteSessionStore) {
        let tmp = TempDir::new().unwrap();
        let store = SqliteSessionStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();
        (tmp, store)
    }

    #[tokio::test]
    async fn session_create_and_get() {
        let (_tmp, store) = temp_session_store().await;
        let session = Session::new();

        store.create(&session).await.unwrap();
        let loaded = store.get(session.id).await.unwrap();

        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().id, session.id);
    }

    #[tokio::test]
    async fn session_get_nonexistent_returns_none() {
        let (_tmp, store) = temp_session_store().await;
        let result = store.get(Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn session_update_overwrites() {
        let (_tmp, store) = temp_session_store().await;
        let mut session = Session::new();
        store.create(&session).await.unwrap();

        session
            .metadata
            .insert("key".into(), serde_json::json!("v2"));
        store.update(&session).await.unwrap();

        let loaded = store.get(session.id).await.unwrap().unwrap();
        assert_eq!(
            loaded.metadata.get("key").unwrap(),
            &serde_json::json!("v2")
        );
    }

    #[tokio::test]
    async fn session_delete_removes() {
        let (_tmp, store) = temp_session_store().await;
        let session = Session::new();
        store.create(&session).await.unwrap();

        store.delete(session.id).await.unwrap();
        assert!(store.get(session.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn session_delete_nonexistent_is_ok() {
        let (_tmp, store) = temp_session_store().await;
        store.delete(Uuid::new_v4()).await.unwrap();
    }

    #[tokio::test]
    async fn session_list_returns_all_ids() {
        let (_tmp, store) = temp_session_store().await;
        let s1 = Session::new();
        let s2 = Session::new();
        store.create(&s1).await.unwrap();
        store.create(&s2).await.unwrap();

        let mut ids = store.list().await.unwrap();
        ids.sort();
        let mut expected = vec![s1.id, s2.id];
        expected.sort();
        assert_eq!(ids, expected);
    }

    #[tokio::test]
    async fn session_count() {
        let (_tmp, store) = temp_session_store().await;
        assert_eq!(store.count().await.unwrap(), 0);

        let s = Session::new();
        store.create(&s).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);

        store.delete(s.id).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn session_query_by_metadata() {
        let (_tmp, store) = temp_session_store().await;

        let mut s1 = Session::new();
        s1.metadata.insert("env".into(), serde_json::json!("prod"));
        store.create(&s1).await.unwrap();

        let mut s2 = Session::new();
        s2.metadata
            .insert("env".into(), serde_json::json!("staging"));
        store.create(&s2).await.unwrap();

        let results = store.query_by_metadata("env", "prod").await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results.contains(&s1.id));
    }

    #[tokio::test]
    async fn session_index_persists_across_instances() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        let session = Session::new();

        {
            let store = SqliteSessionStore::new(dir.clone()).await.unwrap();
            store.create(&session).await.unwrap();
        }

        {
            let store2 = SqliteSessionStore::new(dir).await.unwrap();
            let loaded = store2.get(session.id).await.unwrap();
            assert!(loaded.is_some());
            assert_eq!(loaded.unwrap().id, session.id);
        }
    }

    #[tokio::test]
    async fn session_instances_refresh_index_before_reads() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        let store1 = SqliteSessionStore::new(dir.clone()).await.unwrap();
        let store2 = SqliteSessionStore::new(dir).await.unwrap();
        let session = Session::new();

        store1.create(&session).await.unwrap();

        let loaded = store2.get(session.id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(store2.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn session_concurrent_creates_across_instances_preserve_index() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        let mut expected_ids = Vec::new();
        let mut handles = Vec::new();

        for _ in 0..10 {
            let store = SqliteSessionStore::new(dir.clone()).await.unwrap();
            let session = Session::new();
            expected_ids.push(session.id);
            handles.push(tokio::spawn(async move {
                store.create(&session).await.unwrap();
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let verifier = SqliteSessionStore::new(dir).await.unwrap();
        let mut ids = verifier.list().await.unwrap();
        ids.sort();
        expected_ids.sort();

        assert_eq!(ids, expected_ids);
    }

    #[tokio::test]
    async fn session_concurrent_creates() {
        let (_tmp, store) = temp_session_store().await;
        let store = Arc::new(store);

        let mut handles = Vec::new();
        let mut expected_ids = Vec::new();

        for _ in 0..10 {
            let s = Session::new();
            expected_ids.push(s.id);
            let store_clone = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                store_clone.create(&s).await.unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let mut ids = store.list().await.unwrap();
        ids.sort();
        expected_ids.sort();
        assert_eq!(ids, expected_ids);
    }

    #[tokio::test]
    async fn session_atomic_write_leaves_no_tmp_files() {
        let (tmp, store) = temp_session_store().await;
        let session = Session::new();
        store.create(&session).await.unwrap();

        // Check that no .tmp files remain in the sessions directory.
        let sessions_dir = tmp.path().join("sessions");
        let mut entries = tokio::fs::read_dir(&sessions_dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(
                !name.ends_with(".tmp"),
                "Temporary file should not remain: {name}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // PersistentUsageStore tests
    // -----------------------------------------------------------------------

    fn make_usage(tenant: &str, role: &str, model: &str, tokens_in: u64) -> UsageRecord {
        UsageRecord {
            tenant_id: tenant.to_string(),
            agent_role: role.to_string(),
            model: model.to_string(),
            tokens_in,
            tokens_out: tokens_in / 2,
            cost_usd: tokens_in as f64 * 0.001,
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn usage_save_and_load() {
        let tmp = TempDir::new().unwrap();
        let store = PersistentUsageStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();

        let records = vec![
            make_usage("t1", "coder", "gpt-4o", 100),
            make_usage("t1", "reviewer", "claude-opus-4-20250514", 200),
        ];
        store.save_usage("t1", &records).await.unwrap();

        let loaded = store.load_usage("t1").await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].tokens_in, 100);
        assert_eq!(loaded[1].tokens_in, 200);
    }

    #[tokio::test]
    async fn usage_load_empty_tenant() {
        let tmp = TempDir::new().unwrap();
        let store = PersistentUsageStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();

        let loaded = store.load_usage("nonexistent").await.unwrap();
        assert!(loaded.is_empty());
    }

    #[tokio::test]
    async fn usage_append_multiple_batches() {
        let tmp = TempDir::new().unwrap();
        let store = PersistentUsageStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();

        store
            .save_usage("t1", &[make_usage("t1", "coder", "gpt-4o", 50)])
            .await
            .unwrap();
        store
            .save_usage("t1", &[make_usage("t1", "coder", "gpt-4o", 75)])
            .await
            .unwrap();

        let loaded = store.load_usage("t1").await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].tokens_in, 50);
        assert_eq!(loaded[1].tokens_in, 75);
    }

    #[tokio::test]
    async fn usage_save_empty_records_is_noop() {
        let tmp = TempDir::new().unwrap();
        let store = PersistentUsageStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();

        store.save_usage("t1", &[]).await.unwrap();
        let loaded = store.load_usage("t1").await.unwrap();
        assert!(loaded.is_empty());
    }

    #[tokio::test]
    async fn usage_load_all_tenants() {
        let tmp = TempDir::new().unwrap();
        let store = PersistentUsageStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();

        store
            .save_usage("alpha", &[make_usage("alpha", "coder", "gpt-4o", 10)])
            .await
            .unwrap();
        store
            .save_usage("beta", &[make_usage("beta", "reviewer", "gpt-4o", 20)])
            .await
            .unwrap();

        let mut tenants = store.load_all_tenants().await.unwrap();
        tenants.sort();
        assert_eq!(tenants, vec!["alpha", "beta"]);
    }

    #[tokio::test]
    async fn usage_persists_across_instances() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();

        {
            let store = PersistentUsageStore::new(dir.clone()).await.unwrap();
            store
                .save_usage("t1", &[make_usage("t1", "coder", "gpt-4o", 42)])
                .await
                .unwrap();
        }

        {
            let store2 = PersistentUsageStore::new(dir).await.unwrap();
            let loaded = store2.load_usage("t1").await.unwrap();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].tokens_in, 42);
        }
    }

    // -----------------------------------------------------------------------
    // PersistentPersonaStore tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn persona_save_and_load() {
        let tmp = TempDir::new().unwrap();
        let store = PersistentPersonaStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();

        let config = serde_json::json!({
            "system_prompt": "You are a senior Rust developer.",
            "temperature": 0.3,
            "model": "claude-opus-4-20250514"
        });
        store
            .save_persona("t1", "coder", config.clone())
            .await
            .unwrap();

        let loaded = store.load_persona("t1", "coder").await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.tenant_id, "t1");
        assert_eq!(loaded.agent_role, "coder");
        assert_eq!(loaded.config, config);
    }

    #[tokio::test]
    async fn persona_load_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let store = PersistentPersonaStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();

        let loaded = store.load_persona("ghost", "phantom").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn persona_overwrite() {
        let tmp = TempDir::new().unwrap();
        let store = PersistentPersonaStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();

        store
            .save_persona("t1", "coder", serde_json::json!({"v": 1}))
            .await
            .unwrap();
        store
            .save_persona("t1", "coder", serde_json::json!({"v": 2}))
            .await
            .unwrap();

        let loaded = store.load_persona("t1", "coder").await.unwrap().unwrap();
        assert_eq!(loaded.config, serde_json::json!({"v": 2}));
    }

    #[tokio::test]
    async fn persona_load_all_for_tenant() {
        let tmp = TempDir::new().unwrap();
        let store = PersistentPersonaStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();

        store
            .save_persona("t1", "coder", serde_json::json!({}))
            .await
            .unwrap();
        store
            .save_persona("t1", "reviewer", serde_json::json!({}))
            .await
            .unwrap();
        store
            .save_persona("t2", "planner", serde_json::json!({}))
            .await
            .unwrap();

        let t1_personas = store.load_all("t1").await.unwrap();
        assert_eq!(t1_personas.len(), 2);

        let t2_personas = store.load_all("t2").await.unwrap();
        assert_eq!(t2_personas.len(), 1);
    }

    #[tokio::test]
    async fn persona_delete() {
        let tmp = TempDir::new().unwrap();
        let store = PersistentPersonaStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();

        store
            .save_persona("t1", "coder", serde_json::json!({}))
            .await
            .unwrap();
        let existed = store.delete_persona("t1", "coder").await.unwrap();
        assert!(existed);

        let loaded = store.load_persona("t1", "coder").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn persona_delete_nonexistent_returns_false() {
        let tmp = TempDir::new().unwrap();
        let store = PersistentPersonaStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();

        let existed = store.delete_persona("ghost", "phantom").await.unwrap();
        assert!(!existed);
    }

    #[tokio::test]
    async fn persona_persists_across_instances() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();

        {
            let store = PersistentPersonaStore::new(dir.clone()).await.unwrap();
            store
                .save_persona("t1", "coder", serde_json::json!({"lang": "rust"}))
                .await
                .unwrap();
        }

        {
            let store2 = PersistentPersonaStore::new(dir).await.unwrap();
            let loaded = store2.load_persona("t1", "coder").await.unwrap().unwrap();
            assert_eq!(loaded.config, serde_json::json!({"lang": "rust"}));
        }
    }

    #[tokio::test]
    async fn persona_load_all_empty_tenant() {
        let tmp = TempDir::new().unwrap();
        let store = PersistentPersonaStore::new(tmp.path().to_path_buf())
            .await
            .unwrap();

        let personas = store.load_all("nonexistent").await.unwrap();
        assert!(personas.is_empty());
    }
}
