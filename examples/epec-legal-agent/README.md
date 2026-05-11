# EPEC Legal Agent — Argentor Demo

AI-powered legal assistant for Argentine labor and energy law, built on the
Argentor framework using native BM25 keyword search (no API key required).

## Corpus

- 25,667 JSONL records (one per page) from scraped jurisprudencia
- Sources: legislacion, epec, ersep, cordoba, saij, csjn, doctrina
- Topics: energia_marco_regulatorio, general_laboral, empleo_publico,
  gremial_sindical, energia_laboral, art_accidentes, despidos_indemnizaciones,
  solidaridad_tercerizacion, derecho_administrativo

## Quick Start

```bash
# 1. Ingest the corpus (builds BM25 index — no API key needed)
cargo run -p epec-legal-agent -- ingest \
  --corpus /path/to/corpus_export.jsonl

# 2. Query with BM25 keyword search (works without API key)
cargo run -p epec-legal-agent -- query \
  "régimen de indemnización por despido sin causa"

# 3. Interactive mode (no question arg = stdin loop)
cargo run -p epec-legal-agent -- query

# 4. With Claude AI answers (needs ANTHROPIC_API_KEY)
ANTHROPIC_API_KEY=sk-... cargo run -p epec-legal-agent -- query \
  "obligaciones de EPEC con usuarios electrodependientes"

# 5. Custom index directory or top-k
cargo run -p epec-legal-agent -- query \
  "estabilidad del empleado público en Córdoba" \
  --index-dir ./epec_index \
  --top-k 10
```

## Subcommands

| Command | Description |
|---------|-------------|
| `ingest --corpus <path>` | Read JSONL, chunk (800 chars, 150 overlap), build BM25 index |
| `query [question]` | BM25 search + optional Claude answer |
| `serve` | Not yet implemented — use `query` mode |

## Architecture

```
corpus_export.jsonl
       |
       v
  ingest.rs  ─────────────────────────────────────────────────
  (chunk, tokenize, build inverted index in parallel)         |
       |                                                       |
       v                                                       v
  epec_index/index.json                               ChunkMeta map
  (SerializedBm25 + chunk metadata)
       |
       v
  query.rs (LegalIndex)
  BM25 search → top-k chunks → format citations
       |
       v
  [optional] argentor-agent query_simple → Claude API → LLM answer
```

## Without API Key

BM25 keyword search returns the most relevant chunks with full citations
(fuente, tema, carátula, score). No network access required after ingest.

## With ANTHROPIC_API_KEY

Retrieved chunks are injected as context into a Claude prompt. The agent
answers strictly from the retrieved corpus — no hallucination outside the
indexed documents.

## License

AGPL-3.0-only
