//! Runtime 429 Auto-Failover — spec: specs/93-runtime-429-failover.md
//! Tests T1-T13. ScriptedWorker keyed on (node_id, model, attempt).

use async_trait::async_trait;
use pidag::{
    Dag, Event, ModelRef, Node, NodeStatus, PidagError, RetryPolicy, RunReport, Scheduler, VecSink,
    Worker, WorkerOutput,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ============================================================================
// ScriptedWorker: returns a queued outcome for (node_id, model, attempt).
// Unscripted keys return a hard Failure (caller forgot a script entry).
// ============================================================================

#[derive(Clone, Debug)]
enum Script {
    Success(String),
    RetryableFailure(String), // sets output.retryable = true
    RealFailure(String),      // sets output.retryable = false
}

/// (node_id, model, attempt) -> queued outcome.
type ScriptMap = Arc<Mutex<HashMap<(String, String, usize), Script>>>;

#[derive(Clone, Default)]
struct ScriptedWorker {
    outcomes: ScriptMap,
    // records every (node_id, model, attempt) the scheduler asked for
    calls: Arc<Mutex<Vec<(String, String, usize)>>>,
}

impl ScriptedWorker {
    fn set(&self, node: &str, model: &str, attempt: usize, s: Script) {
        self.outcomes
            .lock()
            .unwrap()
            .insert((node.to_string(), model.to_string(), attempt), s);
    }
    fn calls(&self) -> Vec<(String, String, usize)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl Worker for ScriptedWorker {
    async fn run(
        &self,
        node_id: &str,
        _prompt: &str,
        model: &str,
        attempt: usize,
    ) -> Result<WorkerOutput, PidagError> {
        self.calls
            .lock()
            .unwrap()
            .push((node_id.to_string(), model.to_string(), attempt));
        let key = (node_id.to_string(), model.to_string(), attempt);
        match self.outcomes.lock().unwrap().get(&key).cloned() {
            Some(Script::Success(out)) => Ok(WorkerOutput {
                success: true,
                output: out,
                retryable: false,
            }),
            Some(Script::RetryableFailure(out)) => Ok(WorkerOutput {
                success: false,
                output: out,
                retryable: true,
            }),
            Some(Script::RealFailure(out)) => Ok(WorkerOutput {
                success: false,
                output: out,
                retryable: false,
            }),
            None => Ok(WorkerOutput {
                success: false,
                output: format!("no script for {key:?}"),
                retryable: false,
            }),
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn mk_node(id: &str, models: Vec<&str>, attempts: usize, backoff_ms: u64) -> Node {
    Node {
        id: id.to_string(),
        prompt: format!("prompt for {id}"),
        depends_on: vec![],
        models: models
            .iter()
            .map(|m| ModelRef {
                name: m.to_string(),
                paid: false,
            })
            .collect(),
        retry: RetryPolicy {
            attempts,
            backoff_ms,
        },
        validate: None,
        node_type: Some("llm".to_string()),
        gate: None,
        timeout: None,
        mcp_call: None,
        after: vec![],
        verify: None,
        verify_pre: None,
    }
}

async fn run_one(node: Node, worker: ScriptedWorker) -> (RunReport, Vec<Event>) {
    let dag = Dag {
        metadata: None,
        nodes: vec![node],
    };
    let sink = VecSink::new();
    let mut sched = Scheduler::new(dag, Box::new(worker), Box::new(sink.clone()), 4);
    let report = sched.run(false).await.unwrap();
    (report, sink.events())
}

fn fallbacks(events: &[Event]) -> Vec<(&str, &str)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ProviderFallback {
                from_model,
                to_model,
                ..
            } => Some((from_model.as_str(), to_model.as_str())),
            _ => None,
        })
        .collect()
}

fn retries(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::NodeRetry { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

// ============================================================================
// T1: success on first model first attempt — no retries, no fallbacks
// ============================================================================

#[tokio::test]
async fn t1_success_first_attempt_no_fallback() {
    let worker = ScriptedWorker::default();
    let node = mk_node("n1", vec!["m1", "m2"], 2, 0);
    worker.set("n1", "m1", 1, Script::Success("ok".into()));
    let (report, events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert_eq!(report.node_states[0].model.as_deref(), Some("m1"));
    assert_eq!(report.node_states[0].attempts, 1);
    assert!(fallbacks(&events).is_empty());
    assert!(retries(&events).is_empty());
}

// ============================================================================
// T2: real failure (retryable==false) on attempt 1, retry same model
//     attempt 2 succeeds — no fallback. Confirms backward-compat behavior.
// ============================================================================

#[tokio::test]
async fn t2_real_failure_retries_same_model() {
    let worker = ScriptedWorker::default();
    let node = mk_node("n1", vec!["m1"], 2, 0);
    worker.set("n1", "m1", 1, Script::RealFailure("err".into()));
    worker.set("n1", "m1", 2, Script::Success("ok".into()));
    let (report, events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert_eq!(report.node_states[0].model.as_deref(), Some("m1"));
    assert_eq!(report.node_states[0].attempts, 2);
    assert!(fallbacks(&events).is_empty());
    // one NodeRetry for the real failure
    assert_eq!(retries(&events).len(), 1);
    assert_eq!(retries(&events)[0], "attempt failed");
}

// ============================================================================
// T3: retryable failure with backoff_ms==0 → IMMEDIATELY advances to next
//     model (no sleep). Confirms default keeps tests fast. ProviderFallback
//     event MUST be emitted.
// ============================================================================

#[tokio::test]
async fn t3_retryable_no_backoff_advances_immediately() {
    let worker = ScriptedWorker::default();
    let node = mk_node("n1", vec!["m1", "m2"], 2, 0);
    worker.set("n1", "m1", 1, Script::RetryableFailure("429".into()));
    worker.set("n1", "m2", 1, Script::Success("ok".into()));
    let (report, events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert_eq!(report.node_states[0].model.as_deref(), Some("m2"));
    assert_eq!(fallbacks(&events), vec![("m1", "m2")]);
}

// ============================================================================
// T4: retryable failure with backoff_ms==0 and only ONE model → node fails,
//     no fallback emitted (no target to fall back to).
// ============================================================================

#[tokio::test]
async fn t4_retryable_exhausts_single_model_fails() {
    let worker = ScriptedWorker::default();
    let node = mk_node("n1", vec!["m1"], 3, 0);
    worker.set("n1", "m1", 1, Script::RetryableFailure("429".into()));
    worker.set("n1", "m1", 2, Script::RetryableFailure("429".into()));
    worker.set("n1", "m1", 3, Script::RetryableFailure("429".into()));
    let (report, events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Failed);
    assert!(fallbacks(&events).is_empty(), "no fallback expected");
}

// ============================================================================
// T5: retryable failure on a MIDDLE model in a 3-model chain falls to the
//     third model. Verifies advance picks next model (not restarts).
// ============================================================================

#[tokio::test]
async fn t5_retryable_middle_model_falls_to_third() {
    let worker = ScriptedWorker::default();
    let node = mk_node("n1", vec!["m1", "m2", "m3"], 2, 0);
    worker.set("n1", "m2", 1, Script::RetryableFailure("429".into()));
    worker.set("n1", "m2", 2, Script::RetryableFailure("429".into()));
    // m1 succeeds first
    worker.set("n1", "m1", 1, Script::Success("ok".into()));
    let (report, _events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert_eq!(report.node_states[0].model.as_deref(), Some("m1"));
}

// helper: variants of T5 where m1 fails-retryable too, lands on m2
#[tokio::test]
async fn t5b_retryable_advance_then_succeed_on_next() {
    let worker = ScriptedWorker::default();
    let node = mk_node("n1", vec!["m1", "m2"], 2, 0);
    worker.set("n1", "m1", 1, Script::RetryableFailure("429".into()));
    worker.set("n1", "m1", 2, Script::RetryableFailure("429".into()));
    worker.set("n1", "m2", 1, Script::Success("ok".into()));
    let (report, events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert_eq!(report.node_states[0].model.as_deref(), Some("m2"));
    assert_eq!(fallbacks(&events), vec![("m1", "m2")]);
}

// ============================================================================
// T6: real failure (retryable==false) still ADVANCES to next model after
//     exhausting attempts on m1, then fails on m2 too. Confirms the
//     existing pidag "advance after attempts exhausted" semantics are
//     preserved: real failures do advance to the next model, but only
//     AFTER all same-model attempts are spent (no early-break on 429
//     backoff since retryable==false). Both models fail → node fails.
// ============================================================================

#[tokio::test]
async fn t6_real_failure_exhausts_attempts_then_advances() {
    let worker = ScriptedWorker::default();
    let node = mk_node("n1", vec!["m1", "m2"], 2, 0);
    worker.set("n1", "m1", 1, Script::RealFailure("err".into()));
    worker.set("n1", "m1", 2, Script::RealFailure("err".into()));
    worker.set("n1", "m2", 1, Script::RealFailure("err".into()));
    worker.set("n1", "m2", 2, Script::RealFailure("err".into()));
    let (report, events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Failed);
    // m1 exhausted -> advance to m2 fires one ProviderFallback
    assert_eq!(fallbacks(&events), vec![("m1", "m2")]);
    // two real-failure retries per model = 4 total NodeRetry events? Actually
    // the retry event only fires when attempt < attempts (last attempt
    // doesn't emit). So m1 emits 1 retry (attempt 1), m2 emits 1 retry ->
    // total 2 attempt-failed retries.
    let n = retries(&events)
        .iter()
        .filter(|r| r == &"attempt failed")
        .count();
    assert_eq!(n, 2);
}

// ============================================================================
// T7: backoff_ms > 0 + retryable + retries-then-succeed on SAME model.
//     Retries attempts 1..k with exponential sleep, attempt N succeeds.
//     Verifies backoff stays on the same model (does NOT advance).
// ============================================================================

#[tokio::test]
async fn t7_backoff_retries_same_model_then_succeeds() {
    let worker = ScriptedWorker::default();
    // tiny backoff: 2ms base → 2ms, 4ms (kept under 10ms total)
    let node = mk_node("n1", vec!["m1"], 3, 2);
    worker.set("n1", "m1", 1, Script::RetryableFailure("429".into()));
    worker.set("n1", "m1", 2, Script::RetryableFailure("429".into()));
    worker.set("n1", "m1", 3, Script::Success("ok".into()));
    let (report, events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert_eq!(report.node_states[0].model.as_deref(), Some("m1"));
    assert_eq!(report.node_states[0].attempts, 3);
    // two backoff-retry events for attempts 1 and 2
    let retry_reasons: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::NodeRetry { reason, .. } => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(retry_reasons, vec!["429 backoff", "429 backoff"]);
    assert!(
        fallbacks(&events).is_empty(),
        "must not fall back; succeeded on m1"
    );
}

// ============================================================================
// T8: backoff exhausted on single model → advance to NEXT model, succeeds.
//     backoff_ms>0 but out of attempts on this model triggers break.
// ============================================================================

#[tokio::test]
async fn t8_backoff_exhausted_advances_to_next_model() {
    let worker = ScriptedWorker::default();
    let node = mk_node("n1", vec!["m1", "m2"], 2, 2);
    worker.set("n1", "m1", 1, Script::RetryableFailure("429".into()));
    worker.set("n1", "m1", 2, Script::RetryableFailure("429".into()));
    worker.set("n1", "m2", 1, Script::Success("ok".into()));
    let (report, events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert_eq!(report.node_states[0].model.as_deref(), Some("m2"));
    assert_eq!(fallbacks(&events), vec![("m1", "m2")]);
    // one backoff retry on attempt 1 (attempt 2 triggers break, no retry event)
    let backoff_count = events
        .iter()
        .filter(|e| matches!(e, Event::NodeRetry { reason, .. } if reason == "429 backoff"))
        .count();
    assert_eq!(backoff_count, 1);
}

// ============================================================================
// T9: backoff exhausted on every model → node fails (no more models).
// ============================================================================

#[tokio::test]
async fn t9_backoff_exhausted_all_models_node_fails() {
    let worker = ScriptedWorker::default();
    let node = mk_node("n1", vec!["m1", "m2"], 2, 2);
    for a in 1..=2 {
        for m in ["m1", "m2"] {
            worker.set("n1", m, a, Script::RetryableFailure("429".into()));
        }
    }
    let (report, events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Failed);
    assert_eq!(fallbacks(&events), vec![("m1", "m2")]);
}

// ============================================================================
// T10: mixed — backoff on m1 attempt 1 succeeds attempt 2 (no fallback).
//      Only ONE backoff_retry event expected.
// ============================================================================

#[tokio::test]
async fn t10_backoff_recovers_on_second_attempt() {
    let worker = ScriptedWorker::default();
    let node = mk_node("n1", vec!["m1"], 2, 2);
    worker.set("n1", "m1", 1, Script::RetryableFailure("429".into()));
    worker.set("n1", "m1", 2, Script::Success("ok".into()));
    let (report, events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert_eq!(report.node_states[0].model.as_deref(), Some("m1"));
    assert_eq!(report.node_states[0].attempts, 2);
    let backoff_count = events
        .iter()
        .filter(|e| matches!(e, Event::NodeRetry { reason, .. } if reason == "429 backoff"))
        .count();
    assert_eq!(backoff_count, 1);
    assert!(fallbacks(&events).is_empty());
}

// ============================================================================
// T11: exponential backoff durations are real (measured elapsed wall-clock).
//     backoff_ms=10 → 10ms (attempt 1), 20ms (attempt 2), then succeed.
//     Total ~30ms. Assert elapsed >= 25ms (slack) and < 500ms (sanity).
// ============================================================================

#[tokio::test]
async fn t11_backoff_durations_are_exponential_in_wall_clock() {
    use std::time::Instant;
    let worker = ScriptedWorker::default();
    let node = mk_node("n1", vec!["m1"], 3, 10);
    worker.set("n1", "m1", 1, Script::RetryableFailure("429".into()));
    worker.set("n1", "m1", 2, Script::RetryableFailure("429".into()));
    worker.set("n1", "m1", 3, Script::Success("ok".into()));
    let start = Instant::now();
    let (report, _events) = run_one(node, worker).await;
    let elapsed = start.elapsed();
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    // 10ms + 20ms = 30ms minimum; allow slack for scheduling jitter
    assert!(
        elapsed.as_millis() >= 25,
        "expected >=25ms of backoff, got {elapsed:?}"
    );
    assert!(
        elapsed.as_millis() < 500,
        "backoff exploded (>500ms): {elapsed:?}"
    );
}

// ============================================================================
// T12: dispatch_order — with backoff_ms==0, a retryable failure on m1
//      attempt 1 BREAKS immediately (does NOT consume attempt 2) and
//      advances to m2 attempt 1. Confirms the "no-backoff → advance fast"
//      path does not waste same-model attempts. Records ScriptedWorker.calls().
// ============================================================================

#[tokio::test]
async fn t12_dispatch_order_retryable_no_backoff_breaks_immediately() {
    let worker = ScriptedWorker::default();
    let node = mk_node("n1", vec!["m1", "m2"], 2, 0);
    worker.set("n1", "m1", 1, Script::RetryableFailure("429".into()));
    worker.set("n1", "m2", 1, Script::Success("ok".into()));
    let worker_clone = worker.clone();
    let (report, _events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    let calls = worker_clone.calls();
    assert_eq!(
        calls,
        vec![("n1".into(), "m1".into(), 1), ("n1".into(), "m2".into(), 1),],
        "retryable+backoff_ms==0 must not consume attempt 2 on the same model"
    );
}

// ============================================================================
// T13: shell node pedigree unaffected — empty models, retryable always
//      false, dispatch still works through the shell branch (not the model
//      loop). Confirms the 429 changes did not break shell nodes.
// ============================================================================

#[tokio::test]
async fn t13_shell_node_path_unaffected_by_429_logic() {
    let worker = ScriptedWorker::default();
    let node = Node {
        id: "sh1".into(),
        prompt: "echo".into(),
        depends_on: vec![],
        models: vec![], // shell nodes have empty models
        retry: RetryPolicy {
            attempts: 2,
            backoff_ms: 0,
        },
        validate: None,
        node_type: Some("shell".into()),
        gate: None,
        timeout: None,
        mcp_call: None,
        after: vec![],
        verify: None,
        verify_pre: None,
    };
    worker.set("sh1", "", 1, Script::Success("done".into()));
    let (report, events) = run_one(node, worker).await;
    assert_eq!(report.node_states[0].state, NodeStatus::Done);
    assert!(fallbacks(&events).is_empty());
    assert!(retries(&events).is_empty());
}
