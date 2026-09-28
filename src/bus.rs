//! In-process event bus: domain events for SSE clients and a wake-up signal for the scheduler.

use std::sync::Arc;

use serde::Serialize;
use tokio::sync::{Notify, broadcast};
use utoipa::ToSchema;

/// A change notification sent to UI clients over `/api/events/stream`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DomainEvent {
    /// e.g. `issue.updated`, `issue.created`, `pr.updated`, `run.updated`, `limits.updated`, `settings.updated`.
    #[serde(rename = "type")]
    pub kind: String,
    pub project: Option<String>,
    pub issue: Option<i64>,
    pub pr: Option<i64>,
    pub run: Option<i64>,
}

#[derive(Clone)]
pub struct Bus {
    tx: broadcast::Sender<DomainEvent>,
    /// Signals the scheduler that something changed.
    pub wake: Arc<Notify>,
    /// Signals the git ref scanner to run now.
    pub scan: Arc<Notify>,
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Bus { tx, wake: Arc::new(Notify::new()), scan: Arc::new(Notify::new()) }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DomainEvent> {
        self.tx.subscribe()
    }

    pub fn emit(&self, kind: &str, project: Option<&str>, issue: Option<i64>, pr: Option<i64>, run: Option<i64>) {
        let _ = self.tx.send(DomainEvent { kind: kind.to_string(), project: project.map(str::to_string), issue, pr, run });
        self.wake.notify_one();
    }

    pub fn issue(&self, project: &str, number: i64) {
        self.emit("issue.updated", Some(project), Some(number), None, None);
    }
    pub fn pr(&self, project: &str, number: i64) {
        self.emit("pr.updated", Some(project), None, Some(number), None);
    }
    pub fn run(&self, project: Option<&str>, run: i64) {
        self.emit("run.updated", project, None, None, Some(run));
    }
}
