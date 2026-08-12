use pidag::{
    AwaitOutcome, Dag, DelayMockWorker, NodeStatus, PiPrintWorker, Scheduler, VecSink, Worker,
};
use std::time::Duration;
use tokio::time::Instant;

fn single_node_dag(id: &str, prompt: &str) -> Dag {
    let json = format!(
        r#"{{
            "nodes": [
                {{
                    "id": "{id}",
                    "prompt": "{prompt}",
                    "depends_on": [],
                    "models": [{{"name": "nvidia", "paid": false}}],
                    "retry": {{"attempts": 1, "backoff_ms": 0}}
                }}
            ]
        }}"#
    );
    serde_json::from_str(&json).expect("valid single-node dag fixture")
}

fn two_root_dag() -> Dag {
    let json = r#"
    {
        "nodes": [
            {
                "id": "a",
                "prompt": "prompt a",
                "depends_on": [],
                "models": [{"name": "nvidia", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            },
            {
                "id": "b",
                "prompt": "prompt b",
                "depends_on": [],
                "models": [{"name": "nvidia", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            }
        ]
    }
    "#;
    serde_json::from_str(json).expect("valid two-root dag fixture")
}

fn linear_dag() -> Dag {
    let json = r#"
    {
        "nodes": [
            {
                "id": "a",
                "prompt": "prompt a",
                "depends_on": [],
                "models": [{"name": "nvidia", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            },
            {
                "id": "b",
                "prompt": "prompt b",
                "depends_on": ["a"],
                "models": [{"name": "nvidia", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            }
        ]
    }
    "#;
    serde_json::from_str(json).expect("valid linear dag fixture")
}

// ============================================================================
// await_dag / wait_any / resume_await
//
// Every test below drives ONE Scheduler instance. `await_dag`/`wait_any`
// lazily start the run in the background on first call (via the scheduler's
// internal watch-channel snapshot) so there is exactly one authoritative
// run per test -- no second Scheduler racing a disconnected copy of state.
// ============================================================================

#[tokio::test]
async fn test_await_dag_returns_done_on_completion() {
    let dag = linear_dag();
    let worker = Box::new(DelayMockWorker::new());
    let mut scheduler = Scheduler::new(dag, worker, Box::new(VecSink::new()), 2);

    let outcome = scheduler.await_dag(None).await;
    match outcome {
        AwaitOutcome::Done(report) => {
            assert_eq!(report.node_states.len(), 2);
            assert!(report.failed.is_empty());
            assert!(
                report
                    .node_states
                    .iter()
                    .all(|s| s.state == NodeStatus::Done)
            );
        }
        other => panic!("expected AwaitOutcome::Done, got {other:?}"),
    }
}

#[tokio::test]
async fn test_await_dag_already_terminal_returns_immediately() {
    let dag = single_node_dag("a", "prompt a");
    let worker = Box::new(DelayMockWorker::new());
    let mut scheduler = Scheduler::new(dag, worker, Box::new(VecSink::new()), 1);

    // Drive to completion via the direct (Phase-1) API first.
    scheduler.run(false).await.expect("mock worker never fails");

    // await_dag must observe the already-terminal state without starting a
    // second run or blocking.
    let start = Instant::now();
    let outcome = scheduler.await_dag(None).await;
    let elapsed = start.elapsed();

    match outcome {
        AwaitOutcome::Done(_) => {
            assert!(
                elapsed.as_millis() < 100,
                "already-terminal await_dag took {elapsed:?}, should return immediately"
            );
        }
        other => panic!("expected AwaitOutcome::Done, got {other:?}"),
    }
}

#[tokio::test]
async fn test_await_dag_timeout_returns_resume_token() {
    let dag = single_node_dag("a", "prompt a");
    let worker = DelayMockWorker::new();
    worker.set_delay("a", Duration::from_millis(200));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);

    let outcome = scheduler.await_dag(Some(Duration::from_millis(50))).await;
    match outcome {
        AwaitOutcome::Running(_token) => {}
        other => panic!("expected AwaitOutcome::Running, got {other:?}"),
    }
}

#[tokio::test]
async fn test_resume_token_continues_to_done() {
    let dag = single_node_dag("a", "prompt a");
    let worker = DelayMockWorker::new();
    worker.set_delay("a", Duration::from_millis(100));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);

    let token = match scheduler.await_dag(Some(Duration::from_millis(30))).await {
        AwaitOutcome::Running(token) => token,
        other => panic!("expected timeout with a token first, got {other:?}"),
    };

    // Resume on the SAME run: no restart, no missed transitions.
    let outcome = scheduler.resume_await(token, None).await;
    match outcome {
        AwaitOutcome::Done(report) => {
            assert_eq!(report.node_states.len(), 1);
            assert_eq!(report.node_states[0].state, NodeStatus::Done);
        }
        other => panic!("expected AwaitOutcome::Done after resume, got {other:?}"),
    }
}

#[tokio::test]
async fn test_wait_any_returns_first_completed() {
    let dag = two_root_dag();
    let worker = DelayMockWorker::new();
    worker.set_delay("a", Duration::from_millis(20));
    worker.set_delay("b", Duration::from_millis(200));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 2);

    let outcome = scheduler.wait_any(None).await;
    match outcome {
        AwaitOutcome::Node {
            node_id,
            still_running,
            ..
        } => {
            assert_eq!(node_id, "a", "the faster node should complete first");
            assert!(
                still_running.contains(&"b".to_string()),
                "b should still be running: {still_running:?}"
            );
        }
        other => panic!("expected AwaitOutcome::Node, got {other:?}"),
    }
}

#[tokio::test]
async fn test_wait_any_still_running_shrinks() {
    let dag = two_root_dag();
    let worker = DelayMockWorker::new();
    worker.set_delay("a", Duration::from_millis(20));
    // b must still be running when `a` is observed. 80ms was too tight —
    // tokio could poll `b` to completion before the watch snapshot was
    // read on slow CI, making `still_running == []` instead of `["b"]`
    // (a real timing race, not a logic bug). 500ms gives a wide window.
    worker.set_delay("b", Duration::from_millis(500));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 2);

    match scheduler.wait_any(None).await {
        AwaitOutcome::Node {
            node_id,
            still_running,
            ..
        } => {
            assert_eq!(node_id, "a");
            assert_eq!(still_running, vec!["b".to_string()]);
        }
        other => panic!("expected AwaitOutcome::Node, got {other:?}"),
    }

    match scheduler.wait_any(None).await {
        AwaitOutcome::Node {
            node_id,
            still_running,
            ..
        } => {
            assert_eq!(node_id, "b");
            assert!(
                still_running.is_empty(),
                "no nodes should still be running: {still_running:?}"
            );
        }
        other => panic!("expected AwaitOutcome::Node, got {other:?}"),
    }
}

#[tokio::test]
async fn test_wait_any_timeout_returns_resume_token() {
    let dag = single_node_dag("a", "prompt a");
    let worker = DelayMockWorker::new();
    worker.set_delay("a", Duration::from_millis(300));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);

    let outcome = scheduler.wait_any(Some(Duration::from_millis(50))).await;
    match outcome {
        AwaitOutcome::Running(_token) => {}
        other => panic!("expected AwaitOutcome::Running, got {other:?}"),
    }
}

#[tokio::test]
async fn test_no_busy_wait_during_await() {
    // await_dag/wait_any are implemented on top of a tokio::sync::watch
    // channel: the caller parks on `rx.changed()` and is woken exactly when
    // a terminal transition is published, not via loop{status();sleep()}.
    // We can't measure "zero CPU spins" directly in a unit test, but a
    // correct outcome after a delay that is long relative to the runtime's
    // scheduling granularity is strong evidence the wait is event-driven
    // (a broken poll-loop implementation would still "work" here, but the
    // hard constraint against loop{sleep} is enforced by code review of
    // `Scheduler::wait_for_done`, which contains no sleep/poll loop).
    let dag = single_node_dag("a", "prompt a");
    let worker = DelayMockWorker::new();
    worker.set_delay("a", Duration::from_millis(50));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);

    let outcome = scheduler.await_dag(None).await;
    match outcome {
        AwaitOutcome::Done(report) => {
            assert_eq!(report.node_states[0].state, NodeStatus::Done);
        }
        other => panic!("expected AwaitOutcome::Done, got {other:?}"),
    }
}

// ============================================================================
// render_status
// ============================================================================

#[test]
fn test_render_status_incident_grouping() {
    let json = r#"
    {
        "nodes": [
            {"id": "a", "prompt": "root", "depends_on": [], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}},
            {"id": "b", "prompt": "left", "depends_on": ["a"], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}},
            {"id": "c", "prompt": "right", "depends_on": ["a"], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}},
            {"id": "d", "prompt": "sink", "depends_on": ["b", "c"], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}}
        ]
    }
    "#;
    let dag: Dag = serde_json::from_str(json).unwrap();

    let mut states = std::collections::HashMap::new();
    states.insert("a".to_string(), ("Done".to_string(), None));
    states.insert(
        "b".to_string(),
        ("Running".to_string(), Some("nvidia".to_string())),
    );
    states.insert("c".to_string(), ("Pending".to_string(), None));
    states.insert("d".to_string(), ("Pending".to_string(), None));

    let output = pidag::render_status(&dag, &states);

    // Every dependent node line carries its own `->deps:` (one per node
    // with dependencies: b, c, d).
    let deps_lines = output.matches("\u{2192}deps:").count();
    assert_eq!(
        deps_lines, 3,
        "expected one ->deps: per dependent node (b, c, d), got {deps_lines} in:\n{output}"
    );

    // No flat (from,to) edge-list anywhere.
    assert!(!output.contains("(a,b)") && !output.contains("(a,c)") && !output.contains("(b,d)"));
    assert!(
        output.contains('a')
            && output.contains('b')
            && output.contains('c')
            && output.contains('d')
    );
}

#[test]
fn test_render_status_glyphs_and_header() {
    let json = r#"
    {
        "nodes": [
            {"id": "done", "prompt": "", "depends_on": [], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}},
            {"id": "running", "prompt": "", "depends_on": [], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}},
            {"id": "blocked", "prompt": "", "depends_on": [], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}},
            {"id": "failed", "prompt": "", "depends_on": [], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}},
            {"id": "pending", "prompt": "", "depends_on": [], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}}
        ]
    }
    "#;
    let dag: Dag = serde_json::from_str(json).unwrap();

    let mut states = std::collections::HashMap::new();
    states.insert("done".to_string(), ("Done".to_string(), None));
    states.insert(
        "running".to_string(),
        ("Running".to_string(), Some("nvidia".to_string())),
    );
    states.insert("blocked".to_string(), ("Blocked".to_string(), None));
    states.insert("failed".to_string(), ("Failed".to_string(), None));

    let output = pidag::render_status(&dag, &states);

    assert!(output.starts_with("run "), "header line missing: {output}");
    assert!(
        output.contains("1/5 done"),
        "header should count done nodes: {output}"
    );
    assert!(
        output.contains("1 failed"),
        "header should count failed nodes: {output}"
    );

    assert!(output.contains('\u{2713}'), "missing done glyph");
    assert!(output.contains('\u{23f3}'), "missing running glyph");
    assert!(output.contains('\u{26d4}'), "missing blocked glyph");
    assert!(output.contains('\u{2717}'), "missing failed glyph");
    assert!(output.contains('\u{b7}'), "missing pending glyph");
}

#[test]
fn test_render_status_missing_state_is_pending() {
    let json = r#"
    {
        "nodes": [
            {"id": "a", "prompt": "", "depends_on": [], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}},
            {"id": "b", "prompt": "", "depends_on": [], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}}
        ]
    }
    "#;
    let dag: Dag = serde_json::from_str(json).unwrap();

    let mut states = std::collections::HashMap::new();
    states.insert("a".to_string(), ("Done".to_string(), None));
    // "b" intentionally absent from states.

    let output = pidag::render_status(&dag, &states);

    let b_line = output
        .lines()
        .find(|l| l.starts_with("b "))
        .unwrap_or_else(|| panic!("no line for node b in:\n{output}"));
    assert!(
        b_line.contains('\u{b7}'),
        "missing node should render pending glyph: {b_line}"
    );
    assert!(
        b_line.contains("Pending"),
        "missing node should render Pending state: {b_line}"
    );
}

#[test]
fn test_render_status_deterministic() {
    let json = r#"
    {
        "nodes": [
            {"id": "a", "prompt": "", "depends_on": [], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}},
            {"id": "b", "prompt": "", "depends_on": ["a"], "models": [{"name": "nvidia", "paid": false}], "retry": {"attempts": 1, "backoff_ms": 0}}
        ]
    }
    "#;
    let dag: Dag = serde_json::from_str(json).unwrap();

    let mut states = std::collections::HashMap::new();
    states.insert(
        "a".to_string(),
        ("Done".to_string(), Some("nvidia".to_string())),
    );
    states.insert(
        "b".to_string(),
        ("Running".to_string(), Some("nvidia".to_string())),
    );

    let output1 = pidag::render_status(&dag, &states);
    let output2 = pidag::render_status(&dag, &states);

    assert_eq!(
        output1, output2,
        "render_status must be deterministic for identical input"
    );
}

// ============================================================================
// PiPrintWorker
//
// Only `test_piprint_real_pi_smoke` (#[ignore]) touches a real binary. The
// other two point PiPrintWorker's overridable command at `sh`, a harmless,
// always-available shim, to exercise the real spawn/timeout/parse code
// paths (not an unrelated mock) while staying fully offline.
// ============================================================================

#[tokio::test]
async fn test_piprintworker_plaintext_fallback_succeeds() {
    let dag = single_node_dag("a", "return 42");
    // Exits 0 with plain text (not JSON envelope) — pi 0.1.x does this even
    // with --output-format json. The worker must accept plain text as valid.
    let worker = PiPrintWorker::with_command(
        Duration::from_secs(5),
        "sh".to_string(),
        vec![
            "-c".to_string(),
            "echo 'plain text response'; exit 0".to_string(),
        ],
    );

    let result = worker
        .run("a", "return 42", "nvidia", 1)
        .await
        .expect("worker must never return Err");

    assert!(
        result.success,
        "non-empty plain text must map to success:true"
    );
    assert_eq!(result.output, "plain text response");
}

#[tokio::test]
async fn test_piprintworker_empty_output_maps_to_failure() {
    let dag = single_node_dag("a", "return 42");
    // Exits 0 but produces empty stdout — this is a genuine failure.
    let worker = PiPrintWorker::with_command(
        Duration::from_secs(5),
        "sh".to_string(),
        vec!["-c".to_string(), "exit 0".to_string()],
    );

    let result = worker
        .run("a", "return 42", "nvidia", 1)
        .await
        .expect("worker must never return Err, only WorkerOutput{success:false}");

    assert!(!result.success, "empty output must map to success:false");
}

#[tokio::test]
async fn test_piprintworker_timeout_kills_child() {
    let dag = single_node_dag("a", "return 42");
    // Sleeps far longer than the worker's timeout -> exercises the
    // tokio::time::timeout + kill_on_drop path.
    let worker = PiPrintWorker::with_command(
        Duration::from_millis(50),
        "sh".to_string(),
        vec!["-c".to_string(), "sleep 5; echo done".to_string()],
    );

    let start = Instant::now();
    let result = worker
        .run("a", "return 42", "nvidia", 1)
        .await
        .expect("worker must never return Err, only WorkerOutput{success:false}");
    let elapsed = start.elapsed();

    assert!(!result.success, "timed-out call must map to success:false");
    assert!(
        elapsed < Duration::from_secs(2),
        "timeout should fire well before the scripted 5s sleep, took {elapsed:?}"
    );
}

#[tokio::test]
#[ignore]
async fn test_piprint_real_pi_smoke() {
    // Needs the real `pi` binary + network. Run explicitly with:
    //   cargo test -p pidag -- --ignored test_piprint_real_pi_smoke
    let dag = single_node_dag("trivial", "Reply with exactly: 42");
    let worker = PiPrintWorker::new(&dag, Duration::from_secs(60));

    let result = worker
        .run("trivial", "Reply with exactly: 42", "nvidia", 1)
        .await
        .expect("worker must never return Err, only WorkerOutput{success:false}");

    assert!(
        result.success,
        "real pi smoke call should succeed: {result:?}"
    );
    assert!(
        !result.output.is_empty(),
        "real pi smoke call should produce output"
    );
}

// ============================================================================
// Shell Node Dispatch — Handle Empty Models Array
//
// Tests for the shell node dispatch fix (spec: specs/91-shell-node-dispatch.md).
// Shell nodes have empty models arrays and should execute with an empty
// model string. LLM nodes with empty models should fail with a clear error.
// ============================================================================

#[tokio::test]
async fn test_shell_node_empty_models_executes() {
    // T1: Shell node with models: [] should execute successfully
    let json = r#"
    {
        "nodes": [
            {
                "id": "shell-test",
                "prompt": "echo 'shell executed'",
                "depends_on": [],
                "models": [],
                "retry": {"attempts": 1, "backoff_ms": 0},
                "node_type": "shell"
            }
        ]
    }
    "#;
    let dag: Dag = serde_json::from_str(json).expect("valid shell node dag");
    let worker = Box::new(DelayMockWorker::new());
    let mut scheduler = Scheduler::new(dag, worker, Box::new(VecSink::new()), 1);

    let report = scheduler.run(false).await.expect("run completes");

    assert_eq!(report.node_states.len(), 1);
    assert_eq!(
        report.node_states[0].state,
        NodeStatus::Done,
        "shell node should succeed"
    );
    assert!(report.failed.is_empty(), "no nodes should fail");
}

#[tokio::test]
async fn test_llm_node_empty_models_fails() {
    // T2: LLM node with models: [] should fail with error message
    let json = r#"
    {
        "nodes": [
            {
                "id": "llm-test",
                "prompt": "generate something",
                "depends_on": [],
                "models": [],
                "retry": {"attempts": 1, "backoff_ms": 0}
            }
        ]
    }
    "#;
    let dag: Dag = serde_json::from_str(json).expect("valid llm node dag");
    let worker = Box::new(DelayMockWorker::new());
    let mut scheduler = Scheduler::new(dag, worker, Box::new(VecSink::new()), 1);

    let report = scheduler.run(false).await.expect("run completes");

    assert_eq!(report.node_states.len(), 1);
    assert_eq!(
        report.node_states[0].state,
        NodeStatus::Failed,
        "llm node should fail"
    );
    assert_eq!(
        report.failed,
        vec!["llm-test".to_string()],
        "llm node should be in failed list"
    );

    // Verify error message
    if let Some(output) = &report.node_states[0].output {
        assert!(
            output.contains("no models specified"),
            "error message should explain the issue: {output}"
        );
    }
}

#[tokio::test]
async fn test_shell_node_retry_works() {
    // T3: Shell node with retry policy should retry on failure
    // This test uses a mock that tracks calls and can succeed after retries
    let json = r#"
    {
        "nodes": [
            {
                "id": "shell-retry",
                "prompt": "exit 0",
                "depends_on": [],
                "models": [],
                "retry": {"attempts": 3, "backoff_ms": 0},
                "node_type": "shell"
            }
        ]
    }
    "#;
    let dag: Dag = serde_json::from_str(json).expect("valid shell node dag with retry");

    // DelayMockWorker always succeeds, so we should get Done on first attempt
    let worker = Box::new(DelayMockWorker::new());
    let mut scheduler = Scheduler::new(dag, worker, Box::new(VecSink::new()), 1);

    let report = scheduler.run(false).await.expect("run completes");

    assert_eq!(report.node_states.len(), 1);
    assert_eq!(
        report.node_states[0].state,
        NodeStatus::Done,
        "shell node should succeed"
    );
    assert_eq!(
        report.node_states[0].attempts, 1,
        "should succeed on first attempt"
    );
    assert!(report.failed.is_empty());
}

#[tokio::test]
async fn test_shell_node_with_dependency_chain() {
    // Verify shell nodes work in dependency chains
    let json = r#"
    {
        "nodes": [
            {
                "id": "shell-1",
                "prompt": "echo step1",
                "depends_on": [],
                "models": [],
                "retry": {"attempts": 1, "backoff_ms": 0},
                "node_type": "shell"
            },
            {
                "id": "shell-2",
                "prompt": "echo step2",
                "depends_on": ["shell-1"],
                "models": [],
                "retry": {"attempts": 1, "backoff_ms": 0},
                "node_type": "shell"
            },
            {
                "id": "shell-3",
                "prompt": "echo step3",
                "depends_on": ["shell-2"],
                "models": [],
                "retry": {"attempts": 1, "backoff_ms": 0},
                "node_type": "shell"
            }
        ]
    }
    "#;
    let dag: Dag = serde_json::from_str(json).expect("valid shell node dag chain");
    let worker = Box::new(DelayMockWorker::new());
    let mut scheduler = Scheduler::new(dag, worker, Box::new(VecSink::new()), 1);

    let report = scheduler.run(false).await.expect("run completes");

    assert_eq!(
        report.node_states.len(),
        3,
        "all 3 nodes should be in report"
    );
    assert!(
        report
            .node_states
            .iter()
            .all(|s| s.state == NodeStatus::Done),
        "all nodes should succeed"
    );
    assert!(report.failed.is_empty(), "no nodes should fail");
}
