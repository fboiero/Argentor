//! Corpus ingestion: reads JSONL records, chunks text, builds a BM25 index,
//! and persists the index plus chunk metadata to disk.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::{CHUNK_OVERLAP, CHUNK_SIZE};

// ---------------------------------------------------------------------------
// Raw corpus record (one JSONL line)
// ---------------------------------------------------------------------------

/// One page record from the JSONL corpus.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct CorpusRecord {
    /// Stable identifier (hash-based).
    pub stable_id: String,
    /// Origin source (legislacion, epec, ersep, …).
    pub fuente: String,
    /// Legal topic area.
    pub tema: String,
    /// Court / tribunal name (empty when not a judicial record).
    #[serde(default)]
    pub tribunal: String,
    /// Case title / heading.
    pub caratula: String,
    /// Document date (ISO or empty string).
    #[serde(default)]
    pub fecha: String,
    /// Original URL.
    #[serde(default)]
    pub url_origen: String,
    /// Source-specific identifier.
    pub id_fuente: String,
    /// Page number within the source document.
    pub page: u32,
    /// Extracted page text — the content we index.
    pub text: String,
}

// ---------------------------------------------------------------------------
// Chunk metadata (persisted alongside the BM25 index)
// ---------------------------------------------------------------------------

/// Metadata stored for each indexed chunk so query results can be annotated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkMeta {
    /// Unique chunk identifier (UUID).
    pub chunk_id: Uuid,
    /// Human-readable title / case heading.
    pub caratula: String,
    /// Origin source.
    pub fuente: String,
    /// Legal topic area.
    pub tema: String,
    /// Court name.
    pub tribunal: String,
    /// Document date.
    pub fecha: String,
    /// Chunk text content.
    pub content: String,
}

// ---------------------------------------------------------------------------
// Persisted index bundle
// ---------------------------------------------------------------------------

/// Everything needed to answer queries, persisted to disk as JSON.
#[derive(Debug, Serialize, Deserialize)]
pub struct IndexBundle {
    /// BM25 index serialised as-is.
    pub bm25: SerializedBm25,
    /// Chunk metadata keyed by UUID.
    pub chunks: HashMap<String, ChunkMeta>,
    /// Total number of source documents ingested.
    pub doc_count: usize,
    /// Total number of chunks in the index.
    pub chunk_count: usize,
}

/// A serialisable representation of `Bm25Index` internals.
///
/// `Bm25Index` does not derive `Serialize` so we replicate its fields here.
#[derive(Debug, Serialize, Deserialize)]
pub struct SerializedBm25 {
    /// term -> (doc_id -> term_frequency)
    pub inverted_index: HashMap<String, HashMap<String, f32>>,
    /// doc_id -> document length
    pub doc_lengths: HashMap<String, f32>,
    /// Total document count.
    pub doc_count: usize,
    /// Average document length.
    pub avg_doc_length: f32,
}

// ---------------------------------------------------------------------------
// Ingest implementation
// ---------------------------------------------------------------------------

/// Statistics returned after ingestion.
#[derive(Debug)]
pub struct IngestStats {
    /// Records ingested.
    pub doc_count: usize,
    /// Chunks created.
    pub chunk_count: usize,
    /// Records skipped (empty text or parse errors).
    pub skipped: usize,
    /// Approximate index file size in MiB.
    pub index_size_mb: f64,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Fixed-size character chunking with overlap.
fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.is_empty() || chunk_size == 0 {
        return vec![];
    }
    let effective_overlap = overlap.min(chunk_size.saturating_sub(1));
    let step = chunk_size.saturating_sub(effective_overlap).max(1);
    let chars: Vec<char> = text.chars().collect();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        let trimmed = chunk.trim().to_string();
        if !trimmed.is_empty() {
            chunks.push(trimmed);
        }
        if end == chars.len() {
            break;
        }
        start += step;
    }
    chunks
}

// ---------------------------------------------------------------------------

/// Core tokenizer matching BM25's internal tokenizer.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|w: &str| w.to_lowercase())
        .filter(|w| w.len() > 1)
        .collect()
}

/// Build index using parallel construction that keeps serialisable state.
pub fn run_v2(corpus_path: &Path, index_dir: &Path) -> anyhow::Result<IngestStats> {
    println!("Opening corpus: {}", corpus_path.display());

    let file = fs::File::open(corpus_path)
        .map_err(|e| anyhow::anyhow!("Cannot open corpus file: {e}"))?;
    let reader = BufReader::new(file);

    // Serializable BM25 internals we build in parallel
    let mut inv_index: HashMap<String, HashMap<String, f32>> = HashMap::new();
    let mut doc_lengths: HashMap<String, f32> = HashMap::new();

    let mut chunks_map: HashMap<String, ChunkMeta> = HashMap::new();
    let mut doc_count = 0usize;
    let mut chunk_count = 0usize;
    let mut skipped = 0usize;

    for (line_no, line_result) in reader.lines().enumerate() {
        let line = line_result.map_err(|e| anyhow::anyhow!("Read error at line {line_no}: {e}"))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let record: CorpusRecord = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  [SKIP] line {line_no}: {e}");
                skipped += 1;
                continue;
            }
        };

        if record.text.trim().is_empty() {
            skipped += 1;
            continue;
        }

        doc_count += 1;

        let text_chunks = chunk_text(&record.text, CHUNK_SIZE, CHUNK_OVERLAP);

        for chunk_content in text_chunks {
            if chunk_content.trim().is_empty() {
                continue;
            }
            let chunk_id = Uuid::new_v4();
            let id_str = chunk_id.to_string();

            // Build BM25 internals
            let tokens = tokenize(&chunk_content);
            let dl = tokens.len() as f32;
            let mut term_freq: HashMap<String, f32> = HashMap::new();
            for token in &tokens {
                *term_freq.entry(token.clone()).or_insert(0.0) += 1.0;
            }
            for (term, freq) in term_freq {
                inv_index
                    .entry(term)
                    .or_default()
                    .insert(id_str.clone(), freq);
            }
            doc_lengths.insert(id_str.clone(), dl);

            // Store metadata
            chunks_map.insert(
                id_str,
                ChunkMeta {
                    chunk_id,
                    caratula: record.caratula.clone(),
                    fuente: record.fuente.clone(),
                    tema: record.tema.clone(),
                    tribunal: record.tribunal.clone(),
                    fecha: record.fecha.clone(),
                    content: chunk_content,
                },
            );
            chunk_count += 1;
        }

        if doc_count % 5000 == 0 {
            println!("  ...{doc_count} records processed, {chunk_count} chunks so far");
        }
    }

    println!(
        "Ingested {doc_count} records → {chunk_count} chunks ({skipped} skipped)"
    );

    let avg_doc_length = if chunk_count > 0 {
        doc_lengths.values().sum::<f32>() / chunk_count as f32
    } else {
        0.0
    };

    let bundle = IndexBundle {
        bm25: SerializedBm25 {
            inverted_index: inv_index,
            doc_lengths,
            doc_count: chunk_count,
            avg_doc_length,
        },
        chunks: chunks_map,
        doc_count,
        chunk_count,
    };

    // Write index to disk
    fs::create_dir_all(index_dir)?;
    let index_path = index_dir.join("index.json");
    let json = serde_json::to_string(&bundle)
        .map_err(|e| anyhow::anyhow!("Failed to serialise index: {e}"))?;
    fs::write(&index_path, &json)?;

    let size_mb = fs::metadata(&index_path)?.len() as f64 / 1_048_576.0;
    println!("Index written to {} ({:.1} MB)", index_path.display(), size_mb);

    Ok(IngestStats {
        doc_count,
        chunk_count,
        skipped,
        index_size_mb: size_mb,
    })
}

/// Load the index bundle from disk.
pub fn load_index(index_dir: &Path) -> anyhow::Result<IndexBundle> {
    let index_path = index_dir.join("index.json");
    let json = fs::read_to_string(&index_path)
        .map_err(|e| anyhow::anyhow!("Cannot read index from {}: {e}", index_path.display()))?;
    let bundle: IndexBundle = serde_json::from_str(&json)
        .map_err(|e| anyhow::anyhow!("Failed to parse index: {e}"))?;
    Ok(bundle)
}
