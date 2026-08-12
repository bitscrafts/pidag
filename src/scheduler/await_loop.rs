//! Completion-signal waiting logic for `await_dag` / `wait_any`.
//!
//! Both loops share the same shape: check the current snapshot first (so an
//! already-terminal condition returns immediately, no wait at all), then
//! park on `watch::Receiver::changed()` until the next update or the
//! deadline. There is no `sleep`/poll anywhere in this file.

use super::{AwaitOutcome, NodeStatus, ResumeToken, Scheduler, Snapshot};
use std::time::Duration;
use tokio::sync::watch;

impl Scheduler {
    pub(super) async fn wait_for_done(
        mut rx: watch::Receiver<Snapshot>,
        timeout: Option<Duration>,
    ) -> AwaitOutcome {
        let deadline = timeout.map(|d| tokio::time::Instant::now() + d);
        loop {
            if let Some(report) = Self::done_report(&rx) {
                return AwaitOutcome::Done(report);
            }

            let wait_result = match deadline {
                Some(dl) => {
                    let remaining = dl.saturating_duration_since(tokio::time::Instant::now());
                    match tokio::time::timeout(remaining, rx.changed()).await {
                        Ok(res) => res,
                        Err(_elapsed) => return AwaitOutcome::Running(ResumeToken(rx)),
                    }
                }
                None => rx.changed().await,
            };

            if wait_result.is_err() {
                // Sender dropped (run task ended without a final mark, e.g.
                // panicked). Report whatever we have rather than hanging.
                if let Some(report) = Self::done_report(&rx) {
                    return AwaitOutcome::Done(report);
                }
                return AwaitOutcome::Running(ResumeToken(rx));
            }
        }
    }

    pub(super) async fn wait_any_inner(
        &mut self,
        mut rx: watch::Receiver<Snapshot>,
        timeout: Option<Duration>,
    ) -> AwaitOutcome {
        let deadline = timeout.map(|d| tokio::time::Instant::now() + d);
        loop {
            if let Some(outcome) = self.next_terminal_node(&rx) {
                return outcome;
            }

            let wait_result = match deadline {
                Some(dl) => {
                    let remaining = dl.saturating_duration_since(tokio::time::Instant::now());
                    match tokio::time::timeout(remaining, rx.changed()).await {
                        Ok(res) => res,
                        Err(_elapsed) => return AwaitOutcome::Running(ResumeToken(rx)),
                    }
                }
                None => rx.changed().await,
            };

            if wait_result.is_err() {
                if let Some(outcome) = self.next_terminal_node(&rx) {
                    return outcome;
                }
                return AwaitOutcome::Running(ResumeToken(rx));
            }
        }
    }

    fn done_report(rx: &watch::Receiver<Snapshot>) -> Option<super::RunReport> {
        let snap = rx.borrow();
        if snap.dag_done {
            snap.report.clone()
        } else {
            None
        }
    }

    /// Find the first terminal node not yet handed back by `wait_any`. On
    /// exhaustion, if the DAG is fully done, returns `Done` instead of
    /// hanging forever waiting for a node that will never come.
    fn next_terminal_node(&mut self, rx: &watch::Receiver<Snapshot>) -> Option<AwaitOutcome> {
        let snap = rx.borrow().clone();

        for node_id in &snap.terminal_order {
            if self.consumed.contains(node_id) {
                continue;
            }
            self.consumed.insert(node_id.clone());
            let node_state = snap.node_state.get(node_id);
            let still_running: Vec<String> = self
                .inner
                .all_node_ids
                .iter()
                .filter(|id| !snap.terminal_order.contains(id))
                .cloned()
                .collect();
            return Some(AwaitOutcome::Node {
                node_id: node_id.clone(),
                state: node_state.map(|s| s.state).unwrap_or(NodeStatus::Pending),
                output: node_state.and_then(|s| s.output.clone()),
                still_running,
            });
        }

        if snap.dag_done
            && let Some(report) = snap.report
        {
            return Some(AwaitOutcome::Done(report));
        }

        None
    }
}
