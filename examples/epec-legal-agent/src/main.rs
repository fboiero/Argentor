//! EPEC Legal Agent — AI-powered assistant for Argentine labor and energy law.
//!
//! Subcommands:
//! - `ingest` — reads the JSONL corpus, builds a BM25 index and persists it.
//! - `query`  — searches the index and optionally calls Claude for an answer.
//! - `serve`  — placeholder (not yet implemented).

mod config;
mod ingest;
mod query;

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use config::{ANTHROPIC_API_KEY_ENV, DEFAULT_INDEX_DIR, DEFAULT_TOP_K};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// EPEC Legal Agent — RAG-powered assistant for Argentine labor and energy law.
#[derive(Parser)]
#[command(name = "epec-legal-agent", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Ingest the JSONL corpus and build the BM25 index.
    Ingest {
        /// Path to corpus_export.jsonl
        #[arg(long)]
        corpus: PathBuf,

        /// Directory where the index will be written (default: ./epec_index)
        #[arg(long, default_value = DEFAULT_INDEX_DIR)]
        index_dir: PathBuf,
    },

    /// Query the index (optionally with Claude for AI answers).
    Query {
        /// The legal question to answer (omit for interactive stdin mode)
        question: Option<String>,

        /// Directory containing the persisted index (default: ./epec_index)
        #[arg(long, default_value = DEFAULT_INDEX_DIR)]
        index_dir: PathBuf,

        /// Number of chunks to retrieve (default: 5)
        #[arg(long, default_value_t = DEFAULT_TOP_K)]
        top_k: usize,

        /// Disable Claude even if ANTHROPIC_API_KEY is set
        #[arg(long)]
        no_llm: bool,
    },

    /// Start the Argentor gateway with the legal agent pre-loaded (not yet implemented).
    Serve,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("epec_legal_agent=info".parse()?),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Ingest { corpus, index_dir } => {
            println!("=== EPEC Legal Agent — Ingest ===");
            let stats = ingest::run_v2(&corpus, &index_dir)?;
            println!("\n--- Ingest complete ---");
            println!("  Documents ingested : {}", stats.doc_count);
            println!("  Chunks created     : {}", stats.chunk_count);
            println!("  Records skipped    : {}", stats.skipped);
            println!("  Index size         : {:.1} MB", stats.index_size_mb);
        }

        Command::Query {
            question,
            index_dir,
            top_k,
            no_llm,
        } => {
            println!("=== EPEC Legal Agent — Query ===");
            let index = query::LegalIndex::load(&index_dir)?;
            let use_llm = !no_llm;

            match question {
                Some(q) => {
                    run_single_query(&index, &q, top_k, use_llm).await?;
                }
                None => {
                    // Interactive stdin loop
                    println!("Interactive mode — type a question and press Enter (Ctrl-C to exit)");
                    let stdin = io::stdin();
                    loop {
                        print!("\nQuestion> ");
                        io::stdout().flush()?;
                        let mut line = String::new();
                        match stdin.lock().read_line(&mut line) {
                            Ok(0) => break, // EOF
                            Ok(_) => {
                                let q = line.trim().to_string();
                                if !q.is_empty() {
                                    run_single_query(&index, &q, top_k, use_llm).await?;
                                }
                            }
                            Err(e) => {
                                eprintln!("Read error: {e}");
                                break;
                            }
                        }
                    }
                }
            }
        }

        Command::Serve => {
            println!("cargo run -- serve not yet implemented, use query mode");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Single query helper
// ---------------------------------------------------------------------------

async fn run_single_query(
    index: &query::LegalIndex,
    question: &str,
    top_k: usize,
    use_llm: bool,
) -> anyhow::Result<()> {
    println!("\nQuestion: {question}");
    println!("{}", "-".repeat(60));

    let result = query::run(index, question, top_k, use_llm).await?;

    println!("\n--- Retrieved context ({} / {} chunks searched, {}ms) ---",
        result.hits.len(),
        result.total_chunks,
        result.query_time_ms,
    );
    println!("{}", result.context_text);

    if let Some(answer) = &result.llm_answer {
        println!("\n--- AI Answer (Claude) ---");
        println!("{answer}");
    } else if use_llm {
        let has_key = std::env::var(ANTHROPIC_API_KEY_ENV)
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if !has_key {
            println!(
                "\n[Set {ANTHROPIC_API_KEY_ENV} for AI-powered answers]"
            );
        }
    }

    println!("\nSources cited: {}", result.hits.len());
    Ok(())
}
