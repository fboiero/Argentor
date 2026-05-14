# RAG Agent Template

Reference profile for a retrieval-augmented question-answering agent over a private document corpus, using Argentor's built-in vector store and local embeddings.

## Use case

Q&A over private documents without sending them to external APIs.

## Files

- `system_prompt.txt` — behaviour: always retrieve before answering, cite sources, refuse to speculate when retrieval returns nothing.
- `config.toml` — agent-profile sketch (model, max turns, strict guardrails, retrieval knobs).

## How to apply

The CLI does not load these files directly. Use them as a reference when wiring your own integration:

1. Copy `system_prompt.txt` into your `AgentRunner` setup (system message slot).
2. Declare each referenced skill in your `argentor.toml` `[[skills]]` block — every skill listed below is implemented in `argentor-builtins`.
3. Adopt the `guardrails.profile = "strict"` hint and `memory.tier = "multi"` hint via the runner configuration.
4. Apply `agent.max_turns = 10` via your `ModelConfig`.
5. Index your corpus with `argentor-memory`'s `FileVectorStore` and `LocalEmbedding`. The `rag.*` knobs in `config.toml` are the parameters you pass when constructing the index and the retrieval pipeline.

## Skills referenced

| Skill | Crate | Purpose |
|-------|-------|---------|
| `memory_search` | `argentor-builtins` | Vector similarity search over indexed documents |
| `web_search` | `argentor-builtins` | Optional fallback for queries not covered by the corpus |
| `file_read` | `argentor-builtins` | Read source files during indexing |

## Retrieval knobs

| Key | Default | Description |
|-----|---------|-------------|
| `rag.chunk_size` | `512` | Token size per chunk |
| `rag.chunk_overlap` | `64` | Overlap between chunks |
| `rag.top_k` | `5` | Number of chunks retrieved per query |
| `rag.similarity_threshold` | `0.75` | Minimum cosine similarity to include a chunk |
| `memory.tier` | `multi` | Short-term + long-term + entity memory |

These fields are advisory — they describe the profile and feed the retrieval pipeline you build, not runtime flags accepted by `argentor-cli`.

## License

AGPL-3.0-only
