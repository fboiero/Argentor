//! Multi-tenant work scheduler.
//!
//! Minimal priority queue for cron-style jobs, keyed by tenant. In
//! production this would back onto Redis Streams / SQS / Temporal. Kept as
//! an in-memory `BinaryHeap` to keep the scaffolding self-contained.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::RwLock;
use thiserror::Error;
use uuid::Uuid;

/// A scheduled job — runs an agent on behalf of a tenant at or after
/// `run_at`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    /// Job UUID.
    pub id: String,
    /// Tenant that owns the job.
    pub tenant_id: String,
    /// Agent configuration id to invoke.
    pub agent_id: String,
    /// Prompt / message to feed the agent.
    pub payload: String,
    /// Earliest UTC time the job may run.
    pub run_at: DateTime<Utc>,
    /// Priority (higher = sooner within same timestamp).
    pub priority: u8,
}

impl ScheduledJob {
    /// Construct a new job.
    pub fn new(
        tenant_id: impl Into<String>,
        agent_id: impl Into<String>,
        payload: impl Into<String>,
        run_at: DateTime<Utc>,
        priority: u8,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            tenant_id: tenant_id.into(),
            agent_id: agent_id.into(),
            payload: payload.into(),
            run_at,
            priority,
        }
    }
}

// Ordering: earliest `run_at` first; within equal timestamps, higher
// priority first. `BinaryHeap` is a max-heap, so we invert the timestamp
// comparison.
impl PartialEq for ScheduledJob {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for ScheduledJob {}
impl Ord for ScheduledJob {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .run_at
            .cmp(&self.run_at)
            .then_with(|| self.priority.cmp(&other.priority))
    }
}
impl PartialOrd for ScheduledJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Errors raised by the scheduler.
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// Job id not found.
    #[error("Job {0} not found")]
    NotFound(String),
    /// Scheduler has been shut down.
    #[error("Scheduler closed")]
    Closed,
}

/// In-memory scheduler. One shared queue — tenant fairness can be layered
/// on top later (weighted fair queueing, token bucket, etc.).
///
/// TODO: add per-tenant fairness (WFQ); replace with Redis Streams backend.
pub struct CloudScheduler {
    queue: RwLock<BinaryHeap<ScheduledJob>>,
}

impl CloudScheduler {
    /// Create an empty scheduler.
    pub fn new() -> Self {
        Self {
            queue: RwLock::new(BinaryHeap::new()),
        }
    }

    /// Enqueue a job. Returns the job id.
    pub fn enqueue(&self, job: ScheduledJob) -> Result<String, SchedulerError> {
        let id = job.id.clone();
        self.queue
            .write()
            .map_err(|_| SchedulerError::Closed)?
            .push(job);
        Ok(id)
    }

    /// Pop the next due job (earliest `run_at` at or before `now`).
    ///
    /// Returns `None` if the queue is empty or the head is not due yet.
    pub fn pop_due(&self, now: DateTime<Utc>) -> Option<ScheduledJob> {
        let mut q = self.queue.write().ok()?;
        if let Some(head) = q.peek() {
            if head.run_at <= now {
                return q.pop();
            }
        }
        None
    }

    /// Pop the next job regardless of `run_at` (for tests / draining).
    pub fn pop(&self) -> Option<ScheduledJob> {
        self.queue.write().ok()?.pop()
    }

    /// Current queue depth.
    pub fn len(&self) -> usize {
        self.queue.read().map(|q| q.len()).unwrap_or(0)
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// All queued jobs for a given tenant (diagnostic / admin use).
    pub fn jobs_for_tenant(&self, tenant_id: &str) -> Vec<ScheduledJob> {
        self.queue
            .read()
            .map(|q| {
                q.iter()
                    .filter(|j| j.tenant_id == tenant_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Default for CloudScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn job(tenant: &str, offset_secs: i64, priority: u8) -> ScheduledJob {
        ScheduledJob::new(
            tenant,
            "agent-1",
            "msg",
            Utc::now() + Duration::seconds(offset_secs),
            priority,
        )
    }

    #[test]
    fn new_scheduler_is_empty() {
        let s = CloudScheduler::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn enqueue_returns_id() {
        let s = CloudScheduler::new();
        let j = job("t1", 0, 0);
        let id = s.enqueue(j.clone()).unwrap();
        assert_eq!(id, j.id);
    }

    #[test]
    fn enqueue_increments_length() {
        let s = CloudScheduler::new();
        s.enqueue(job("t1", 0, 0)).unwrap();
        s.enqueue(job("t1", 0, 0)).unwrap();
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn pop_due_returns_earliest() {
        let s = CloudScheduler::new();
        let later = job("t1", 60, 0);
        let sooner = job("t1", -10, 0);
        s.enqueue(later.clone()).unwrap();
        s.enqueue(sooner.clone()).unwrap();
        let popped = s.pop_due(Utc::now()).unwrap();
        assert_eq!(popped.id, sooner.id);
    }

    #[test]
    fn pop_due_skips_future_jobs() {
        let s = CloudScheduler::new();
        s.enqueue(job("t1", 3_600, 0)).unwrap();
        assert!(s.pop_due(Utc::now()).is_none());
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn pop_due_respects_priority_on_ties() {
        let s = CloudScheduler::new();
        let now = Utc::now();
        let low = ScheduledJob::new("t1", "a", "m", now, 1);
        let high = ScheduledJob::new("t1", "a", "m", now, 9);
        s.enqueue(low.clone()).unwrap();
        s.enqueue(high.clone()).unwrap();
        let popped = s.pop_due(now).unwrap();
        assert_eq!(popped.id, high.id);
    }

    #[test]
    fn pop_drains_queue() {
        let s = CloudScheduler::new();
        s.enqueue(job("t1", 0, 0)).unwrap();
        s.enqueue(job("t1", 0, 0)).unwrap();
        assert!(s.pop().is_some());
        assert!(s.pop().is_some());
        assert!(s.pop().is_none());
    }

    #[test]
    fn jobs_for_tenant_filters() {
        let s = CloudScheduler::new();
        s.enqueue(job("t1", 0, 0)).unwrap();
        s.enqueue(job("t2", 0, 0)).unwrap();
        s.enqueue(job("t1", 0, 0)).unwrap();
        assert_eq!(s.jobs_for_tenant("t1").len(), 2);
        assert_eq!(s.jobs_for_tenant("t2").len(), 1);
    }

    #[test]
    fn scheduled_job_has_uuid() {
        let j = job("t1", 0, 0);
        assert!(!j.id.is_empty());
    }

    #[test]
    fn scheduled_job_serde_roundtrip() {
        let j = job("t1", 0, 5);
        let json = serde_json::to_string(&j).unwrap();
        let back: ScheduledJob = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, j.id);
        assert_eq!(back.priority, j.priority);
    }

    #[test]
    fn empty_after_draining() {
        let s = CloudScheduler::new();
        s.enqueue(job("t1", 0, 0)).unwrap();
        s.pop();
        assert!(s.is_empty());
    }

    #[test]
    fn pop_due_on_empty_returns_none() {
        let s = CloudScheduler::new();
        assert!(s.pop_due(Utc::now()).is_none());
    }

    #[test]
    fn multiple_tenants_share_queue() {
        let s = CloudScheduler::new();
        for i in 0..10 {
            s.enqueue(job(&format!("t{i}"), 0, 0)).unwrap();
        }
        assert_eq!(s.len(), 10);
    }

    #[test]
    fn default_scheduler_is_empty() {
        let s = CloudScheduler::default();
        assert!(s.is_empty());
    }

    #[test]
    fn pop_due_chain_drains_due_jobs() {
        let s = CloudScheduler::new();
        let now = Utc::now();
        s.enqueue(ScheduledJob::new(
            "t1",
            "a",
            "m",
            now - Duration::seconds(10),
            0,
        ))
        .unwrap();
        s.enqueue(ScheduledJob::new(
            "t1",
            "a",
            "m",
            now - Duration::seconds(5),
            0,
        ))
        .unwrap();
        s.enqueue(ScheduledJob::new(
            "t1",
            "a",
            "m",
            now + Duration::seconds(60),
            0,
        ))
        .unwrap();
        let first = s.pop_due(now).unwrap();
        let second = s.pop_due(now).unwrap();
        assert!(first.run_at <= second.run_at);
        assert!(s.pop_due(now).is_none());
    }
}
