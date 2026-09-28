//! Live run handles: cancellation, follow-up messages, transcript broadcast, and pending permission prompts.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::domain::models::RunEvent;

pub struct RunHandle {
    pub cancel: CancellationToken,
    pub cancel_reason: std::sync::Arc<Mutex<Option<String>>>,
    pub followups: mpsc::UnboundedSender<String>,
    pub events: broadcast::Sender<RunEvent>,
    pub finished: watch::Receiver<bool>,
}

#[derive(Default)]
pub struct Registry {
    runs: Mutex<HashMap<i64, RunHandle>>,
    permissions: Mutex<HashMap<i64, oneshot::Sender<String>>>,
}

pub struct RunChannels {
    pub cancel: CancellationToken,
    pub cancel_reason: std::sync::Arc<Mutex<Option<String>>>,
    pub followups: mpsc::UnboundedReceiver<String>,
    pub events: broadcast::Sender<RunEvent>,
    pub finished: watch::Sender<bool>,
}

impl Registry {
    pub fn register(&self, run_id: i64) -> RunChannels {
        let cancel = CancellationToken::new();
        let (ftx, frx) = mpsc::unbounded_channel();
        let (etx, _) = broadcast::channel(512);
        let (dtx, drx) = watch::channel(false);
        let reason = std::sync::Arc::new(Mutex::new(None));
        self.runs.lock().unwrap().insert(
            run_id,
            RunHandle { cancel: cancel.clone(), cancel_reason: reason.clone(), followups: ftx, events: etx.clone(), finished: drx },
        );
        RunChannels { cancel, cancel_reason: reason, followups: frx, events: etx, finished: dtx }
    }

    pub fn unregister(&self, run_id: i64) {
        self.runs.lock().unwrap().remove(&run_id);
    }

    pub fn is_live(&self, run_id: i64) -> bool {
        self.runs.lock().unwrap().contains_key(&run_id)
    }

    pub fn live_ids(&self) -> Vec<i64> {
        self.runs.lock().unwrap().keys().copied().collect()
    }

    pub fn subscribe(&self, run_id: i64) -> Option<broadcast::Receiver<RunEvent>> {
        self.runs.lock().unwrap().get(&run_id).map(|h| h.events.subscribe())
    }

    pub fn send_followup(&self, run_id: i64, text: String) -> bool {
        self.runs.lock().unwrap().get(&run_id).is_some_and(|h| h.followups.send(text).is_ok())
    }

    pub fn cancel(&self, run_id: i64, reason: &str) -> bool {
        let runs = self.runs.lock().unwrap();
        match runs.get(&run_id) {
            Some(h) => {
                *h.cancel_reason.lock().unwrap() = Some(reason.to_string());
                h.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Cancel and wait until the run task has finished cleanup. Returns false if the run wasn't live
    /// or didn't stop within `timeout`.
    pub async fn cancel_and_wait(&self, run_id: i64, reason: &str, timeout: Duration) -> bool {
        let finished = {
            let runs = self.runs.lock().unwrap();
            match runs.get(&run_id) {
                Some(h) => {
                    *h.cancel_reason.lock().unwrap() = Some(reason.to_string());
                    h.cancel.cancel();
                    h.finished.clone()
                }
                None => return false,
            }
        };
        let mut f = finished;
        tokio::time::timeout(timeout, async move {
            while !*f.borrow() {
                if f.changed().await.is_err() {
                    break;
                }
            }
        })
        .await
        .is_ok()
    }

    pub fn add_permission_waiter(&self, id: i64, tx: oneshot::Sender<String>) {
        self.permissions.lock().unwrap().insert(id, tx);
    }

    pub fn answer_permission(&self, id: i64, option_id: String) -> bool {
        match self.permissions.lock().unwrap().remove(&id) {
            Some(tx) => tx.send(option_id).is_ok(),
            None => false,
        }
    }

    pub fn drop_permission_waiter(&self, id: i64) {
        self.permissions.lock().unwrap().remove(&id);
    }
}
