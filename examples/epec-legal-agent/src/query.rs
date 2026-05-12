//! Query module: BM25 search over the persisted index, optional Claude integration.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use crate::config::ANTHROPIC_API_KEY_ENV;
use crate::ingest::{ChunkMeta, IndexBundle};

// BM25 constants (must match bm25.rs internals)
const K1: f32 = 1.2;
const B: f32 = 0.75;

// ---------------------------------------------------------------------------
// In-memory searcher built from the persisted index
// ---------------------------------------------------------------------------

/// Loaded and ready-to-search BM25 index.
pub struct LegalIndex {
    bundle: IndexBundle,
}

impl LegalIndex {
    /// Load from disk.
    pub fn load(index_dir: &Path) -> anyhow::Result<Self> {
        let bundle = crate::ingest::load_index(index_dir)?;
        println!(
            "Index loaded: {} chunks from {} docs",
            bundle.chunk_count, bundle.doc_count
        );
        Ok(Self { bundle })
    }

    /// Total number of indexed chunks.
    pub fn total_chunks(&self) -> usize {
        self.bundle.chunk_count
    }

    /// Total number of source documents.
    pub fn total_docs(&self) -> usize {
        self.bundle.doc_count
    }

    /// BM25 search — returns top-k `(score, ChunkMeta)` pairs.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(f32, &ChunkMeta)> {
        let bm25 = &self.bundle.bm25;
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() || bm25.doc_count == 0 {
            return vec![];
        }

        let n = bm25.doc_count as f32;
        let mut scores: HashMap<&str, f32> = HashMap::new();

        for token in &query_tokens {
            if let Some(postings) = bm25.inverted_index.get(token) {
                let df = postings.len() as f32;
                let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();

                for (doc_id, &tf) in postings {
                    let dl = bm25.doc_lengths.get(doc_id.as_str()).copied().unwrap_or(0.0);
                    let avgdl = if bm25.avg_doc_length > 0.0 {
                        bm25.avg_doc_length
                    } else {
                        1.0
                    };
                    let num = tf * (K1 + 1.0);
                    let den = tf + K1 * (1.0 - B + B * dl / avgdl);
                    let term_score = idf * num / den;
                    *scores.entry(doc_id.as_str()).or_insert(0.0) += term_score;
                }
            }
        }

        let mut results: Vec<(&str, f32)> = scores.into_iter().collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);

        results
            .into_iter()
            .filter_map(|(id, score)| {
                self.bundle.chunks.get(id).map(|meta| (score, meta))
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tokenizer (mirrors bm25.rs)
// ---------------------------------------------------------------------------

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|w: &str| w.to_lowercase())
        .filter(|w| w.len() > 1)
        .collect()
}

// ---------------------------------------------------------------------------
// Query runner
// ---------------------------------------------------------------------------

/// Result of a single query.
pub struct QueryResult {
    /// Top retrieved chunks with scores.
    pub hits: Vec<(f32, ChunkMeta)>,
    /// Formatted citation text ready for display.
    pub context_text: String,
    /// Total chunks searched.
    pub total_chunks: usize,
    /// Wall-clock query time in ms.
    pub query_time_ms: u64,
    /// LLM-generated answer (None if no API key or not requested).
    pub llm_answer: Option<String>,
}

/// Run a query against the loaded index, optionally calling Claude for an answer.
pub async fn run(
    index: &LegalIndex,
    question: &str,
    top_k: usize,
    use_llm: bool,
) -> anyhow::Result<QueryResult> {
    let start = Instant::now();

    let hits_refs = index.search(question, top_k);
    let query_time_ms = start.elapsed().as_millis() as u64;

    // Clone hits to own them
    let hits: Vec<(f32, ChunkMeta)> = hits_refs
        .into_iter()
        .map(|(s, m)| (s, m.clone()))
        .collect();

    let context_text = format_citations(&hits);
    let total_chunks = index.bundle.chunk_count;

    let llm_answer = if use_llm {
        match std::env::var(ANTHROPIC_API_KEY_ENV) {
            Ok(api_key) if !api_key.is_empty() => {
                Some(ask_claude(&api_key, question, &context_text).await?)
            }
            _ => None,
        }
    } else {
        None
    };

    Ok(QueryResult {
        hits,
        context_text,
        total_chunks,
        query_time_ms,
        llm_answer,
    })
}

/// Format retrieved chunks as numbered citations.
pub fn format_citations(hits: &[(f32, ChunkMeta)]) -> String {
    if hits.is_empty() {
        return "No se encontraron resultados relevantes.".to_string();
    }
    let mut parts = Vec::new();
    for (i, (score, meta)) in hits.iter().enumerate() {
        let tribunal_line = if meta.tribunal.is_empty() {
            String::new()
        } else {
            format!(" | Tribunal: {}", meta.tribunal)
        };
        let fecha_line = if meta.fecha.is_empty() {
            String::new()
        } else {
            format!(" | Fecha: {}", meta.fecha)
        };
        let header = format!(
            "[{}] Fuente: {} | Tema: {}{}{}| Carátula: {} (score: {:.3})",
            i + 1,
            meta.fuente,
            meta.tema,
            tribunal_line,
            fecha_line,
            meta.caratula,
            score
        );
        // Show first 400 chars of chunk content
        let preview: String = meta.content.chars().take(400).collect();
        parts.push(format!("{header}\n\"{preview}\""));
    }
    parts.join("\n\n")
}

// ---------------------------------------------------------------------------
// Claude integration
// ---------------------------------------------------------------------------

/// Call Claude with the retrieved context as system prompt.
async fn ask_claude(api_key: &str, question: &str, context: &str) -> anyhow::Result<String> {
    use argentor_agent::query::{query_simple, QueryOptions};

    let system_prompt = format!(
        "Sos un asistente legal especializado en derecho laboral argentino y regulación de energía eléctrica (EPEC/ERSEP). \
Respondé en español basándote EXCLUSIVAMENTE en el siguiente contexto recuperado del corpus legal:\n\n{context}\n\n\
Si la información del contexto no es suficiente para responder con certeza, indicalo claramente. \
No inventes normativa ni fallos. Citá las fuentes usando los números de referencia [1], [2], etc."
    );

    let options = QueryOptions::claude(api_key)
        .system_prompt(system_prompt)
        .max_turns(1);

    let answer = query_simple(question, options).await
        .map_err(|e| anyhow::anyhow!("Claude API error: {e}"))?;

    Ok(answer)
}

