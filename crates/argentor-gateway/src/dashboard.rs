//! Embedded web dashboard for the Argentor gateway.
//!
//! Serves a single-page HTML dashboard that communicates with the control plane
//! REST API to display deployment, agent, and health information.

use axum::{
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};

/// The dashboard HTML page, embedded at compile time.
const DASHBOARD_HTML: &str = include_str!("../dashboard.html");
/// The audit dashboard HTML page, embedded at compile time.
const AUDIT_DASHBOARD_HTML: &str = include_str!("../../../dashboard/audit.html");

/// Creates a router that serves the embedded dashboard.
///
/// Mounts `GET /dashboard` which returns the full SPA HTML page.
/// The dashboard itself fetches data from the control plane API endpoints
/// using client-side JavaScript.
pub fn dashboard_router() -> Router {
    Router::new()
        .route("/dashboard", get(dashboard_handler))
        .route("/dashboard/audit", get(audit_dashboard_handler))
}

/// Serves the embedded HTML dashboard.
async fn dashboard_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        DASHBOARD_HTML,
    )
}

/// Serves the embedded audit dashboard.
async fn audit_dashboard_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        AUDIT_DASHBOARD_HTML,
    )
}
