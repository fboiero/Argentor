//! Enterprise readiness report for the gateway product surface.
//!
//! This module turns existing gateway capabilities into a compact operational
//! report that product teams can expose as a golden-path status endpoint.

use serde::{Deserialize, Serialize};

/// High-level readiness posture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessPosture {
    /// The gateway has enough core controls wired to be considered ready.
    Ready,
    /// The gateway is usable, but one or more enterprise controls need setup.
    NeedsConfiguration,
    /// Required runtime pieces are missing.
    NotReady,
}

/// Status for a single readiness check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatus {
    /// The capability is active for this running gateway instance.
    Active,
    /// The capability exists in Argentor but still needs deployment-specific setup.
    Available,
    /// The capability needs attention before this instance is production ready.
    Attention,
}

/// One item in the enterprise readiness report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnterpriseReadinessCheck {
    /// Stable machine-readable check identifier.
    pub id: String,
    /// Human-readable group for dashboards and docs.
    pub category: String,
    /// Short check title.
    pub title: String,
    /// Current status.
    pub status: ReadinessStatus,
    /// Operational detail for humans.
    pub detail: String,
}

/// Dynamic runtime inputs used to build a readiness report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseReadinessInput {
    /// Number of skills registered in the gateway registry.
    pub skills_registered: usize,
    /// Number of active WebSocket connections.
    pub active_connections: usize,
    /// Number of active sessions attached to WebSocket connections.
    pub active_sessions: usize,
    /// Gateway uptime in seconds.
    pub uptime_seconds: i64,
    /// Whether the configured session store can be queried.
    pub sessions_reachable: bool,
    /// Whether the REST API is mounted.
    pub rest_api_mounted: bool,
    /// Whether API authentication is configured for this gateway instance.
    pub auth_configured: bool,
    /// Whether per-API-key rate limiting is configured.
    pub per_key_rate_limit_configured: bool,
    /// Whether the control plane routes are mounted.
    pub control_plane_mounted: bool,
    /// Whether proxy management routes are mounted.
    pub proxy_management_mounted: bool,
    /// Whether A2A protocol routes are mounted.
    pub a2a_mounted: bool,
    /// Whether Prometheus metrics are configured.
    pub metrics_configured: bool,
}

/// Enterprise readiness response returned by the gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnterpriseReadinessReport {
    /// Product version reported by the gateway crate.
    pub version: String,
    /// Overall posture.
    pub posture: ReadinessPosture,
    /// Integer score from 0 to 100.
    pub score: u8,
    /// Runtime counters useful for dashboards.
    pub runtime: EnterpriseRuntimeSnapshot,
    /// Individual checks.
    pub checks: Vec<EnterpriseReadinessCheck>,
    /// Ordered follow-up actions.
    pub next_actions: Vec<String>,
}

/// Runtime counters included in the readiness report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnterpriseRuntimeSnapshot {
    /// Number of skills registered in the gateway registry.
    pub skills_registered: usize,
    /// Number of active WebSocket connections.
    pub active_connections: usize,
    /// Number of active WebSocket sessions.
    pub active_sessions: usize,
    /// Gateway uptime in seconds.
    pub uptime_seconds: i64,
}

/// Build an enterprise readiness report from current gateway state.
pub fn build_enterprise_readiness_report(
    input: EnterpriseReadinessInput,
) -> EnterpriseReadinessReport {
    let checks = vec![
        check(
            "rest_api",
            "runtime",
            "REST API mounted",
            if input.rest_api_mounted {
                ReadinessStatus::Active
            } else {
                ReadinessStatus::Attention
            },
            if input.rest_api_mounted {
                "REST management endpoints are available under /api/v1."
            } else {
                "Mount RestApiState so sessions, skills, metrics, and readiness are available."
            },
        ),
        check(
            "session_store",
            "runtime",
            "Session store reachable",
            if input.sessions_reachable {
                ReadinessStatus::Active
            } else {
                ReadinessStatus::Attention
            },
            if input.sessions_reachable {
                "Session persistence responded to a list operation."
            } else {
                "Session persistence did not respond successfully."
            },
        ),
        check(
            "skills_registry",
            "agent_runtime",
            "Skills registry loaded",
            if input.skills_registered > 0 {
                ReadinessStatus::Active
            } else {
                ReadinessStatus::Attention
            },
            if input.skills_registered > 0 {
                "At least one skill is registered for agent execution."
            } else {
                "Register built-in or tenant-approved skills before serving agent traffic."
            },
        ),
        check(
            "security_controls",
            "security",
            "API authentication configured",
            if input.auth_configured {
                ReadinessStatus::Active
            } else {
                ReadinessStatus::Attention
            },
            if input.auth_configured {
                "Gateway authentication middleware is configured with accepted API keys."
            } else {
                "Configure API keys or SSO before exposing the gateway beyond local development."
            },
        ),
        check(
            "per_key_rate_limits",
            "security",
            "Per-key rate limits configured",
            if input.per_key_rate_limit_configured {
                ReadinessStatus::Active
            } else {
                ReadinessStatus::Available
            },
            if input.per_key_rate_limit_configured {
                "Per-API-key rate limiting is active for authenticated callers."
            } else {
                "Per-key rate limiting is available; wire it for tenant or API-key quotas."
            },
        ),
        check(
            "human_approval",
            "governance",
            "Human approval channel available",
            ReadinessStatus::Available,
            "WebSocket approval flow can be wired for high-risk tool execution.",
        ),
        check(
            "observability",
            "operations",
            "Metrics and OpenAPI available",
            if input.metrics_configured {
                ReadinessStatus::Active
            } else {
                ReadinessStatus::Available
            },
            if input.metrics_configured {
                "Prometheus metrics are configured and /openapi.json is exposed."
            } else {
                "OpenAPI is exposed; configure the metrics collector for Prometheus output."
            },
        ),
        check(
            "enterprise_gateway",
            "product",
            "Enterprise gateway surface",
            if input.control_plane_mounted && input.proxy_management_mounted {
                ReadinessStatus::Active
            } else if input.control_plane_mounted
                || input.proxy_management_mounted
                || input.a2a_mounted
            {
                ReadinessStatus::Available
            } else {
                ReadinessStatus::Attention
            },
            if input.control_plane_mounted && input.proxy_management_mounted {
                "Control plane and proxy management routes are mounted for enterprise operation."
            } else {
                "Mount control plane and proxy management routes for the full enterprise gateway path."
            },
        ),
    ];

    let active = checks
        .iter()
        .filter(|c| c.status == ReadinessStatus::Active)
        .count();
    let available = checks
        .iter()
        .filter(|c| c.status == ReadinessStatus::Available)
        .count();
    let attention = checks
        .iter()
        .filter(|c| c.status == ReadinessStatus::Attention)
        .count();

    let weighted = active * 2 + available;
    let max = checks.len() * 2;
    let score = ((weighted * 100) / max) as u8;
    let posture = if attention > 0 {
        ReadinessPosture::NeedsConfiguration
    } else if score >= 70 {
        ReadinessPosture::Ready
    } else {
        ReadinessPosture::NeedsConfiguration
    };

    EnterpriseReadinessReport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        posture,
        score,
        runtime: EnterpriseRuntimeSnapshot {
            skills_registered: input.skills_registered,
            active_connections: input.active_connections,
            active_sessions: input.active_sessions,
            uptime_seconds: input.uptime_seconds.max(0),
        },
        next_actions: next_actions(&checks),
        checks,
    }
}

fn check(
    id: &str,
    category: &str,
    title: &str,
    status: ReadinessStatus,
    detail: &str,
) -> EnterpriseReadinessCheck {
    EnterpriseReadinessCheck {
        id: id.to_string(),
        category: category.to_string(),
        title: title.to_string(),
        status,
        detail: detail.to_string(),
    }
}

fn next_actions(checks: &[EnterpriseReadinessCheck]) -> Vec<String> {
    let mut actions: Vec<String> = checks
        .iter()
        .filter(|check| check.status == ReadinessStatus::Attention)
        .map(|check| format!("Resolve {}: {}", check.id, check.detail))
        .collect();

    actions.extend([
        "Wire deployment-specific auth, SSO, rate limits, and approval policy.".to_string(),
        "Run the enterprise golden path smoke test before tagging a release.".to_string(),
    ]);

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_scores_active_runtime_with_available_enterprise_controls() {
        let report = build_enterprise_readiness_report(EnterpriseReadinessInput {
            skills_registered: 42,
            active_connections: 2,
            active_sessions: 1,
            uptime_seconds: 60,
            sessions_reachable: true,
            rest_api_mounted: true,
            auth_configured: true,
            per_key_rate_limit_configured: true,
            control_plane_mounted: true,
            proxy_management_mounted: true,
            a2a_mounted: true,
            metrics_configured: true,
        });

        assert_eq!(report.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(report.posture, ReadinessPosture::Ready);
        assert!(report.score >= 70);
        assert_eq!(report.runtime.skills_registered, 42);
        assert!(report
            .next_actions
            .iter()
            .any(|a| a.contains("golden path")));
    }

    #[test]
    fn report_marks_missing_runtime_dependencies_for_attention() {
        let report = build_enterprise_readiness_report(EnterpriseReadinessInput {
            skills_registered: 0,
            active_connections: 0,
            active_sessions: 0,
            uptime_seconds: -1,
            sessions_reachable: false,
            rest_api_mounted: false,
            auth_configured: false,
            per_key_rate_limit_configured: false,
            control_plane_mounted: false,
            proxy_management_mounted: false,
            a2a_mounted: false,
            metrics_configured: false,
        });

        assert_eq!(report.posture, ReadinessPosture::NeedsConfiguration);
        assert_eq!(report.runtime.uptime_seconds, 0);
        assert!(report.score < 70);
        assert!(report
            .checks
            .iter()
            .any(|c| c.id == "skills_registry" && c.status == ReadinessStatus::Attention));
        assert!(report
            .next_actions
            .iter()
            .any(|a| a.contains("skills_registry")));
    }
}
