//! Skill Marketplace — registry system for discovering, downloading, and publishing skills.
//!
//! The marketplace supports local catalog management, search, install, and
//! dependency resolution via [`MarketplaceCatalog`], [`MarketplaceManager`], and
//! [`MarketplaceSearch`]. These components are fully functional and require no
//! external dependencies.
//!
//! Remote registry support is available through [`MarketplaceClient`]. Enable the
//! `registry` feature flag on `argentor-skills` to get a fully functional HTTP
//! client backed by [`reqwest`]. Without the feature, `MarketplaceClient` methods
//! return a descriptive configuration error.

use crate::vetting::{SkillIndex, SkillManifest, SkillVetter, VettingResult};
use argentor_core::ArgentorResult;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Dependency on another skill with a semver version requirement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillDependency {
    /// Name of the required skill.
    pub name: String,
    /// Semver range requirement (e.g., ">=1.0.0").
    pub version_req: String,
}

/// Extended metadata for a skill published to the marketplace catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceEntry {
    /// Core manifest with name, version, checksum, capabilities, etc.
    pub manifest: SkillManifest,
    /// Total number of downloads.
    pub downloads: u64,
    /// Average rating (0.0 -- 5.0).
    pub rating: f32,
    /// Number of individual ratings submitted.
    pub ratings_count: u32,
    /// High-level categories (e.g., "data", "web", "security").
    pub categories: Vec<String>,
    /// Whether this entry is editorially featured.
    pub featured: bool,
    /// RFC 3339 timestamp of initial publication.
    pub published_at: String,
    /// RFC 3339 timestamp of the most recent update.
    pub updated_at: String,
    /// Size of the WASM binary in bytes.
    pub size_bytes: u64,
    /// URL to download the WASM binary.
    pub download_url: Option<String>,
    /// Homepage or documentation URL.
    pub homepage: Option<String>,
    /// Free-form keywords for discoverability.
    pub keywords: Vec<String>,
    /// Skills this entry depends on.
    pub dependencies: Vec<SkillDependency>,
}

/// Search query parameters for the marketplace catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceSearch {
    /// Free-text search across name, description, and keywords.
    pub query: Option<String>,
    /// Filter by category.
    pub category: Option<String>,
    /// Filter by author.
    pub author: Option<String>,
    /// Minimum average rating threshold.
    pub min_rating: Option<f32>,
    /// All listed tags must be present in the entry's manifest tags.
    pub tags: Vec<String>,
    /// Sort order for results.
    pub sort_by: SortBy,
    /// Maximum number of results to return.
    pub limit: usize,
    /// Number of results to skip (for pagination).
    pub offset: usize,
}

impl Default for MarketplaceSearch {
    fn default() -> Self {
        Self {
            query: None,
            category: None,
            author: None,
            min_rating: None,
            tags: Vec::new(),
            sort_by: SortBy::Relevance,
            limit: 50,
            offset: 0,
        }
    }
}

/// Sort order for marketplace search results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortBy {
    /// Relevance to the search query (default).
    Relevance,
    /// Most downloaded first.
    Downloads,
    /// Highest rated first.
    Rating,
    /// Most recently updated first.
    Recent,
    /// Alphabetical by name.
    Name,
}

// ---------------------------------------------------------------------------
// MarketplaceCatalog — in-memory searchable catalog
// ---------------------------------------------------------------------------

/// In-memory searchable catalog of marketplace entries.
///
/// # Examples
///
/// ```no_run
/// use argentor_skills::marketplace::{MarketplaceCatalog, MarketplaceSearch, SortBy};
///
/// let catalog = MarketplaceCatalog::new();
///
/// // Search for skills by keyword
/// let query = MarketplaceSearch {
///     query: Some("calculator".into()),
///     sort_by: SortBy::Downloads,
///     ..Default::default()
/// };
/// let results = catalog.search(&query);
///
/// // List editorially featured skills
/// let featured = catalog.list_featured();
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceCatalog {
    entries: Vec<MarketplaceEntry>,
}

impl MarketplaceCatalog {
    /// Create an empty catalog.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add an entry to the catalog. Replaces any existing entry with the same name.
    pub fn add(&mut self, entry: MarketplaceEntry) {
        self.entries
            .retain(|e| e.manifest.name != entry.manifest.name);
        self.entries.push(entry);
    }

    /// Remove an entry by name. Returns `true` if an entry was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.manifest.name != name);
        self.entries.len() < before
    }

    /// Look up an entry by skill name.
    pub fn get(&self, name: &str) -> Option<&MarketplaceEntry> {
        self.entries.iter().find(|e| e.manifest.name == name)
    }

    /// Search the catalog using a structured query.
    pub fn search(&self, query: &MarketplaceSearch) -> Vec<&MarketplaceEntry> {
        let mut results: Vec<&MarketplaceEntry> = self
            .entries
            .iter()
            .filter(|e| Self::matches(e, query))
            .collect();

        Self::sort_entries(&mut results, query.sort_by);

        // Pagination
        results
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect()
    }

    /// Return all featured entries.
    pub fn list_featured(&self) -> Vec<&MarketplaceEntry> {
        self.entries.iter().filter(|e| e.featured).collect()
    }

    /// Return entries belonging to the given category.
    pub fn list_by_category(&self, category: &str) -> Vec<&MarketplaceEntry> {
        let cat_lower = category.to_lowercase();
        self.entries
            .iter()
            .filter(|e| e.categories.iter().any(|c| c.to_lowercase() == cat_lower))
            .collect()
    }

    /// Return the most popular entries sorted by download count (descending).
    pub fn list_popular(&self, limit: usize) -> Vec<&MarketplaceEntry> {
        let mut items: Vec<&MarketplaceEntry> = self.entries.iter().collect();
        items.sort_by_key(|entry| std::cmp::Reverse(entry.downloads));
        items.into_iter().take(limit).collect()
    }

    /// Return the top-rated entries sorted by rating (descending).
    pub fn list_top_rated(&self, limit: usize) -> Vec<&MarketplaceEntry> {
        let mut items: Vec<&MarketplaceEntry> = self.entries.iter().collect();
        items.sort_by(|a, b| {
            b.rating
                .partial_cmp(&a.rating)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        items.into_iter().take(limit).collect()
    }

    /// Return the most recently updated entries.
    pub fn list_recent(&self, limit: usize) -> Vec<&MarketplaceEntry> {
        let mut items: Vec<&MarketplaceEntry> = self.entries.iter().collect();
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        items.into_iter().take(limit).collect()
    }

    /// Total number of entries in the catalog.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Return all unique categories across the catalog, sorted alphabetically.
    pub fn categories(&self) -> Vec<String> {
        let mut set: HashSet<String> = HashSet::new();
        for entry in &self.entries {
            for cat in &entry.categories {
                set.insert(cat.clone());
            }
        }
        let mut cats: Vec<String> = set.into_iter().collect();
        cats.sort();
        cats
    }

    /// Persist the catalog as JSON to a file. Writes to a temporary file first,
    /// then atomically renames to prevent partial writes.
    pub fn save(&self, path: &Path) -> ArgentorResult<()> {
        let content = serde_json::to_string_pretty(self).map_err(|e| {
            argentor_core::ArgentorError::Config(format!("Failed to serialize catalog: {e}"))
        })?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, content).map_err(|e| {
            argentor_core::ArgentorError::Config(format!("Failed to write catalog tmp: {e}"))
        })?;
        std::fs::rename(&tmp, path).map_err(|e| {
            argentor_core::ArgentorError::Config(format!("Failed to rename catalog: {e}"))
        })?;
        Ok(())
    }

    /// Load a catalog from a JSON file.
    pub fn load(path: &Path) -> ArgentorResult<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let content = std::fs::read_to_string(path).map_err(|e| {
            argentor_core::ArgentorError::Config(format!("Failed to read catalog: {e}"))
        })?;
        serde_json::from_str(&content).map_err(|e| {
            argentor_core::ArgentorError::Config(format!("Failed to parse catalog: {e}"))
        })
    }

    // -- private helpers --

    fn matches(entry: &MarketplaceEntry, query: &MarketplaceSearch) -> bool {
        // Free-text search (case-insensitive substring in name, description, keywords)
        if let Some(ref q) = query.query {
            let q_lower = q.to_lowercase();
            let in_name = entry.manifest.name.to_lowercase().contains(&q_lower);
            let in_desc = entry.manifest.description.to_lowercase().contains(&q_lower);
            let in_kw = entry
                .keywords
                .iter()
                .any(|k| k.to_lowercase().contains(&q_lower));
            if !in_name && !in_desc && !in_kw {
                return false;
            }
        }
        // Category filter
        if let Some(ref cat) = query.category {
            let cat_lower = cat.to_lowercase();
            if !entry
                .categories
                .iter()
                .any(|c| c.to_lowercase() == cat_lower)
            {
                return false;
            }
        }
        // Author filter
        if let Some(ref author) = query.author {
            if entry.manifest.author.to_lowercase() != author.to_lowercase() {
                return false;
            }
        }
        // Minimum rating filter
        if let Some(min) = query.min_rating {
            if entry.rating < min {
                return false;
            }
        }
        // All specified tags must be present
        for tag in &query.tags {
            let tag_lower = tag.to_lowercase();
            if !entry
                .manifest
                .tags
                .iter()
                .any(|t| t.to_lowercase() == tag_lower)
            {
                return false;
            }
        }
        true
    }

    fn sort_entries(results: &mut [&MarketplaceEntry], sort_by: SortBy) {
        match sort_by {
            SortBy::Downloads => {
                results.sort_by_key(|entry| std::cmp::Reverse(entry.downloads));
            }
            SortBy::Rating => results.sort_by(|a, b| {
                b.rating
                    .partial_cmp(&a.rating)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            SortBy::Recent => results.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
            SortBy::Name => results.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name)),
            SortBy::Relevance => { /* keep insertion order */ }
        }
    }
}

impl Default for MarketplaceCatalog {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// MarketplaceClient — remote registry HTTP client
// ---------------------------------------------------------------------------

/// HTTP client for interacting with a remote Argentor skill registry.
///
/// # Feature flag
///
/// Enable the `registry` feature on `argentor-skills` to get a fully functional
/// HTTP client backed by [`reqwest`]. Without the feature, all methods return a
/// configuration error directing callers to use [`MarketplaceCatalog`] for local
/// skill management instead.
///
/// ```toml
/// [dependencies]
/// argentor-skills = { version = "...", features = ["registry"] }
/// ```
///
/// # API contract
///
/// The client expects a JSON REST API at `{base_url}/api/v1/skills/…`:
///
/// | Method   | Endpoint                                           | Auth     |
/// |----------|----------------------------------------------------|----------|
/// | `search` | `GET  /api/v1/skills/search?q=…&category=…&limit=…`| none     |
/// | `get`    | `GET  /api/v1/skills/{name}`                       | none     |
/// | `download`| `GET /api/v1/skills/{name}/download?version=…`    | none     |
/// | `publish`| `POST /api/v1/skills` (multipart)                  | Bearer   |
/// | `rate`   | `POST /api/v1/skills/{name}/rate` (JSON body)      | Bearer   |
pub struct MarketplaceClient {
    /// Base URL of the registry API (no trailing slash).
    base_url: String,
    #[cfg(feature = "registry")]
    http: reqwest::Client,
}

/// Response wrapper used by the registry search endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    /// Matching entries.
    pub results: Vec<MarketplaceEntry>,
    /// Total number of matches (may exceed `results.len()` due to pagination).
    pub total: usize,
}

/// JSON body sent when rating a skill.
#[cfg(feature = "registry")]
#[derive(Debug, Serialize)]
struct RateRequest {
    rating: f32,
}

// ---- implementation when `registry` is enabled ----

#[cfg(feature = "registry")]
impl MarketplaceClient {
    /// Create a client pointing at the given registry URL.
    ///
    /// The URL should not include a trailing slash. Example:
    /// `https://registry.argentor.dev`
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Create a client pointing at the default Argentor registry.
    pub fn default_registry() -> Self {
        Self::new("https://registry.argentor.dev")
    }

    /// Create a client with a custom [`reqwest::Client`] (useful for tests
    /// or custom TLS / proxy configuration).
    pub fn with_http_client(base_url: impl Into<String>, http: reqwest::Client) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http,
        }
    }

    /// Return the base URL this client is configured to talk to.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Search the remote registry for skills matching the given query.
    pub async fn search(&self, query: &MarketplaceSearch) -> ArgentorResult<Vec<MarketplaceEntry>> {
        let mut url = format!("{}/api/v1/skills/search", self.base_url);
        let mut params: Vec<(&str, String)> = Vec::new();

        if let Some(ref q) = query.query {
            params.push(("q", q.clone()));
        }
        if let Some(ref cat) = query.category {
            params.push(("category", cat.clone()));
        }
        if let Some(ref author) = query.author {
            params.push(("author", author.clone()));
        }
        if let Some(min) = query.min_rating {
            params.push(("min_rating", min.to_string()));
        }
        for tag in &query.tags {
            params.push(("tag", tag.clone()));
        }
        params.push(("sort", format!("{:?}", query.sort_by)));
        params.push(("limit", query.limit.to_string()));
        params.push(("offset", query.offset.to_string()));

        if !params.is_empty() {
            url.push('?');
            let qs: Vec<String> = params
                .iter()
                .map(|(k, v)| format!("{k}={}", urlencoded(v)))
                .collect();
            url.push_str(&qs.join("&"));
        }

        let resp = self.http.get(&url).send().await.map_err(|e| {
            argentor_core::ArgentorError::Http(format!("Search request failed: {e}"))
        })?;

        handle_error_status(&resp, "search")?;

        let body: SearchResponse = resp.json().await.map_err(|e| {
            argentor_core::ArgentorError::Http(format!("Failed to parse search response: {e}"))
        })?;

        Ok(body.results)
    }

    /// Get a single skill entry by name from the remote registry.
    pub async fn get(&self, name: &str) -> ArgentorResult<MarketplaceEntry> {
        let url = format!("{}/api/v1/skills/{}", self.base_url, urlencoded(name));

        let resp =
            self.http.get(&url).send().await.map_err(|e| {
                argentor_core::ArgentorError::Http(format!("Get request failed: {e}"))
            })?;

        handle_error_status(&resp, "get")?;

        resp.json().await.map_err(|e| {
            argentor_core::ArgentorError::Http(format!("Failed to parse skill entry: {e}"))
        })
    }

    /// Download the WASM binary for a skill. If `version` is `None` the
    /// registry returns the latest version.
    pub async fn download(&self, name: &str, version: Option<&str>) -> ArgentorResult<Vec<u8>> {
        let mut url = format!(
            "{}/api/v1/skills/{}/download",
            self.base_url,
            urlencoded(name),
        );
        if let Some(v) = version {
            url.push_str(&format!("?version={}", urlencoded(v)));
        }

        let resp = self.http.get(&url).send().await.map_err(|e| {
            argentor_core::ArgentorError::Http(format!("Download request failed: {e}"))
        })?;

        handle_error_status(&resp, "download")?;

        resp.bytes().await.map(|b| b.to_vec()).map_err(|e| {
            argentor_core::ArgentorError::Http(format!("Failed to read download body: {e}"))
        })
    }

    /// Publish a skill to the remote registry.
    ///
    /// Sends a multipart request with the manifest JSON and WASM binary.
    /// Requires a valid API key for authorization.
    pub async fn publish(
        &self,
        manifest: &SkillManifest,
        wasm_bytes: &[u8],
        api_key: &str,
    ) -> ArgentorResult<()> {
        let url = format!("{}/api/v1/skills", self.base_url);

        let manifest_json = serde_json::to_string(manifest).map_err(|e| {
            argentor_core::ArgentorError::Config(format!("Failed to serialize manifest: {e}"))
        })?;

        let manifest_part = reqwest::multipart::Part::text(manifest_json)
            .file_name("manifest.json")
            .mime_str("application/json")
            .map_err(|e| {
                argentor_core::ArgentorError::Http(format!("Failed to build manifest part: {e}"))
            })?;

        let wasm_part = reqwest::multipart::Part::bytes(wasm_bytes.to_vec())
            .file_name("skill.wasm")
            .mime_str("application/wasm")
            .map_err(|e| {
                argentor_core::ArgentorError::Http(format!("Failed to build WASM part: {e}"))
            })?;

        let form = reqwest::multipart::Form::new()
            .part("manifest", manifest_part)
            .part("wasm", wasm_part);

        let resp = self
            .http
            .post(&url)
            .bearer_auth(api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                argentor_core::ArgentorError::Http(format!("Publish request failed: {e}"))
            })?;

        handle_error_status(&resp, "publish")?;
        Ok(())
    }

    /// Submit a rating (0.0 -- 5.0) for a skill. Requires a valid API key.
    pub async fn rate(&self, name: &str, rating: f32, api_key: &str) -> ArgentorResult<()> {
        if !(0.0..=5.0).contains(&rating) {
            return Err(argentor_core::ArgentorError::Config(format!(
                "Rating must be between 0.0 and 5.0, got {rating}"
            )));
        }

        let url = format!("{}/api/v1/skills/{}/rate", self.base_url, urlencoded(name),);

        let resp = self
            .http
            .post(&url)
            .bearer_auth(api_key)
            .json(&RateRequest { rating })
            .send()
            .await
            .map_err(|e| argentor_core::ArgentorError::Http(format!("Rate request failed: {e}")))?;

        handle_error_status(&resp, "rate")?;
        Ok(())
    }
}

// ---- stub implementation when `registry` is NOT enabled ----

#[cfg(not(feature = "registry"))]
impl MarketplaceClient {
    /// Create a client pointing at the given registry URL.
    ///
    /// **Note:** the `registry` feature is not enabled. All methods will
    /// return a configuration error. Enable the feature to get a working
    /// HTTP client.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// Create a client pointing at the default Argentor registry.
    pub fn default_registry() -> Self {
        Self::new("https://registry.argentor.dev")
    }

    /// Return the base URL this client is configured to talk to.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Search the remote registry.
    ///
    /// Always returns an error — enable the `registry` feature for HTTP support.
    pub async fn search(
        &self,
        _query: &MarketplaceSearch,
    ) -> ArgentorResult<Vec<MarketplaceEntry>> {
        Err(argentor_core::ArgentorError::Config(
            "Remote registry requires the `registry` feature. \
             Enable it in Cargo.toml or use MarketplaceCatalog for local skill management."
                .into(),
        ))
    }

    /// Get a single entry by name from the remote registry.
    ///
    /// Always returns an error — enable the `registry` feature for HTTP support.
    pub async fn get(&self, _name: &str) -> ArgentorResult<MarketplaceEntry> {
        Err(argentor_core::ArgentorError::Config(
            "Remote registry requires the `registry` feature. \
             Enable it in Cargo.toml or use MarketplaceCatalog for local skill management."
                .into(),
        ))
    }

    /// Download WASM bytes for a skill from the remote registry.
    ///
    /// Always returns an error — enable the `registry` feature for HTTP support.
    pub async fn download(&self, _name: &str, _version: Option<&str>) -> ArgentorResult<Vec<u8>> {
        Err(argentor_core::ArgentorError::Config(
            "Remote registry requires the `registry` feature. \
             Enable it in Cargo.toml or use MarketplaceCatalog for local skill management."
                .into(),
        ))
    }

    /// Publish a skill to the remote registry.
    ///
    /// Always returns an error — enable the `registry` feature for HTTP support.
    pub async fn publish(
        &self,
        _manifest: &SkillManifest,
        _wasm_bytes: &[u8],
        _api_key: &str,
    ) -> ArgentorResult<()> {
        Err(argentor_core::ArgentorError::Config(
            "Remote registry requires the `registry` feature. \
             Enable it in Cargo.toml or use MarketplaceCatalog for local skill management."
                .into(),
        ))
    }

    /// Submit a rating for a skill.
    ///
    /// Always returns an error — enable the `registry` feature for HTTP support.
    pub async fn rate(&self, _name: &str, _rating: f32, _api_key: &str) -> ArgentorResult<()> {
        Err(argentor_core::ArgentorError::Config(
            "Remote registry requires the `registry` feature. \
             Enable it in Cargo.toml or use MarketplaceCatalog for local skill management."
                .into(),
        ))
    }
}

// ---- shared helpers ----

/// Minimal percent-encoding for URL path segments and query values.
#[cfg(any(feature = "registry", test))]
fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// Check a response status and convert HTTP errors into `ArgentorError`.
#[cfg(feature = "registry")]
fn handle_error_status(resp: &reqwest::Response, operation: &str) -> ArgentorResult<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }

    match status.as_u16() {
        401 => Err(argentor_core::ArgentorError::Security(format!(
            "Registry {operation}: authentication required (401). Provide a valid API key."
        ))),
        403 => Err(argentor_core::ArgentorError::Security(format!(
            "Registry {operation}: access denied (403). Check your API key permissions."
        ))),
        404 => Err(argentor_core::ArgentorError::Config(format!(
            "Registry {operation}: resource not found (404)."
        ))),
        409 => Err(argentor_core::ArgentorError::Config(format!(
            "Registry {operation}: conflict (409). The skill version may already exist."
        ))),
        429 => Err(argentor_core::ArgentorError::Http(format!(
            "Registry {operation}: rate limited (429). Try again later."
        ))),
        code if (500..600).contains(&code) => Err(argentor_core::ArgentorError::Http(format!(
            "Registry {operation}: server error ({code}). Try again later."
        ))),
        code => Err(argentor_core::ArgentorError::Http(format!(
            "Registry {operation}: unexpected status {code}."
        ))),
    }
}

// ---------------------------------------------------------------------------
// Compatibility & upgrade info
// ---------------------------------------------------------------------------

/// Result of a compatibility check for installing a marketplace entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityResult {
    /// Whether the entry is compatible with the current environment.
    pub compatible: bool,
    /// Human-readable descriptions of any incompatibilities found.
    pub issues: Vec<String>,
}

/// Information about an available upgrade for an installed skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeInfo {
    /// Skill name.
    pub name: String,
    /// Currently installed version.
    pub installed_version: String,
    /// Version available in the catalog.
    pub available_version: String,
}

// ---------------------------------------------------------------------------
// MarketplaceManager — orchestrates catalog + index + vetting
// ---------------------------------------------------------------------------

/// High-level manager that ties the catalog, local index, and vetter together
/// to provide install, uninstall, dependency resolution, and upgrade detection.
pub struct MarketplaceManager {
    catalog: MarketplaceCatalog,
    index: SkillIndex,
    vetter: SkillVetter,
    skills_dir: PathBuf,
    catalog_path: PathBuf,
}

impl MarketplaceManager {
    /// Create a new manager with default vetter and empty catalog/index.
    pub fn new(skills_dir: PathBuf, catalog_path: PathBuf) -> Self {
        Self {
            catalog: MarketplaceCatalog::new(),
            index: SkillIndex::new(),
            vetter: SkillVetter::new(),
            skills_dir,
            catalog_path,
        }
    }

    /// Replace the vetter with a custom one (builder pattern).
    pub fn with_vetter(mut self, vetter: SkillVetter) -> Self {
        self.vetter = vetter;
        self
    }

    /// Access the underlying catalog.
    pub fn catalog(&self) -> &MarketplaceCatalog {
        &self.catalog
    }

    /// Access the local installed-skills index.
    pub fn installed(&self) -> &SkillIndex {
        &self.index
    }

    /// Install a skill from raw bytes, running the full vetting pipeline first.
    pub fn install_from_bytes(
        &mut self,
        manifest: SkillManifest,
        wasm_bytes: &[u8],
    ) -> ArgentorResult<VettingResult> {
        self.index
            .install(manifest, wasm_bytes, &self.skills_dir, &self.vetter)
    }

    /// Uninstall a skill by name. Returns `true` if it was installed and removed.
    pub fn uninstall(&mut self, name: &str) -> ArgentorResult<bool> {
        self.index.uninstall(name, &self.skills_dir)
    }

    /// Check whether a skill is currently installed.
    pub fn is_installed(&self, name: &str) -> bool {
        self.index.get(name).is_some()
    }

    /// Return the installed version of a skill, if any.
    pub fn installed_version(&self, name: &str) -> Option<&str> {
        self.index
            .get(name)
            .map(|entry| entry.manifest.version.as_str())
    }

    /// Resolve the dependency graph for an entry using topological sort
    /// (Kahn's algorithm). Returns an ordered list of skill names that
    /// must be installed before the given entry.
    pub fn resolve_dependencies(&self, entry: &MarketplaceEntry) -> ArgentorResult<Vec<String>> {
        // Build adjacency list and in-degree map
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        let mut in_degree: HashMap<String, usize> = HashMap::new();

        // Seed with the target entry
        let mut to_visit: VecDeque<String> = VecDeque::new();
        to_visit.push_back(entry.manifest.name.clone());
        graph.entry(entry.manifest.name.clone()).or_default();
        in_degree.entry(entry.manifest.name.clone()).or_insert(0);

        while let Some(current) = to_visit.pop_front() {
            let deps = if current == entry.manifest.name {
                &entry.dependencies
            } else if let Some(cat_entry) = self.catalog.get(&current) {
                &cat_entry.dependencies
            } else {
                continue;
            };

            for dep in deps {
                graph.entry(dep.name.clone()).or_default();
                graph.entry(current.clone()).or_default();
                // dep -> current (current depends on dep)
                graph
                    .entry(dep.name.clone())
                    .or_default()
                    .push(current.clone());
                *in_degree.entry(current.clone()).or_insert(0) += 1;
                in_degree.entry(dep.name.clone()).or_insert(0);

                if !graph.contains_key(&dep.name) || !to_visit.contains(&dep.name) {
                    to_visit.push_back(dep.name.clone());
                }
            }
        }

        // Kahn's algorithm
        let mut queue: VecDeque<String> = VecDeque::new();
        for (node, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(node.clone());
            }
        }

        let mut order: Vec<String> = Vec::new();
        while let Some(node) = queue.pop_front() {
            order.push(node.clone());
            if let Some(neighbors) = graph.get(&node) {
                for neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            queue.push_back(neighbor.clone());
                        }
                    }
                }
            }
        }

        if order.len() != in_degree.len() {
            return Err(argentor_core::ArgentorError::Config(
                "Circular dependency detected in skill graph".into(),
            ));
        }

        // Remove the entry itself — caller only needs the *dependencies* in order
        order.retain(|n| n != &entry.manifest.name);
        Ok(order)
    }

    /// Check whether a marketplace entry is compatible with the current environment.
    /// Validates dependency availability and version constraints.
    pub fn check_compatibility(&self, entry: &MarketplaceEntry) -> CompatibilityResult {
        let mut issues = Vec::new();

        // Check that all dependencies exist in the catalog or are installed
        for dep in &entry.dependencies {
            let in_catalog = self.catalog.get(&dep.name).is_some();
            let installed = self.index.get(&dep.name).is_some();
            if !in_catalog && !installed {
                issues.push(format!(
                    "Missing dependency '{}' (requires {})",
                    dep.name, dep.version_req
                ));
            }
        }

        // Check min_argentor_version (placeholder — always passes for now)
        if let Some(ref _min_ver) = entry.manifest.min_argentor_version {
            // Future: compare against actual argentor version
        }

        CompatibilityResult {
            compatible: issues.is_empty(),
            issues,
        }
    }

    /// Replace the catalog entries with a fresh set (e.g., after syncing with remote).
    pub fn update_catalog(&mut self, entries: Vec<MarketplaceEntry>) {
        self.catalog = MarketplaceCatalog::new();
        for entry in entries {
            self.catalog.add(entry);
        }
    }

    /// List installed skills that have a newer version available in the catalog.
    pub fn list_upgradable(&self) -> Vec<UpgradeInfo> {
        self.index
            .list()
            .iter()
            .filter_map(|installed| {
                let catalog_entry = self.catalog.get(&installed.manifest.name)?;
                if catalog_entry.manifest.version != installed.manifest.version {
                    Some(UpgradeInfo {
                        name: installed.manifest.name.clone(),
                        installed_version: installed.manifest.version.clone(),
                        available_version: catalog_entry.manifest.version.clone(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    /// Persist both the catalog and the index to disk.
    pub fn save(&self) -> ArgentorResult<()> {
        self.catalog.save(&self.catalog_path)?;
        let index_path = self.skills_dir.join("index.json");
        self.index.save(&index_path)?;
        Ok(())
    }

    /// Load catalog and index from disk.
    pub fn load(skills_dir: PathBuf, catalog_path: PathBuf) -> ArgentorResult<Self> {
        let catalog = MarketplaceCatalog::load(&catalog_path)?;
        let index_path = skills_dir.join("index.json");
        let index = SkillIndex::load(&index_path)?;
        Ok(Self {
            catalog,
            index,
            vetter: SkillVetter::new(),
            skills_dir,
            catalog_path,
        })
    }
}

// ---------------------------------------------------------------------------
// Built-in catalog entries for the 18 utility skills
// ---------------------------------------------------------------------------

/// Configuration for creating a built-in skill marketplace entry.
struct BuiltinEntryConfig<'a> {
    name: &'a str,
    version: &'a str,
    description: &'a str,
    author: &'a str,
    categories: &'a [&'a str],
    tags: &'a [&'a str],
    keywords: &'a [&'a str],
    rating: f32,
    downloads: u64,
}

/// Create a `MarketplaceEntry` with reasonable defaults for a built-in skill.
fn builtin_entry(cfg: BuiltinEntryConfig<'_>) -> MarketplaceEntry {
    MarketplaceEntry {
        manifest: SkillManifest {
            name: cfg.name.to_string(),
            version: cfg.version.to_string(),
            description: cfg.description.to_string(),
            author: cfg.author.to_string(),
            license: Some("AGPL-3.0-only".to_string()),
            checksum: "builtin".to_string(),
            capabilities: Vec::new(),
            signature: None,
            signer_key: None,
            min_argentor_version: Some("0.1.0".to_string()),
            tags: cfg
                .tags
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            repository: Some("https://github.com/fboiero/Agentor".to_string()),
        },
        downloads: cfg.downloads,
        rating: cfg.rating,
        ratings_count: (cfg.downloads / 10).max(1) as u32,
        categories: cfg
            .categories
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        featured: cfg.rating >= 4.5,
        published_at: "2025-01-01T00:00:00Z".to_string(),
        updated_at: "2025-06-01T00:00:00Z".to_string(),
        size_bytes: 0,
        download_url: None,
        homepage: Some("https://github.com/fboiero/Agentor".to_string()),
        keywords: cfg
            .keywords
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        dependencies: Vec::new(),
    }
}

/// Returns `MarketplaceEntry` instances for the 36 built-in utility skills.
///
/// Categories used: `data`, `text`, `crypto`, `encoding`, `web`, `search`,
/// `security`, `ai`, `document`.
pub fn builtin_catalog_entries() -> Vec<MarketplaceEntry> {
    vec![
        // -- Data & Text (6) --
        builtin_entry(BuiltinEntryConfig {
            name: "calculator",
            version: "1.0.0",
            description: "Pure-math calculator for precise arithmetic, trigonometry, and expression evaluation",
            author: "argentor",
            categories: &["data"],
            tags: &["math", "arithmetic", "calculator"],
            keywords: &["calculate", "math", "expression", "eval"],
            rating: 4.8,
            downloads: 12000,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "text_transform",
            version: "1.0.0",
            description: "String manipulation: case conversion, trim, reverse, repeat, slugify, and more",
            author: "argentor",
            categories: &["text"],
            tags: &["text", "string", "transform"],
            keywords: &["uppercase", "lowercase", "slugify", "trim", "reverse"],
            rating: 4.6,
            downloads: 9500,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "json_query",
            version: "1.0.0",
            description: "JSON querying, extraction, and manipulation with JSONPath-like syntax",
            author: "argentor",
            categories: &["data"],
            tags: &["json", "query", "data"],
            keywords: &["jsonpath", "extract", "filter", "transform"],
            rating: 4.7,
            downloads: 11000,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "regex_tool",
            version: "1.0.0",
            description: "Regex operations: match, search, replace, split, capture groups",
            author: "argentor",
            categories: &["text"],
            tags: &["regex", "pattern", "text"],
            keywords: &["regex", "match", "replace", "split", "capture"],
            rating: 4.5,
            downloads: 8800,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "data_validator",
            version: "1.0.0",
            description: "Format validation for email, URL, UUID, IP, date, JSON, semver, and more",
            author: "argentor",
            categories: &["data"],
            tags: &["validation", "format", "data"],
            keywords: &["validate", "email", "url", "uuid", "ip", "semver"],
            rating: 4.4,
            downloads: 7600,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "datetime_tool",
            version: "1.0.0",
            description: "Date/time operations: current time, parsing, formatting, duration calculation",
            author: "argentor",
            categories: &["data"],
            tags: &["datetime", "time", "date"],
            keywords: &["now", "parse", "format", "duration", "timezone"],
            rating: 4.5,
            downloads: 9200,
        }),
        // -- Crypto & Encoding (3) --
        builtin_entry(BuiltinEntryConfig {
            name: "hash_tool",
            version: "1.0.0",
            description: "Cryptographic hashing: SHA-256, SHA-512, HMAC-SHA256, BLAKE3",
            author: "argentor",
            categories: &["crypto"],
            tags: &["hash", "crypto", "security"],
            keywords: &["sha256", "sha512", "hmac", "blake3", "digest"],
            rating: 4.7,
            downloads: 10500,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "encode_decode",
            version: "1.0.0",
            description: "Encoding and decoding: Base64, hex, URL-encoding, HTML entities, JWT decode",
            author: "argentor",
            categories: &["encoding"],
            tags: &["encoding", "base64", "hex"],
            keywords: &["base64", "hex", "url", "html", "jwt", "encode", "decode"],
            rating: 4.6,
            downloads: 9800,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "uuid_generator",
            version: "1.0.0",
            description: "UUID generation (v4, v7) and parsing with validation",
            author: "argentor",
            categories: &["crypto", "data"],
            tags: &["uuid", "generator", "id"],
            keywords: &["uuid", "v4", "v7", "unique", "identifier"],
            rating: 4.3,
            downloads: 7200,
        }),
        // -- Web & Search (4) --
        builtin_entry(BuiltinEntryConfig {
            name: "web_search",
            version: "1.0.0",
            description: "Web search using DuckDuckGo HTML endpoint, no API key required",
            author: "argentor",
            categories: &["web", "search"],
            tags: &["web", "search", "duckduckgo"],
            keywords: &["search", "web", "duckduckgo", "query", "internet"],
            rating: 4.5,
            downloads: 14000,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "web_scraper",
            version: "1.0.0",
            description: "Web scraping: extract text, links, metadata, and structured data from pages",
            author: "argentor",
            categories: &["web"],
            tags: &["web", "scraper", "html"],
            keywords: &["scrape", "html", "extract", "links", "metadata"],
            rating: 4.4,
            downloads: 8500,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "rss_reader",
            version: "1.0.0",
            description: "RSS and Atom feed reader: fetch, parse, search, and filter feed entries",
            author: "argentor",
            categories: &["web", "data"],
            tags: &["rss", "atom", "feed"],
            keywords: &["rss", "atom", "feed", "news", "subscribe"],
            rating: 4.3,
            downloads: 6500,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "dns_lookup",
            version: "1.0.0",
            description: "DNS resolution, reverse lookup, and network connectivity checks",
            author: "argentor",
            categories: &["web", "security"],
            tags: &["dns", "network", "lookup"],
            keywords: &["dns", "resolve", "reverse", "ip", "domain", "connectivity"],
            rating: 4.2,
            downloads: 5800,
        }),
        // -- Security & AI (5) --
        builtin_entry(BuiltinEntryConfig {
            name: "prompt_guard",
            version: "1.0.0",
            description: "Prompt injection detection and PII scanning for safe LLM interactions",
            author: "argentor",
            categories: &["security", "ai"],
            tags: &["security", "prompt", "injection", "pii"],
            keywords: &["prompt", "injection", "guard", "pii", "scan", "detect"],
            rating: 4.8,
            downloads: 13000,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "secret_scanner",
            version: "1.0.0",
            description: "Detect leaked credentials, API keys, tokens, and secrets in text or code",
            author: "argentor",
            categories: &["security"],
            tags: &["security", "secrets", "scanner"],
            keywords: &["secret", "credential", "api_key", "token", "leak", "scan"],
            rating: 4.7,
            downloads: 11500,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "diff_tool",
            version: "1.0.0",
            description: "Text diff generation and unified patch output for comparing files or strings",
            author: "argentor",
            categories: &["text", "data"],
            tags: &["diff", "patch", "compare"],
            keywords: &["diff", "unified", "patch", "compare", "delta"],
            rating: 4.4,
            downloads: 7800,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "summarizer",
            version: "1.0.0",
            description: "Extractive text summarization: key sentence extraction and text condensation",
            author: "argentor",
            categories: &["text", "ai"],
            tags: &["summarize", "nlp", "text"],
            keywords: &["summarize", "extract", "condense", "key_sentences", "nlp"],
            rating: 4.5,
            downloads: 9000,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "code_analysis",
            version: "1.0.0",
            description: "Language-aware code analysis: complexity metrics, dependency graphs, and linting hints",
            author: "argentor",
            categories: &["data", "ai"],
            tags: &["code", "analysis", "metrics"],
            keywords: &["code", "complexity", "lint", "ast", "dependency"],
            rating: 4.6,
            downloads: 10200,
        }),
        // -- Data & Text (new) (4) --
        builtin_entry(BuiltinEntryConfig {
            name: "csv_processor",
            version: "1.0.0",
            description: "CSV parsing, column selection, filtering, sorting, statistics, and CSV/JSON conversion",
            author: "argentor",
            categories: &["data"],
            tags: &["csv", "data", "parsing"],
            keywords: &["csv", "parse", "filter", "sort", "statistics", "json"],
            rating: 4.5,
            downloads: 8200,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "yaml_processor",
            version: "1.0.0",
            description: "YAML parse/stringify, validate, merge, and YAML/JSON conversion",
            author: "argentor",
            categories: &["data"],
            tags: &["yaml", "data", "parsing"],
            keywords: &["yaml", "parse", "validate", "merge", "json", "config"],
            rating: 4.4,
            downloads: 7800,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "markdown_renderer",
            version: "1.0.0",
            description: "Markdown processing: plain text conversion, extract headings/links/code blocks, TOC generation",
            author: "argentor",
            categories: &["text"],
            tags: &["markdown", "text", "render"],
            keywords: &["markdown", "headings", "links", "toc", "code_blocks", "plain_text"],
            rating: 4.5,
            downloads: 8500,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "template_engine",
            version: "1.0.0",
            description: "Simple {{variable}} template rendering with conditionals, loops, and defaults",
            author: "argentor",
            categories: &["text"],
            tags: &["template", "render", "text"],
            keywords: &["template", "render", "variable", "conditional", "loop", "mustache"],
            rating: 4.6,
            downloads: 9100,
        }),
        // -- Versioning & Config (3) --
        builtin_entry(BuiltinEntryConfig {
            name: "semver_tool",
            version: "1.0.0",
            description: "Semantic version parse, compare, bump (major/minor/patch), range matching",
            author: "argentor",
            categories: &["data"],
            tags: &["semver", "version", "compare"],
            keywords: &["semver", "version", "bump", "compare", "range", "sort"],
            rating: 4.4,
            downloads: 7500,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "env_manager",
            version: "1.0.0",
            description: "Environment variable operations: read, list, check, .env parsing, and variable expansion",
            author: "argentor",
            categories: &["data", "security"],
            tags: &["env", "config", "dotenv"],
            keywords: &["env", "environment", "dotenv", "variable", "config", "expand"],
            rating: 4.3,
            downloads: 7200,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "cron_parser",
            version: "1.0.0",
            description: "Parse cron expressions, calculate next occurrences, validate, and describe",
            author: "argentor",
            categories: &["data"],
            tags: &["cron", "schedule", "time"],
            keywords: &["cron", "schedule", "next", "validate", "describe", "recurring"],
            rating: 4.5,
            downloads: 8000,
        }),
        // -- Crypto & Network (new) (3) --
        builtin_entry(BuiltinEntryConfig {
            name: "jwt_tool",
            version: "1.0.0",
            description: "JWT decode (no verification), inspect claims, check expiry, extract header/payload",
            author: "argentor",
            categories: &["crypto", "security"],
            tags: &["jwt", "token", "decode"],
            keywords: &["jwt", "decode", "claims", "expiry", "header", "payload"],
            rating: 4.6,
            downloads: 9500,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "ip_tools",
            version: "1.0.0",
            description: "IP parsing, CIDR validation, subnet calculator, IP classification, reverse DNS",
            author: "argentor",
            categories: &["web", "security"],
            tags: &["ip", "network", "cidr"],
            keywords: &["ip", "cidr", "subnet", "ipv4", "ipv6", "network", "classify"],
            rating: 4.4,
            downloads: 7800,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "file_hasher",
            version: "1.0.0",
            description: "Hash file contents (SHA-256, SHA-512), checksum verification, bulk hashing",
            author: "argentor",
            categories: &["crypto", "data"],
            tags: &["hash", "file", "checksum"],
            keywords: &["hash", "file", "sha256", "sha512", "checksum", "verify", "integrity"],
            rating: 4.5,
            downloads: 8300,
        }),
        // -- Observability (2) --
        builtin_entry(BuiltinEntryConfig {
            name: "metrics_collector",
            version: "1.0.0",
            description: "In-memory counter/gauge/histogram collection, Prometheus and JSON export",
            author: "argentor",
            categories: &["data"],
            tags: &["metrics", "monitoring", "prometheus"],
            keywords: &["metrics", "counter", "gauge", "histogram", "prometheus", "monitoring"],
            rating: 4.5,
            downloads: 8700,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "color_converter",
            version: "1.0.0",
            description: "Color conversion (Hex/RGB/HSL), named colors, contrast ratio, lighten/darken",
            author: "argentor",
            categories: &["data"],
            tags: &["color", "conversion", "design"],
            keywords: &["color", "hex", "rgb", "hsl", "contrast", "lighten", "darken", "wcag"],
            rating: 4.3,
            downloads: 6800,
        }),
        // -- Document Loaders (6) --
        builtin_entry(BuiltinEntryConfig {
            name: "pdf_loader",
            version: "1.0.0",
            description: "PDF document loader: extract text, metadata, count pages, extract page range",
            author: "argentor-team",
            categories: &["document", "data"],
            tags: &["pdf", "document", "rag", "loader"],
            keywords: &["pdf", "extract", "text", "metadata", "pages", "document"],
            rating: 4.6,
            downloads: 11000,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "docx_loader",
            version: "1.0.0",
            description: "DOCX (Word) document loader: extract text, paragraphs, tables, word count",
            author: "argentor-team",
            categories: &["document", "data"],
            tags: &["docx", "word", "document", "rag"],
            keywords: &["docx", "word", "paragraphs", "tables", "extract", "text"],
            rating: 4.5,
            downloads: 9500,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "html_loader",
            version: "1.0.0",
            description: "HTML to text loader: strip tags, extract links, images, metadata (title, description)",
            author: "argentor-team",
            categories: &["document", "web"],
            tags: &["html", "loader", "rag", "document"],
            keywords: &["html", "strip", "tags", "links", "metadata", "extract"],
            rating: 4.5,
            downloads: 10500,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "epub_loader",
            version: "1.0.0",
            description: "EPUB ebook loader: extract chapters, text, metadata (title, author, language)",
            author: "argentor-team",
            categories: &["document", "data"],
            tags: &["epub", "ebook", "document", "rag"],
            keywords: &["epub", "ebook", "chapters", "text", "metadata", "opf"],
            rating: 4.4,
            downloads: 7600,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "excel_loader",
            version: "1.0.0",
            description: "XLSX spreadsheet loader: list sheets, read sheet, get cell, count rows, CSV/JSON export",
            author: "argentor-team",
            categories: &["document", "data"],
            tags: &["excel", "xlsx", "spreadsheet", "loader"],
            keywords: &["excel", "xlsx", "sheet", "cell", "csv", "json", "spreadsheet"],
            rating: 4.6,
            downloads: 10200,
        }),
        builtin_entry(BuiltinEntryConfig {
            name: "pptx_loader",
            version: "1.0.0",
            description: "PowerPoint (PPTX) loader: extract text, slides, count slides, speaker notes",
            author: "argentor-team",
            categories: &["document", "data"],
            tags: &["pptx", "powerpoint", "presentation", "rag"],
            keywords: &["pptx", "powerpoint", "slides", "text", "speaker", "notes"],
            rating: 4.4,
            downloads: 8200,
        }),
    ]
}

// ---------------------------------------------------------------------------
// SkillCache — local ~/.argentor/skills/ directory for downloaded WASM binaries
// ---------------------------------------------------------------------------

/// A locally cached skill entry downloaded from the marketplace registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkill {
    /// Skill name.
    pub name: String,
    /// Installed semantic version.
    pub version: String,
    /// Absolute path to the cached WASM binary.
    pub wasm_path: PathBuf,
    /// SHA-256 checksum of the cached binary.
    pub checksum: String,
    /// Registry URL the skill was downloaded from.
    pub registry_url: String,
    /// RFC 3339 timestamp when the skill was installed.
    pub installed_at: String,
}

/// A lightweight view of a catalog entry for search results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Skill name.
    pub name: String,
    /// Current version.
    pub version: String,
    /// Human-readable description.
    pub description: String,
    /// Rating (0.0–5.0).
    pub rating: f32,
    /// Total download count.
    pub downloads: u64,
    /// Category list.
    pub categories: Vec<String>,
    /// Keyword tags.
    pub keywords: Vec<String>,
}

impl From<&MarketplaceEntry> for CatalogEntry {
    fn from(e: &MarketplaceEntry) -> Self {
        Self {
            name: e.manifest.name.clone(),
            version: e.manifest.version.clone(),
            description: e.manifest.description.clone(),
            rating: e.rating,
            downloads: e.downloads,
            categories: e.categories.clone(),
            keywords: e.keywords.clone(),
        }
    }
}

/// Local skill cache backed by `~/.argentor/skills/`.
///
/// Stores WASM binaries on disk and tracks them in a JSON index file
/// (`~/.argentor/skills/cache-index.json`).  All operations are purely local
/// (no HTTP).  Download from a registry is handled by
/// [`MarketplaceCatalog::download_skill`].
///
/// # Default cache directory
///
/// `~/.argentor/skills/` — created automatically when first used.
///
/// # Registry URL format
///
/// `{registry_url}/skills/{name}/{version}/{name}.wasm`
pub struct SkillCache {
    /// Root directory for cached WASM binaries.
    pub cache_dir: PathBuf,
    /// Base URL of the skill registry (no trailing slash).
    pub registry_url: String,
    /// In-memory index of installed skills (mirrors the on-disk JSON).
    installed: Vec<InstalledSkill>,
}

impl SkillCache {
    /// Default registry URL used when no override is provided.
    pub const DEFAULT_REGISTRY: &'static str = "https://registry.argentor.dev";

    /// Create a new cache rooted at `~/.argentor/skills/` with the default registry.
    ///
    /// Returns an error if the home directory cannot be determined or the cache
    /// directory cannot be created.
    pub fn new() -> ArgentorResult<Self> {
        let home = home_dir().ok_or_else(|| {
            argentor_core::ArgentorError::Config(
                "Cannot determine home directory for skill cache".into(),
            )
        })?;
        Self::with_dir_and_registry(
            home.join(".argentor").join("skills"),
            Self::DEFAULT_REGISTRY,
        )
    }

    /// Create a cache at a custom directory with the default registry.
    pub fn with_dir(cache_dir: impl Into<PathBuf>) -> ArgentorResult<Self> {
        Self::with_dir_and_registry(cache_dir, Self::DEFAULT_REGISTRY)
    }

    /// Create a cache at a custom directory and a custom registry URL.
    pub fn with_dir_and_registry(
        cache_dir: impl Into<PathBuf>,
        registry_url: impl Into<String>,
    ) -> ArgentorResult<Self> {
        let cache_dir = cache_dir.into();
        std::fs::create_dir_all(&cache_dir).map_err(|e| {
            argentor_core::ArgentorError::Config(format!(
                "Failed to create skill cache dir {}: {e}",
                cache_dir.display()
            ))
        })?;
        let mut cache = Self {
            cache_dir,
            registry_url: registry_url.into().trim_end_matches('/').to_string(),
            installed: Vec::new(),
        };
        cache.load_index()?;
        Ok(cache)
    }

    /// Return the download URL for a skill.
    ///
    /// Format: `{registry_url}/skills/{name}/{version}/{name}.wasm`
    pub fn download_url(&self, name: &str, version: &str) -> String {
        format!(
            "{}/skills/{}/{}/{}.wasm",
            self.registry_url, name, version, name
        )
    }

    /// Return the local path where a skill WASM binary is (or would be) cached.
    pub fn wasm_path(&self, name: &str, version: &str) -> PathBuf {
        self.cache_dir.join(format!("{name}-{version}.wasm"))
    }

    /// Check whether a specific version of a skill is already cached.
    pub fn is_cached(&self, name: &str, version: &str) -> bool {
        self.installed
            .iter()
            .any(|s| s.name == name && s.version == version && s.wasm_path.exists())
    }

    /// Return the cache entry for a skill if it is installed.
    pub fn get(&self, name: &str) -> Option<&InstalledSkill> {
        self.installed.iter().find(|s| s.name == name)
    }

    /// List all locally cached skills.
    pub fn list(&self) -> &[InstalledSkill] {
        &self.installed
    }

    /// Register a WASM binary in the local cache.
    ///
    /// Writes `wasm_bytes` to disk and updates the index.  Replaces any
    /// previously cached version of the same skill.
    pub fn store(
        &mut self,
        name: &str,
        version: &str,
        wasm_bytes: &[u8],
    ) -> ArgentorResult<PathBuf> {
        use sha2::{Digest, Sha256};

        let path = self.wasm_path(name, version);
        std::fs::write(&path, wasm_bytes).map_err(|e| {
            argentor_core::ArgentorError::Config(format!(
                "Failed to write cached WASM for {name}-{version}: {e}"
            ))
        })?;

        let mut hasher = Sha256::new();
        hasher.update(wasm_bytes);
        let checksum = hex::encode(hasher.finalize());

        // Replace any existing entry for the same skill name
        self.installed.retain(|s| s.name != name);
        self.installed.push(InstalledSkill {
            name: name.to_string(),
            version: version.to_string(),
            wasm_path: path.clone(),
            checksum,
            registry_url: self.registry_url.clone(),
            installed_at: chrono::Utc::now().to_rfc3339(),
        });

        self.save_index()?;
        Ok(path)
    }

    /// Remove a cached skill from disk and the index.
    ///
    /// Returns `true` if an entry was found and removed.
    pub fn remove(&mut self, name: &str) -> ArgentorResult<bool> {
        if let Some(pos) = self.installed.iter().position(|s| s.name == name) {
            let entry = self.installed.remove(pos);
            if entry.wasm_path.exists() {
                std::fs::remove_file(&entry.wasm_path).map_err(|e| {
                    argentor_core::ArgentorError::Config(format!(
                        "Failed to remove cached WASM {}: {e}",
                        entry.wasm_path.display()
                    ))
                })?;
            }
            self.save_index()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // -- private helpers --

    fn index_path(&self) -> PathBuf {
        self.cache_dir.join("cache-index.json")
    }

    fn load_index(&mut self) -> ArgentorResult<()> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&path).map_err(|e| {
            argentor_core::ArgentorError::Config(format!("Failed to read cache index: {e}"))
        })?;
        self.installed = serde_json::from_str(&content).map_err(|e| {
            argentor_core::ArgentorError::Config(format!("Failed to parse cache index: {e}"))
        })?;
        Ok(())
    }

    fn save_index(&self) -> ArgentorResult<()> {
        let path = self.index_path();
        let content = serde_json::to_string_pretty(&self.installed).map_err(|e| {
            argentor_core::ArgentorError::Config(format!("Failed to serialize cache index: {e}"))
        })?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, content).map_err(|e| {
            argentor_core::ArgentorError::Config(format!("Failed to write cache index tmp: {e}"))
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            argentor_core::ArgentorError::Config(format!("Failed to rename cache index: {e}"))
        })?;
        Ok(())
    }
}

/// Return the user's home directory, cross-platform.
fn home_dir() -> Option<PathBuf> {
    // std::env::home_dir() is deprecated but still works; use HOME env var as primary.
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
}

// ---------------------------------------------------------------------------
// MarketplaceCatalog — download, install, search extensions
// ---------------------------------------------------------------------------

impl MarketplaceCatalog {
    /// Search the catalog by free-text query across name, description, and keywords.
    ///
    /// Returns lightweight [`CatalogEntry`] views sorted by relevance (download count).
    pub fn search_entries(&self, query: &str) -> Vec<CatalogEntry> {
        let q = query.to_lowercase();
        let mut results: Vec<&MarketplaceEntry> = self
            .entries
            .iter()
            .filter(|e| {
                e.manifest.name.to_lowercase().contains(&q)
                    || e.manifest.description.to_lowercase().contains(&q)
                    || e.keywords.iter().any(|k| k.to_lowercase().contains(&q))
                    || e.manifest
                        .tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&q))
            })
            .collect();
        results.sort_by_key(|entry| std::cmp::Reverse(entry.downloads));
        results.iter().map(|e| CatalogEntry::from(*e)).collect()
    }

    /// Download a WASM skill binary from the registry into the provided [`SkillCache`].
    ///
    /// Returns the local [`PathBuf`] of the cached `.wasm` file.
    ///
    /// This method is **always available** (no feature flag required) because it
    /// uses only `std::fs` and the cache directory.  The HTTP download is delegated
    /// to the caller via the `fetch_bytes` callback so the core crate stays free of
    /// network dependencies.
    ///
    /// # Parameters
    ///
    /// - `name` — skill name (must match a catalog entry)
    /// - `version` — exact semver string (e.g. `"1.0.0"`)
    /// - `cache` — mutable reference to the local [`SkillCache`]
    /// - `fetch_bytes` — async-compatible closure `async fn(url: &str) -> ArgentorResult<Vec<u8>>`
    ///
    /// # Errors
    ///
    /// Returns an error if the skill is not found in the catalog, if the download
    /// fails, or if writing to the cache directory fails.
    pub async fn download_skill<F, Fut>(
        &self,
        name: &str,
        version: &str,
        cache: &mut SkillCache,
        fetch_bytes: F,
    ) -> ArgentorResult<PathBuf>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = ArgentorResult<Vec<u8>>>,
    {
        // Fast-path: already cached
        if cache.is_cached(name, version) {
            return Ok(cache.wasm_path(name, version));
        }

        // Look up catalog entry to get the download URL
        let entry = self.get(name).ok_or_else(|| {
            argentor_core::ArgentorError::Config(format!("Skill '{name}' not found in catalog"))
        })?;

        let url = entry
            .download_url
            .clone()
            .unwrap_or_else(|| cache.download_url(name, version));

        let bytes = fetch_bytes(url).await?;
        let path = cache.store(name, version, &bytes)?;
        Ok(path)
    }

    /// Install a skill: download it (if needed) and return its [`InstalledSkill`] record.
    ///
    /// This is the high-level API that combines `download_skill` with registration in
    /// the cache index.  After calling this, `cache.get(name)` will return the entry.
    ///
    /// The `fetch_bytes` callback follows the same contract as in [`download_skill`].
    pub async fn install_skill<F, Fut>(
        &self,
        name: &str,
        version: &str,
        cache: &mut SkillCache,
        fetch_bytes: F,
    ) -> ArgentorResult<InstalledSkill>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = ArgentorResult<Vec<u8>>>,
    {
        let _ = self
            .download_skill(name, version, cache, fetch_bytes)
            .await?;
        cache.get(name).cloned().ok_or_else(|| {
            argentor_core::ArgentorError::Config(format!(
                "Skill '{name}' was downloaded but not found in cache index"
            ))
        })
    }

    /// List all skills currently present in the local cache.
    pub fn installed_skills(cache: &SkillCache) -> Vec<InstalledSkill> {
        cache.list().to_vec()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // -- Helpers --

    fn make_manifest(name: &str, version: &str) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            version: version.to_string(),
            description: format!("Test skill {name}"),
            author: "tester".to_string(),
            license: Some("MIT".to_string()),
            checksum: "abc123".to_string(),
            capabilities: Vec::new(),
            signature: None,
            signer_key: None,
            min_argentor_version: None,
            tags: vec!["test".to_string()],
            repository: None,
        }
    }

    fn make_entry(name: &str, version: &str) -> MarketplaceEntry {
        MarketplaceEntry {
            manifest: make_manifest(name, version),
            downloads: 100,
            rating: 4.0,
            ratings_count: 10,
            categories: vec!["data".to_string()],
            featured: false,
            published_at: "2025-01-01T00:00:00Z".to_string(),
            updated_at: "2025-06-01T00:00:00Z".to_string(),
            size_bytes: 1024,
            download_url: None,
            homepage: None,
            keywords: vec!["test".to_string()],
            dependencies: Vec::new(),
        }
    }

    // Test helper with many params — acceptable for test ergonomics.
    #[allow(clippy::too_many_arguments)]
    fn make_entry_full(
        name: &str,
        version: &str,
        author: &str,
        categories: Vec<&str>,
        tags: Vec<&str>,
        keywords: Vec<&str>,
        downloads: u64,
        rating: f32,
        featured: bool,
        updated_at: &str,
    ) -> MarketplaceEntry {
        let mut entry = make_entry(name, version);
        entry.manifest.author = author.to_string();
        entry.manifest.tags = tags.into_iter().map(String::from).collect();
        entry.categories = categories.into_iter().map(String::from).collect();
        entry.keywords = keywords.into_iter().map(String::from).collect();
        entry.downloads = downloads;
        entry.rating = rating;
        entry.featured = featured;
        entry.updated_at = updated_at.to_string();
        entry
    }

    fn minimal_wasm() -> Vec<u8> {
        vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]
    }

    // -- Catalog CRUD --

    #[test]
    fn catalog_add_and_get() {
        let mut catalog = MarketplaceCatalog::new();
        assert_eq!(catalog.count(), 0);

        catalog.add(make_entry("alpha", "1.0.0"));
        assert_eq!(catalog.count(), 1);

        let entry = catalog.get("alpha").unwrap();
        assert_eq!(entry.manifest.version, "1.0.0");
    }

    #[test]
    fn catalog_add_replaces_existing() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry("alpha", "1.0.0"));
        catalog.add(make_entry("alpha", "2.0.0"));
        assert_eq!(catalog.count(), 1);
        assert_eq!(catalog.get("alpha").unwrap().manifest.version, "2.0.0");
    }

    #[test]
    fn catalog_remove_existing() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry("alpha", "1.0.0"));
        assert!(catalog.remove("alpha"));
        assert_eq!(catalog.count(), 0);
        assert!(catalog.get("alpha").is_none());
    }

    #[test]
    fn catalog_remove_nonexistent() {
        let mut catalog = MarketplaceCatalog::new();
        assert!(!catalog.remove("ghost"));
    }

    #[test]
    fn catalog_get_nonexistent() {
        let catalog = MarketplaceCatalog::new();
        assert!(catalog.get("nope").is_none());
    }

    // -- Search: query text --

    #[test]
    fn search_by_query_name() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry("calculator", "1.0.0"));
        catalog.add(make_entry("text_transform", "1.0.0"));

        let search = MarketplaceSearch {
            query: Some("calc".into()),
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].manifest.name, "calculator");
    }

    #[test]
    fn search_by_query_description() {
        let mut catalog = MarketplaceCatalog::new();
        let mut entry = make_entry("foo", "1.0.0");
        entry.manifest.description = "A powerful data cruncher".to_string();
        catalog.add(entry);

        let search = MarketplaceSearch {
            query: Some("cruncher".into()),
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_by_query_keyword() {
        let mut catalog = MarketplaceCatalog::new();
        let mut entry = make_entry("bar", "1.0.0");
        entry.keywords = vec!["awesome".to_string()];
        catalog.add(entry);

        let search = MarketplaceSearch {
            query: Some("awesome".into()),
            ..Default::default()
        };
        assert_eq!(catalog.search(&search).len(), 1);
    }

    #[test]
    fn search_case_insensitive() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry("Calculator", "1.0.0"));

        let search = MarketplaceSearch {
            query: Some("CALCULATOR".into()),
            ..Default::default()
        };
        assert_eq!(catalog.search(&search).len(), 1);
    }

    // -- Search: category --

    #[test]
    fn search_by_category() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "a",
            "1.0.0",
            "x",
            vec!["web"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "b",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));

        let search = MarketplaceSearch {
            category: Some("web".into()),
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].manifest.name, "a");
    }

    // -- Search: author --

    #[test]
    fn search_by_author() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "s1",
            "1.0.0",
            "alice",
            vec!["data"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "s2",
            "1.0.0",
            "bob",
            vec!["data"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));

        let search = MarketplaceSearch {
            author: Some("alice".into()),
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].manifest.name, "s1");
    }

    // -- Search: min_rating --

    #[test]
    fn search_by_min_rating() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "high",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            4.8,
            false,
            "2025-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "low",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            2.1,
            false,
            "2025-01-01T00:00:00Z",
        ));

        let search = MarketplaceSearch {
            min_rating: Some(4.0),
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].manifest.name, "high");
    }

    // -- Search: tags --

    #[test]
    fn search_by_tags_all_must_match() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "s1",
            "1.0.0",
            "x",
            vec!["data"],
            vec!["rust", "wasm"],
            vec![],
            0,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "s2",
            "1.0.0",
            "x",
            vec!["data"],
            vec!["rust"],
            vec![],
            0,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));

        let search = MarketplaceSearch {
            tags: vec!["rust".into(), "wasm".into()],
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].manifest.name, "s1");
    }

    // -- Sort by all variants --

    #[test]
    fn sort_by_downloads() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "a",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            500,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "b",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            1000,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));

        let search = MarketplaceSearch {
            sort_by: SortBy::Downloads,
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results[0].manifest.name, "b");
        assert_eq!(results[1].manifest.name, "a");
    }

    #[test]
    fn sort_by_rating() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "a",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "b",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            5.0,
            false,
            "2025-01-01T00:00:00Z",
        ));

        let search = MarketplaceSearch {
            sort_by: SortBy::Rating,
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results[0].manifest.name, "b");
    }

    #[test]
    fn sort_by_recent() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "old",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2024-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "new",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2025-06-01T00:00:00Z",
        ));

        let search = MarketplaceSearch {
            sort_by: SortBy::Recent,
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results[0].manifest.name, "new");
    }

    #[test]
    fn sort_by_name() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry("zebra", "1.0.0"));
        catalog.add(make_entry("alpha", "1.0.0"));

        let search = MarketplaceSearch {
            sort_by: SortBy::Name,
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results[0].manifest.name, "alpha");
        assert_eq!(results[1].manifest.name, "zebra");
    }

    #[test]
    fn sort_by_relevance_preserves_order() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry("first", "1.0.0"));
        catalog.add(make_entry("second", "1.0.0"));

        let search = MarketplaceSearch {
            sort_by: SortBy::Relevance,
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results[0].manifest.name, "first");
        assert_eq!(results[1].manifest.name, "second");
    }

    // -- Pagination --

    #[test]
    fn pagination_limit() {
        let mut catalog = MarketplaceCatalog::new();
        for i in 0..10 {
            catalog.add(make_entry(&format!("skill_{i}"), "1.0.0"));
        }

        let search = MarketplaceSearch {
            limit: 3,
            ..Default::default()
        };
        assert_eq!(catalog.search(&search).len(), 3);
    }

    #[test]
    fn pagination_offset() {
        let mut catalog = MarketplaceCatalog::new();
        for i in 0..5 {
            catalog.add(make_entry(&format!("s{i}"), "1.0.0"));
        }

        let search = MarketplaceSearch {
            sort_by: SortBy::Name,
            offset: 2,
            limit: 50,
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].manifest.name, "s2");
    }

    #[test]
    fn pagination_offset_and_limit() {
        let mut catalog = MarketplaceCatalog::new();
        for i in 0..10 {
            catalog.add(make_entry(&format!("s{i:02}"), "1.0.0"));
        }

        let search = MarketplaceSearch {
            sort_by: SortBy::Name,
            offset: 3,
            limit: 2,
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].manifest.name, "s03");
        assert_eq!(results[1].manifest.name, "s04");
    }

    // -- Featured, popular, top-rated, recent --

    #[test]
    fn list_featured() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "feat",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            5.0,
            true,
            "2025-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "nope",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));

        let featured = catalog.list_featured();
        assert_eq!(featured.len(), 1);
        assert_eq!(featured[0].manifest.name, "feat");
    }

    #[test]
    fn list_popular() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "low",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            10,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "high",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            9999,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));

        let popular = catalog.list_popular(1);
        assert_eq!(popular.len(), 1);
        assert_eq!(popular[0].manifest.name, "high");
    }

    #[test]
    fn list_top_rated() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "mid",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            3.5,
            false,
            "2025-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "best",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            4.9,
            false,
            "2025-01-01T00:00:00Z",
        ));

        let top = catalog.list_top_rated(1);
        assert_eq!(top[0].manifest.name, "best");
    }

    #[test]
    fn list_recent() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "old",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2020-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "fresh",
            "1.0.0",
            "x",
            vec!["data"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2025-12-01T00:00:00Z",
        ));

        let recent = catalog.list_recent(1);
        assert_eq!(recent[0].manifest.name, "fresh");
    }

    // -- Categories --

    #[test]
    fn categories_returns_unique_sorted() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "a",
            "1.0.0",
            "x",
            vec!["web", "data"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "b",
            "1.0.0",
            "x",
            vec!["data", "crypto"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));

        let cats = catalog.categories();
        assert_eq!(cats, vec!["crypto", "data", "web"]);
    }

    #[test]
    fn list_by_category() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry_full(
            "a",
            "1.0.0",
            "x",
            vec!["security"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));
        catalog.add(make_entry_full(
            "b",
            "1.0.0",
            "x",
            vec!["web"],
            vec![],
            vec![],
            0,
            3.0,
            false,
            "2025-01-01T00:00:00Z",
        ));

        let sec = catalog.list_by_category("security");
        assert_eq!(sec.len(), 1);
        assert_eq!(sec[0].manifest.name, "a");
    }

    // -- Manager: install / uninstall lifecycle --

    #[test]
    fn manager_install_and_uninstall() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let catalog_path = dir.path().join("catalog.json");

        let mut mgr = MarketplaceManager::new(skills_dir, catalog_path);

        let wasm = minimal_wasm();
        let manifest = SkillManifest {
            name: "test_skill".to_string(),
            version: "1.0.0".to_string(),
            description: "A test".to_string(),
            author: "tester".to_string(),
            license: Some("MIT".to_string()),
            checksum: SkillManifest::compute_checksum(&wasm),
            capabilities: Vec::new(),
            signature: None,
            signer_key: None,
            min_argentor_version: None,
            tags: Vec::new(),
            repository: None,
        };

        let result = mgr.install_from_bytes(manifest, &wasm).unwrap();
        assert!(result.passed);
        assert!(mgr.is_installed("test_skill"));
        assert_eq!(mgr.installed_version("test_skill"), Some("1.0.0"));

        assert!(mgr.uninstall("test_skill").unwrap());
        assert!(!mgr.is_installed("test_skill"));
    }

    #[test]
    fn manager_uninstall_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            MarketplaceManager::new(dir.path().join("skills"), dir.path().join("catalog.json"));
        assert!(!mgr.uninstall("ghost").unwrap());
    }

    // -- Dependency resolution --

    #[test]
    fn dependency_resolution_linear_chain() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            MarketplaceManager::new(dir.path().join("skills"), dir.path().join("catalog.json"));

        // Chain: c depends on b, b depends on a
        let mut a = make_entry("a", "1.0.0");
        a.dependencies = Vec::new();

        let mut b = make_entry("b", "1.0.0");
        b.dependencies = vec![SkillDependency {
            name: "a".to_string(),
            version_req: ">=1.0.0".to_string(),
        }];

        let mut c = make_entry("c", "1.0.0");
        c.dependencies = vec![SkillDependency {
            name: "b".to_string(),
            version_req: ">=1.0.0".to_string(),
        }];

        mgr.update_catalog(vec![a, b, c.clone()]);

        let order = mgr.resolve_dependencies(&c).unwrap();
        // a must come before b
        let pos_a = order.iter().position(|n| n == "a").unwrap();
        let pos_b = order.iter().position(|n| n == "b").unwrap();
        assert!(pos_a < pos_b);
        assert!(!order.contains(&"c".to_string()));
    }

    #[test]
    fn dependency_resolution_diamond() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            MarketplaceManager::new(dir.path().join("skills"), dir.path().join("catalog.json"));

        // Diamond: d depends on b and c; both b and c depend on a
        let a = make_entry("a", "1.0.0");

        let mut b = make_entry("b", "1.0.0");
        b.dependencies = vec![SkillDependency {
            name: "a".to_string(),
            version_req: ">=1.0.0".to_string(),
        }];

        let mut c = make_entry("c", "1.0.0");
        c.dependencies = vec![SkillDependency {
            name: "a".to_string(),
            version_req: ">=1.0.0".to_string(),
        }];

        let mut d = make_entry("d", "1.0.0");
        d.dependencies = vec![
            SkillDependency {
                name: "b".to_string(),
                version_req: ">=1.0.0".to_string(),
            },
            SkillDependency {
                name: "c".to_string(),
                version_req: ">=1.0.0".to_string(),
            },
        ];

        mgr.update_catalog(vec![a, b, c, d.clone()]);

        let order = mgr.resolve_dependencies(&d).unwrap();
        assert_eq!(order.len(), 3); // a, b, c (not d itself)
        let pos_a = order.iter().position(|n| n == "a").unwrap();
        let pos_b = order.iter().position(|n| n == "b").unwrap();
        let pos_c = order.iter().position(|n| n == "c").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_a < pos_c);
    }

    #[test]
    fn dependency_resolution_no_deps() {
        let dir = tempfile::tempdir().unwrap();
        let mgr =
            MarketplaceManager::new(dir.path().join("skills"), dir.path().join("catalog.json"));

        let entry = make_entry("standalone", "1.0.0");
        let order = mgr.resolve_dependencies(&entry).unwrap();
        assert!(order.is_empty());
    }

    // -- Compatibility check --

    #[test]
    fn compatibility_check_all_deps_present() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            MarketplaceManager::new(dir.path().join("skills"), dir.path().join("catalog.json"));

        let dep = make_entry("dep_a", "1.0.0");
        mgr.update_catalog(vec![dep]);

        let mut entry = make_entry("main", "1.0.0");
        entry.dependencies = vec![SkillDependency {
            name: "dep_a".to_string(),
            version_req: ">=1.0.0".to_string(),
        }];

        let result = mgr.check_compatibility(&entry);
        assert!(result.compatible);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn compatibility_check_missing_dep() {
        let dir = tempfile::tempdir().unwrap();
        let mgr =
            MarketplaceManager::new(dir.path().join("skills"), dir.path().join("catalog.json"));

        let mut entry = make_entry("needs_missing", "1.0.0");
        entry.dependencies = vec![SkillDependency {
            name: "nonexistent".to_string(),
            version_req: ">=1.0.0".to_string(),
        }];

        let result = mgr.check_compatibility(&entry);
        assert!(!result.compatible);
        assert_eq!(result.issues.len(), 1);
        assert!(result.issues[0].contains("nonexistent"));
    }

    // -- Upgradable detection --

    #[test]
    fn list_upgradable_detects_newer_version() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let catalog_path = dir.path().join("catalog.json");

        let mut mgr = MarketplaceManager::new(skills_dir, catalog_path);

        // Install v1
        let wasm = minimal_wasm();
        let manifest = SkillManifest {
            name: "upgradable".to_string(),
            version: "1.0.0".to_string(),
            description: "test".to_string(),
            author: "tester".to_string(),
            license: None,
            checksum: SkillManifest::compute_checksum(&wasm),
            capabilities: Vec::new(),
            signature: None,
            signer_key: None,
            min_argentor_version: None,
            tags: Vec::new(),
            repository: None,
        };
        mgr.install_from_bytes(manifest, &wasm).unwrap();

        // Catalog has v2
        let catalog_entry = make_entry("upgradable", "2.0.0");
        mgr.update_catalog(vec![catalog_entry]);

        let ups = mgr.list_upgradable();
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].name, "upgradable");
        assert_eq!(ups[0].installed_version, "1.0.0");
        assert_eq!(ups[0].available_version, "2.0.0");
    }

    #[test]
    fn list_upgradable_empty_when_current() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let catalog_path = dir.path().join("catalog.json");

        let mut mgr = MarketplaceManager::new(skills_dir, catalog_path);

        let wasm = minimal_wasm();
        let manifest = SkillManifest {
            name: "current".to_string(),
            version: "1.0.0".to_string(),
            description: "test".to_string(),
            author: "tester".to_string(),
            license: None,
            checksum: SkillManifest::compute_checksum(&wasm),
            capabilities: Vec::new(),
            signature: None,
            signer_key: None,
            min_argentor_version: None,
            tags: Vec::new(),
            repository: None,
        };
        mgr.install_from_bytes(manifest, &wasm).unwrap();

        let catalog_entry = make_entry("current", "1.0.0"); // same version
        mgr.update_catalog(vec![catalog_entry]);

        assert!(mgr.list_upgradable().is_empty());
    }

    // -- Persistence save/load roundtrip --

    #[test]
    fn catalog_persistence_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");

        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry("alpha", "1.0.0"));
        catalog.add(make_entry("beta", "2.0.0"));
        catalog.save(&path).unwrap();

        let loaded = MarketplaceCatalog::load(&path).unwrap();
        assert_eq!(loaded.count(), 2);
        assert_eq!(loaded.get("alpha").unwrap().manifest.version, "1.0.0");
        assert_eq!(loaded.get("beta").unwrap().manifest.version, "2.0.0");
    }

    #[test]
    fn catalog_load_nonexistent_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does_not_exist.json");
        let catalog = MarketplaceCatalog::load(&path).unwrap();
        assert_eq!(catalog.count(), 0);
    }

    #[test]
    fn manager_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let catalog_path = dir.path().join("catalog.json");

        let mut mgr = MarketplaceManager::new(skills_dir.clone(), catalog_path.clone());

        // Install a skill
        let wasm = minimal_wasm();
        let manifest = SkillManifest {
            name: "persisted".to_string(),
            version: "1.0.0".to_string(),
            description: "test".to_string(),
            author: "tester".to_string(),
            license: None,
            checksum: SkillManifest::compute_checksum(&wasm),
            capabilities: Vec::new(),
            signature: None,
            signer_key: None,
            min_argentor_version: None,
            tags: Vec::new(),
            repository: None,
        };
        mgr.install_from_bytes(manifest, &wasm).unwrap();

        // Add a catalog entry
        mgr.update_catalog(vec![make_entry("catalog_item", "3.0.0")]);
        mgr.save().unwrap();

        // Reload
        let loaded = MarketplaceManager::load(skills_dir, catalog_path).unwrap();
        assert!(loaded.is_installed("persisted"));
        assert!(loaded.catalog().get("catalog_item").is_some());
    }

    // -- Builtin catalog entries --

    #[test]
    fn builtin_entries_count() {
        let entries = builtin_catalog_entries();
        assert_eq!(entries.len(), 36);
    }

    #[test]
    fn builtin_entries_unique_names() {
        let entries = builtin_catalog_entries();
        let names: HashSet<&str> = entries.iter().map(|e| e.manifest.name.as_str()).collect();
        assert_eq!(names.len(), 36);
    }

    #[test]
    fn builtin_entries_valid_ratings() {
        for entry in builtin_catalog_entries() {
            assert!(
                (0.0..=5.0).contains(&entry.rating),
                "Rating for {} out of range: {}",
                entry.manifest.name,
                entry.rating
            );
        }
    }

    #[test]
    fn builtin_entries_have_categories() {
        for entry in builtin_catalog_entries() {
            assert!(
                !entry.categories.is_empty(),
                "Skill {} has no categories",
                entry.manifest.name
            );
        }
    }

    #[test]
    fn builtin_entries_have_keywords() {
        for entry in builtin_catalog_entries() {
            assert!(
                !entry.keywords.is_empty(),
                "Skill {} has no keywords",
                entry.manifest.name
            );
        }
    }

    #[test]
    fn builtin_entries_featured_correct() {
        // Featured flag is set when rating >= 4.5
        for entry in builtin_catalog_entries() {
            if entry.rating >= 4.5 {
                assert!(
                    entry.featured,
                    "Skill {} has rating {} but is not featured",
                    entry.manifest.name, entry.rating
                );
            }
        }
    }

    #[test]
    fn builtin_entries_searchable_in_catalog() {
        let mut catalog = MarketplaceCatalog::new();
        for entry in builtin_catalog_entries() {
            catalog.add(entry);
        }
        assert_eq!(catalog.count(), 36);

        // Search for "hash"
        let search = MarketplaceSearch {
            query: Some("hash".into()),
            ..Default::default()
        };
        let results = catalog.search(&search);
        assert!(!results.is_empty());
        assert!(results.iter().any(|e| e.manifest.name == "hash_tool"));
    }

    // -- Empty catalog edge cases --

    #[test]
    fn empty_catalog_search_returns_empty() {
        let catalog = MarketplaceCatalog::new();
        let search = MarketplaceSearch::default();
        assert!(catalog.search(&search).is_empty());
    }

    #[test]
    fn empty_catalog_listings() {
        let catalog = MarketplaceCatalog::new();
        assert!(catalog.list_featured().is_empty());
        assert!(catalog.list_popular(10).is_empty());
        assert!(catalog.list_top_rated(10).is_empty());
        assert!(catalog.list_recent(10).is_empty());
        assert!(catalog.list_by_category("any").is_empty());
        assert!(catalog.categories().is_empty());
    }

    #[test]
    fn empty_catalog_count() {
        let catalog = MarketplaceCatalog::new();
        assert_eq!(catalog.count(), 0);
    }

    // -- MarketplaceClient: construction --

    #[test]
    fn client_base_url_strips_trailing_slash() {
        let client = MarketplaceClient::new("https://example.com/");
        assert_eq!(client.base_url(), "https://example.com");
    }

    #[test]
    fn client_default_registry_url() {
        let client = MarketplaceClient::default_registry();
        assert_eq!(client.base_url(), "https://registry.argentor.dev");
    }

    // -- MarketplaceClient: no-feature stub tests --

    #[cfg(not(feature = "registry"))]
    #[tokio::test]
    async fn client_search_returns_error_without_feature() {
        let client = MarketplaceClient::default_registry();
        let search = MarketplaceSearch::default();
        let err = client.search(&search).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("registry"),
            "error should mention feature: {msg}"
        );
    }

    #[cfg(not(feature = "registry"))]
    #[tokio::test]
    async fn client_get_returns_error_without_feature() {
        let client = MarketplaceClient::new("https://example.com");
        assert!(client.get("anything").await.is_err());
    }

    #[cfg(not(feature = "registry"))]
    #[tokio::test]
    async fn client_download_returns_error_without_feature() {
        let client = MarketplaceClient::default_registry();
        assert!(client.download("skill", None).await.is_err());
    }

    #[cfg(not(feature = "registry"))]
    #[tokio::test]
    async fn client_publish_returns_error_without_feature() {
        let client = MarketplaceClient::default_registry();
        let manifest = make_manifest("test", "1.0.0");
        assert!(client.publish(&manifest, &[], "key").await.is_err());
    }

    #[cfg(not(feature = "registry"))]
    #[tokio::test]
    async fn client_rate_returns_error_without_feature() {
        let client = MarketplaceClient::default_registry();
        assert!(client.rate("skill", 5.0, "key").await.is_err());
    }

    // -- MarketplaceClient: response parsing tests (registry feature) --

    #[cfg(feature = "registry")]
    #[test]
    fn parse_search_response_json() {
        let json = r#"{
            "results": [
                {
                    "manifest": {
                        "name": "calc",
                        "version": "1.0.0",
                        "description": "Calculator",
                        "author": "test",
                        "license": "MIT",
                        "checksum": "abc",
                        "capabilities": [],
                        "signature": null,
                        "signer_key": null,
                        "min_argentor_version": null,
                        "tags": ["math"],
                        "repository": null
                    },
                    "downloads": 100,
                    "rating": 4.5,
                    "ratings_count": 10,
                    "categories": ["data"],
                    "featured": true,
                    "published_at": "2025-01-01T00:00:00Z",
                    "updated_at": "2025-06-01T00:00:00Z",
                    "size_bytes": 2048,
                    "download_url": "https://example.com/calc.wasm",
                    "homepage": null,
                    "keywords": ["calculator"],
                    "dependencies": []
                }
            ],
            "total": 1
        }"#;
        let resp: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.total, 1);
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].manifest.name, "calc");
        assert_eq!(resp.results[0].rating, 4.5);
    }

    #[cfg(feature = "registry")]
    #[test]
    fn parse_skill_entry_json() {
        let json = r#"{
            "manifest": {
                "name": "web_scraper",
                "version": "2.1.0",
                "description": "Scrapes websites",
                "author": "alice",
                "license": "AGPL-3.0-only",
                "checksum": "deadbeef",
                "capabilities": ["net:http"],
                "signature": null,
                "signer_key": null,
                "min_argentor_version": "0.2.0",
                "tags": ["web", "scraper"],
                "repository": "https://github.com/example/scraper"
            },
            "downloads": 5000,
            "rating": 4.2,
            "ratings_count": 50,
            "categories": ["web"],
            "featured": false,
            "published_at": "2025-03-15T12:00:00Z",
            "updated_at": "2025-09-01T08:30:00Z",
            "size_bytes": 65536,
            "download_url": null,
            "homepage": "https://example.com/scraper",
            "keywords": ["html", "extract"],
            "dependencies": [
                {"name": "http_client", "version_req": ">=1.0.0"}
            ]
        }"#;
        let entry: MarketplaceEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.manifest.name, "web_scraper");
        assert_eq!(entry.manifest.version, "2.1.0");
        assert_eq!(entry.downloads, 5000);
        assert_eq!(entry.dependencies.len(), 1);
        assert_eq!(entry.dependencies[0].name, "http_client");
    }

    #[cfg(feature = "registry")]
    #[test]
    fn parse_search_response_empty_results() {
        let json = r#"{"results": [], "total": 0}"#;
        let resp: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.total, 0);
        assert!(resp.results.is_empty());
    }

    #[cfg(feature = "registry")]
    #[test]
    fn handle_error_status_success() {
        // 200 OK should return Ok(())
        let resp = http::Response::builder().status(200).body("").unwrap();
        let reqwest_resp = reqwest::Response::from(resp);
        assert!(handle_error_status(&reqwest_resp, "test").is_ok());
    }

    #[cfg(feature = "registry")]
    #[test]
    fn handle_error_status_not_found() {
        let resp = http::Response::builder().status(404).body("").unwrap();
        let reqwest_resp = reqwest::Response::from(resp);
        let err = handle_error_status(&reqwest_resp, "get").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("404"), "should mention 404: {msg}");
        assert!(msg.contains("not found"), "should mention not found: {msg}");
    }

    #[cfg(feature = "registry")]
    #[test]
    fn handle_error_status_unauthorized() {
        let resp = http::Response::builder().status(401).body("").unwrap();
        let reqwest_resp = reqwest::Response::from(resp);
        let err = handle_error_status(&reqwest_resp, "publish").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("401"), "should mention 401: {msg}");
        assert!(
            msg.contains("authentication"),
            "should mention authentication: {msg}"
        );
    }

    #[cfg(feature = "registry")]
    #[test]
    fn handle_error_status_server_error() {
        let resp = http::Response::builder().status(500).body("").unwrap();
        let reqwest_resp = reqwest::Response::from(resp);
        let err = handle_error_status(&reqwest_resp, "search").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("500"), "should mention 500: {msg}");
    }

    #[cfg(feature = "registry")]
    #[test]
    fn handle_error_status_rate_limited() {
        let resp = http::Response::builder().status(429).body("").unwrap();
        let reqwest_resp = reqwest::Response::from(resp);
        let err = handle_error_status(&reqwest_resp, "search").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("429"), "should mention 429: {msg}");
        assert!(
            msg.contains("rate limited"),
            "should mention rate limited: {msg}"
        );
    }

    #[cfg(feature = "registry")]
    #[test]
    fn rate_validates_range() {
        // This doesn't need a server — validation happens client-side
        let client = MarketplaceClient::new("https://example.com");

        // Spawn a runtime to call the async method
        let rt = tokio::runtime::Runtime::new().unwrap();

        let err = rt.block_on(client.rate("skill", -1.0, "key")).unwrap_err();
        assert!(err.to_string().contains("between 0.0 and 5.0"));

        let err = rt.block_on(client.rate("skill", 5.1, "key")).unwrap_err();
        assert!(err.to_string().contains("between 0.0 and 5.0"));
    }

    // -- URL encoding helper --

    #[test]
    fn urlencoded_simple() {
        assert_eq!(urlencoded("hello"), "hello");
    }

    #[test]
    fn urlencoded_spaces_and_special() {
        let encoded = urlencoded("hello world!");
        assert_eq!(encoded, "hello%20world%21");
    }

    #[test]
    fn urlencoded_preserves_safe_chars() {
        assert_eq!(urlencoded("a-b_c.d~e"), "a-b_c.d~e");
    }

    // ---------------------------------------------------------------------------
    // SkillCache tests
    // ---------------------------------------------------------------------------

    #[test]
    fn skill_cache_store_and_retrieve() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = SkillCache::with_dir(tmp.path()).unwrap();

        let wasm = b"\x00asm\x01\x00\x00\x00hello";
        let path = cache.store("my_skill", "1.0.0", wasm).unwrap();
        assert!(path.exists(), "WASM file should exist after store");
        assert_eq!(cache.list().len(), 1);

        let entry = cache.get("my_skill").unwrap();
        assert_eq!(entry.name, "my_skill");
        assert_eq!(entry.version, "1.0.0");
        assert!(!entry.checksum.is_empty());
    }

    #[test]
    fn skill_cache_is_cached() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = SkillCache::with_dir(tmp.path()).unwrap();

        assert!(!cache.is_cached("echo", "1.0.0"));
        cache.store("echo", "1.0.0", b"\x00asm").unwrap();
        assert!(cache.is_cached("echo", "1.0.0"));
    }

    #[test]
    fn skill_cache_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = SkillCache::with_dir(tmp.path()).unwrap();

        cache.store("calc", "2.0.0", b"\x00asm").unwrap();
        assert!(cache.is_cached("calc", "2.0.0"));

        let removed = cache.remove("calc").unwrap();
        assert!(removed, "remove should return true for existing skill");
        assert!(!cache.is_cached("calc", "2.0.0"));
        assert!(cache.list().is_empty());

        // Removing again should return false, not error
        let removed_again = cache.remove("calc").unwrap();
        assert!(!removed_again);
    }

    #[test]
    fn skill_cache_index_persists_across_reload() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let mut cache = SkillCache::with_dir(tmp.path()).unwrap();
            cache.store("hello", "1.2.3", b"\x00asmhello").unwrap();
        }
        // Reload from disk
        let cache2 = SkillCache::with_dir(tmp.path()).unwrap();
        assert_eq!(cache2.list().len(), 1);
        let e = cache2.get("hello").unwrap();
        assert_eq!(e.version, "1.2.3");
    }

    #[test]
    fn skill_cache_replaces_existing_version() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = SkillCache::with_dir(tmp.path()).unwrap();

        cache.store("sk", "1.0.0", b"v1").unwrap();
        cache.store("sk", "2.0.0", b"v2").unwrap();
        // Only the latest should remain in the index
        assert_eq!(cache.list().len(), 1);
        assert_eq!(cache.get("sk").unwrap().version, "2.0.0");
    }

    #[test]
    fn skill_cache_download_url_format() {
        let tmp = tempfile::tempdir().unwrap();
        let cache =
            SkillCache::with_dir_and_registry(tmp.path(), "https://registry.example.com").unwrap();
        let url = cache.download_url("echo", "1.0.0");
        assert_eq!(
            url,
            "https://registry.example.com/skills/echo/1.0.0/echo.wasm"
        );
    }

    #[tokio::test]
    async fn catalog_download_skill_uses_fetch_callback() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = SkillCache::with_dir(tmp.path()).unwrap();

        let mut catalog = MarketplaceCatalog::new();
        let entry = make_entry("echo", "1.0.0");
        catalog.add(entry);

        let fake_wasm = b"\x00asmFAKE".to_vec();
        let result = catalog
            .download_skill("echo", "1.0.0", &mut cache, |_url| async {
                Ok(b"\x00asmFAKE".to_vec())
            })
            .await
            .unwrap();

        assert!(result.exists());
        let contents = std::fs::read(&result).unwrap();
        assert_eq!(contents, fake_wasm);
    }

    #[tokio::test]
    async fn catalog_download_skill_fast_path_if_cached() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = SkillCache::with_dir(tmp.path()).unwrap();
        cache.store("echo", "1.0.0", b"\x00asmCACHED").unwrap();

        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry("echo", "1.0.0"));

        // fetch_bytes should NOT be called because it's already cached
        let result = catalog
            .download_skill("echo", "1.0.0", &mut cache, |_url| async {
                // The real assertion is that the returned path contains the originally
                // cached bytes — i.e., this fetch_bytes closure was NOT invoked.
                Ok(b"\x00asmNEW".to_vec())
            })
            .await
            .unwrap();

        let contents = std::fs::read(&result).unwrap();
        // Should still be the originally cached bytes
        assert_eq!(contents, b"\x00asmCACHED");
    }

    #[tokio::test]
    async fn catalog_download_skill_error_on_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = SkillCache::with_dir(tmp.path()).unwrap();
        let catalog = MarketplaceCatalog::new(); // empty catalog

        let err = catalog
            .download_skill("nonexistent", "1.0.0", &mut cache, |_url| async {
                Ok(vec![])
            })
            .await
            .unwrap_err();

        assert!(err.to_string().contains("nonexistent"));
    }

    #[tokio::test]
    async fn catalog_install_skill_returns_installed_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = SkillCache::with_dir(tmp.path()).unwrap();

        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry("calc", "1.0.0"));

        let installed = catalog
            .install_skill("calc", "1.0.0", &mut cache, |_url| async {
                Ok(b"\x00asmCALC".to_vec())
            })
            .await
            .unwrap();

        assert_eq!(installed.name, "calc");
        assert_eq!(installed.version, "1.0.0");
        assert!(installed.wasm_path.exists());
    }

    #[test]
    fn catalog_installed_skills_delegates_to_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = SkillCache::with_dir(tmp.path()).unwrap();
        cache.store("a", "1.0.0", b"a").unwrap();
        cache.store("b", "2.0.0", b"b").unwrap();

        let installed = MarketplaceCatalog::installed_skills(&cache);
        assert_eq!(installed.len(), 2);
        assert!(installed.iter().any(|s| s.name == "a"));
        assert!(installed.iter().any(|s| s.name == "b"));
    }

    #[test]
    fn catalog_search_entries_by_name() {
        let mut catalog = MarketplaceCatalog::new();
        catalog.add(make_entry("calculator", "1.0.0"));
        catalog.add(make_entry("web_search", "1.0.0"));
        catalog.add(make_entry("calc_plus", "1.0.0"));

        let results = catalog.search_entries("calc");
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|e| e.name == "calculator"));
        assert!(results.iter().any(|e| e.name == "calc_plus"));
    }

    #[test]
    fn catalog_search_entries_by_keyword() {
        let mut catalog = MarketplaceCatalog::new();
        let mut entry = make_entry("my_tool", "1.0.0");
        entry.keywords = vec!["arithmetic".to_string(), "math".to_string()];
        catalog.add(entry);
        catalog.add(make_entry("other", "1.0.0"));

        let results = catalog.search_entries("arithmetic");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "my_tool");
    }

    #[test]
    fn catalog_entry_from_marketplace_entry() {
        let entry = make_entry("echo", "1.5.0");
        let cat: CatalogEntry = CatalogEntry::from(&entry);
        assert_eq!(cat.name, "echo");
        assert_eq!(cat.version, "1.5.0");
        assert_eq!(cat.rating, 4.0);
    }
}
