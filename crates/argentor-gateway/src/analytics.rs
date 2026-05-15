//! Business analytics endpoints for the SaaS dashboard.
//!
//! Provides an [`AnalyticsEngine`] that collects interaction and quality events,
//! computes business metrics, and exposes them through REST endpoints mounted
//! under `/api/v1/analytics/`.
//!
//! # Endpoints
//!
//! - `GET /api/v1/analytics/{tenant_id}/dashboard` — Full analytics dashboard
//! - `GET /api/v1/analytics/{tenant_id}/agents/{role}` — Per-agent performance
//! - `GET /api/v1/analytics/{tenant_id}/funnel` — Sales conversion funnel
//! - `GET /api/v1/analytics/{tenant_id}/trends?days=30` — Daily metric trends

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Cost model
// ---------------------------------------------------------------------------

/// Legacy fallback: estimated cost per 1 000 tokens (blended input/output).
const DEFAULT_COST_PER_1K_TOKENS: f64 = 0.003;

/// Per-model pricing with separate input/output token costs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Cost per 1 000 input tokens in USD.
    pub input_cost_per_1k: f64,
    /// Cost per 1 000 output tokens in USD.
    pub output_cost_per_1k: f64,
}

impl ModelPricing {
    /// Blended cost per 1 000 tokens (average of input + output).
    pub fn blended_cost_per_1k(&self) -> f64 {
        (self.input_cost_per_1k + self.output_cost_per_1k) / 2.0
    }
}

/// Configurable pricing table, keyed by model identifier.
///
/// When a model is not found in the table, `fallback_cost_per_1k` is used
/// (defaults to the legacy `0.003` value for backward compatibility).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingTable {
    /// Per-model pricing entries.
    pub models: HashMap<String, ModelPricing>,
    /// Fallback blended cost per 1 000 tokens when model is not in the table.
    pub fallback_cost_per_1k: f64,
}

impl Default for PricingTable {
    fn default() -> Self {
        default_pricing()
    }
}

impl PricingTable {
    /// Look up the blended cost per 1 000 tokens for a model.
    ///
    /// Falls back to `fallback_cost_per_1k` when the model is unknown.
    pub fn cost_per_1k(&self, model: Option<&str>) -> f64 {
        model
            .and_then(|m| self.models.get(m))
            .map(ModelPricing::blended_cost_per_1k)
            .unwrap_or(self.fallback_cost_per_1k)
    }
}

/// Returns a [`PricingTable`] with reasonable defaults for well-known models.
///
/// Prices reflect approximate public API pricing as of early 2025.
pub fn default_pricing() -> PricingTable {
    let mut models = HashMap::new();

    // Anthropic Claude
    models.insert(
        "claude-sonnet-4-20250514".into(),
        ModelPricing {
            input_cost_per_1k: 0.003,
            output_cost_per_1k: 0.015,
        },
    );
    models.insert(
        "claude-opus-4-20250514".into(),
        ModelPricing {
            input_cost_per_1k: 0.015,
            output_cost_per_1k: 0.075,
        },
    );
    models.insert(
        "claude-3-5-sonnet-20241022".into(),
        ModelPricing {
            input_cost_per_1k: 0.003,
            output_cost_per_1k: 0.015,
        },
    );
    models.insert(
        "claude-3-5-haiku-20241022".into(),
        ModelPricing {
            input_cost_per_1k: 0.001,
            output_cost_per_1k: 0.005,
        },
    );

    // OpenAI
    models.insert(
        "gpt-4o".into(),
        ModelPricing {
            input_cost_per_1k: 0.0025,
            output_cost_per_1k: 0.01,
        },
    );
    models.insert(
        "gpt-4o-mini".into(),
        ModelPricing {
            input_cost_per_1k: 0.00015,
            output_cost_per_1k: 0.0006,
        },
    );
    models.insert(
        "gpt-4-turbo".into(),
        ModelPricing {
            input_cost_per_1k: 0.01,
            output_cost_per_1k: 0.03,
        },
    );

    // Google Gemini
    models.insert(
        "gemini-2.0-flash".into(),
        ModelPricing {
            input_cost_per_1k: 0.0001,
            output_cost_per_1k: 0.0004,
        },
    );
    models.insert(
        "gemini-1.5-pro".into(),
        ModelPricing {
            input_cost_per_1k: 0.00125,
            output_cost_per_1k: 0.005,
        },
    );

    PricingTable {
        models,
        fallback_cost_per_1k: DEFAULT_COST_PER_1K_TOKENS,
    }
}

// ---------------------------------------------------------------------------
// Types — Events
// ---------------------------------------------------------------------------

/// Outcome of a customer interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InteractionOutcome {
    /// Issue was fully resolved by the agent.
    Resolved,
    /// Issue was escalated to a human.
    Escalated,
    /// Interaction is still open / in progress.
    Pending,
    /// Customer abandoned the interaction.
    Abandoned,
}

/// A single customer interaction event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionEvent {
    /// Tenant that owns this interaction.
    pub tenant_id: String,
    /// Agent role that handled the interaction.
    pub agent_role: String,
    /// Communication channel (email, chat, ticket).
    pub channel: String,
    /// Optional customer identifier for funnel tracking.
    pub customer_id: Option<String>,
    /// How the interaction concluded.
    pub outcome: InteractionOutcome,
    /// Duration of the interaction in milliseconds.
    pub duration_ms: u64,
    /// LLM tokens consumed during the interaction.
    pub tokens_used: u64,
    /// When the interaction occurred.
    pub timestamp: DateTime<Utc>,
}

/// Stage in the sales funnel for conversion tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FunnelStage {
    /// Raw lead entered the system.
    Lead,
    /// Lead was qualified by scoring.
    Qualified,
    /// Outreach was sent.
    Contacted,
    /// Customer responded.
    Responded,
    /// A demo or meeting was scheduled.
    DemoScheduled,
    /// Deal was closed / converted.
    Converted,
}

/// A quality evaluation event for a single agent response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityEvent {
    /// Tenant that owns this evaluation.
    pub tenant_id: String,
    /// Agent role that was evaluated.
    pub agent_role: String,
    /// Overall quality score (0.0 – 1.0).
    pub overall_score: f32,
    /// Per-criterion scores (e.g. "accuracy" → 0.9).
    pub criteria_scores: HashMap<String, f32>,
    /// When the evaluation was recorded.
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Types — Dashboard / Responses
// ---------------------------------------------------------------------------

/// Per-agent slice of the dashboard.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentDashboard {
    /// Number of interactions handled by this agent.
    pub interactions: u64,
    /// Average quality score.
    pub avg_quality: f32,
    /// Percentage of interactions resolved without escalation.
    pub resolution_rate: f64,
    /// Average interaction duration in milliseconds.
    pub avg_duration_ms: u64,
    /// Total cost attributed to this agent.
    pub cost_usd: f64,
}

/// Per-channel statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelStats {
    /// Number of interactions on this channel.
    pub interactions: u64,
    /// Percentage of interactions resolved without escalation.
    pub resolution_rate: f64,
    /// Average interaction duration in milliseconds.
    pub avg_duration_ms: u64,
}

/// Aggregated metrics for a single calendar day.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyMetric {
    /// Date in YYYY-MM-DD format.
    pub date: String,
    /// Total interactions on this day.
    pub interactions: u64,
    /// Interactions that were resolved.
    pub resolutions: u64,
    /// Interactions that were escalated.
    pub escalations: u64,
    /// Average quality score across all evaluations.
    pub avg_quality: f32,
    /// Total estimated cost.
    pub cost_usd: f64,
}

/// Full analytics dashboard response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsDashboard {
    /// Tenant this dashboard belongs to.
    pub tenant_id: String,
    /// Human-readable period label (e.g. "last_30d").
    pub period: String,
    /// Total number of interactions in the period.
    pub total_interactions: u64,
    /// Percentage of interactions resolved without escalation (0.0 – 1.0).
    pub resolution_rate: f64,
    /// Average interaction duration in milliseconds.
    pub avg_response_time_ms: u64,
    /// Average quality score across all evaluations.
    pub avg_quality_score: f32,
    /// Estimated customer satisfaction score derived from quality + resolution.
    pub csat_estimate: f32,
    /// Total estimated cost in USD.
    pub cost_total_usd: f64,
    /// Cost per interaction in USD.
    pub cost_per_interaction_usd: f64,
    /// Breakdown by agent role.
    pub by_agent: HashMap<String, AgentDashboard>,
    /// Breakdown by communication channel.
    pub by_channel: HashMap<String, ChannelStats>,
    /// Daily time-series of metrics.
    pub trend: Vec<DailyMetric>,
}

/// Detailed performance metrics for a single agent role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPerformance {
    /// Agent role identifier.
    pub agent_role: String,
    /// Total interactions handled.
    pub total_calls: u64,
    /// Average quality score.
    pub avg_quality: f32,
    /// Resolution rate (0.0 – 1.0).
    pub resolution_rate: f64,
    /// Average interaction duration in milliseconds.
    pub avg_duration_ms: u64,
    /// Escalation rate (0.0 – 1.0).
    pub escalation_rate: f64,
    /// Most frequent topics / channels.
    pub top_topics: Vec<String>,
}

/// Sales conversion funnel metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionFunnel {
    /// Total leads that entered the funnel.
    pub total_leads: u64,
    /// Leads that passed qualification.
    pub qualified: u64,
    /// Leads that were contacted.
    pub contacted: u64,
    /// Leads that responded.
    pub responded: u64,
    /// Leads that scheduled a demo.
    pub demo_scheduled: u64,
    /// Leads that converted.
    pub converted: u64,
    /// Overall conversion rate (converted / total_leads).
    pub conversion_rate: f64,
    /// Average number of days from lead to conversion.
    pub avg_time_to_conversion_days: f64,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Unified error type for analytics handlers.
#[derive(Debug)]
pub enum AnalyticsError {
    /// The requested tenant or resource was not found.
    NotFound(String),
    /// A query parameter was invalid.
    BadRequest(String),
}

impl std::fmt::Display for AnalyticsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(msg) => write!(f, "Not found: {msg}"),
            Self::BadRequest(msg) => write!(f, "Bad request: {msg}"),
        }
    }
}

impl IntoResponse for AnalyticsError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match &self {
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
        };
        let body = serde_json::json!({ "error": message });
        (status, axum::Json(body)).into_response()
    }
}

// ---------------------------------------------------------------------------
// AnalyticsEngine
// ---------------------------------------------------------------------------

/// Internal storage for a tenant's analytics data.
#[derive(Debug, Clone, Default)]
struct TenantData {
    interactions: Vec<InteractionEvent>,
    quality: Vec<QualityEvent>,
    funnel: HashMap<FunnelStage, u64>,
    /// Sum of days-to-conversion for converted leads (for average calculation).
    conversion_days_sum: f64,
    /// Number of converted leads that have timing data.
    conversion_count: u64,
}

/// Thread-safe analytics engine that computes business metrics from recorded events.
///
/// Usage:
/// ```ignore
/// let engine = AnalyticsEngine::new();
/// engine.record_interaction(event).await;
/// let dashboard = engine.get_dashboard("tenant_1", "last_30d").await;
/// ```
#[derive(Clone)]
pub struct AnalyticsEngine {
    data: Arc<RwLock<HashMap<String, TenantData>>>,
    pricing: Arc<PricingTable>,
}

impl AnalyticsEngine {
    /// Create a new empty analytics engine with default pricing.
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            pricing: Arc::new(PricingTable::default()),
        }
    }

    /// Create a new analytics engine with custom pricing.
    pub fn with_pricing(pricing: PricingTable) -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            pricing: Arc::new(pricing),
        }
    }

    /// Returns a reference to the current pricing table.
    pub fn pricing(&self) -> &PricingTable {
        &self.pricing
    }

    /// Record a customer interaction event.
    pub async fn record_interaction(&self, event: InteractionEvent) {
        let mut data = self.data.write().await;
        let tenant = data.entry(event.tenant_id.clone()).or_default();
        tenant.interactions.push(event);
    }

    /// Record a quality evaluation event.
    pub async fn record_quality_score(&self, event: QualityEvent) {
        let mut data = self.data.write().await;
        let tenant = data.entry(event.tenant_id.clone()).or_default();
        tenant.quality.push(event);
    }

    /// Record a funnel stage advancement for a tenant.
    pub async fn record_funnel_event(
        &self,
        tenant_id: &str,
        stage: FunnelStage,
        days_to_conversion: Option<f64>,
    ) {
        let mut data = self.data.write().await;
        let tenant = data.entry(tenant_id.to_string()).or_default();
        *tenant.funnel.entry(stage).or_insert(0) += 1;
        if let Some(days) = days_to_conversion {
            tenant.conversion_days_sum += days;
            tenant.conversion_count += 1;
        }
    }

    /// Compute the full analytics dashboard for a tenant and period.
    pub async fn get_dashboard(&self, tenant_id: &str, period: &str) -> AnalyticsDashboard {
        let data = self.data.read().await;
        let empty = TenantData::default();
        let tenant = data.get(tenant_id).unwrap_or(&empty);

        let cutoff = period_to_cutoff(period);
        let interactions: Vec<&InteractionEvent> = tenant
            .interactions
            .iter()
            .filter(|i| i.timestamp >= cutoff)
            .collect();
        let quality: Vec<&QualityEvent> = tenant
            .quality
            .iter()
            .filter(|q| q.timestamp >= cutoff)
            .collect();

        let total = interactions.len() as u64;
        let resolved = interactions
            .iter()
            .filter(|i| i.outcome == InteractionOutcome::Resolved)
            .count() as u64;
        let resolution_rate = if total > 0 {
            resolved as f64 / total as f64
        } else {
            0.0
        };

        let total_duration: u64 = interactions.iter().map(|i| i.duration_ms).sum();
        let avg_response_time_ms = total_duration.checked_div(total).unwrap_or(0);

        let total_tokens: u64 = interactions.iter().map(|i| i.tokens_used).sum();
        let cost_per_1k = self.pricing.cost_per_1k(None);
        let cost_total_usd = (total_tokens as f64 / 1000.0) * cost_per_1k;
        let cost_per_interaction_usd = if total > 0 {
            cost_total_usd / total as f64
        } else {
            0.0
        };

        let avg_quality_score = if quality.is_empty() {
            0.0
        } else {
            quality.iter().map(|q| q.overall_score).sum::<f32>() / quality.len() as f32
        };

        // CSAT estimate: weighted combination of quality and resolution
        let csat_estimate = avg_quality_score * 0.6 + (resolution_rate as f32) * 0.4;

        // By agent
        let mut by_agent: HashMap<String, AgentDashboard> = HashMap::new();
        for i in &interactions {
            let entry = by_agent.entry(i.agent_role.clone()).or_default();
            entry.interactions += 1;
            entry.avg_duration_ms += i.duration_ms; // accumulate, will divide later
            entry.cost_usd += (i.tokens_used as f64 / 1000.0) * cost_per_1k;
            if i.outcome == InteractionOutcome::Resolved {
                entry.resolution_rate += 1.0; // count, will divide later
            }
        }
        // Per-agent quality
        let mut agent_quality: HashMap<String, (f32, u32)> = HashMap::new();
        for q in &quality {
            let entry = agent_quality
                .entry(q.agent_role.clone())
                .or_insert((0.0, 0));
            entry.0 += q.overall_score;
            entry.1 += 1;
        }
        for (role, dashboard) in &mut by_agent {
            let count = dashboard.interactions;
            if count > 0 {
                dashboard.resolution_rate /= count as f64;
                dashboard.avg_duration_ms /= count;
            }
            if let Some((sum, cnt)) = agent_quality.get(role) {
                if *cnt > 0 {
                    dashboard.avg_quality = sum / *cnt as f32;
                }
            }
        }

        // By channel
        let mut by_channel: HashMap<String, ChannelStats> = HashMap::new();
        for i in &interactions {
            let entry = by_channel.entry(i.channel.clone()).or_default();
            entry.interactions += 1;
            entry.avg_duration_ms += i.duration_ms;
            if i.outcome == InteractionOutcome::Resolved {
                entry.resolution_rate += 1.0;
            }
        }
        for stats in by_channel.values_mut() {
            let count = stats.interactions;
            if count > 0 {
                stats.resolution_rate /= count as f64;
                stats.avg_duration_ms /= count;
            }
        }

        // Daily trend
        let trend = build_daily_trend(&interactions, &quality, cost_per_1k);

        AnalyticsDashboard {
            tenant_id: tenant_id.to_string(),
            period: period.to_string(),
            total_interactions: total,
            resolution_rate,
            avg_response_time_ms,
            avg_quality_score,
            csat_estimate,
            cost_total_usd,
            cost_per_interaction_usd,
            by_agent,
            by_channel,
            trend,
        }
    }

    /// Get detailed performance metrics for a specific agent role within a tenant.
    pub async fn get_agent_performance(
        &self,
        tenant_id: &str,
        agent_role: &str,
    ) -> AgentPerformance {
        let data = self.data.read().await;
        let empty = TenantData::default();
        let tenant = data.get(tenant_id).unwrap_or(&empty);

        let interactions: Vec<&InteractionEvent> = tenant
            .interactions
            .iter()
            .filter(|i| i.agent_role == agent_role)
            .collect();
        let quality: Vec<&QualityEvent> = tenant
            .quality
            .iter()
            .filter(|q| q.agent_role == agent_role)
            .collect();

        let total = interactions.len() as u64;
        let resolved = interactions
            .iter()
            .filter(|i| i.outcome == InteractionOutcome::Resolved)
            .count() as u64;
        let escalated = interactions
            .iter()
            .filter(|i| i.outcome == InteractionOutcome::Escalated)
            .count() as u64;

        let resolution_rate = if total > 0 {
            resolved as f64 / total as f64
        } else {
            0.0
        };
        let escalation_rate = if total > 0 {
            escalated as f64 / total as f64
        } else {
            0.0
        };

        let total_duration: u64 = interactions.iter().map(|i| i.duration_ms).sum();
        let avg_duration_ms = total_duration.checked_div(total).unwrap_or(0);

        let avg_quality = if quality.is_empty() {
            0.0
        } else {
            quality.iter().map(|q| q.overall_score).sum::<f32>() / quality.len() as f32
        };

        // Top topics: count channels as proxy for topics
        let mut channel_counts: HashMap<String, u64> = HashMap::new();
        for i in &interactions {
            *channel_counts.entry(i.channel.clone()).or_insert(0) += 1;
        }
        let mut sorted_channels: Vec<(String, u64)> = channel_counts.into_iter().collect();
        sorted_channels.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let top_topics: Vec<String> = sorted_channels
            .into_iter()
            .take(5)
            .map(|(ch, _)| ch)
            .collect();

        AgentPerformance {
            agent_role: agent_role.to_string(),
            total_calls: total,
            avg_quality,
            resolution_rate,
            avg_duration_ms,
            escalation_rate,
            top_topics,
        }
    }

    /// Get the sales conversion funnel for a tenant.
    pub async fn get_conversion_funnel(&self, tenant_id: &str) -> ConversionFunnel {
        let data = self.data.read().await;
        let empty = TenantData::default();
        let tenant = data.get(tenant_id).unwrap_or(&empty);

        let total_leads = *tenant.funnel.get(&FunnelStage::Lead).unwrap_or(&0);
        let qualified = *tenant.funnel.get(&FunnelStage::Qualified).unwrap_or(&0);
        let contacted = *tenant.funnel.get(&FunnelStage::Contacted).unwrap_or(&0);
        let responded = *tenant.funnel.get(&FunnelStage::Responded).unwrap_or(&0);
        let demo_scheduled = *tenant.funnel.get(&FunnelStage::DemoScheduled).unwrap_or(&0);
        let converted = *tenant.funnel.get(&FunnelStage::Converted).unwrap_or(&0);

        let conversion_rate = if total_leads > 0 {
            converted as f64 / total_leads as f64
        } else {
            0.0
        };

        let avg_time_to_conversion_days = if tenant.conversion_count > 0 {
            tenant.conversion_days_sum / tenant.conversion_count as f64
        } else {
            0.0
        };

        ConversionFunnel {
            total_leads,
            qualified,
            contacted,
            responded,
            demo_scheduled,
            converted,
            conversion_rate,
            avg_time_to_conversion_days,
        }
    }

    /// Get daily trend metrics for a tenant, for the last `days` days.
    pub async fn get_trends(&self, tenant_id: &str, days: u32) -> Vec<DailyMetric> {
        let data = self.data.read().await;
        let empty = TenantData::default();
        let tenant = data.get(tenant_id).unwrap_or(&empty);

        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        let interactions: Vec<&InteractionEvent> = tenant
            .interactions
            .iter()
            .filter(|i| i.timestamp >= cutoff)
            .collect();
        let quality: Vec<&QualityEvent> = tenant
            .quality
            .iter()
            .filter(|q| q.timestamp >= cutoff)
            .collect();

        build_daily_trend(&interactions, &quality, self.pricing.cost_per_1k(None))
    }
}

impl Default for AnalyticsEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a period string like "last_7d" or "last_30d" into a UTC cutoff timestamp.
fn period_to_cutoff(period: &str) -> DateTime<Utc> {
    let days = if period.starts_with("last_") && period.ends_with('d') {
        period
            .trim_start_matches("last_")
            .trim_end_matches('d')
            .parse::<i64>()
            .unwrap_or(30)
    } else {
        30
    };
    Utc::now() - chrono::Duration::days(days)
}

/// Build daily aggregated metrics from interaction and quality events.
fn build_daily_trend(
    interactions: &[&InteractionEvent],
    quality: &[&QualityEvent],
    cost_per_1k: f64,
) -> Vec<DailyMetric> {
    // Group interactions by date
    let mut by_date: HashMap<NaiveDate, (u64, u64, u64, u64)> = HashMap::new(); // (count, resolved, escalated, tokens)
    for i in interactions {
        let date = i.timestamp.date_naive();
        let entry = by_date.entry(date).or_insert((0, 0, 0, 0));
        entry.0 += 1;
        if i.outcome == InteractionOutcome::Resolved {
            entry.1 += 1;
        }
        if i.outcome == InteractionOutcome::Escalated {
            entry.2 += 1;
        }
        entry.3 += i.tokens_used;
    }

    // Group quality by date
    let mut quality_by_date: HashMap<NaiveDate, (f32, u32)> = HashMap::new();
    for q in quality {
        let date = q.timestamp.date_naive();
        let entry = quality_by_date.entry(date).or_insert((0.0, 0));
        entry.0 += q.overall_score;
        entry.1 += 1;
    }

    // Merge into daily metrics
    let mut all_dates: Vec<NaiveDate> = by_date
        .keys()
        .chain(quality_by_date.keys())
        .copied()
        .collect();
    all_dates.sort();
    all_dates.dedup();

    all_dates
        .into_iter()
        .map(|date| {
            let (count, resolved, escalated, tokens) =
                by_date.get(&date).copied().unwrap_or((0, 0, 0, 0));
            let avg_quality = quality_by_date
                .get(&date)
                .map(|(sum, cnt)| if *cnt > 0 { sum / *cnt as f32 } else { 0.0 })
                .unwrap_or(0.0);
            let cost_usd = (tokens as f64 / 1000.0) * cost_per_1k;
            DailyMetric {
                date: date.format("%Y-%m-%d").to_string(),
                interactions: count,
                resolutions: resolved,
                escalations: escalated,
                avg_quality,
                cost_usd,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Axum state & query params
// ---------------------------------------------------------------------------

/// Shared state for analytics endpoints.
pub struct AnalyticsState {
    /// The analytics engine.
    pub engine: AnalyticsEngine,
}

/// Query parameters for the trends endpoint.
#[derive(Debug, Deserialize)]
struct TrendsQuery {
    /// Number of days to include (default: 30).
    days: Option<u32>,
}

/// Query parameters for the dashboard endpoint.
#[derive(Debug, Deserialize)]
struct DashboardQuery {
    /// Period label, e.g. "last_7d", "last_30d" (default: "last_30d").
    period: Option<String>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Create the analytics Axum router with all endpoints.
pub fn analytics_router(state: Arc<AnalyticsState>) -> Router {
    Router::new()
        .route(
            "/api/v1/analytics/{tenant_id}/dashboard",
            get(dashboard_handler),
        )
        .route(
            "/api/v1/analytics/{tenant_id}/agents/{role}",
            get(agent_performance_handler),
        )
        .route("/api/v1/analytics/{tenant_id}/funnel", get(funnel_handler))
        .route("/api/v1/analytics/{tenant_id}/trends", get(trends_handler))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/analytics/{tenant_id}/dashboard
async fn dashboard_handler(
    State(state): State<Arc<AnalyticsState>>,
    Path(tenant_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<DashboardQuery>,
) -> impl IntoResponse {
    let period = params.period.unwrap_or_else(|| "last_30d".to_string());
    let dashboard = state.engine.get_dashboard(&tenant_id, &period).await;
    axum::Json(dashboard).into_response()
}

/// GET /api/v1/analytics/{tenant_id}/agents/{role}
async fn agent_performance_handler(
    State(state): State<Arc<AnalyticsState>>,
    Path((tenant_id, role)): Path<(String, String)>,
) -> impl IntoResponse {
    let performance = state.engine.get_agent_performance(&tenant_id, &role).await;
    axum::Json(performance).into_response()
}

/// GET /api/v1/analytics/{tenant_id}/funnel
async fn funnel_handler(
    State(state): State<Arc<AnalyticsState>>,
    Path(tenant_id): Path<String>,
) -> impl IntoResponse {
    let funnel = state.engine.get_conversion_funnel(&tenant_id).await;
    axum::Json(funnel).into_response()
}

/// GET /api/v1/analytics/{tenant_id}/trends
async fn trends_handler(
    State(state): State<Arc<AnalyticsState>>,
    Path(tenant_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<TrendsQuery>,
) -> impl IntoResponse {
    let days = params.days.unwrap_or(30);
    let trends = state.engine.get_trends(&tenant_id, days).await;
    axum::Json(trends).into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    // -- Helpers -----------------------------------------------------------

    fn make_interaction(
        tenant: &str,
        role: &str,
        channel: &str,
        outcome: InteractionOutcome,
        duration_ms: u64,
        tokens: u64,
    ) -> InteractionEvent {
        InteractionEvent {
            tenant_id: tenant.to_string(),
            agent_role: role.to_string(),
            channel: channel.to_string(),
            customer_id: None,
            outcome,
            duration_ms,
            tokens_used: tokens,
            timestamp: Utc::now(),
        }
    }

    fn make_interaction_at(
        tenant: &str,
        role: &str,
        channel: &str,
        outcome: InteractionOutcome,
        duration_ms: u64,
        tokens: u64,
        timestamp: DateTime<Utc>,
    ) -> InteractionEvent {
        InteractionEvent {
            tenant_id: tenant.to_string(),
            agent_role: role.to_string(),
            channel: channel.to_string(),
            customer_id: None,
            outcome,
            duration_ms,
            tokens_used: tokens,
            timestamp,
        }
    }

    fn make_quality(tenant: &str, role: &str, score: f32) -> QualityEvent {
        QualityEvent {
            tenant_id: tenant.to_string(),
            agent_role: role.to_string(),
            overall_score: score,
            criteria_scores: HashMap::new(),
            timestamp: Utc::now(),
        }
    }

    fn make_quality_at(
        tenant: &str,
        role: &str,
        score: f32,
        timestamp: DateTime<Utc>,
    ) -> QualityEvent {
        QualityEvent {
            tenant_id: tenant.to_string(),
            agent_role: role.to_string(),
            overall_score: score,
            criteria_scores: HashMap::new(),
            timestamp,
        }
    }

    fn make_state() -> Arc<AnalyticsState> {
        Arc::new(AnalyticsState {
            engine: AnalyticsEngine::new(),
        })
    }

    // -- Unit tests --------------------------------------------------------

    #[tokio::test]
    async fn test_engine_new_is_empty() {
        let engine = AnalyticsEngine::new();
        let dashboard = engine.get_dashboard("t1", "last_30d").await;
        assert_eq!(dashboard.total_interactions, 0);
        assert_eq!(dashboard.resolution_rate, 0.0);
        assert_eq!(dashboard.avg_quality_score, 0.0);
    }

    #[tokio::test]
    async fn test_record_interaction() {
        let engine = AnalyticsEngine::new();
        let event = make_interaction(
            "t1",
            "sales",
            "chat",
            InteractionOutcome::Resolved,
            500,
            100,
        );
        engine.record_interaction(event).await;
        let dashboard = engine.get_dashboard("t1", "last_30d").await;
        assert_eq!(dashboard.total_interactions, 1);
    }

    #[tokio::test]
    async fn test_resolution_rate() {
        let engine = AnalyticsEngine::new();
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
            ))
            .await;
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Escalated,
                800,
                200,
            ))
            .await;
        let dashboard = engine.get_dashboard("t1", "last_30d").await;
        assert_eq!(dashboard.total_interactions, 2);
        assert!((dashboard.resolution_rate - 0.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_avg_response_time() {
        let engine = AnalyticsEngine::new();
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                400,
                100,
            ))
            .await;
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                600,
                100,
            ))
            .await;
        let dashboard = engine.get_dashboard("t1", "last_30d").await;
        assert_eq!(dashboard.avg_response_time_ms, 500);
    }

    #[tokio::test]
    async fn test_quality_scoring() {
        let engine = AnalyticsEngine::new();
        engine
            .record_quality_score(make_quality("t1", "sales", 0.8))
            .await;
        engine
            .record_quality_score(make_quality("t1", "sales", 0.6))
            .await;
        let dashboard = engine.get_dashboard("t1", "last_30d").await;
        assert!((dashboard.avg_quality_score - 0.7).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_csat_estimate() {
        let engine = AnalyticsEngine::new();
        // All resolved → resolution_rate = 1.0
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
            ))
            .await;
        // Quality = 0.8
        engine
            .record_quality_score(make_quality("t1", "sales", 0.8))
            .await;
        let dashboard = engine.get_dashboard("t1", "last_30d").await;
        // csat = 0.8 * 0.6 + 1.0 * 0.4 = 0.48 + 0.4 = 0.88
        assert!((dashboard.csat_estimate - 0.88).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_cost_calculation() {
        let engine = AnalyticsEngine::new();
        // 10_000 tokens → (10_000 / 1000) * 0.003 = 0.03 USD
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                10_000,
            ))
            .await;
        let dashboard = engine.get_dashboard("t1", "last_30d").await;
        assert!((dashboard.cost_total_usd - 0.03).abs() < 0.0001);
        assert!((dashboard.cost_per_interaction_usd - 0.03).abs() < 0.0001);
    }

    #[tokio::test]
    async fn test_by_agent_breakdown() {
        let engine = AnalyticsEngine::new();
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
            ))
            .await;
        engine
            .record_interaction(make_interaction(
                "t1",
                "support",
                "email",
                InteractionOutcome::Escalated,
                1000,
                200,
            ))
            .await;
        let dashboard = engine.get_dashboard("t1", "last_30d").await;
        assert_eq!(dashboard.by_agent.len(), 2);
        assert_eq!(dashboard.by_agent["sales"].interactions, 1);
        assert_eq!(dashboard.by_agent["support"].interactions, 1);
    }

    #[tokio::test]
    async fn test_by_channel_breakdown() {
        let engine = AnalyticsEngine::new();
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
            ))
            .await;
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "email",
                InteractionOutcome::Resolved,
                700,
                100,
            ))
            .await;
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Escalated,
                600,
                100,
            ))
            .await;
        let dashboard = engine.get_dashboard("t1", "last_30d").await;
        assert_eq!(dashboard.by_channel.len(), 2);
        assert_eq!(dashboard.by_channel["chat"].interactions, 2);
        assert_eq!(dashboard.by_channel["email"].interactions, 1);
    }

    #[tokio::test]
    async fn test_agent_performance() {
        let engine = AnalyticsEngine::new();
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
            ))
            .await;
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "email",
                InteractionOutcome::Escalated,
                700,
                200,
            ))
            .await;
        engine
            .record_quality_score(make_quality("t1", "sales", 0.9))
            .await;

        let perf = engine.get_agent_performance("t1", "sales").await;
        assert_eq!(perf.total_calls, 2);
        assert!((perf.resolution_rate - 0.5).abs() < f64::EPSILON);
        assert!((perf.escalation_rate - 0.5).abs() < f64::EPSILON);
        assert_eq!(perf.avg_duration_ms, 600);
        assert!((perf.avg_quality - 0.9).abs() < 0.001);
        assert_eq!(perf.top_topics.len(), 2);
    }

    #[tokio::test]
    async fn test_agent_performance_empty() {
        let engine = AnalyticsEngine::new();
        let perf = engine.get_agent_performance("t1", "nonexistent").await;
        assert_eq!(perf.total_calls, 0);
        assert_eq!(perf.avg_quality, 0.0);
        assert_eq!(perf.resolution_rate, 0.0);
    }

    #[tokio::test]
    async fn test_conversion_funnel() {
        let engine = AnalyticsEngine::new();
        // Simulate a funnel
        for _ in 0..100 {
            engine
                .record_funnel_event("t1", FunnelStage::Lead, None)
                .await;
        }
        for _ in 0..60 {
            engine
                .record_funnel_event("t1", FunnelStage::Qualified, None)
                .await;
        }
        for _ in 0..40 {
            engine
                .record_funnel_event("t1", FunnelStage::Contacted, None)
                .await;
        }
        for _ in 0..25 {
            engine
                .record_funnel_event("t1", FunnelStage::Responded, None)
                .await;
        }
        for _ in 0..10 {
            engine
                .record_funnel_event("t1", FunnelStage::DemoScheduled, None)
                .await;
        }
        for _ in 0..5 {
            engine
                .record_funnel_event("t1", FunnelStage::Converted, Some(14.0))
                .await;
        }

        let funnel = engine.get_conversion_funnel("t1").await;
        assert_eq!(funnel.total_leads, 100);
        assert_eq!(funnel.qualified, 60);
        assert_eq!(funnel.contacted, 40);
        assert_eq!(funnel.responded, 25);
        assert_eq!(funnel.demo_scheduled, 10);
        assert_eq!(funnel.converted, 5);
        assert!((funnel.conversion_rate - 0.05).abs() < f64::EPSILON);
        assert!((funnel.avg_time_to_conversion_days - 14.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_conversion_funnel_empty() {
        let engine = AnalyticsEngine::new();
        let funnel = engine.get_conversion_funnel("t1").await;
        assert_eq!(funnel.total_leads, 0);
        assert_eq!(funnel.conversion_rate, 0.0);
        assert_eq!(funnel.avg_time_to_conversion_days, 0.0);
    }

    #[tokio::test]
    async fn test_daily_trend_ordering() {
        let engine = AnalyticsEngine::new();
        let now = Utc::now();
        let yesterday = now - chrono::Duration::days(1);
        let two_days_ago = now - chrono::Duration::days(2);

        engine
            .record_interaction(make_interaction_at(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
                two_days_ago,
            ))
            .await;
        engine
            .record_interaction(make_interaction_at(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Escalated,
                700,
                200,
                yesterday,
            ))
            .await;
        engine
            .record_interaction(make_interaction_at(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                300,
                50,
                now,
            ))
            .await;

        let trends = engine.get_trends("t1", 7).await;
        assert!(trends.len() >= 2); // at least 2 distinct days (yesterday and today or 3)
                                    // Verify chronological order
        for window in trends.windows(2) {
            assert!(window[0].date <= window[1].date);
        }
    }

    #[tokio::test]
    async fn test_daily_trend_metrics() {
        let engine = AnalyticsEngine::new();
        let now = Utc::now();

        engine
            .record_interaction(make_interaction_at(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                1000,
                now,
            ))
            .await;
        engine
            .record_interaction(make_interaction_at(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Escalated,
                700,
                2000,
                now,
            ))
            .await;
        engine
            .record_quality_score(make_quality_at("t1", "sales", 0.85, now))
            .await;

        let trends = engine.get_trends("t1", 1).await;
        assert!(!trends.is_empty());
        let today = &trends[trends.len() - 1];
        assert_eq!(today.interactions, 2);
        assert_eq!(today.resolutions, 1);
        assert_eq!(today.escalations, 1);
        assert!((today.avg_quality - 0.85).abs() < 0.001);
        // cost = (3000 / 1000) * 0.003 = 0.009
        assert!((today.cost_usd - 0.009).abs() < 0.0001);
    }

    #[tokio::test]
    async fn test_period_filtering() {
        let engine = AnalyticsEngine::new();
        let now = Utc::now();
        let old = now - chrono::Duration::days(60);

        engine
            .record_interaction(make_interaction_at(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
                old,
            ))
            .await;
        engine
            .record_interaction(make_interaction_at(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
                now,
            ))
            .await;

        let dashboard_7 = engine.get_dashboard("t1", "last_7d").await;
        assert_eq!(dashboard_7.total_interactions, 1);

        let dashboard_90 = engine.get_dashboard("t1", "last_90d").await;
        assert_eq!(dashboard_90.total_interactions, 2);
    }

    #[tokio::test]
    async fn test_multi_tenant_isolation() {
        let engine = AnalyticsEngine::new();
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
            ))
            .await;
        engine
            .record_interaction(make_interaction(
                "t2",
                "support",
                "email",
                InteractionOutcome::Escalated,
                700,
                200,
            ))
            .await;

        let d1 = engine.get_dashboard("t1", "last_30d").await;
        let d2 = engine.get_dashboard("t2", "last_30d").await;
        assert_eq!(d1.total_interactions, 1);
        assert_eq!(d2.total_interactions, 1);
        assert!((d1.resolution_rate - 1.0).abs() < f64::EPSILON);
        assert!((d2.resolution_rate - 0.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_default_period_parsing() {
        // "last_30d" → 30 days
        let cutoff = period_to_cutoff("last_30d");
        let expected = Utc::now() - chrono::Duration::days(30);
        let diff = (cutoff - expected).num_seconds().abs();
        assert!(diff < 2); // within 2 seconds
    }

    #[tokio::test]
    async fn test_invalid_period_defaults_to_30d() {
        let cutoff = period_to_cutoff("invalid");
        let expected = Utc::now() - chrono::Duration::days(30);
        let diff = (cutoff - expected).num_seconds().abs();
        assert!(diff < 2);
    }

    #[tokio::test]
    async fn test_interaction_outcome_serialization() {
        let resolved = serde_json::to_string(&InteractionOutcome::Resolved).unwrap();
        assert_eq!(resolved, "\"Resolved\"");
        let escalated: InteractionOutcome = serde_json::from_str("\"Escalated\"").unwrap();
        assert_eq!(escalated, InteractionOutcome::Escalated);
    }

    #[tokio::test]
    async fn test_engine_clone() {
        let engine = AnalyticsEngine::new();
        engine
            .record_interaction(make_interaction(
                "t1",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
            ))
            .await;
        let cloned = engine.clone();
        let dashboard = cloned.get_dashboard("t1", "last_30d").await;
        assert_eq!(dashboard.total_interactions, 1);
    }

    // -- HTTP handler tests ------------------------------------------------

    #[tokio::test]
    async fn test_dashboard_endpoint() {
        let state = make_state();
        state
            .engine
            .record_interaction(make_interaction(
                "acme",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
            ))
            .await;

        let app = analytics_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/analytics/acme/dashboard?period=last_30d")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let dashboard: AnalyticsDashboard = serde_json::from_slice(&body).unwrap();
        assert_eq!(dashboard.tenant_id, "acme");
        assert_eq!(dashboard.total_interactions, 1);
    }

    #[tokio::test]
    async fn test_agent_performance_endpoint() {
        let state = make_state();
        state
            .engine
            .record_interaction(make_interaction(
                "acme",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
            ))
            .await;
        state
            .engine
            .record_quality_score(make_quality("acme", "sales", 0.9))
            .await;

        let app = analytics_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/analytics/acme/agents/sales")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let perf: AgentPerformance = serde_json::from_slice(&body).unwrap();
        assert_eq!(perf.agent_role, "sales");
        assert_eq!(perf.total_calls, 1);
    }

    #[tokio::test]
    async fn test_funnel_endpoint() {
        let state = make_state();
        state
            .engine
            .record_funnel_event("acme", FunnelStage::Lead, None)
            .await;
        state
            .engine
            .record_funnel_event("acme", FunnelStage::Converted, Some(7.0))
            .await;

        let app = analytics_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/analytics/acme/funnel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let funnel: ConversionFunnel = serde_json::from_slice(&body).unwrap();
        assert_eq!(funnel.total_leads, 1);
        assert_eq!(funnel.converted, 1);
    }

    #[tokio::test]
    async fn test_trends_endpoint() {
        let state = make_state();
        state
            .engine
            .record_interaction(make_interaction(
                "acme",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
            ))
            .await;

        let app = analytics_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/analytics/acme/trends?days=7")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let trends: Vec<DailyMetric> = serde_json::from_slice(&body).unwrap();
        assert!(!trends.is_empty());
    }

    #[tokio::test]
    async fn test_trends_endpoint_default_days() {
        let state = make_state();
        state
            .engine
            .record_interaction(make_interaction(
                "acme",
                "sales",
                "chat",
                InteractionOutcome::Resolved,
                500,
                100,
            ))
            .await;

        let app = analytics_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/analytics/acme/trends")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_quality_criteria_scores() {
        let engine = AnalyticsEngine::new();
        let mut criteria = HashMap::new();
        criteria.insert("accuracy".to_string(), 0.95);
        criteria.insert("clarity".to_string(), 0.85);
        let event = QualityEvent {
            tenant_id: "t1".to_string(),
            agent_role: "sales".to_string(),
            overall_score: 0.9,
            criteria_scores: criteria,
            timestamp: Utc::now(),
        };
        engine.record_quality_score(event).await;
        let dashboard = engine.get_dashboard("t1", "last_30d").await;
        assert!((dashboard.avg_quality_score - 0.9).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_all_outcome_variants() {
        let engine = AnalyticsEngine::new();
        engine
            .record_interaction(make_interaction(
                "t1",
                "a",
                "chat",
                InteractionOutcome::Resolved,
                100,
                10,
            ))
            .await;
        engine
            .record_interaction(make_interaction(
                "t1",
                "a",
                "chat",
                InteractionOutcome::Escalated,
                200,
                20,
            ))
            .await;
        engine
            .record_interaction(make_interaction(
                "t1",
                "a",
                "chat",
                InteractionOutcome::Pending,
                300,
                30,
            ))
            .await;
        engine
            .record_interaction(make_interaction(
                "t1",
                "a",
                "chat",
                InteractionOutcome::Abandoned,
                400,
                40,
            ))
            .await;

        let dashboard = engine.get_dashboard("t1", "last_30d").await;
        assert_eq!(dashboard.total_interactions, 4);
        // Only 1 of 4 resolved
        assert!((dashboard.resolution_rate - 0.25).abs() < f64::EPSILON);
    }
}
