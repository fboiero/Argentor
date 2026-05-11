//! Configuration and paths for the EPEC Legal Agent.

/// Default BM25 index directory (relative to CWD when running the binary).
pub const DEFAULT_INDEX_DIR: &str = "./epec_index";

/// Default number of top chunks to retrieve.
pub const DEFAULT_TOP_K: usize = 5;

/// Chunk size in characters.
pub const CHUNK_SIZE: usize = 800;

/// Character overlap between consecutive chunks.
pub const CHUNK_OVERLAP: usize = 150;

/// Environment variable name for the Anthropic API key.
pub const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
