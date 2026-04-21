//! Usage-based billing integration.
//!
//! Provides a `BillingProvider` trait abstracted over Stripe/Paddle, plus a
//! `UsageMeter` that aggregates run costs into line items. All stubs —
//! production wires the `StripeProvider` impl under a feature flag and
//! persists invoices to PostgreSQL.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use thiserror::Error;
use uuid::Uuid;

/// A single line on an invoice — one per billable dimension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvoiceLineItem {
    /// Short description ("Agent runs", "Tokens", "Storage").
    pub description: String,
    /// Unit count (runs, 1K-token units, MB-months).
    pub quantity: u64,
    /// Price per unit in USD.
    pub unit_price_usd: f64,
    /// Subtotal for this line (quantity × unit_price).
    pub subtotal_usd: f64,
}

impl InvoiceLineItem {
    /// Construct a line item; `subtotal_usd` is derived.
    pub fn new(description: impl Into<String>, quantity: u64, unit_price_usd: f64) -> Self {
        let subtotal_usd = (quantity as f64) * unit_price_usd;
        Self {
            description: description.into(),
            quantity,
            unit_price_usd,
            subtotal_usd,
        }
    }
}

/// An invoice for one tenant for one billing period.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Invoice {
    /// Invoice UUID.
    pub id: String,
    /// Tenant the invoice is for.
    pub tenant_id: String,
    /// Period start (UTC).
    pub period_start: DateTime<Utc>,
    /// Period end (UTC).
    pub period_end: DateTime<Utc>,
    /// Line items.
    pub lines: Vec<InvoiceLineItem>,
    /// Sum of all line subtotals.
    pub total_usd: f64,
    /// Whether the invoice is paid.
    pub paid: bool,
}

impl Invoice {
    /// Build an invoice from lines; total is auto-summed.
    pub fn new(
        tenant_id: impl Into<String>,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
        lines: Vec<InvoiceLineItem>,
    ) -> Self {
        let total_usd = lines.iter().map(|l| l.subtotal_usd).sum();
        Self {
            id: Uuid::new_v4().to_string(),
            tenant_id: tenant_id.into(),
            period_start,
            period_end,
            lines,
            total_usd,
            paid: false,
        }
    }
}

/// Errors surfaced by the billing subsystem.
#[derive(Debug, Error)]
pub enum BillingError {
    /// Underlying provider (Stripe/Paddle) returned an error.
    #[error("Provider error: {0}")]
    Provider(String),
    /// Invoice was not found.
    #[error("Invoice {0} not found")]
    NotFound(String),
    /// Provider rejected the charge (insufficient funds, declined card).
    #[error("Payment declined: {0}")]
    Declined(String),
}

/// Abstraction over payment providers (Stripe, Paddle, manual wire).
#[async_trait]
pub trait BillingProvider: Send + Sync {
    /// Charge an invoice. Returns `Ok(())` if the provider accepts it.
    async fn charge(&self, invoice: &Invoice) -> Result<(), BillingError>;

    /// Provider name for audit logging.
    fn name(&self) -> &'static str;
}

/// Stub provider that always succeeds. Replace with real Stripe/Paddle impl
/// behind `#[cfg(feature = "stripe")]`.
pub struct StubBillingProvider;

#[async_trait]
impl BillingProvider for StubBillingProvider {
    async fn charge(&self, _invoice: &Invoice) -> Result<(), BillingError> {
        Ok(())
    }
    fn name(&self) -> &'static str {
        "stub"
    }
}

/// Aggregates tenant usage → invoices.
///
/// TODO: persist rates + invoices to PostgreSQL. Wire Stripe webhook handler
/// for payment status sync.
pub struct UsageMeter {
    /// Price per agent run in USD (0.0 if runs are free and only tokens bill).
    pub price_per_run_usd: f64,
    /// Price per 1K tokens in USD.
    pub price_per_1k_tokens_usd: f64,
    /// Price per MB-month of storage.
    pub price_per_mb_month_usd: f64,
    invoices: RwLock<HashMap<String, Invoice>>,
}

impl UsageMeter {
    /// Construct a meter with default indicative pricing.
    pub fn new() -> Self {
        Self {
            price_per_run_usd: 0.0, // free, tokens carry the cost
            price_per_1k_tokens_usd: 0.002,
            price_per_mb_month_usd: 0.02,
            invoices: RwLock::new(HashMap::new()),
        }
    }

    /// Build (and store) an invoice from raw usage numbers.
    pub fn build_invoice(
        &self,
        tenant_id: &str,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
        runs: u64,
        tokens: u64,
        storage_mb: u64,
    ) -> Invoice {
        let lines = vec![
            InvoiceLineItem::new("Agent runs", runs, self.price_per_run_usd),
            InvoiceLineItem::new(
                "Tokens (per 1K)",
                tokens / 1_000,
                self.price_per_1k_tokens_usd,
            ),
            InvoiceLineItem::new(
                "Storage (MB-month)",
                storage_mb,
                self.price_per_mb_month_usd,
            ),
        ];
        let invoice = Invoice::new(tenant_id, period_start, period_end, lines);
        if let Ok(mut guard) = self.invoices.write() {
            guard.insert(invoice.id.clone(), invoice.clone());
        }
        invoice
    }

    /// Fetch a stored invoice by id.
    pub fn get_invoice(&self, id: &str) -> Option<Invoice> {
        self.invoices.read().ok()?.get(id).cloned()
    }

    /// Mark an invoice paid.
    pub fn mark_paid(&self, id: &str) -> Result<(), BillingError> {
        let mut guard = self
            .invoices
            .write()
            .map_err(|e| BillingError::Provider(e.to_string()))?;
        let inv = guard
            .get_mut(id)
            .ok_or_else(|| BillingError::NotFound(id.to_string()))?;
        inv.paid = true;
        Ok(())
    }

    /// All invoices stored (unpaged — production paginates).
    pub fn all_invoices(&self) -> Vec<Invoice> {
        self.invoices
            .read()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Invoices for a specific tenant.
    pub fn for_tenant(&self, tenant_id: &str) -> Vec<Invoice> {
        self.invoices
            .read()
            .map(|g| {
                g.values()
                    .filter(|i| i.tenant_id == tenant_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Default for UsageMeter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn period() -> (DateTime<Utc>, DateTime<Utc>) {
        let end = Utc::now();
        let start = end - Duration::days(30);
        (start, end)
    }

    #[test]
    fn line_item_computes_subtotal() {
        let l = InvoiceLineItem::new("Runs", 100, 0.01);
        assert!((l.subtotal_usd - 1.0).abs() < 1e-9);
    }

    #[test]
    fn invoice_sums_lines() {
        let (s, e) = period();
        let inv = Invoice::new(
            "t1",
            s,
            e,
            vec![
                InvoiceLineItem::new("A", 10, 1.0),
                InvoiceLineItem::new("B", 5, 2.0),
            ],
        );
        assert!((inv.total_usd - 20.0).abs() < 1e-9);
    }

    #[test]
    fn invoice_has_uuid() {
        let (s, e) = period();
        let inv = Invoice::new("t1", s, e, vec![]);
        assert!(!inv.id.is_empty());
        assert!(!inv.paid);
    }

    #[test]
    fn build_invoice_stores() {
        let m = UsageMeter::new();
        let (s, e) = period();
        let inv = m.build_invoice("t1", s, e, 100, 50_000, 10);
        assert!(m.get_invoice(&inv.id).is_some());
    }

    #[test]
    fn build_invoice_line_count() {
        let m = UsageMeter::new();
        let (s, e) = period();
        let inv = m.build_invoice("t1", s, e, 100, 50_000, 10);
        assert_eq!(inv.lines.len(), 3);
    }

    #[test]
    fn mark_paid_updates_flag() {
        let m = UsageMeter::new();
        let (s, e) = period();
        let inv = m.build_invoice("t1", s, e, 1, 0, 0);
        m.mark_paid(&inv.id).unwrap();
        assert!(m.get_invoice(&inv.id).unwrap().paid);
    }

    #[test]
    fn mark_paid_missing_errors() {
        let m = UsageMeter::new();
        assert!(matches!(
            m.mark_paid("nope"),
            Err(BillingError::NotFound(_))
        ));
    }

    #[test]
    fn for_tenant_filters() {
        let m = UsageMeter::new();
        let (s, e) = period();
        m.build_invoice("t1", s, e, 1, 0, 0);
        m.build_invoice("t2", s, e, 1, 0, 0);
        assert_eq!(m.for_tenant("t1").len(), 1);
    }

    #[test]
    fn all_invoices_returns_everything() {
        let m = UsageMeter::new();
        let (s, e) = period();
        m.build_invoice("t1", s, e, 1, 0, 0);
        m.build_invoice("t2", s, e, 1, 0, 0);
        assert_eq!(m.all_invoices().len(), 2);
    }

    #[tokio::test]
    async fn stub_provider_always_succeeds() {
        let p = StubBillingProvider;
        let (s, e) = period();
        let inv = Invoice::new("t1", s, e, vec![]);
        assert!(p.charge(&inv).await.is_ok());
        assert_eq!(p.name(), "stub");
    }

    #[test]
    fn invoice_serde_roundtrip() {
        let (s, e) = period();
        let inv = Invoice::new("t1", s, e, vec![InvoiceLineItem::new("X", 1, 1.0)]);
        let json = serde_json::to_string(&inv).unwrap();
        let back: Invoice = serde_json::from_str(&json).unwrap();
        assert_eq!(back, inv);
    }

    #[test]
    fn pricing_scales_with_tokens() {
        let m = UsageMeter::new();
        let (s, e) = period();
        let small = m.build_invoice("t1", s, e, 0, 1_000, 0);
        let large = m.build_invoice("t2", s, e, 0, 100_000, 0);
        assert!(large.total_usd > small.total_usd);
    }

    #[test]
    fn pricing_includes_storage() {
        let m = UsageMeter::new();
        let (s, e) = period();
        let inv = m.build_invoice("t1", s, e, 0, 0, 100);
        assert!(inv.total_usd > 0.0);
    }

    #[test]
    fn default_meter_is_priced() {
        let m = UsageMeter::default();
        assert!(m.price_per_1k_tokens_usd > 0.0);
    }

    #[test]
    fn zero_usage_zero_cost() {
        let m = UsageMeter::new();
        let (s, e) = period();
        let inv = m.build_invoice("t1", s, e, 0, 0, 0);
        assert_eq!(inv.total_usd, 0.0);
    }

    #[test]
    fn billing_error_displays() {
        let e = BillingError::Declined("insufficient funds".into());
        assert!(format!("{e}").contains("declined"));
    }
}
