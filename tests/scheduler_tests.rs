use async_trait::async_trait;
use pidag::{
    Dag, Event, ModelRef, Node, NodeStatus, PidagError, RetryPolicy, Scheduler, Store, VecSink,
    Worker, WorkerOutput, core::event::RedbSink, store::MockStore,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

// ============================================================================
// MockWorker: scriptable, records dispatch order and max concurrency
// ============================================================================

#[derive(Clone)]
struct MockWorkerScript {
    outcomes: Arc<std::collections::HashMap<(String, String, usize), MockOutcome>>,
    dispatch_order: Arc<Mutex<Vec<(String, String)>>>,
    concurrent_count: Arc<AtomicUsize>,
    peak_concurrency: Arc<AtomicUsize>,
}

#[derive(Clone, Debug)]
enum MockOutcome {
    Success(String),
    Failure,
    FailureWithMessage(String),
}

#[async_trait]
impl Worker for MockWorkerScript {
    async fn run(
        &self,
        node_id: &str,
        _prompt: &str,
        model: &str,
        attempt: usize,
    ) -> Result<WorkerOutput, PidagError> {
        // Increment concurrent counter and track peak
        let current = self.concurrent_count.fetch_add(1, Ordering::SeqCst) + 1;
        let mut peak = self.peak_concurrency.load(Ordering::SeqCst);
        while current > peak
            && self
                .peak_concurrency
                .compare_exchange(peak, current, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            peak = self.peak_concurrency.load(Ordering::SeqCst);
        }

        // Yield to allow other tasks to run
        tokio::task::yield_now().await;

        // Record dispatch order
        self.dispatch_order
            .lock()
            .unwrap()
            .push((node_id.to_string(), model.to_string()));

        // Look up outcome in script (Arc<HashMap> is not mutable, so just use it)
        let key = (node_id.to_string(), model.to_string(), attempt);
        let result = if let Some(outcome) = self.outcomes.get(&key) {
            match outcome {
                MockOutcome::Success(output) => Ok(WorkerOutput {
                    success: true,
                    output: output.clone(),
                    retryable: false,
                }),
                MockOutcome::Failure => Ok(WorkerOutput {
                    success: false,
                    output: "worker failed".to_string(),
                    retryable: false,
                }),
                MockOutcome::FailureWithMessage(msg) => Ok(WorkerOutput {
                    success: false,
                    output: msg.clone(),
                    retryable: false,
                }),
            }
        } else {
            // Default: fail (script must define all expected calls)
            Ok(WorkerOutput {
                success: false,
                output: "no script entry".to_string(),
                retryable: false,
            })
        };

        // Yield again before releasing
        tokio::task::yield_now().await;

        // Decrement concurrent counter
        self.concurrent_count.fetch_sub(1, Ordering::SeqCst);

        result
    }
}

impl MockWorkerScript {
    fn new() -> Self {
        Self {
            outcomes: Arc::new(std::collections::HashMap::new()),
            dispatch_order: Arc::new(Mutex::new(Vec::new())),
            concurrent_count: Arc::new(AtomicUsize::new(0)),
            peak_concurrency: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn script(self, node: &str, model: &str, attempt: usize, outcome: MockOutcome) -> Self {
        // Clone the hashmap, insert, create new Arc
        let mut new_outcomes = (*self.outcomes).clone();
        new_outcomes.insert((node.to_string(), model.to_string(), attempt), outcome);
        Self {
            outcomes: Arc::new(new_outcomes),
            dispatch_order: Arc::clone(&self.dispatch_order),
            concurrent_count: Arc::clone(&self.concurrent_count),
            peak_concurrency: Arc::clone(&self.peak_concurrency),
        }
    }

    fn peak_concurrency(&self) -> usize {
        self.peak_concurrency.load(Ordering::SeqCst)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn test_parse_valid_dag() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "b", "prompt": "p2", "depends_on": ["a"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "c", "prompt": "p3", "depends_on": ["b"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Result<Dag, _> =
        serde_json::from_str(json).map_err(|e| PidagError::Parse(e.to_string()));
    assert!(dag.is_ok());
    let dag = dag.unwrap();
    assert_eq!(dag.nodes.len(), 3);
    assert_eq!(dag.nodes[0].id, "a");
    assert_eq!(dag.nodes[1].id, "b");
    assert_eq!(dag.nodes[2].id, "c");
}

#[test]
fn test_parse_invalid_json_error() {
    let json = r#"{ invalid json }"#;
    let dag: Result<Dag, _> =
        serde_json::from_str(json).map_err(|e| PidagError::Parse(e.to_string()));
    assert!(dag.is_err());
}

#[test]
fn test_validate_rejects_cycle() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": ["b"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "b", "prompt": "p2", "depends_on": ["a"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    let result = dag.validate();
    assert!(matches!(result, Err(PidagError::Cycle)));
}

#[test]
fn test_validate_rejects_dangling_dependency() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": ["nonexistent"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    let result = dag.validate();
    assert!(matches!(result, Err(PidagError::UnknownDependency)));
}

#[test]
fn test_topo_sort_linear() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "b", "prompt": "p2", "depends_on": ["a"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "c", "prompt": "p3", "depends_on": ["b"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();
    let order = dag.topo_sort().unwrap();
    assert_eq!(order, vec!["a", "b", "c"]);
}

#[test]
fn test_topo_sort_diamond() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "b", "prompt": "p2", "depends_on": ["a"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "c", "prompt": "p3", "depends_on": ["a"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "d", "prompt": "p4", "depends_on": ["b", "c"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();
    let order = dag.topo_sort().unwrap();

    assert_eq!(order[0], "a");
    let b_idx = order.iter().position(|&x| x == "b").unwrap();
    let c_idx = order.iter().position(|&x| x == "c").unwrap();
    let d_idx = order.iter().position(|&x| x == "d").unwrap();

    assert!(b_idx < d_idx);
    assert!(c_idx < d_idx);
}

#[test]
fn test_topo_roots_multi_root() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "b", "prompt": "p2", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();
    let roots = dag.ready_nodes().unwrap();
    assert_eq!(roots.len(), 2);
    assert!(roots.contains(&"a".to_string()));
    assert!(roots.contains(&"b".to_string()));
}

#[tokio::test]
async fn test_scheduler_runs_single_node() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new().script(
        "a",
        "model1",
        1,
        MockOutcome::Success("output1".to_string()),
    );

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states.len(), 1);
    let node_state = &report.node_states[0];
    assert_eq!(node_state.node_id, "a");
    assert_eq!(node_state.state, NodeStatus::Done);
    assert_eq!(node_state.output, Some("output1".to_string()));
}

#[tokio::test]
async fn test_scheduler_respects_dependencies() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "b", "prompt": "p2", "depends_on": ["a"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new()
        .script(
            "a",
            "model1",
            1,
            MockOutcome::Success("output_a".to_string()),
        )
        .script(
            "b",
            "model1",
            1,
            MockOutcome::Success("output_b".to_string()),
        );

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states.len(), 2);
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert_eq!(report.node_states[1].state, NodeStatus::Done);
}

#[tokio::test]
async fn test_scheduler_parallel_dispatch() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "b", "prompt": "p2", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "c", "prompt": "p3", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new()
        .script("a", "model1", 1, MockOutcome::Success("out_a".to_string()))
        .script("b", "model1", 1, MockOutcome::Success("out_b".to_string()))
        .script("c", "model1", 1, MockOutcome::Success("out_c".to_string()));

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker.clone()), Box::new(sink), 3);
    scheduler.run(false).await.unwrap();

    assert_eq!(worker.peak_concurrency(), 3);
}

#[tokio::test]
async fn test_scheduler_concurrency_limit() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "b", "prompt": "p2", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "c", "prompt": "p3", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new()
        .script("a", "model1", 1, MockOutcome::Success("out_a".to_string()))
        .script("b", "model1", 1, MockOutcome::Success("out_b".to_string()))
        .script("c", "model1", 1, MockOutcome::Success("out_c".to_string()));

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker.clone()), Box::new(sink), 1);
    scheduler.run(false).await.unwrap();

    assert_eq!(worker.peak_concurrency(), 1);
}

#[tokio::test]
async fn test_scheduler_retry_on_failure() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 2, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new()
        .script("a", "model1", 1, MockOutcome::Failure)
        .script(
            "a",
            "model1",
            2,
            MockOutcome::Success("output_a".to_string()),
        );

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert_eq!(report.node_states[0].attempts, 2);

    let events = sink.events();
    let retry_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::NodeRetry { .. }))
        .collect();
    assert_eq!(retry_events.len(), 1);
}

#[tokio::test]
async fn test_scheduler_provider_fallback() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}, {"name": "model2", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new()
        .script("a", "model1", 1, MockOutcome::Failure)
        .script(
            "a",
            "model2",
            1,
            MockOutcome::Success("output_a".to_string()),
        );

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert_eq!(report.node_states[0].model, Some("model2".to_string()));

    let events = sink.events();
    let fallback_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::ProviderFallback { .. }))
        .collect();
    assert_eq!(fallback_events.len(), 1);
}

#[tokio::test]
async fn test_scheduler_exhausts_retries() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new().script("a", "model1", 1, MockOutcome::Failure);

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states[0].state, NodeStatus::Failed);
    assert!(!report.failed.is_empty());
}

#[tokio::test]
async fn test_scheduler_validate_clause_fail() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": "expected_string"}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new().script(
        "a",
        "model1",
        1,
        MockOutcome::Success("wrong_output".to_string()),
    );

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states[0].state, NodeStatus::Failed);
}

#[tokio::test]
async fn test_scheduler_blocks_dependents_on_failure() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "b", "prompt": "p2", "depends_on": ["a"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new().script("a", "model1", 1, MockOutcome::Failure);

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink), 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states[0].state, NodeStatus::Failed);
    assert_eq!(report.node_states[1].state, NodeStatus::Blocked);
}

#[tokio::test]
async fn test_paid_fallback_skipped_without_allow_paid() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": true}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new();

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink), 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states[0].state, NodeStatus::Failed);
}

#[tokio::test]
async fn test_paid_fallback_used_with_allow_paid() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": true}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new().script(
        "a",
        "model1",
        1,
        MockOutcome::Success("output_a".to_string()),
    );

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink), 1);
    let report = scheduler.run(true).await.unwrap();

    assert_eq!(report.node_states[0].state, NodeStatus::Done);
}

#[tokio::test]
async fn test_event_log_lifecycle_order() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new().script(
        "a",
        "model1",
        1,
        MockOutcome::Success("output_a".to_string()),
    );

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    scheduler.run(false).await.unwrap();

    let events = sink.events();

    assert!(events.len() >= 3);
    assert!(matches!(events[0], Event::DagSubmitted));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::NodeDispatched { .. }))
    );
    assert!(events.iter().any(|e| matches!(e, Event::NodeDone { .. })));
    assert!(matches!(events[events.len() - 1], Event::DagDone { .. }));
}

#[test]
fn test_jsonl_sink_writes_parseable_lines() {
    // Just test that events are JSON serializable
    let event1 = Event::DagSubmitted;
    let json1 = serde_json::to_string(&event1).unwrap();
    let line1 = format!("{}\n", json1);

    let event2 = Event::DagDone {
        successful_nodes: 1,
        failed_nodes: 0,
    };
    let json2 = serde_json::to_string(&event2).unwrap();
    let line2 = format!("{}\n", json2);

    let lines = vec![line1.trim(), line2.trim()];
    assert_eq!(lines.len(), 2);

    for line in lines {
        let _: Event = serde_json::from_str(line).unwrap();
    }
}

#[tokio::test]
async fn test_no_silent_transition() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null},
        {"id": "b", "prompt": "p2", "depends_on": ["a"], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new()
        .script("a", "model1", 1, MockOutcome::Success("out_a".to_string()))
        .script("b", "model1", 1, MockOutcome::Success("out_b".to_string()));

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    scheduler.run(false).await.unwrap();

    let events = sink.events();

    let lifecycle_count = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::NodeDispatched { .. }
                    | Event::NodeDone { .. }
                    | Event::NodeFailed { .. }
                    | Event::NodeBlocked { .. }
                    | Event::NodeRetry { .. }
                    | Event::ProviderFallback { .. }
            )
        })
        .count();

    let total_transitions = 2 * 2; // 2 nodes, each needs Dispatched + Done
    assert!(lifecycle_count >= total_transitions);
}

// Regression test for the 2026-08-02 audit P1 #4:
// `Event::NodeDispatched { attempt }` must report the actual retry attempt
// that the worker executed, not a hardcoded `1`. Prior to the fix the outer
// loop set `task_attempt = 1` regardless of which attempt landed.
#[tokio::test]
async fn test_node_dispatched_reports_actual_attempt_on_success() {
    // attempts=3; succeed on the 3rd attempt. The NodeDispatched event for
    // the (eventually successful) node must carry attempt == 3, not 1.
    let json = r#"
    {
      "nodes": [
        {"id": "node-3rd-times", "prompt": "p", "depends_on": [],
         "models": [{"name": "model1", "paid": false}],
         "retry": {"attempts": 3, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new()
        .script("node-3rd-times", "model1", 1, MockOutcome::Failure)
        .script("node-3rd-times", "model1", 2, MockOutcome::Failure)
        .script(
            "node-3rd-times",
            "model1",
            3,
            MockOutcome::Success("ok".to_string()),
        );

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    scheduler.run(false).await.unwrap();

    let events = sink.events();

    let dispatched_attempt = events.iter().find_map(|e| match e {
        Event::NodeDispatched { attempt, .. } => Some(*attempt),
        _ => None,
    });
    assert_eq!(
        dispatched_attempt,
        Some(3),
        "NodeDispatched.attempt must reflect the actual landed attempt (3), not 1"
    );

    // The node did succeed on attempt 3; we still expect a NodeDone after.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::NodeDone { node_id, .. } if node_id == "node-3rd-times"))
    );
}

// On hard failure (all attempts exhausted, real non-retryable errors),
// NodeDispatched.attempt must report the last attempt actually run
// (node.retry.attempts), not 1.
#[tokio::test]
async fn test_node_dispatched_reports_last_attempt_on_failure() {
    let json = r#"
    {
      "nodes": [
        {"id": "always-fails", "prompt": "p", "depends_on": [],
         "models": [{"name": "model1", "paid": false}],
         "retry": {"attempts": 2, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new()
        .script("always-fails", "model1", 1, MockOutcome::Failure)
        .script("always-fails", "model1", 2, MockOutcome::Failure);

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    scheduler.run(false).await.unwrap();

    let dispatch_attempt = sink.events().iter().find_map(|e| match e {
        Event::NodeDispatched { attempt, .. } => Some(*attempt),
        _ => None,
    });
    assert_eq!(
        dispatch_attempt,
        Some(2),
        "NodeDispatched.attempt must be the last attempt actually run (2) on hard failure"
    );
}

// ============================================================================
// P1 #5: node-dispatch timeout (HANDOFF 2026-08-02 audit). A node whose
// `timeout` is set must wrap each `worker.run` in `tokio::time::timeout`
// and treat the deadline-elapse as a non-retryable hard failure.
// ============================================================================

#[derive(Clone)]
struct DelayWorker {
    delay_ms: u64,
    output: String,
    dispatch_order: Arc<Mutex<Vec<(String, String)>>>,
}

impl DelayWorker {
    fn new(delay_ms: u64, output: String) -> Self {
        Self {
            delay_ms,
            output,
            dispatch_order: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl Worker for DelayWorker {
    async fn run(
        &self,
        node_id: &str,
        _prompt: &str,
        model: &str,
        _attempt: usize,
    ) -> Result<WorkerOutput, PidagError> {
        self.dispatch_order
            .lock()
            .unwrap()
            .push((node_id.to_string(), model.to_string()));
        if self.delay_ms > 0 {
            // Use std::thread::sleep (real wall-clock) deliberately, NOT
            // tokio::time::sleep (runtime virtual clock). The default
            // #[tokio::test] runtime pauses its clock, so a
            // tokio::time::sleep here would never advance the runtime's
            // virtual time and tokio::time::timeout's deadline would
            // likewise never fire — masking the dispatch-deadline logic
            // under test. thread::sleep blocks on the OS thread, so the
            // 20ms runtime timeout deadline fires while the worker is
            // still on real wall time, matching production (a worker
            // process wedged on real wall time). Wrapped in
            // spawn_blocking so we don't stall the runtime thread.
            let delay = std::time::Duration::from_millis(self.delay_ms);
            tokio::task::spawn_blocking(move || {
                std::thread::sleep(delay);
            })
            .await
            .ok();
        }
        Ok(WorkerOutput {
            success: true,
            output: self.output.clone(),
            retryable: false,
        })
    }
}

// Multi-thread runtime is required here (not the default `#[tokio::test]`
// current-thread flavor): `DelayWorker::run` parks a blocking-pool thread on
// real wall time via `std::thread::sleep` inside `spawn_blocking`. On a
// current-thread runtime, the main task `await`s the `spawn_blocking`
// `JoinHandle`, parking the executor — and the runtime's time driver
// (which advances the `tokio::time::timeout(20ms, ...)` deadline) only
// fires while the executor is polling. So on current-thread the 20ms
// deadline NEVER fires before the 80ms worker sleep completes, the test
// sees `Done` instead of `Failed`, and assertion fails. The multi-thread
// runtime dedicates a timer-driver thread that advances deadlines while
// the worker's blocking sleep runs on a separate blocking-pool thread,
// so the 20ms deadline fires deterministically. Models production reality
// (a worker process wedged on real wall time, observed from a separate
// scheduler thread).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_node_timeout_marks_node_failed_when_worker_exceeds_deadline() {
    use std::time::Duration;
    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "n1".to_string(),
            prompt: "p".to_string(),
            depends_on: vec![],
            models: vec![ModelRef {
                name: "m1".to_string(),
                paid: false,
            }],
            retry: RetryPolicy {
                attempts: 2,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("llm".to_string()),
            gate: None,
            // deadline shorter than the 80 ms worker sleep -> must elapse
            timeout: Some(Duration::from_millis(20)),
            mcp_call: None,
            after: vec![],
            verify: None,
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };
    // Worker would succeed, but only after 80ms — the 20ms deadline fires first.
    let worker = DelayWorker::new(80, "done".to_string());
    let worker_handle = worker.clone();
    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(
        report.node_states[0].state,
        NodeStatus::Failed,
        "node exceeding its timeout must be marked Failed"
    );
    let msg = report.node_states[0].output.as_deref().unwrap_or("");
    assert!(
        msg.contains("timed out"),
        "failure output must mention timeout; got {msg:?}"
    );
    // Hard (non-retryable) failure: attempts budget must NOT be burned on backoff.
    assert_eq!(
        report.node_states[0].attempts, 0,
        "timeout is non-retryable; final_state.attempts stays at its init 0"
    );
    // Exactly one worker invocation recorded — the timeout canceled attempt 1,
    // it was a hard failure, so attempt 2 was never dispatched.
    assert_eq!(
        worker_handle.dispatch_order.lock().unwrap().len(),
        1,
        "non-retryable timeout must not dispatch a second attempt"
    );
}

#[tokio::test]
async fn test_node_timeout_none_preserves_worker_success() {
    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "n2".to_string(),
            prompt: "p".to_string(),
            depends_on: vec![],
            models: vec![ModelRef {
                name: "m1".to_string(),
                paid: false,
            }],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("llm".to_string()),
            gate: None,
            timeout: None, // no scheduler-level deadline
            mcp_call: None,
            after: vec![],
            verify: None,
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };
    let worker = DelayWorker::new(20, "ok".to_string());
    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.unwrap();
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert_eq!(report.node_states[0].attempts, 1);
}

// ============================================================================
// spec-14 (Bug A): conditional gate nodes — fire on source-fail, skip on pass
// ============================================================================

fn gate_dag(v1_deps: &[&str], fix_depends: Vec<&str>, fix_gate: Option<&str>, after: bool) -> Dag {
    // Build a small dag serde_json manually.
    let mut nodes = String::new();
    // v1 (leaf or depends per v1_deps)
    nodes.push_str(&format!(
        r#"{{"id":"v1","prompt":"p","depends_on":[{}],"models":[{{"name":"model1","paid":false}}],"retry":{{"attempts":1,"backoff_ms":0}},"validate":null}},"#,
        v1_deps.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(",")
    ));
    // v1b (done leaf, used in the multi-dep test)
    if fix_depends.contains(&"v1b") || v1_deps.contains(&"v1b") {
        nodes.push_str(r#"{"id":"v1b","prompt":"p","depends_on":[],"models":[{"name":"model1","paid":false}],"retry":{"attempts":1,"backoff_ms":0},"validate":null},"#);
    }
    let gate_str = match fix_gate {
        Some(g) => format!("\"{g}\""),
        None => "null".to_string(),
    };
    let fix_deps_str = fix_depends
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(",");
    nodes.push_str(&format!(
        r#"{{"id":"fix","prompt":"p","depends_on":[{fix_deps_str}],"models":[{{"name":"model1","paid":false}}],"retry":{{"attempts":1,"backoff_ms":0}},"validate":null,"gate":{gate_str}}},"#
    ));
    if after {
        nodes.push_str(r#"{"id":"after","prompt":"p","depends_on":["fix"],"models":[{"name":"model1","paid":false}],"retry":{"attempts":1,"backoff_ms":0},"validate":null}"#);
    } else {
        // remove trailing comma
        nodes = nodes.trim_end_matches(',').to_string();
    }
    let json = format!(r#"{{"nodes":[{nodes}]}}"#);
    let dag: Dag = serde_json::from_str(&json).expect("valid gate dag");
    dag.validate().expect("dag valid");
    dag
}

fn state_of<'r>(
    report: &'r pidag::scheduler::RunReport,
    id: &str,
) -> &'r pidag::scheduler::NodeState {
    report
        .node_states
        .iter()
        .find(|s| s.node_id == id)
        .unwrap_or_else(|| panic!("node {id} missing from report"))
}

#[tokio::test]
async fn test_gate_fires_on_source_fail_then_downstream() {
    // v1 fails -> `fix` (gate v1:fail) must dispatch -> `after` (dep fix) runs.
    let dag = gate_dag(&[], vec!["v1"], Some("v1:fail"), true);
    let worker = MockWorkerScript::new()
        .script("v1", "model1", 1, MockOutcome::Failure)
        .script(
            "fix",
            "model1",
            1,
            MockOutcome::Success("fixed".to_string()),
        )
        .script(
            "after",
            "model1",
            1,
            MockOutcome::Success("done".to_string()),
        );

    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 2);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(state_of(&report, "v1").state, NodeStatus::Failed);
    assert_eq!(state_of(&report, "fix").state, NodeStatus::Done);
    assert_eq!(
        state_of(&report, "fix").attempts,
        1,
        "fix node must actually dispatch (fire on fail)"
    );
    assert_eq!(state_of(&report, "after").state, NodeStatus::Done);
    assert!(report.failed.contains(&"v1".to_string()));
}

#[tokio::test]
async fn test_gate_skips_on_source_pass() {
    // v1 done -> `fix` (gate v1:fail) must be SKIPPED (not run, not failed);
    // its dependent `after` still proceeds via the cascade.
    let dag = gate_dag(&[], vec!["v1"], Some("v1:fail"), true);
    let worker = MockWorkerScript::new()
        .script("v1", "model1", 1, MockOutcome::Success("ok".to_string()))
        .script(
            "after",
            "model1",
            1,
            MockOutcome::Success("done".to_string()),
        );

    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 2);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(state_of(&report, "v1").state, NodeStatus::Done);
    assert_eq!(state_of(&report, "fix").state, NodeStatus::Done);
    assert_eq!(
        state_of(&report, "fix").attempts,
        0,
        "skipped gate node must NOT dispatch"
    );
    assert_eq!(state_of(&report, "after").state, NodeStatus::Done);
    assert!(report.failed.is_empty());
}

#[tokio::test]
async fn test_gate_blocks_when_not_matching_source() {
    // v1 fails but `fix` gate names a different source -> Blocked (existing).
    let dag = gate_dag(&[], vec!["v1"], Some("other:fail"), false);
    let worker = MockWorkerScript::new().script("v1", "model1", 1, MockOutcome::Failure);

    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 2);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(state_of(&report, "v1").state, NodeStatus::Failed);
    assert_eq!(state_of(&report, "fix").state, NodeStatus::Blocked);
}

#[tokio::test]
async fn test_gate_waits_for_all_deps() {
    // fix depends on [v1, v1b]; v1 fails, v1b done -> fix fires only after BOTH.
    let dag = gate_dag(&[], vec!["v1", "v1b"], Some("v1:fail"), false);
    let worker = MockWorkerScript::new()
        .script("v1", "model1", 1, MockOutcome::Failure)
        .script("v1b", "model1", 1, MockOutcome::Success("ok".to_string()))
        .script(
            "fix",
            "model1",
            1,
            MockOutcome::Success("fixed".to_string()),
        );

    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 3);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(state_of(&report, "v1").state, NodeStatus::Failed);
    assert_eq!(state_of(&report, "v1b").state, NodeStatus::Done);
    assert_eq!(
        state_of(&report, "fix").state,
        NodeStatus::Done,
        "fix fires after all deps terminal (v1 failed, v1b done)"
    );
    assert_eq!(state_of(&report, "fix").attempts, 1);
}

#[tokio::test]
async fn test_gate_skip_cascades_chain() {
    // n0 done -> g1 (gate n0:fail) skipped -> g2 (dep g1) must still run.
    let json = r#"
    {
      "nodes": [
        {"id":"n0","prompt":"p","depends_on":[],"models":[{"name":"model1","paid":false}],"retry":{"attempts":1,"backoff_ms":0},"validate":null},
        {"id":"g1","prompt":"p","depends_on":["n0"],"models":[{"name":"model1","paid":false}],"retry":{"attempts":1,"backoff_ms":0},"validate":null,"gate":"n0:fail"},
        {"id":"g2","prompt":"p","depends_on":["g1"],"models":[{"name":"model1","paid":false}],"retry":{"attempts":1,"backoff_ms":0},"validate":null}
      ]
    }
    "#;
    let dag2: Dag = serde_json::from_str(json).unwrap();
    dag2.validate().unwrap();
    let worker2 = MockWorkerScript::new()
        .script("n0", "model1", 1, MockOutcome::Success("ok".to_string()))
        .script("g2", "model1", 1, MockOutcome::Success("done".to_string()));

    let mut scheduler = Scheduler::new(dag2, Box::new(worker2), Box::new(VecSink::new()), 3);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(state_of(&report, "n0").state, NodeStatus::Done);
    assert_eq!(state_of(&report, "g1").state, NodeStatus::Done);
    assert_eq!(state_of(&report, "g1").attempts, 0, "g1 is skipped");
    assert_eq!(
        state_of(&report, "g2").state,
        NodeStatus::Done,
        "g2 proceeds after g1 skipped"
    );
    assert!(report.failed.is_empty());
    let _ = worker2;
}

// ============================================================================
// Failure reporting tests
// ============================================================================

/// T1: A worker returning success:false with a custom error message should produce
/// a NodeFailed event with that error message (truncated to 8 KB).
#[tokio::test]
async fn test_failed_node_error_carries_worker_output() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let error_msg = "boom: model not found";
    let worker = MockWorkerScript::new().script(
        "a",
        "model1",
        1,
        MockOutcome::FailureWithMessage(error_msg.to_string()),
    );

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states[0].state, NodeStatus::Failed);

    let events = sink.events();
    let failed_events: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let Event::NodeFailed { node_id, error } = e {
                if node_id == "a" {
                    Some(error.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    assert_eq!(failed_events.len(), 1);
    assert_eq!(failed_events[0], error_msg);
}

/// T2: Empty worker output should fall back to "execution failed" in the error field.
#[tokio::test]
async fn test_failed_node_error_falls_back_when_output_empty() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let worker = MockWorkerScript::new().script(
        "a",
        "model1",
        1,
        MockOutcome::FailureWithMessage("".to_string()),
    );

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states[0].state, NodeStatus::Failed);

    let events = sink.events();
    let failed_events: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let Event::NodeFailed { node_id, error } = e {
                if node_id == "a" {
                    Some(error.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    assert_eq!(failed_events.len(), 1);
    assert_eq!(failed_events[0], "execution failed");
}

/// T3: A large worker output (>8 KB) should be truncated with a marker indicating
/// how many bytes were cut.
#[tokio::test]
async fn test_failed_node_error_is_truncated() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    // Create a 100 KB error message
    let large_error = "x".repeat(102400);
    let worker = MockWorkerScript::new().script(
        "a",
        "model1",
        1,
        MockOutcome::FailureWithMessage(large_error.clone()),
    );

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states[0].state, NodeStatus::Failed);

    let events = sink.events();
    let failed_events: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let Event::NodeFailed { node_id, error } = e {
                if node_id == "a" {
                    Some(error.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    assert_eq!(failed_events.len(), 1);
    let error = &failed_events[0];

    // Should end with truncation marker
    assert!(
        error.contains("… [truncated"),
        "error should contain truncation marker"
    );
    // Should be around 8 KB (a bit more for the marker)
    assert!(
        error.len() < 8500,
        "error should be capped at ~8 KB, got {} bytes",
        error.len()
    );
    // Should contain the truncation count
    assert!(
        error.contains("truncated 94"),
        "error should contain truncation count, got: {}",
        error
    );
}

/// T4: A failed node's output should be stored as an artifact for retrieval via `pidag show`.
#[tokio::test]
async fn test_failed_node_artifact_is_stored() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    let error_msg = "Model not found: /deepseek-v4-flash";
    let worker = MockWorkerScript::new().script(
        "a",
        "model1",
        1,
        MockOutcome::FailureWithMessage(error_msg.to_string()),
    );

    let store = Arc::new(MockStore::new());
    let run_id = "test-run-123".to_string();

    // Pre-seed the run
    let run = pidag::store::RunMeta {
        run_id: run_id.clone(),
        dag_json: "{}".to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        successful_nodes: 0,
        failed_nodes: 0,
    };
    store.put_run(&run).await.expect("put_run");

    let event_sink = Box::new(RedbSink::new(store.clone(), run_id.clone()));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), event_sink, 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states[0].state, NodeStatus::Failed);

    // Verify the artifact was stored
    let stored_artifact = store
        .get_artifact(&run_id, "a")
        .await
        .expect("get_artifact")
        .expect("artifact should exist");

    assert_eq!(stored_artifact, error_msg);
}

/// T5: A large artifact (>8 KB) should be truncated when stored.
#[tokio::test]
async fn test_failed_node_artifact_truncated_when_stored() {
    let json = r#"
    {
      "nodes": [
        {"id": "a", "prompt": "p1", "depends_on": [], "models": [{"name": "model1", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null}
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    dag.validate().unwrap();

    // Create a 100 KB error message
    let large_error = "error output line\n".repeat(6000);
    let worker = MockWorkerScript::new().script(
        "a",
        "model1",
        1,
        MockOutcome::FailureWithMessage(large_error.clone()),
    );

    let store = Arc::new(MockStore::new());
    let run_id = "test-run-456".to_string();

    let run = pidag::store::RunMeta {
        run_id: run_id.clone(),
        dag_json: "{}".to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        successful_nodes: 0,
        failed_nodes: 0,
    };
    store.put_run(&run).await.expect("put_run");

    let event_sink = Box::new(RedbSink::new(store.clone(), run_id.clone()));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), event_sink, 1);
    let report = scheduler.run(false).await.unwrap();

    assert_eq!(report.node_states[0].state, NodeStatus::Failed);

    // Verify the artifact was stored and truncated
    let stored_artifact = store
        .get_artifact(&run_id, "a")
        .await
        .expect("get_artifact")
        .expect("artifact should exist");

    // The artifact should be truncated to around 8 KB
    assert!(
        stored_artifact.len() <= 8500,
        "artifact should be truncated to ~8 KB, got {} bytes",
        stored_artifact.len()
    );
    // Should end with truncation marker
    assert!(
        stored_artifact.contains("… [truncated"),
        "artifact should contain truncation marker"
    );
}

// ============================================================================
// R3 Tests: Transitive Skip Cascade
// ============================================================================

/// Test R3a: Skip cascades two levels.
///
/// Chain: A → gated B (gate = "A:fail") → gated C (gate = "B:fail"), where A succeeds.
/// Both B and C must end Done with attempts: 0, the run must complete, and
/// no node may be left Pending.
///
/// This test MUST fail against current code, which only cascades one level and
/// leaves C in a dispatched state (wrong-behaviour bug).
#[tokio::test]
async fn test_skip_cascades_two_levels() {
    // Create DAG: A (succeeds) → B (gate="A:fail") → C (gate="B:fail")
    let json = r#"
    {
      "nodes": [
        {
          "id": "a",
          "prompt": "p1",
          "node_type": "shell",
          "depends_on": [],
          "models": [],
          "retry": {"attempts": 1, "backoff_ms": 0},
          "validate": null,
          "gate": null,
          "after": []
        },
        {
          "id": "b",
          "prompt": "p2",
          "node_type": "shell",
          "depends_on": ["a"],
          "models": [],
          "retry": {"attempts": 1, "backoff_ms": 0},
          "validate": null,
          "gate": "a:fail",
          "after": []
        },
        {
          "id": "c",
          "prompt": "p3",
          "node_type": "shell",
          "depends_on": ["b"],
          "models": [],
          "retry": {"attempts": 1, "backoff_ms": 0},
          "validate": null,
          "gate": "b:fail",
          "after": []
        }
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).expect("Failed to parse DAG");

    // Shell nodes always succeed (empty models array).
    // Note: C has NO script entry. If C is dispatched (the bug), it will fail
    // with "no script entry". If C is correctly skipped, it will be Done with attempts: 0.
    let worker = MockWorkerScript::new()
        .script("a", "", 1, MockOutcome::Success(String::new()))
        .script("b", "", 1, MockOutcome::Success(String::new()));
    // Intentionally omitting script for "c" to catch if it's dispatched

    let store = Arc::new(MockStore::new());
    let run_id = "test-r3a-cascade".to_string();

    let run = pidag::store::RunMeta {
        run_id: run_id.clone(),
        dag_json: json.to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        successful_nodes: 0,
        failed_nodes: 0,
    };
    store.put_run(&run).await.expect("put_run");

    let event_sink = Box::new(RedbSink::new(store.clone(), run_id.clone()));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), event_sink, 1);
    let report = scheduler.run(false).await.expect("run should succeed");

    // Verify the report
    let mut a_state = None;
    let mut b_state = None;
    let mut c_state = None;

    for state in &report.node_states {
        match state.node_id.as_str() {
            "a" => a_state = Some(state),
            "b" => b_state = Some(state),
            "c" => c_state = Some(state),
            _ => {}
        }
    }

    let a = a_state.expect("Node A should exist");
    let b = b_state.expect("Node B should exist");
    let c = c_state.expect("Node C should exist");

    // A should be Done (it executed)
    assert_eq!(
        a.state,
        NodeStatus::Done,
        "Node A should be Done, got {}",
        a.state
    );

    // B should be Done with attempts: 0 (skipped, not executed)
    assert_eq!(
        b.state,
        NodeStatus::Done,
        "Node B should be Done (skipped), got {}",
        b.state
    );
    assert_eq!(
        b.attempts, 0,
        "Node B should have attempts: 0 (skipped), got {}",
        b.attempts
    );

    // C should be Done with attempts: 0 (cascaded skip, not executed)
    assert_eq!(
        c.state,
        NodeStatus::Done,
        "Node C should be Done (cascaded skip), got {}",
        c.state
    );
    assert_eq!(
        c.attempts, 0,
        "Node C should have attempts: 0 (cascaded skip), got {}",
        c.attempts
    );

    // No node should be Pending
    for state in &report.node_states {
        assert!(
            state.state != NodeStatus::Pending,
            "Node {} should not be Pending after run",
            state.node_id
        );
    }
}

/// Test R3b: Skip cascade releases after-edges.
///
/// Chain: A → B (gate="A:fail") → C (gate="B:fail"), plus D with after=["C"].
/// D must dispatch when C is skipped and becomes terminal.
#[tokio::test]
async fn test_skip_cascade_releases_after_edges() {
    // Create DAG: A (succeeds) → B (gate="A:fail") → C (gate="B:fail"), D after C
    let json = r#"
    {
      "nodes": [
        {
          "id": "a",
          "prompt": "p1",
          "node_type": "shell",
          "depends_on": [],
          "models": [],
          "retry": {"attempts": 1, "backoff_ms": 0},
          "validate": null,
          "gate": null,
          "after": []
        },
        {
          "id": "b",
          "prompt": "p2",
          "node_type": "shell",
          "depends_on": ["a"],
          "models": [],
          "retry": {"attempts": 1, "backoff_ms": 0},
          "validate": null,
          "gate": "a:fail",
          "after": []
        },
        {
          "id": "c",
          "prompt": "p3",
          "node_type": "shell",
          "depends_on": ["b"],
          "models": [],
          "retry": {"attempts": 1, "backoff_ms": 0},
          "validate": null,
          "gate": "b:fail",
          "after": []
        },
        {
          "id": "d",
          "prompt": "p4",
          "node_type": "shell",
          "depends_on": [],
          "models": [],
          "retry": {"attempts": 1, "backoff_ms": 0},
          "validate": null,
          "gate": null,
          "after": ["c"]
        }
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).expect("Failed to parse DAG");

    // Shell nodes always succeed
    let worker = MockWorkerScript::new()
        .script("a", "", 1, MockOutcome::Success(String::new()))
        .script("b", "", 1, MockOutcome::Success(String::new()))
        .script("c", "", 1, MockOutcome::Success(String::new()))
        .script("d", "", 1, MockOutcome::Success(String::new()));

    let store = Arc::new(MockStore::new());
    let run_id = "test-r3b-after".to_string();

    let run = pidag::store::RunMeta {
        run_id: run_id.clone(),
        dag_json: json.to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        successful_nodes: 0,
        failed_nodes: 0,
    };
    store.put_run(&run).await.expect("put_run");

    let event_sink = Box::new(RedbSink::new(store.clone(), run_id.clone()));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), event_sink, 2);
    let report = scheduler.run(false).await.expect("run should succeed");

    // Find node D in the report
    let d_state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "d")
        .expect("Node D should exist");

    // D should be Done (dispatched after C became terminal)
    assert_eq!(
        d_state.state,
        NodeStatus::Done,
        "Node D should be Done (after-edge satisfied by skipped C), got {}",
        d_state.state
    );

    // No node should be Pending
    for state in &report.node_states {
        assert!(
            state.state != NodeStatus::Pending,
            "Node {} should not be Pending after run",
            state.node_id
        );
    }
}

/// Test R3c: Skip cascade terminates cleanly.
///
/// A skipped node whose dependent is already terminal should not cause
/// an infinite loop. The run should complete.
#[tokio::test]
async fn test_skip_cascade_terminates() {
    // Create DAG where skip might theoretically loop
    let json = r#"
    {
      "nodes": [
        {
          "id": "a",
          "prompt": "p1",
          "node_type": "shell",
          "depends_on": [],
          "models": [],
          "retry": {"attempts": 1, "backoff_ms": 0},
          "validate": null,
          "gate": null,
          "after": []
        },
        {
          "id": "b",
          "prompt": "p2",
          "node_type": "shell",
          "depends_on": ["a"],
          "models": [],
          "retry": {"attempts": 1, "backoff_ms": 0},
          "validate": null,
          "gate": "a:fail",
          "after": []
        }
      ]
    }
    "#;

    let dag: Dag = serde_json::from_str(json).expect("Failed to parse DAG");

    // Shell nodes always succeed
    let worker = MockWorkerScript::new()
        .script("a", "", 1, MockOutcome::Success(String::new()))
        .script("b", "", 1, MockOutcome::Success(String::new()));

    let store = Arc::new(MockStore::new());
    let run_id = "test-r3c-terminate".to_string();

    let run = pidag::store::RunMeta {
        run_id: run_id.clone(),
        dag_json: json.to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        successful_nodes: 0,
        failed_nodes: 0,
    };
    store.put_run(&run).await.expect("put_run");

    let event_sink = Box::new(RedbSink::new(store.clone(), run_id.clone()));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), event_sink, 1);

    // This should complete without hanging or looping
    let report = scheduler.run(false).await.expect("run should complete");

    // Verify the run completed
    assert_eq!(report.node_states.len(), 2, "Should have 2 nodes in report");

    // No node should be Pending
    for state in &report.node_states {
        assert!(
            state.state != NodeStatus::Pending,
            "Node {} should not be Pending after run",
            state.node_id
        );
    }
}
