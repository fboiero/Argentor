//! HTTP server for the EPEC Legal Agent web interface.
//!
//! Routes:
//!   GET  /          — serves the single-page chat UI
//!   POST /api/query — BM25 search, returns JSON results
//!   GET  /api/stats — index statistics

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::DEFAULT_INDEX_DIR;
use crate::ingest::ChunkMeta;
use crate::query::LegalIndex;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

struct AppState {
    index: LegalIndex,
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct QueryRequest {
    question: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
}

fn default_top_k() -> usize {
    5
}

#[derive(Serialize)]
struct SearchResult {
    rank: usize,
    score: f32,
    fuente: String,
    tema: String,
    caratula: String,
    fecha: String,
    tribunal: String,
    text: String,
}

#[derive(Serialize)]
struct QueryResponse {
    results: Vec<SearchResult>,
    query_time_ms: u64,
    total_chunks: usize,
}

#[derive(Serialize)]
struct StatsResponse {
    total_chunks: usize,
    total_docs: usize,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn index_html() -> Response {
    let html = include_str!("../static/index.html");
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

async fn api_query(
    State(state): State<Arc<AppState>>,
    Json(req): Json<QueryRequest>,
) -> Result<Json<QueryResponse>, (StatusCode, String)> {
    if req.question.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "question is required".to_string()));
    }
    let top_k = req.top_k.clamp(1, 20);

    let start = Instant::now();
    let hits = state.index.search(&req.question, top_k);
    let query_time_ms = start.elapsed().as_millis() as u64;
    let total_chunks = state.index.total_chunks();

    let results: Vec<SearchResult> = hits
        .into_iter()
        .enumerate()
        .map(|(i, (score, meta))| SearchResult {
            rank: i + 1,
            score,
            fuente: meta.fuente.clone(),
            tema: meta.tema.clone(),
            caratula: meta.caratula.clone(),
            fecha: meta.fecha.clone(),
            tribunal: meta.tribunal.clone(),
            text: meta.content.clone(),
        })
        .collect();

    Ok(Json(QueryResponse {
        results,
        query_time_ms,
        total_chunks,
    }))
}

async fn api_stats(State(state): State<Arc<AppState>>) -> Json<StatsResponse> {
    Json(StatsResponse {
        total_chunks: state.index.total_chunks(),
        total_docs: state.index.total_docs(),
    })
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn run(port: u16) -> Result<()> {
    let index_dir = PathBuf::from(DEFAULT_INDEX_DIR);
    info!("Loading BM25 index from {} …", index_dir.display());
    let index = LegalIndex::load(&index_dir)?;

    let state = Arc::new(AppState { index });

    let app = Router::new()
        .route("/", get(index_html))
        .route("/api/query", post(api_query))
        .route("/api/stats", get(api_stats))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("EPEC Legal Agent web UI → http://localhost:{port}/");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
