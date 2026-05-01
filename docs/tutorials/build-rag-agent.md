# Build a RAG Agent

> Load documents, embed them, query with semantic search, and feed results to an Argentor agent as grounded context.

This guide gets you from an empty knowledge base to a working RAG agent in one sitting. For a deeper treatment of every RAG API, see [Tutorial 4: RAG Pipeline](./04-rag-pipeline.md).

---

## What you need

- Completed [Tutorial 1: First Agent](./01-first-agent.md)
- `ANTHROPIC_API_KEY` (or any supported provider key)
- A folder of `.md` or `.txt` files to index

---

## 1. Add the memory crate

```toml
[dependencies]
argentor-memory  = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-agent   = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-core    = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-security = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-session = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-skills  = { git = "https://github.com/fboiero/Agentor", branch = "master" }
tokio  = { version = "1", features = ["full"] }
anyhow = "1"
```

---

## 2. Load documents

Create a `src/main.rs` that reads every Markdown file in a directory:

```rust
use argentor_memory::{
    ChunkingStrategy, Document, FileVectorStore,
    LocalEmbedding, RagConfig, RagPipeline, EmbeddingProvider, VectorStore,
};
use std::collections::HashMap;
use std::sync::Arc;

async fn load_documents(dir: &str) -> anyhow::Result<Vec<Document>> {
    let mut docs = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let is_text = path.extension()
            .map(|e| e == "md" || e == "txt")
            .unwrap_or(false);

        if !is_text { continue; }

        let content = std::fs::read_to_string(&path)?;
        let id = path.file_stem().unwrap().to_string_lossy().into_owned();

        docs.push(Document {
            id: id.clone(),
            title: id,
            content,
            source: path.display().to_string(),
            metadata: HashMap::new(),
            category: None,
        });
    }

    println!("Loaded {} documents", docs.len());
    Ok(docs)
}
```

---

## 3. Embed and store

Wire up the embedding provider and vector store, then ingest:

```rust
async fn build_pipeline() -> anyhow::Result<RagPipeline> {
    // FileVectorStore persists embeddings to disk — survives restarts.
    let store: Arc<dyn VectorStore> = Arc::new(
        FileVectorStore::new("./vector-data.jsonl").await?
    );

    // LocalEmbedding: zero-cost TF-IDF bag-of-words, 256 dims.
    // For better recall, swap with OpenAiEmbeddingProvider.
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(LocalEmbedding::default());

    let config = RagConfig {
        chunking: ChunkingStrategy::Semantic { max_chunk_tokens: 512 },
        top_k: 5,
        min_relevance_score: 0.25,
        include_metadata: true,
        max_context_tokens: 4096,
    };

    Ok(RagPipeline::new(store, embedder, config))
}
```

Ingest your documents:

```rust
let rag = build_pipeline().await?;
let docs = load_documents("./knowledge").await?;

for doc in &docs {
    rag.ingest(doc).await?;
}

println!("Ingested {} documents into vector store", docs.len());
```

Each call to `ingest()` chunks the document, embeds every chunk, and writes a `MemoryEntry` to the store. Re-ingesting the same `Document.id` overwrites the previous entry — so re-runs are safe.

---

## 4. Query with semantic search

```rust
let query = "How does the WASM sandbox enforce capability limits?";
let result = rag.query(query).await?;

println!("Found {} chunks in {} ms", result.chunks.len(), result.query_time_ms);

for chunk in &result.chunks {
    println!(
        "  [{:.3}] {} — {}",
        chunk.score,
        chunk.document_title,
        &chunk.chunk.content[..chunk.chunk.content.len().min(100)],
    );
}
```

Expected output:

```
Found 5 chunks in 8 ms
  [0.851] security-model — WASM plugins run inside wasmtime with no ambient...
  [0.782] skills — Each skill declares capabilities in its SkillDescriptor...
  [0.701] architecture — The WasmSkillRuntime loads .wasm files at runtime...
```

`result.context_text` is a pre-formatted string you pass directly to the agent.

---

## 5. Feed results to the agent as context

```rust
use argentor_agent::{AgentRunner, LlmProvider, ModelConfig};
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::SkillRegistry;
use std::path::PathBuf;
use std::sync::Arc;

let user_question = "How does the WASM sandbox enforce capability limits?";

// Retrieve relevant chunks.
let rag_result = rag.query(user_question).await?;

// Build a grounded system prompt.
let system_prompt = format!(
    "You are a helpful assistant. Answer using ONLY the context below.\n\
     If the answer is not in the context, say \"I don't know based on the provided documents.\"\n\n\
     === CONTEXT ===\n{}\n=== END CONTEXT ===",
    rag_result.context_text,
);

// Configure the agent.
let config = ModelConfig {
    provider: LlmProvider::Claude,
    model_id: "claude-sonnet-4-20250514".into(),
    api_key: std::env::var("ANTHROPIC_API_KEY")?,
    api_base_url: None,
    temperature: 0.2,   // low temperature reduces hallucination
    max_tokens: 1024,
    max_turns: 3,
    fallback_models: vec![],
    retry_policy: None,
};

let runner = AgentRunner::new(
    config,
    Arc::new(SkillRegistry::new()),
    PermissionSet::new(),
    Arc::new(AuditLog::new(PathBuf::from("./audit"))),
)
.with_system_prompt(system_prompt);

let mut session = Session::new();
let answer = runner.run(&mut session, user_question).await?;

println!("\n{answer}");
```

---

## 6. Putting it all together

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let rag = build_pipeline().await?;

    // One-time ingestion: skip if vector-data.jsonl already exists.
    if !std::path::Path::new("./vector-data.jsonl").exists() {
        let docs = load_documents("./knowledge").await?;
        for doc in &docs {
            rag.ingest(doc).await?;
        }
    }

    let question = std::env::args().nth(1)
        .unwrap_or_else(|| "What is Argentor?".into());

    let rag_result = rag.query(&question).await?;

    let system_prompt = format!(
        "Answer from context only.\n\n=== CONTEXT ===\n{}\n=== END CONTEXT ===",
        rag_result.context_text,
    );

    let config = ModelConfig {
        provider: LlmProvider::Claude,
        model_id: "claude-sonnet-4-20250514".into(),
        api_key: std::env::var("ANTHROPIC_API_KEY")?,
        api_base_url: None,
        temperature: 0.2,
        max_tokens: 1024,
        max_turns: 3,
        fallback_models: vec![],
        retry_policy: None,
    };

    let runner = AgentRunner::new(
        config,
        Arc::new(SkillRegistry::new()),
        PermissionSet::new(),
        Arc::new(AuditLog::new(PathBuf::from("./audit"))),
    )
    .with_system_prompt(system_prompt);

    let mut session = Session::new();
    let answer = runner.run(&mut session, &question).await?;

    println!("{answer}");
    Ok(())
}
```

Run it:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
cargo run -- "What is Argentor?"
```

---

## Upgrading to production-quality embeddings

`LocalEmbedding` is fine for smoke tests. For real corpora, swap it in one line:

```rust
use argentor_memory::OpenAiEmbeddingProvider;

let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
    OpenAiEmbeddingProvider::new(std::env::var("OPENAI_API_KEY")?, "text-embedding-3-small")
);
```

Everything else stays the same — `EmbeddingProvider` is a trait.

---

## Common issues

**Scores are all below 0.3** — `LocalEmbedding` has weak semantic recall. Switch to `OpenAiEmbeddingProvider` or `CohereEmbeddingProvider`.

**LLM ignores the context** — Tighten the system prompt: add `"Do not use your training data."` and set `temperature: 0.0`.

**Duplicate chunks after re-ingest** — Make sure `Document.id` stays stable across runs. Reusing the same ID overwrites the old entry.

**Memory grows unbounded** — You're ingesting the same files on every startup. Gate ingestion on whether the vector store file exists (see the example above).

---

## Next steps

- [Tutorial 4: RAG Pipeline](./04-rag-pipeline.md) — hybrid BM25+vector search, query expansion, Pinecone/Weaviate/pgvector backends
- [Tutorial 3: Multi-Agent Orchestration](./03-multi-agent-orchestration.md) — share a RAG store across a worker team
- [Tutorial 7: Agent Intelligence](./07-agent-intelligence.md) — combine RAG with extended thinking
