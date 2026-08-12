//! The Phase-1 scheduling loop (topo-ordered dispatch with retry/fallback),
//! extracted so it can run either inline (via `Scheduler::run`) or spawned
//! in the background (via `Scheduler::ensure_started`). Every terminal
//! transition is published to `Inner` so `await_dag`/`wait_any` callers
//! never need to poll.

use super::{Inner, NodeState, NodeStatus, RunReport, Scheduler};
use crate::core::dag::{Dag, ModelRef, Node, QuorumConfig, Verify};
use crate::core::error::PidagError;
use crate::core::event::{Event, EventSink};
use crate::worker::{Worker, WorkerOutput};
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::{Semaphore, mpsc};

/// Typed dispatch event replacing the colon-delimited string protocol.
/// Encodes retry attempts, backoff retries, and model fallbacks with type safety.
#[derive(Debug, Clone)]
pub enum DispatchEvent {
    /// A retry of the current model after a transient failure.
    Retry { attempt: usize },
    /// A retry with exponential backoff after a transient failure.
    BackoffRetry { attempt: usize },
    /// A fallback to a different model from a previous one.
    Fallback { from: String, to: String },
    /// Retries exhausted on a model after a retryable (429/503/quota) failure.
    ///
    /// The colon-delimited protocol emitted this as `retry_exhausted:..` and the
    /// consumer's match had no arm for it, so it fell through a catch-all and was
    /// silently discarded. Making it a variant forces the consumer to say so
    /// explicitly rather than losing it by omission.
    RetryExhausted { model: String },
    /// A `Verify::Critic` arm was dispatched to a worker (spec-37, C10). Only
    /// pushed once the paid-model gate has cleared and a real dispatch is
    /// about to happen — never for a critic skipped by `allow_paid` (C7) or
    /// short-circuited by an earlier `All` arm (C6b) — so its presence in
    /// the event log means a verification step actually spent a model call,
    /// not just that one was configured.
    CriticDispatched { model: String },
}

impl Scheduler {
    pub(super) async fn execute(
        dag: Dag,
        worker: Arc<dyn Worker>,
        event_sink: Arc<tokio::sync::Mutex<Box<dyn EventSink>>>,
        concurrency: usize,
        inner: Arc<Inner>,
        allow_paid: bool,
        checkpoint: Option<&crate::scheduler::Checkpoint>,
    ) -> Result<RunReport, PidagError> {
        // spec-38 F5/G4: expand for_each/quorum at load, before validation,
        // so the scheduler below executes a fixed topology it did not
        // choose. Idempotent (Dag::expand, N1) -- safe even if a caller
        // (e.g. `pidag run`) already expanded and persisted this same dag.
        let dag = dag.expand()?;

        // Validate DAG
        dag.validate()?;

        // T3: Create an unbounded mpsc channel for events. The scheduler sends and moves on;
        // a single consumer task owns the sink.
        let (tx, rx) = mpsc::unbounded_channel::<Event>();
        let write_failures = Arc::new(AtomicUsize::new(0));
        let write_failures_clone = Arc::clone(&write_failures);

        // Spawn the consumer task that owns the sink and processes events
        let consumer_handle = tokio::spawn(async move {
            let mut rx = rx;
            let mut sink = event_sink.lock().await;
            while let Some(event) = rx.recv().await {
                if let Err(e) = sink.emit(&event).await {
                    eprintln!("Event emission failed: {}", e);
                    write_failures_clone.fetch_add(1, Ordering::SeqCst);
                }
            }
            // Channel is drained, consumer task ends
        });

        // Emit DAG submitted
        tx.send(Event::DagSubmitted)
            .map_err(|_| PidagError::WorkerFailed)?;

        // Initialize state for all nodes
        let mut node_state: HashMap<String, NodeState> = dag
            .nodes
            .iter()
            .map(|n| {
                (
                    n.id.clone(),
                    NodeState {
                        node_id: n.id.clone(),
                        state: NodeStatus::Pending,
                        model: None,
                        attempts: 0,
                        output: None,
                    },
                )
            })
            .collect();

        // Build in-degree map and adjacency (only for depends_on, not after)
        let mut in_degree: HashMap<String, usize> =
            dag.nodes.iter().map(|n| (n.id.clone(), 0)).collect();

        let mut dependents: HashMap<String, Vec<String>> = dag
            .nodes
            .iter()
            .map(|n| (n.id.clone(), Vec::new()))
            .collect();

        for node in &dag.nodes {
            for dep in &node.depends_on {
                *in_degree.entry(node.id.clone()).or_insert(0) += 1;
                if let Some(deps) = dependents.get_mut(dep) {
                    deps.push(node.id.clone());
                }
            }
        }

        // Build after-edge reverse index and pending count
        let mut after_dependents: HashMap<String, Vec<String>> = dag
            .nodes
            .iter()
            .map(|n| (n.id.clone(), Vec::new()))
            .collect();

        let mut after_pending: HashMap<String, usize> =
            dag.nodes.iter().map(|n| (n.id.clone(), 0)).collect();

        for node in &dag.nodes {
            if !node.after.is_empty() {
                after_pending.insert(node.id.clone(), node.after.len());
                for after_id in &node.after {
                    if let Some(dependents_list) = after_dependents.get_mut(after_id) {
                        dependents_list.push(node.id.clone());
                    }
                }
            }
        }

        // Track which nodes are terminal (for after edge satisfaction)
        let mut terminal_nodes: HashSet<String> = HashSet::new();

        // Apply checkpoint from a prior interrupted run (Spec-08). Completed
        // nodes seed as `Done`, never dispatch, and pre-decrement their
        // dependents' in-degree so a node whose only unfinished dependency
        // is already done becomes ready immediately. Terminal Failed/Blocked
        // nodes stay terminal and cascade-dependents become Blocked (mirroring
        // the live Failed-dependents block below). stale_running and
        // retryable-failed nodes are left `Pending` so the normal `ready`
        // queue dispatches them as attempt 1.
        let mut terminal_from_checkpoint: HashSet<String> = HashSet::new();
        if let Some(cp) = checkpoint {
            // 1. Completed nodes: Done + record_terminal + emit NodeDone +
            //    decrement dependents' in-degree.
            for id in &cp.completed_nodes {
                if dag.get_node(id).is_none() {
                    continue;
                }
                // I7: seed output from checkpoint hydration
                let output = cp.outputs.get(id).cloned();
                let done_state = NodeState {
                    node_id: id.clone(),
                    state: NodeStatus::Done,
                    model: None,
                    attempts: 1,
                    output,
                };
                node_state.insert(id.clone(), done_state.clone());
                inner.tx.send_modify(|snap| {
                    snap.record_terminal(id, done_state.clone());
                });
                tx.send(Event::NodeDone {
                    node_id: id.clone(),
                    model: String::new(),
                    output: String::new(),
                })
                .ok();
                terminal_from_checkpoint.insert(id.clone());
                terminal_nodes.insert(id.clone());
                if let Some(deps) = dependents.get(id) {
                    for dependent in deps {
                        if let Some(degree) = in_degree.get_mut(dependent)
                            && *degree > 0
                        {
                            *degree -= 1;
                        }
                    }
                }
                // Decrement after_pending for nodes waiting on this terminal node
                if let Some(after_deps) = after_dependents.get(id) {
                    for dependent in after_deps {
                        if let Some(pending) = after_pending.get_mut(dependent)
                            && *pending > 0
                        {
                            *pending -= 1;
                        }
                    }
                }
            }
            // 2. Terminal Failed nodes (only when retry_failed is OFF — those
            //    are the only ones load_checkpoint leaves in `failed_nodes`;
            //    retryable failed nodes are EXCLUDED from the set by
            //    load_checkpoint so they fall through to Pending).
            for id in &cp.failed_nodes {
                if dag.get_node(id).is_none() {
                    continue;
                }
                // I7: seed output from checkpoint hydration
                let output = cp.outputs.get(id).cloned();
                let failed_state = NodeState {
                    node_id: id.clone(),
                    state: NodeStatus::Failed,
                    model: None,
                    attempts: 1,
                    output,
                };
                node_state.insert(id.clone(), failed_state.clone());
                inner
                    .tx
                    .send_modify(|snap| snap.record_terminal(id, failed_state.clone()));
                tx.send(Event::NodeFailed {
                    node_id: id.clone(),
                    error: "failed in prior run".to_string(),
                })
                .ok();
                terminal_from_checkpoint.insert(id.clone());
                terminal_nodes.insert(id.clone());
                // Decrement after_pending for nodes waiting on this terminal node
                if let Some(after_deps) = after_dependents.get(id) {
                    for dependent in after_deps {
                        if let Some(pending) = after_pending.get_mut(dependent)
                            && *pending > 0
                        {
                            *pending -= 1;
                        }
                    }
                }
            }
            // 3. Blocked nodes (terminal).
            for id in &cp.blocked_nodes {
                if dag.get_node(id).is_none() {
                    continue;
                }
                // I7: seed output from checkpoint hydration
                let output = cp.outputs.get(id).cloned();
                let blocked_state = NodeState {
                    node_id: id.clone(),
                    state: NodeStatus::Blocked,
                    model: None,
                    attempts: 0,
                    output,
                };
                node_state.insert(id.clone(), blocked_state.clone());
                inner
                    .tx
                    .send_modify(|snap| snap.record_terminal(id, blocked_state.clone()));
                tx.send(Event::NodeBlocked {
                    node_id: id.clone(),
                })
                .ok();
                terminal_from_checkpoint.insert(id.clone());
                terminal_nodes.insert(id.clone());
                // Decrement after_pending for nodes waiting on this terminal node
                if let Some(after_deps) = after_dependents.get(id) {
                    for dependent in after_deps {
                        if let Some(pending) = after_pending.get_mut(dependent)
                            && *pending > 0
                        {
                            *pending -= 1;
                        }
                    }
                }
            }
            // 4. Cascade: any node whose ALL dependencies are terminal (Done
            //    from checkpoint, or Failed/Blocked above) but is itself not
            //    terminal yet and is not intended for retry must be Blocked
            //    (its parent failed). A node is Blocked only if it has at
            //    least one Failed/Blocked dependency; a node whose deps are
            //    all Done is READY (handled by the in-degree decrement in
            //    step 1, which already zeroed it). We only need to propagate
            //    Blocked for failed/blocked deps that the live loop would
            //    otherwise have done at runtime.
            for node in &dag.nodes {
                if terminal_from_checkpoint.contains(&node.id) {
                    continue;
                }
                let has_terminal_fail = node.depends_on.iter().any(|dep| {
                    terminal_from_checkpoint.contains(dep)
                        && matches!(
                            node_state.get(dep).map(|s| s.state.as_str()),
                            Some("Failed") | Some("Blocked")
                        )
                });
                if !has_terminal_fail {
                    continue;
                }
                let blocked_state = NodeState {
                    node_id: node.id.clone(),
                    state: NodeStatus::Blocked,
                    model: None,
                    attempts: 0,
                    output: None,
                };
                node_state.insert(node.id.clone(), blocked_state.clone());
                inner.tx.send_modify(|snap| {
                    snap.record_terminal(&node.id, blocked_state.clone());
                });
                tx.send(Event::NodeBlocked {
                    node_id: node.id.clone(),
                })
                .ok();
                terminal_from_checkpoint.insert(node.id.clone());
                terminal_nodes.insert(node.id.clone());
            }
            // 5. stale_running + retryable-failed (the latter are excluded
            //    from `failed_nodes` by load_checkpoint when retry_failed is
            //    ON) stay Pending — the ready queue below dispatches them as
            //    attempt 1, exactly the spec's R5.
        }

        // Initialize ready queue: only nodes with in_degree == 0 AND after_pending == 0
        // (or nodes with no after edges at all, which have after_pending == 0 by default).
        // Nodes with unsatisfied after edges must wait for those edges to be satisfied.
        let mut ready: VecDeque<String> = in_degree
            .iter()
            .filter(|(id, degree)| {
                **degree == 0
                    && !terminal_from_checkpoint.contains(*id)
                    && after_pending.get(*id).map(|p| *p == 0).unwrap_or(true)
            })
            .map(|(id, _)| id.clone())
            .collect();

        // Track which nodes have been enqueued to prevent double-push
        let mut enqueued: HashSet<String> = ready.iter().cloned().collect();

        // Helper macro to push a node to ready if both in_degree and after_pending are satisfied
        // and the node is not already enqueued
        macro_rules! try_enqueue {
            ($node_id:expr) => {
                if !enqueued.contains($node_id)
                    && in_degree.get($node_id).map(|d| *d == 0).unwrap_or(false)
                    && after_pending.get($node_id).map(|p| *p == 0).unwrap_or(true)
                {
                    ready.push_back($node_id.clone());
                    enqueued.insert($node_id.clone());
                }
            };
        }

        let semaphore = Arc::new(Semaphore::new(concurrency));
        let mut task_set = tokio::task::JoinSet::new();

        loop {
            while let Some(node_id) = ready.pop_front() {
                // spec-14 guard: never re-dispatch a node that is already
                // terminal this run (e.g. a gated fix node that was SKIPPED
                // on its gate-source passing, but whose own dependents still
                // decrement its in-degree).
                if node_state
                    .get(&node_id)
                    .map(|s| s.state != NodeStatus::Pending)
                    .unwrap_or(false)
                {
                    continue;
                }
                if let Some(node) = dag.get_node(&node_id) {
                    let mut node = node.clone();
                    // I1, I2, I6, I8: Interpolate output references at dispatch time
                    node.prompt = Self::interpolate_outputs(&node.prompt, &node_state);

                    if node.node_type.as_deref() == Some("quorum") {
                        // spec-38 F7a/G5: quorum is arithmetic, not dispatch.
                        // This node only became ready because every id in
                        // `quorum.of` is already terminal (wired into
                        // `after`, never `depends_on`, at expansion time --
                        // F9, G7), so `node_state` already holds every
                        // verdict this needs. Neither `worker` nor a
                        // subprocess is referenced anywhere in this branch
                        // -- that omission, not a runtime check, is what
                        // makes F7a true.
                        let snapshot = node_state.clone();
                        task_set.spawn(async move {
                            let state = Self::compute_quorum(&node, &snapshot);
                            (
                                node_id,
                                None::<String>,
                                1usize,
                                state,
                                None::<String>,
                                Vec::<DispatchEvent>::new(),
                                false,
                            )
                        });
                    } else {
                        let worker = Arc::clone(&worker);
                        let semaphore = Arc::clone(&semaphore);

                        let task_model = node.models.first().map(|m| m.name.clone());
                        task_set.spawn(async move {
                            let _permit = semaphore.acquire().await.ok();
                            let (state, model_used, events, final_attempt, verify_failed) =
                                Self::dispatch_node(&node, worker.as_ref(), allow_paid).await;
                            (
                                node_id,
                                task_model,
                                final_attempt,
                                state,
                                model_used,
                                events,
                                verify_failed,
                            )
                        });
                    }
                }
            }

            if task_set.is_empty() {
                break;
            }

            if let Some(Ok((
                node_id,
                task_model,
                task_attempt,
                state,
                model_used,
                dispatch_events,
                verify_failed,
            ))) = task_set.join_next().await
            {
                tx.send(Event::NodeDispatched {
                    node_id: node_id.clone(),
                    model: task_model.clone().unwrap_or_default(),
                    attempt: task_attempt,
                })
                .ok();

                for evt in dispatch_events {
                    // Typed, so a node id containing ':' cannot shift field
                    // indices and silently produce a wrong ProviderFallback --
                    // which the old colon-delimited protocol allowed, since
                    // ids come from user-authored templates and the gate
                    // syntax "<node>:fail" already reserves that character.
                    match evt {
                        DispatchEvent::Retry { .. } => {
                            tx.send(Event::NodeRetry {
                                node_id: node_id.clone(),
                                reason: "attempt failed".to_string(),
                            })
                            .ok();
                        }
                        DispatchEvent::BackoffRetry { .. } => {
                            // Retryable failure (429/503/quota) with exponential
                            // backoff before retrying the SAME model (NVIDIA
                            // guidance strategy 1).
                            tx.send(Event::NodeRetry {
                                node_id: node_id.clone(),
                                reason: "429 backoff".to_string(),
                            })
                            .ok();
                        }
                        DispatchEvent::Fallback { from, to } => {
                            tx.send(Event::ProviderFallback {
                                node_id: node_id.clone(),
                                from_model: from,
                                to_model: to,
                            })
                            .ok();
                        }
                        DispatchEvent::RetryExhausted { .. } => {
                            // Deliberately not emitted as an event: there is no
                            // Event variant for it, and the colon protocol
                            // discarded it too. Recorded here so the omission is
                            // a decision rather than a catch-all swallowing it.
                        }
                        DispatchEvent::CriticDispatched { model } => {
                            // C10: a critic dispatch is visible in the event
                            // log like any other model call, distinguishable
                            // from the producing node's own dispatch via the
                            // "::verify" suffix on node_id.
                            tx.send(Event::NodeDispatched {
                                node_id: format!("{node_id}::verify"),
                                model,
                                attempt: 1,
                            })
                            .ok();
                        }
                    }
                }

                node_state.insert(node_id.clone(), state.clone());

                if state.state == NodeStatus::Done {
                    tx.send(Event::NodeDone {
                        node_id: node_id.clone(),
                        model: model_used.clone().unwrap_or_default(),
                        output: state.output.clone().unwrap_or_default(),
                    })
                    .ok();

                    inner.tx.send_modify(|snap| {
                        snap.record_terminal(&node_id, state.clone());
                    });
                    terminal_nodes.insert(node_id.clone());

                    // Decrement after_pending for nodes waiting on this terminal node
                    if let Some(after_deps) = after_dependents.get(&node_id) {
                        for dependent in after_deps {
                            if let Some(pending) = after_pending.get_mut(dependent)
                                && *pending > 0
                            {
                                *pending -= 1;
                                try_enqueue!(dependent);
                            }
                        }
                    }
                } else if state.state == NodeStatus::Failed {
                    // Check if this is a verify failure
                    if verify_failed {
                        // Emit NodeVerifyFailed with both worker claim and verify output
                        // The output format is "worker: <output>\nverify: <output>"
                        if let Some(ref output) = state.output {
                            // Extract worker claim and verify output from combined output
                            let (worker_claim, verify_output) = if let Some(idx) =
                                output.find("\nverify: ")
                            {
                                let worker = &output[..idx].strip_prefix("worker: ").unwrap_or("");
                                let verify = &output[idx + 9..];
                                (worker.to_string(), verify.to_string())
                            } else {
                                (output.clone(), String::new())
                            };
                            tx.send(Event::NodeVerifyFailed {
                                node_id: node_id.clone(),
                                worker_claim,
                                verify_output,
                            })
                            .ok();
                        }
                    } else {
                        let error_msg = if let Some(ref output) = state.output {
                            if output.is_empty() {
                                "execution failed".to_string()
                            } else {
                                // Truncate to 8 KB and append truncation marker if needed
                                const MAX_ERROR_LEN: usize = 8192;
                                if output.len() > MAX_ERROR_LEN {
                                    let truncated =
                                        Self::truncate_at_char_boundary(output, MAX_ERROR_LEN);
                                    format!(
                                        "{}… [truncated {} bytes]",
                                        truncated,
                                        output.len() - truncated.len()
                                    )
                                } else {
                                    output.clone()
                                }
                            }
                        } else {
                            "execution failed".to_string()
                        };

                        tx.send(Event::NodeFailed {
                            node_id: node_id.clone(),
                            error: error_msg,
                        })
                        .ok();
                    }

                    inner.tx.send_modify(|snap| {
                        snap.record_terminal(&node_id, state.clone());
                    });
                    terminal_nodes.insert(node_id.clone());

                    // Decrement after_pending for nodes waiting on this terminal node
                    if let Some(after_deps) = after_dependents.get(&node_id) {
                        for dependent in after_deps {
                            if let Some(pending) = after_pending.get_mut(dependent)
                                && *pending > 0
                            {
                                *pending -= 1;
                                try_enqueue!(dependent);
                            }
                        }
                    }
                }

                // spec-14 (Bug A): honor conditional gate nodes. This runs for
                // BOTH a Done and a Failed source (previously it only ran on
                // Done, so a failed source never fired/skipped/blocked its
                // dependents). A gated node `D` with `gate: Some("<X>:fail")`
                // is the FIX node for `X`: it fires if `X` FAILED, skipped if
                // `X` PASSED.
                let finished_node_id = &node_id;

                if let Some(deps) = dependents.get(finished_node_id) {
                    for dependent in deps {
                        let dep_gated_on_this = dag
                            .get_node(dependent)
                            .map(|n| n.gate.as_deref() == Some(&format!("{finished_node_id}:fail")))
                            .unwrap_or(false);

                        if state.state == NodeStatus::Done {
                            if dep_gated_on_this {
                                // Trigger condition is false (source passed):
                                // skip this fix node — record it as a no-op Done
                                // and cascade to ITS dependents (R3: transitively to
                                // all downstream nodes that are gated on skipped nodes).

                                // R3: Worklist-based transitive skip cascade.
                                // Collect the skip roots to process transitively.
                                let mut skip_worklist = vec![dependent.clone()];
                                let mut skipped_processed = std::collections::HashSet::new();

                                while let Some(to_skip) = skip_worklist.pop() {
                                    // R3.4: Guard against processing the same node twice
                                    if skipped_processed.contains(&to_skip) {
                                        continue;
                                    }
                                    skipped_processed.insert(to_skip.clone());

                                    // If this node is not already marked terminal, mark it Done.
                                    // (The initial `dependent` is already marked above; subsequent
                                    // nodes from the worklist are marked here.)
                                    if !terminal_nodes.contains(&to_skip) {
                                        let skip = NodeState {
                                            node_id: to_skip.clone(),
                                            state: NodeStatus::Done,
                                            model: None,
                                            attempts: 0,
                                            output: None,
                                        };
                                        node_state.insert(to_skip.clone(), skip.clone());
                                        terminal_from_checkpoint.insert(to_skip.clone());
                                        terminal_nodes.insert(to_skip.clone());
                                        inner.tx.send_modify(|snap| {
                                            snap.record_terminal(&to_skip, skip);
                                        });
                                        tx.send(Event::NodeDone {
                                            node_id: to_skip.clone(),
                                            model: String::new(),
                                            output: String::new(),
                                        })
                                        .ok();
                                    }

                                    // R3.3.2: Decrement after_pending for nodes waiting on this skipped node.
                                    if let Some(after_deps) = after_dependents.get(&to_skip) {
                                        for after_dep in after_deps {
                                            if let Some(pending) = after_pending.get_mut(after_dep)
                                                && *pending > 0
                                            {
                                                *pending -= 1;
                                                try_enqueue!(after_dep);
                                            }
                                        }
                                    }

                                    // R3.3.3: Process depends_on-dependents. For each dependent D:
                                    // - If D.gate == "<to_skip>:fail", push D to worklist (it's skipped too)
                                    // - Else, decrement in_degree and try-enqueue at zero.
                                    if let Some(nested) = dependents.get(&to_skip) {
                                        for g in nested {
                                            if !node_state
                                                .get(g)
                                                .map(|s| s.state == NodeStatus::Pending)
                                                .unwrap_or(true)
                                            {
                                                continue;
                                            }

                                            let g_gated_on_skip = dag
                                                .get_node(g)
                                                .map(|n| {
                                                    n.gate.as_deref()
                                                        == Some(&format!("{to_skip}:fail"))
                                                })
                                                .unwrap_or(false);

                                            if g_gated_on_skip {
                                                // R3: g is a fix node for to_skip:fail;
                                                // it should also be skipped.
                                                skip_worklist.push(g.clone());
                                            } else {
                                                // Normal (non-gated) dependent: decrement in_degree.
                                                if let Some(degree) = in_degree.get_mut(g) {
                                                    *degree = degree.saturating_sub(1);
                                                    if *degree == 0 {
                                                        try_enqueue!(g);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                continue;
                            }
                            // Normal (non-gated) dependent on a succeeded node:
                            // decrement in-degree; dispatch when all deps are done and all after edges are satisfied.
                            if let Some(degree) = in_degree.get_mut(dependent) {
                                *degree = degree.saturating_sub(1);
                                if *degree == 0 {
                                    try_enqueue!(dependent);
                                }
                            }
                        } else {
                            // state.state == NodeStatus::Failed
                            if dep_gated_on_this {
                                // FIX node for this failure: dispatch it once all
                                // its deps are done and all after edges are satisfied.
                                if let Some(degree) = in_degree.get_mut(dependent) {
                                    *degree = degree.saturating_sub(1);
                                    if *degree == 0 {
                                        try_enqueue!(dependent);
                                    }
                                }
                                continue;
                            }
                            // Not gated: a failed source blocks this dependent.
                            let blocked_state = NodeState {
                                node_id: dependent.clone(),
                                state: NodeStatus::Blocked,
                                model: None,
                                attempts: 0,
                                output: None,
                            };
                            node_state
                                .entry(dependent.clone())
                                .or_insert_with(|| blocked_state.clone())
                                .state = NodeStatus::Blocked;
                            terminal_from_checkpoint.insert(dependent.clone());
                            terminal_nodes.insert(dependent.clone());

                            inner.tx.send_modify(|snap| {
                                snap.record_terminal(dependent, blocked_state.clone());
                            });

                            tx.send(Event::NodeBlocked {
                                node_id: dependent.clone(),
                            })
                            .ok();

                            // Decrement after_pending for nodes waiting on this blocked node
                            if let Some(after_deps) = after_dependents.get(dependent) {
                                for after_dep in after_deps {
                                    if let Some(pending) = after_pending.get_mut(after_dep)
                                        && *pending > 0
                                    {
                                        *pending -= 1;
                                        try_enqueue!(after_dep);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let failed_nodes: Vec<String> = node_state
            .values()
            .filter(|s| s.state == NodeStatus::Failed)
            .map(|s| s.node_id.clone())
            .collect();

        let successful_nodes = node_state
            .values()
            .filter(|s| s.state == NodeStatus::Done)
            .count();
        let failed_count = failed_nodes.len();

        tx.send(Event::DagDone {
            successful_nodes,
            failed_nodes: failed_count,
        })
        .map_err(|_| PidagError::WorkerFailed)?;

        // T4: Drop the sender and await the consumer task to drain all events.
        // This ensures that all events are flushed to the sink before we return.
        drop(tx);
        consumer_handle
            .await
            .map_err(|_| PidagError::WorkerFailed)?;

        let mut node_states: Vec<_> = node_state.values().cloned().collect();
        node_states.sort_by(|a, b| a.node_id.cmp(&b.node_id));

        let report = RunReport {
            node_states,
            failed: failed_nodes,
            store_write_failures: write_failures.load(Ordering::SeqCst),
        };

        inner.tx.send_modify(|snap| {
            snap.dag_done = true;
            snap.report = Some(report.clone());
        });

        Ok(report)
    }

    pub(super) async fn dispatch_node(
        node: &Node,
        worker: &dyn Worker,
        allow_paid: bool,
    ) -> (NodeState, Option<String>, Vec<DispatchEvent>, usize, bool) {
        // Returns: (state, model_used, events, final_attempt, verify_failed_flag)
        let mut final_state = NodeState {
            node_id: node.id.clone(),
            state: NodeStatus::Failed,
            model: None,
            attempts: 0,
            output: None,
        };
        // Last attempt index actually executed on this node. Starts at 0
        // (nothing has run yet); updated on every worker invocation so the
        // outer loop can report the real attempt in `Event::NodeDispatched`.
        // See HANDOFF 2026-08-02 audit P1 #4.
        let mut last_attempt: usize = 0;

        let mut events = Vec::new();
        let mut verify_failed = false;

        // R5: Run verify_pre before dispatch and capture baseline token.
        // If verify_pre exits non-zero, fail the node immediately (R5.4).
        let verify_pre_token = if let Some(ref verify_pre_cmd) = node.verify_pre {
            let timeout = node.timeout.unwrap_or_else(|| Duration::from_secs(5));
            match Self::run_verify_pre(verify_pre_cmd, timeout).await {
                Ok(token) => Some(token),
                Err(err_msg) => {
                    final_state.state = NodeStatus::Failed;
                    final_state.output = Some(format!(
                        "verify_pre failed for node {}: {}",
                        node.id, err_msg
                    ));
                    return (final_state, None, events, 0, verify_failed);
                }
            }
        } else {
            None
        };

        // Handle empty models array: shell nodes execute with empty model string,
        // LLM nodes fail with a clear error.
        if node.models.is_empty() {
            if node.node_type.as_deref() == Some("shell") {
                // Shell node: execute worker with empty model string, respecting retry attempts
                for attempt in 1..=node.retry.attempts {
                    last_attempt = attempt;
                    // `run_with_node_timeout` returns `Ok(Some(o))` when the worker
                    // came back with a `WorkerOutput` (success OR worker-failure),
                    // `Ok(None)` when the worker itself returned `Err` (existing
                    // behaviour: swallowed, treated as "no usable result this
                    // attempt"), and `Err(DispatchTimeout)` when the node-level
                    // deadline elapsed — a TERMINAL hard failure: do not retry the
                    // attempt, do not advance to a fallback model; record the
                    // timeout and return immediately. Per the 2026-08-02 audit
                    // P1 #5 design ("non-retryable hard failure").
                    let output =
                        match Self::run_with_node_timeout(node, worker, &node.prompt, "", attempt)
                            .await
                        {
                            Ok(Some(o)) => o,
                            Ok(None) => continue,
                            Err(_timeout) => {
                                final_state.state = NodeStatus::Failed;
                                final_state.output =
                                    Some(format!("node {} attempt {attempt} timed out", node.id));
                                return (final_state, None, events, last_attempt, verify_failed);
                            }
                        };
                    if output.success {
                        final_state.state = NodeStatus::Done;
                        final_state.model = None;
                        final_state.attempts = attempt;
                        final_state.output = Some(output.output.clone());
                        // Apply verify check (V1, V5: must verify before marking Done)
                        let (updated_state, vf, critic_events) = Self::apply_verify_check(
                            node,
                            final_state,
                            &output.output,
                            verify_pre_token.as_deref(),
                            worker,
                            allow_paid,
                        )
                        .await;
                        final_state = updated_state;
                        verify_failed = vf;
                        events.extend(critic_events);
                        return (final_state, None, events, last_attempt, verify_failed);
                    } else {
                        // Track failure output for final state
                        final_state.output = Some(output.output);
                        if attempt < node.retry.attempts {
                            events.push(DispatchEvent::Retry { attempt });
                        }
                    }
                }
            } else {
                // LLM node with no models specified: this is an error
                final_state.state = NodeStatus::Failed;
                final_state.output = Some("no models specified for LLM node".to_string());
            }
            return (final_state, None, events, last_attempt, verify_failed);
        }

        // Existing logic for non-empty models array
        let mut prev_model: Option<String> = None;
        let mut first_model = true;
        let mut last_failure_output: Option<String> = None;

        for model in &node.models {
            if model.paid && !allow_paid {
                continue;
            }

            // On the 2nd+ model for this node, record a fallback event from the
            // previous (exhausted) model to the one we are about to try.
            if !first_model && let Some(prev) = &prev_model {
                events.push(DispatchEvent::Fallback {
                    from: prev.clone(),
                    to: model.name.clone(),
                });
            }
            first_model = false;

            for attempt in 1..=node.retry.attempts {
                last_attempt = attempt;
                let output = match Self::run_with_node_timeout(
                    node,
                    worker,
                    &node.prompt,
                    &model.name,
                    attempt,
                )
                .await
                {
                    Ok(Some(o)) => o,
                    Ok(None) => continue,
                    Err(_timeout) => {
                        // TERMINAL hard failure: a node-level deadline elapse
                        // is the host's fault (worker wedged / network
                        // wedged), not a transient provider signal — do not
                        // retry the attempt, do not advance to the next
                        // ModelRef. Record the timeout and return. Per the
                        // 2026-08-02 audit P1 #5 design comment.
                        final_state.state = NodeStatus::Failed;
                        final_state.output = Some(format!(
                            "node {} model {} attempt {attempt} timed out",
                            node.id, model.name
                        ));
                        return (final_state, None, events, last_attempt, verify_failed);
                    }
                };
                if output.success {
                    if let Some(validate_str) = &node.validate
                        && !output.output.contains(validate_str)
                    {
                        if attempt < node.retry.attempts {
                            events.push(DispatchEvent::Retry { attempt });
                        }
                        continue;
                    }

                    final_state.state = NodeStatus::Done;
                    final_state.model = Some(model.name.clone());
                    final_state.attempts = attempt;
                    final_state.output = Some(output.output.clone());
                    // Apply verify check (V1, V5: must verify before marking Done)
                    let (updated_state, vf, critic_events) = Self::apply_verify_check(
                        node,
                        final_state,
                        &output.output,
                        verify_pre_token.as_deref(),
                        worker,
                        allow_paid,
                    )
                    .await;
                    final_state = updated_state;
                    verify_failed = vf;
                    events.extend(critic_events);
                    return (
                        final_state,
                        Some(model.name.clone()),
                        events,
                        last_attempt,
                        verify_failed,
                    );
                }

                // Track the last failure output for final error reporting
                last_failure_output = Some(output.output.clone());

                // ---- Failure branch: classify by `output.retryable` ----
                // Retryable (HTTP 429 / 503 / quota / rate-limit / conn reset):
                //   - If we still have attempts on THIS model AND backoff is
                //     configured (>0): sleep base * 2^(attempt-1), then retry
                //     the same model (NVIDIA guidance strategy 1).
                //   - Else (out of attempts OR backoff_ms == 0): break to the
                //     NEXT ModelRef (NVIDIA guidance strategy 3 = fallback).
                // Real failure (retryable == false): keep the existing
                //   no-sleep, retry-same-model behavior.
                // See specs/93-runtime-429-failover.md (R3, R6-bis).
                if output.retryable {
                    if attempt < node.retry.attempts && node.retry.backoff_ms > 0 {
                        let delay = node.retry.backoff_ms * (1u64 << (attempt - 1));
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        events.push(DispatchEvent::BackoffRetry { attempt });
                    } else {
                        // No more backoff retries on this model → advance.
                        events.push(DispatchEvent::RetryExhausted {
                            model: model.name.clone(),
                        });
                        break;
                    }
                } else if attempt < node.retry.attempts {
                    events.push(DispatchEvent::Retry { attempt });
                }
            }

            prev_model = Some(model.name.clone());
        }

        // Set the final state output to the last failure output before returning
        final_state.output = last_failure_output;

        (final_state, None, events, last_attempt, verify_failed)
    }

    /// Interpolate {{<node_id>.output}} references in a prompt (I1, I2, I6, I8).
    /// - Substitutes each {{X.output}} with the output of node X from node_state
    /// - Works for any terminal state (Done, Failed, Blocked) — I2
    /// - Missing output substitutes empty string — I8
    /// - Caps interpolated output at 32 KB, keeping the tail with truncation marker — I6
    fn interpolate_outputs(prompt: &str, node_state: &HashMap<String, NodeState>) -> String {
        let mut result = String::new();
        let mut last_end = 0;

        let mut i = 0;
        while let Some(start) = prompt[i..].find("{{") {
            let start_abs = i + start;
            let after_open = start_abs + 2;

            // Find the closing }}
            if let Some(offset) = prompt[after_open..].find("}}") {
                let end = after_open + offset;
                let content = &prompt[after_open..end];

                // Parse the reference (guaranteed valid by dag.validate())
                if let Some(dot_pos) = content.find('.') {
                    let node_ref = &content[..dot_pos];
                    let suffix = &content[dot_pos + 1..];

                    if suffix == "output" {
                        // Append the text before this placeholder
                        result.push_str(&prompt[last_end..start_abs]);

                        // Get the output from node_state (I2: any terminal state)
                        let output = node_state
                            .get(node_ref)
                            .and_then(|ns| ns.output.as_deref())
                            .unwrap_or("");

                        // I6: cap at 32 KB, keeping the tail with char-safe truncation.
                        // Keep the tail (not head) because validator and build output accumulates
                        // and the actionable part is at the end.
                        const MAX_INTERPOLATED: usize = 32 * 1024; // 32 KB
                        if output.len() > MAX_INTERPOLATED {
                            // Find a safe char boundary at or after tail_start
                            let tail_start = output.len() - MAX_INTERPOLATED;
                            let safe_tail_start =
                                Self::find_char_boundary_after(output, tail_start);
                            let tail = &output[safe_tail_start..];
                            let dropped = output.len() - tail.len();
                            result.push_str(&format!("[… {} bytes truncated …]\n", dropped));
                            result.push_str(tail);
                        } else {
                            result.push_str(output);
                        }

                        last_end = end + 2;
                        i = end + 2;
                        continue;
                    }
                }

                i = end + 2;
            } else {
                break;
            }
        }

        // Append any remaining text after the last placeholder
        result.push_str(&prompt[last_end..]);
        result
    }

    /// Truncate a string at or before a byte position, ensuring no mid-UTF-8-character cuts.
    fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
        if s.len() <= max_bytes {
            return s;
        }
        match s.char_indices().rfind(|(pos, _)| *pos < max_bytes) {
            Some((pos, ch)) => &s[..pos + ch.len_utf8()],
            None => "",
        }
    }

    /// Find a safe character boundary at or after a byte position.
    /// Used to skip partial UTF-8 sequences when seeking to a tail.
    fn find_char_boundary_after(s: &str, pos: usize) -> usize {
        if pos >= s.len() {
            return s.len();
        }
        // If we're already at a boundary, return it
        if s.is_char_boundary(pos) {
            return pos;
        }
        // Otherwise, find the next boundary
        for i in pos + 1..=s.len() {
            if s.is_char_boundary(i) {
                return i;
            }
        }
        s.len()
    }

    /// Drive a single `Worker::run` invocation, applying the node-level
    /// wall-clock deadline (`Node::timeout`) as a defense-in-depth bound on
    /// top of whatever the worker enforces internally.
    ///
    /// Returns:
    /// - `Ok(Some(o))` — worker returned a usable `WorkerOutput` (success or
    ///   worker-attributed failure). The scheduler treats it per the retry/
    ///   backoff/fallback matrix.
    /// - `Ok(None)` — worker itself returned `Err(PidagError)`. Pre-existing
    ///   behaviour (2026-08-02 audit P2-3 noted the trait `Err` path is
    ///   effectively dead — every concrete worker maps failures to
    ///   `Ok(WorkerOutput{success:false,...})`); preserve that swallow by
    ///   mapping to `None` so the caller treats this attempt as a no-op and
    ///   advances.
    /// - `Err(DispatchTimeout)` — node deadline elapsed. Mapped by the caller
    ///   to a non-retryable hard-failure `WorkerOutput` (a timeout is the
    ///   host's fault, not a transient provider signal — don't burn a
    ///   backoff retry on it).
    ///
    /// When `node.timeout` is `None`, no `tokio::time::timeout` wrapper is
    /// applied — the worker's own timeout (e.g. `PiPrintWorker.timeout`,
    /// `RealShellWorker.timeout`) bounds execution. This preserves the
    /// pre-existing behaviour byte-identically for nodes that don't opt in.
    /// See HANDOFF 2026-08-02 audit P1 #5.
    async fn run_with_node_timeout(
        node: &Node,
        worker: &dyn Worker,
        prompt: &str,
        model: &str,
        attempt: usize,
    ) -> Result<Option<WorkerOutput>, DispatchTimeout> {
        let attempt_fut = worker.run(&node.id, prompt, model, attempt);
        match node.timeout {
            Some(deadline) => match tokio::time::timeout(deadline, attempt_fut).await {
                Ok(Ok(output)) => Ok(Some(output)),
                Ok(Err(_)) => Ok(None),
                Err(_elapsed) => Err(DispatchTimeout),
            },
            None => match attempt_fut.await {
                Ok(output) => Ok(Some(output)),
                Err(_pidag_err) => Ok(None),
            },
        }
    }

    /// Apply verify check to a node that succeeded. If node.verify is set and
    /// verify fails, mutates state to Failed and uses verify output as the artifact.
    /// Returns (state, verify_failed_flag, critic_events). Called just before
    /// returning from dispatch_node for Done states. The flag is used to emit
    /// the correct event. verify_pre_token is the captured stdout from
    /// verify_pre (if present), which is exposed to verify as the
    /// PIDAG_VERIFY_PRE environment variable. `worker`/`allow_paid` are
    /// threaded through so a `Verify::Critic` arm can dispatch through the
    /// same `&dyn Worker` and paid-model gate as any other node (spec-37,
    /// C3, C7) -- `apply_verify_check` previously had no worker handle at
    /// all, which is exactly what made a critic verify unreachable before
    /// this spec.
    async fn apply_verify_check(
        node: &Node,
        mut state: NodeState,
        worker_output: &str,
        verify_pre_token: Option<&str>,
        worker: &dyn Worker,
        allow_paid: bool,
    ) -> (NodeState, bool, Vec<DispatchEvent>) {
        let mut events = Vec::new();
        if let Some(ref verify) = node.verify {
            let result = Self::eval_verify(
                verify,
                node,
                worker_output,
                verify_pre_token,
                worker,
                allow_paid,
                &mut events,
            )
            .await;

            if let Err(reason) = result {
                // Verify failed: change state to Failed, use the reason as
                // the artifact. Store both facts: worker_output and reason.
                // Format: "worker: <worker_output>\nverify: <reason>"
                // Truncate to 8 KB like NodeFailed. Same convention for
                // every `Verify` arm (Shell/Critic/All) -- see C5's note on
                // event.rs -- so this is byte-identical to the pre-spec-37
                // Shell-only formatting when `verify` is `Verify::Shell` (N1).
                state.state = NodeStatus::Failed;
                const MAX_ERROR_LEN: usize = 8192;
                let combined = format!("worker: {}\nverify: {}", worker_output, reason);
                if combined.len() > MAX_ERROR_LEN {
                    let truncated = Self::truncate_at_char_boundary(&combined, MAX_ERROR_LEN);
                    state.output = Some(format!(
                        "{}… [truncated {} bytes]",
                        truncated,
                        combined.len() - truncated.len()
                    ));
                } else {
                    state.output = Some(combined);
                }
                return (state, true, events);
            }
        }
        (state, false, events)
    }

    /// Evaluate one `Verify` arm. `Ok(())` is a pass; `Err(reason)` is a
    /// fail with a human-readable cause. Boxed/pinned because `All` recurses
    /// into this same function and a plain `async fn` cannot call itself
    /// (the compiler cannot compute an infinite-size future); this is the
    /// standard shape for recursive async in Rust and adds no dependency
    /// (N3).
    fn eval_verify<'a>(
        verify: &'a Verify,
        node: &'a Node,
        worker_output: &'a str,
        verify_pre_token: Option<&'a str>,
        worker: &'a dyn Worker,
        allow_paid: bool,
        events: &'a mut Vec<DispatchEvent>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            match verify {
                Verify::Shell(cmd) => {
                    // Unchanged from pre-spec-37: same command, same
                    // timeout/env discipline, same (bool, String) shape (N1).
                    let timeout = node.timeout.unwrap_or_else(|| Duration::from_secs(5));
                    let (ok, output) = Self::run_verify(cmd, timeout, verify_pre_token).await;
                    if ok { Ok(()) } else { Err(output) }
                }
                Verify::Critic { prompt, models } => {
                    Self::eval_critic(
                        node,
                        prompt,
                        models,
                        worker_output,
                        worker,
                        allow_paid,
                        events,
                    )
                    .await
                }
                Verify::All(arms) => {
                    // C6: every arm must pass; short-circuit on the first
                    // failure (C6b) so a free shell check that already
                    // failed never triggers a paid critic dispatch. The
                    // reason names which arm failed (C6, error-handling
                    // expectations).
                    for (i, arm) in arms.iter().enumerate() {
                        if let Err(reason) = Self::eval_verify(
                            arm,
                            node,
                            worker_output,
                            verify_pre_token,
                            worker,
                            allow_paid,
                            events,
                        )
                        .await
                        {
                            let kind = match arm {
                                Verify::Shell(_) => "shell",
                                Verify::Critic { .. } => "critic",
                                Verify::All(_) => "all",
                            };
                            return Err(format!("arm {i} ({kind}) failed: {reason}"));
                        }
                    }
                    Ok(())
                }
            }
        })
    }

    /// Dispatch a `Verify::Critic` arm through `&dyn Worker` and judge the
    /// reply. Fail-closed throughout (G4): the only path to `Ok(())` is a
    /// worker reply whose trimmed text starts with a `PASS` token; every
    /// other outcome -- unparseable reply, empty reply, worker `Err`,
    /// exhausted fallbacks, a timeout, or the paid-model gate blocking every
    /// configured model -- is `Err(reason)` with a cause-naming reason
    /// (error handling expectations).
    async fn eval_critic(
        node: &Node,
        prompt_template: &str,
        models: &[ModelRef],
        worker_output: &str,
        worker: &dyn Worker,
        allow_paid: bool,
        events: &mut Vec<DispatchEvent>,
    ) -> Result<(), String> {
        // C7: a critic whose models are all paid is subject to the same
        // allow_paid gate as any other dispatch, and must say so explicitly
        // rather than fail with a generic message -- it is a configuration
        // problem, not a verification result. Checked BEFORE dispatch so a
        // blocked critic is provably never dispatched (no worker call, no
        // CriticDispatched event).
        if !models.is_empty() && !models.iter().any(|m| !m.paid || allow_paid) {
            return Err(
                "critic blocked: allow_paid is disabled and every configured critic model is paid"
                    .to_string(),
            );
        }

        // C3b: the producing node's own output is available to the critic
        // prompt via the spec-29 interpolation already built -- reused
        // as-is here (`interpolate_outputs`), keyed by this node's own id,
        // since at verify time the worker output exists but has not yet
        // been recorded in the scheduler's node_state map.
        let mut self_output = HashMap::new();
        self_output.insert(
            node.id.clone(),
            NodeState {
                node_id: node.id.clone(),
                state: NodeStatus::Done,
                model: None,
                attempts: 0,
                output: Some(worker_output.to_string()),
            },
        );
        let critic_prompt = Self::interpolate_outputs(prompt_template, &self_output);

        // C3: dispatch through the same Worker trait, retry and
        // model-fallback machinery a normal node uses -- reuse dispatch_node
        // itself rather than duplicating that machinery. The synthetic
        // critic node has `verify: None`, so this cannot recurse further.
        let critic_node = Node {
            id: format!("{}::verify", node.id),
            prompt: critic_prompt,
            depends_on: Vec::new(),
            models: models.to_vec(),
            retry: node.retry.clone(),
            validate: None,
            node_type: None,
            gate: None,
            timeout: node.timeout,
            mcp_call: None,
            after: Vec::new(),
            verify: None,
            verify_pre: None,

            for_each: None,
            quorum: None,
        };

        // C10: record the spend before dispatch, once the allow_paid gate
        // above has cleared, so verification cost is visible in the event
        // log even when the critic itself fails.
        if let Some(first_model) = models.iter().find(|m| !m.paid || allow_paid) {
            events.push(DispatchEvent::CriticDispatched {
                model: first_model.name.clone(),
            });
        }

        let (state, _model_used, sub_events, _attempt, _verify_failed) =
            Self::dispatch_node(&critic_node, worker, allow_paid).await;
        events.extend(sub_events);

        match state.state {
            NodeStatus::Done => {
                // C4: fail-closed verdict parsing. Anything that is not a
                // leading PASS/FAIL token -- including an empty reply -- is
                // a FAIL with the raw reply preserved as the reason.
                let reply = state.output.unwrap_or_default();
                let (passed, reason) = Self::parse_critic_verdict(&reply);
                if passed { Ok(()) } else { Err(reason) }
            }
            _ => {
                // Worker error, exhausted fallbacks, or a node-level
                // timeout: dispatch_node already classified this as Failed.
                // Fail closed here too (C4e) -- never Done.
                let detail = state
                    .output
                    .unwrap_or_else(|| "critic worker produced no output".to_string());
                Err(format!("critic dispatch failed: {detail}"))
            }
        }
    }

    /// Parse a critic's reply into (passed, reason). The accepted form is
    /// narrow and specified: a leading `PASS` or `FAIL` token, optionally
    /// followed by a separator (whitespace/`-`/`:`) and a reason. Anything
    /// that does not start with one of those two tokens -- including a
    /// reply that merely *contains* "PASS" somewhere, e.g. "this does not
    /// PASS" -- is treated as unparseable and fails closed with the raw
    /// reply as the reason (C4c, C4d). This is the guard the naive
    /// `reply.contains("PASS")` implementation fails: that check reads
    /// "this does not PASS" as a pass, laundering an unverified result as a
    /// verified one (G4).
    fn parse_critic_verdict(reply: &str) -> (bool, String) {
        let trimmed = reply.trim();
        for (token, passed) in [("PASS", true), ("FAIL", false)] {
            if let Some(rest) = trimmed.strip_prefix(token) {
                // Require a word boundary after the token so "PASSED" or
                // "FAILURE" don't spuriously match a bare PASS/FAIL token.
                let boundary_ok = rest
                    .chars()
                    .next()
                    .map(|c| !c.is_alphanumeric())
                    .unwrap_or(true);
                if boundary_ok {
                    let reason = rest
                        .trim_start_matches(|c: char| c.is_whitespace() || c == '-' || c == ':')
                        .trim()
                        .to_string();
                    let reason = if reason.is_empty() {
                        trimmed.to_string()
                    } else {
                        reason
                    };
                    return (passed, reason);
                }
            }
        }
        // No leading PASS/FAIL token (including an empty reply): fail
        // closed, raw reply preserved as the reason (C4c, G4).
        (false, trimmed.to_string())
    }

    /// spec-38 F7/F9/F10: quorum is arithmetic over already-terminal
    /// outputs, never a dispatch. Every id in `quorum.of` was added to this
    /// node's `after` set at expansion time (F9), so by the time this node
    /// is popped off the ready queue every one of them is terminal in
    /// `node_state` -- Done, Failed, or Blocked all count (spec-38 premise
    /// 1: `interpolate_outputs`-style reads work for any terminal state,
    /// and this is the same idea applied to raw output lookup). Verdicts
    /// are parsed with `Self::parse_critic_verdict` -- the SAME function
    /// `Verify::Critic` uses (F8, G6) -- so "what counts as a pass" cannot
    /// drift between the two call sites.
    fn compute_quorum(node: &Node, node_state: &HashMap<String, NodeState>) -> NodeState {
        // `node.quorum` is guaranteed `Some` for a `node_type == "quorum"`
        // node that reached the ready queue: `Dag::validate` (F11) rejects
        // a quorum node with no config or an out-of-bounds `min_pass`
        // before the run starts. The empty-config fallback below is
        // defense-in-depth, not a reachable path.
        let empty = QuorumConfig {
            of: Vec::new(),
            min_pass: 0,
        };
        let cfg = node.quorum.as_ref().unwrap_or(&empty);

        let mut pass_count = 0usize;
        let mut lines = Vec::with_capacity(cfg.of.len());
        for id in &cfg.of {
            // F10, error-handling expectations: a counted node with no
            // recorded output at all (e.g. Blocked, or Failed before
            // producing anything) reports as an unparseable reply -- FAIL,
            // never an error and never a silent pass.
            let output = node_state
                .get(id)
                .and_then(|s| s.output.as_deref())
                .unwrap_or("");
            let (passed, reason) = Self::parse_critic_verdict(output);
            if passed {
                pass_count += 1;
            }
            lines.push(format!(
                "{id}: {} - {reason}",
                if passed { "PASS" } else { "FAIL" }
            ));
        }

        let total = cfg.of.len();
        // `min_pass == 0` is rejected by `Dag::validate` (F11); `&& cfg.min_pass > 0`
        // is defense-in-depth against a hand-built (unvalidated) `Node`, not
        // a path a validated DAG can reach.
        let done = cfg.min_pass > 0 && pass_count >= cfg.min_pass;

        let summary = format!(
            "quorum: {pass_count}/{total} passed (min_pass={})\n{}",
            cfg.min_pass,
            lines.join("\n")
        );

        NodeState {
            node_id: node.id.clone(),
            state: if done {
                NodeStatus::Done
            } else {
                NodeStatus::Failed
            },
            model: None,
            attempts: 1,
            output: Some(summary),
        }
    }

    /// Run verify_pre before dispatch to capture a baseline token.
    /// Trims trailing whitespace and caps output at 4 KB (R5.3).
    /// Returns Ok(token) on success (exit 0), or Err(error_msg) on failure.
    async fn run_verify_pre(
        verify_pre_cmd: &str,
        shell_timeout: Duration,
    ) -> Result<String, String> {
        let mut cmd = Command::new("bash");
        cmd.arg("-c");
        cmd.arg(verify_pre_cmd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                return Err(format!("verify_pre spawn failed: {err}"));
            }
        };

        let awaited = tokio::time::timeout(shell_timeout, child.wait_with_output()).await;

        let output = match awaited {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                return Err(format!("verify_pre process error: {err}"));
            }
            Err(_elapsed) => {
                return Err("verify_pre timed out".to_string());
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let error_msg = if stderr.is_empty() { stdout } else { stderr };
            return Err(error_msg);
        }

        // Capture stdout, trim trailing whitespace, cap at 4 KB (R5.3).
        // Slice on a char boundary: a naive `&s[..MAX_TOKEN]` panics with
        // "byte index is not a char boundary" whenever the cut lands mid-
        // codepoint, and a verify_pre command is free to emit UTF-8.
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let trimmed = stdout.trim_end();
        const MAX_TOKEN: usize = 4096;
        Ok(Self::truncate_at_char_boundary(trimmed, MAX_TOKEN).to_string())
    }

    /// Run a verify command in the DAG's project_root with the same
    /// cwd/env/timeout discipline as a shell node. Returns (success, output).
    /// Timeout is the shell-node timeout (5s default, configurable per node).
    /// Reuses the RealShellWorker pattern: bash -c, captures stdout+stderr.
    /// verify_pre_token is exposed as the PIDAG_VERIFY_PRE environment variable if present.
    async fn run_verify(
        verify_cmd: &str,
        shell_timeout: Duration,
        verify_pre_token: Option<&str>,
    ) -> (bool, String) {
        let mut cmd = Command::new("bash");
        cmd.arg("-c");
        cmd.arg(verify_cmd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        // Set PIDAG_VERIFY_PRE environment variable if verify_pre captured a token (R5)
        if let Some(token) = verify_pre_token {
            cmd.env("PIDAG_VERIFY_PRE", token);
        }

        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                return (false, format!("verify spawn failed: {err}"));
            }
        };

        let awaited = tokio::time::timeout(shell_timeout, child.wait_with_output()).await;

        let output = match awaited {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                return (false, format!("verify process error: {err}"));
            }
            Err(_elapsed) => {
                return (false, "verify timed out".to_string());
            }
        };

        let success = output.status.success();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let combined_output = if stderr.is_empty() {
            stdout
        } else if stdout.is_empty() {
            stderr
        } else {
            format!("{}\n{}", stdout, stderr)
        };

        (success, combined_output)
    }
}

/// Deadline-elapsed marker for `Scheduler::run_with_node_timeout`. Distinct
/// from `PidagError` so callers can apply retry semantics (non-retryable
/// here) without conflating it with worker-spawn or store errors. No public
/// surfaced — `pub(super)` would do, but the module boundary is `impl`
/// private, so a unit struct with a private marker field is used.
struct DispatchTimeout;
