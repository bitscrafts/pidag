//! TDD Contract for spec-39 ("budget ceilings: --max-tokens / --max-model-calls").
//!
//! See `specs/39-budget-ceilings.md` for the full contract; test ids below
//! (`B1a`, `B2a`, ...) match its TDD Contract table. Two ceilings, counted
//! in units pidag can actually observe (tokens, model-consuming dispatches)
//! -- deliberately NOT dollars; see the spec's Overview for why.

use async_trait::async_trait;
use pidag::scheduler::{BudgetCounters, BudgetLimits};
use pidag::{
    AgentWorker, Checkpoint, Dag, MockBackend, MockCapabilities, ModelRef, Node, NodeStatus,
    PidagError, RealShellWorker, RedbSink, RedbStore, ResumeDecision, RetryPolicy, RunMeta,
    Scheduler, Store, TokenUsage, VecSink, Verify, Worker, WorkerOutput, load_checkpoint,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ============================================================================
// Shared test fixtures
// ============================================================================

fn model(name: &str) -> ModelRef {
    ModelRef {
        name: name.to_string(),
        paid: false,
    }
}

fn base_node(id: &str) -> Node {
    Node {
        id: id.to_string(),
        prompt: format!("prompt for {id}"),
        depends_on: vec![],
        models: vec![],
        retry: RetryPolicy {
            attempts: 1,
            backoff_ms: 0,
        },
        validate: None,
        node_type: None,
        gate: None,
        timeout: None,
        mcp_call: None,
        after: vec![],
        verify: None,
        verify_pre: None,
        for_each: None,
        quorum: None,
    }
}

/// A base node with a single (unpaid) model attached -- a model-consuming
/// (`B2`) node.
fn llm_node(id: &str) -> Node {
    let mut n = base_node(id);
    n.models = vec![model("m")];
    n
}

fn dag(nodes: Vec<Node>) -> Dag {
    Dag {
        nodes,
        metadata: None,
    }
}

/// A dependency chain n[0] -> n[1] -> ... -> n[last]. Used instead of an
/// independent fan-out whenever a test needs dispatch to be serialized
/// through the ready-queue one node at a time regardless of `--concurrency`
/// -- the correct way to observe a `--max-tokens` breach deterministically,
/// since (unlike `--max-model-calls`) tokens are not knowable before a call
/// completes, so a wave of simultaneously-ready independent nodes can all
/// be dispatched together before any of them reports usage back (this is
/// exactly B5's disclosed in-flight overshoot, bounded by `--concurrency`).
fn chain(ids: &[&str]) -> Vec<Node> {
    let mut nodes = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let mut n = llm_node(id);
        if i > 0 {
            n.depends_on = vec![ids[i - 1].to_string()];
        }
        nodes.push(n);
    }
    nodes
}

fn state_of<'a>(report: &'a pidag::RunReport, id: &str) -> &'a pidag::NodeState {
    report
        .node_states
        .iter()
        .find(|s| s.node_id == id)
        .unwrap_or_else(|| panic!("no state recorded for node '{id}', report={report:?}"))
}

/// What a scripted worker call returns for a given node id.
#[derive(Clone, Debug)]
enum Outcome {
    Success {
        output: String,
        usage: Option<TokenUsage>,
    },
    Failure(String),
}

/// Records every dispatch (node_id, prompt, model, attempt) and returns a
/// scripted outcome keyed by node_id. Never spawns a subprocess or touches
/// a backend -- pure Rust, so a test that only ever exercises
/// `ScriptedWorker` and still observes a ceiling trip is proof the budget
/// accumulator lives in the scheduler, not in any particular worker.
#[derive(Clone, Default)]
struct ScriptedWorker {
    calls: Arc<Mutex<Vec<(String, String, String, usize)>>>,
    outcomes: Arc<Mutex<HashMap<String, Outcome>>>,
}

impl ScriptedWorker {
    fn new() -> Self {
        Self::default()
    }

    fn script(&self, node_id: &str, outcome: Outcome) {
        self.outcomes
            .lock()
            .unwrap()
            .insert(node_id.to_string(), outcome);
    }

    fn calls(&self) -> Vec<(String, String, String, usize)> {
        self.calls.lock().unwrap().clone()
    }

    fn call_count(&self, node_id: &str) -> usize {
        self.calls().iter().filter(|(id, ..)| id == node_id).count()
    }
}

#[async_trait]
impl Worker for ScriptedWorker {
    async fn run(
        &self,
        node_id: &str,
        prompt: &str,
        model: &str,
        attempt: usize,
    ) -> Result<WorkerOutput, PidagError> {
        self.calls.lock().unwrap().push((
            node_id.to_string(),
            prompt.to_string(),
            model.to_string(),
            attempt,
        ));
        let outcome = self.outcomes.lock().unwrap().get(node_id).cloned();
        match outcome {
            Some(Outcome::Success { output, usage }) => Ok(WorkerOutput {
                success: true,
                output,
                retryable: false,
                usage,
            }),
            Some(Outcome::Failure(s)) => Ok(WorkerOutput {
                success: false,
                output: s,
                retryable: false,
                usage: None,
            }),
            None => Ok(WorkerOutput {
                success: true,
                output: format!("default output for {node_id}"),
                retryable: false,
                usage: None,
            }),
        }
    }
}

/// Scratch directory for this file's subprocess-driven tests. Fresh per
/// sub-path, never `/tmp` (hard rule N6).
fn scratch(name: &str) -> PathBuf {
    let tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("_tmp/budget_ceiling_tests");
    let dir = tmp.join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".pidag")).expect("create scratch dir");
    dir
}

/// A minimal, valid `.pidag/config.toml` selecting the in-process `mock`
/// backend -- no live model credentials exist in this environment (or in
/// CI), so every subprocess test in this file drives `pidag run` against
/// `MockBackend`, never a real provider.
fn write_mock_config(dir: &std::path::Path) {
    std::fs::write(
        dir.join(".pidag/config.toml"),
        "[project]\nroot = \".\"\n\n[worker]\ndefault_model = \"m\"\ntimeout_secs = 5\n\n\
         [agent]\nbackend = \"mock\"\n",
    )
    .expect("write config.toml");
}

// ============================================================================
// B1a: test_worker_output_carries_usage
// ============================================================================
#[tokio::test]
async fn test_worker_output_carries_usage() {
    let usage = TokenUsage {
        input_tokens: 10,
        output_tokens: 5,
        total_tokens: 15,
    };
    let backend = Arc::new(
        MockBackend::with_capabilities(MockCapabilities {
            token_usage: true,
            ..Default::default()
        })
        .with_usage_per_call(usage.clone()),
    );
    let d = dag(vec![llm_node("n")]);
    let worker = AgentWorker::new(backend, &d, Duration::from_secs(5));

    let output = worker
        .run("n", "prompt", "m", 1)
        .await
        .expect("AgentWorker::run");

    assert!(output.success);
    let got = output
        .usage
        .expect("WorkerOutput.usage must be Some with the backend's figures");
    assert_eq!(got.total_tokens, 15);
    assert_eq!(got.input_tokens, 10);
    assert_eq!(got.output_tokens, 5);
}

// ============================================================================
// B1b: test_shell_worker_usage_is_none
// ============================================================================
#[tokio::test]
async fn test_shell_worker_usage_is_none() {
    let d = dag(vec![]);
    let worker = RealShellWorker::new(&d, Duration::from_secs(5));

    let output = worker
        .run("n", "echo hi", "", 1)
        .await
        .expect("RealShellWorker::run");

    assert!(output.success);
    assert!(
        output.usage.is_none(),
        "a shell worker has no usage source: usage must be None, not Some(0) -- a fabricated \
         zero would silently under-count against --max-tokens"
    );
}

// ============================================================================
// B2a: test_model_call_ceiling_aborts
// ============================================================================
#[tokio::test]
async fn test_model_call_ceiling_aborts() {
    let worker = ScriptedWorker::new();
    let nodes: Vec<Node> = ["n1", "n2", "n3", "n4", "n5"]
        .iter()
        .map(|id| llm_node(id))
        .collect();
    let d = dag(nodes);
    let mut scheduler = Scheduler::new(d, Box::new(worker.clone()), Box::new(VecSink::new()), 5);

    let report = scheduler
        .run_with_budget(
            false,
            BudgetLimits {
                max_model_calls: Some(3),
                max_tokens: None,
            },
        )
        .await
        .expect("run");

    assert!(
        report.breach.is_some(),
        "expected a breach, report={report:?}"
    );
    let dispatched: std::collections::HashSet<String> =
        worker.calls().into_iter().map(|(id, ..)| id).collect();
    assert!(
        dispatched.len() <= 3,
        "at most 3 of the 5 nodes may ever be dispatched, got {dispatched:?}"
    );
    assert_eq!(report.model_calls, 3);
}

// ============================================================================
// B2b: test_shell_and_quorum_are_not_model_calls
// ============================================================================
#[tokio::test]
async fn test_shell_and_quorum_are_not_model_calls() {
    let worker = ScriptedWorker::new();
    let mut shell_ids = Vec::new();
    let mut nodes = Vec::new();
    for i in 0..10 {
        let id = format!("s{i}");
        let mut n = base_node(&id);
        n.node_type = Some("shell".to_string());
        n.models = vec![]; // no models => not a model call (B2b, G8)
        worker.script(
            &id,
            Outcome::Success {
                output: "PASS - ok".into(),
                usage: None,
            },
        );
        shell_ids.push(id.clone());
        nodes.push(n);
    }
    let mut quorum = base_node("q");
    quorum.node_type = Some("quorum".to_string());
    quorum.quorum = Some(pidag::QuorumConfig {
        of: shell_ids.clone(),
        min_pass: 10,
    });
    nodes.push(quorum);

    let d = dag(nodes);
    let mut scheduler = Scheduler::new(d, Box::new(worker.clone()), Box::new(VecSink::new()), 10);

    let report = scheduler
        .run_with_budget(
            false,
            BudgetLimits {
                max_model_calls: Some(1),
                max_tokens: None,
            },
        )
        .await
        .expect("run");

    assert!(
        report.breach.is_none(),
        "shell nodes and a quorum node consume no model and must never trip --max-model-calls: \
         report={report:?}"
    );
    assert!(
        report.failed.is_empty(),
        "all nodes must complete: {report:?}"
    );
    assert_eq!(
        worker.calls().len(),
        10,
        "every shell node must have been dispatched"
    );
    assert_eq!(state_of(&report, "q").state, NodeStatus::Done);
}

// ============================================================================
// B3: test_token_ceiling_aborts
// ============================================================================
#[tokio::test]
async fn test_token_ceiling_aborts() {
    let worker = ScriptedWorker::new();
    let usage = TokenUsage {
        input_tokens: 60,
        output_tokens: 40,
        total_tokens: 100,
    };
    for id in ["n1", "n2", "n3", "n4", "n5"] {
        worker.script(
            id,
            Outcome::Success {
                output: format!("out-{id}"),
                usage: Some(usage.clone()),
            },
        );
    }
    let d = dag(chain(&["n1", "n2", "n3", "n4", "n5"]));
    let mut scheduler = Scheduler::new(d, Box::new(worker.clone()), Box::new(VecSink::new()), 4);

    let report = scheduler
        .run_with_budget(
            false,
            BudgetLimits {
                max_model_calls: None,
                max_tokens: Some(250),
            },
        )
        .await
        .expect("run");

    assert!(
        report.breach.is_some(),
        "cumulative tokens (100 per node) must exceed 250 and abort the run: report={report:?}"
    );
    assert!(
        report.total_tokens > 250,
        "the breach is recorded AFTER the call that tripped it, not suppressed to a value \
         under the ceiling: report={report:?}"
    );
    let dispatched: Vec<String> = worker.calls().into_iter().map(|(id, ..)| id).collect();
    assert!(
        !dispatched.contains(&"n4".to_string()) && !dispatched.contains(&"n5".to_string()),
        "n4/n5 must never be dispatched once the chain already breached earlier: {dispatched:?}"
    );
}

// ============================================================================
// B4a: test_max_tokens_without_capability_is_startup_error
// ============================================================================
#[test]
fn test_max_tokens_without_capability_is_startup_error() {
    let dir = scratch("b4a");
    write_mock_config(&dir); // "mock" backend: token_usage capability is false

    let d = dag(vec![llm_node("n1")]);
    let dag_path = dir.join("dag.json");
    std::fs::write(&dag_path, serde_json::to_string(&d).unwrap()).unwrap();
    let vault_path = dir.join(".pidag/pidag.redb");

    let bin = env!("CARGO_BIN_EXE_pidag");
    let output = Command::new(bin)
        .args([
            "run",
            dag_path.to_str().unwrap(),
            "--max-tokens",
            "100",
            "--vault",
            vault_path.to_str().unwrap(),
        ])
        .output()
        .expect("spawn pidag");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "--max-tokens against a backend that cannot report usage must be a startup error, not \
         a silently unenforced flag: stderr={stderr}"
    );
    assert!(
        stderr.to_lowercase().contains("token_usage")
            || stderr.to_lowercase().contains("token usage"),
        "the error must explain the capability problem: stderr={stderr}"
    );
    assert!(
        stderr.contains("mock"),
        "the error must name the backend: stderr={stderr}"
    );
    assert!(
        stderr.contains("--max-model-calls"),
        "the error must direct the operator to the alternative flag: stderr={stderr}"
    );
    assert!(
        !vault_path.exists(),
        "the run must never start at all -- the vault must never even be opened: stderr={stderr}"
    );
}

// ============================================================================
// B4b: test_missing_usage_from_capable_backend_is_an_error
// ============================================================================
#[tokio::test]
async fn test_missing_usage_from_capable_backend_is_an_error() {
    // Claims token_usage but is NOT configured with `with_usage_per_call` --
    // its replies carry usage: None, exactly like a real backend that
    // declares the capability yet fails to populate it on a given call.
    let backend = Arc::new(MockBackend::with_capabilities(MockCapabilities {
        token_usage: true,
        ..Default::default()
    }));
    let d = dag(vec![llm_node("n")]);
    let worker = AgentWorker::new(backend, &d, Duration::from_secs(5));
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 1);

    let report = scheduler
        .run_with_budget(
            false,
            BudgetLimits {
                max_model_calls: None,
                max_tokens: Some(1000),
            },
        )
        .await
        .expect("run");

    let breach = report.breach.expect(
        "a backend that CLAIMS token_usage but returns usage: None, under an active \
         --max-tokens ceiling, must be a hard error -- never silently treated as zero spend \
         (B4b, G6)",
    );
    assert!(
        breach.message.contains("node 'n'"),
        "the error must name the node: {}",
        breach.message
    );
    assert!(
        breach.message.to_lowercase().contains("usage"),
        "the error must explain that usage reporting failed: {}",
        breach.message
    );
}

// ============================================================================
// B5a: test_breach_stops_further_dispatch
// ============================================================================
#[tokio::test]
async fn test_breach_stops_further_dispatch() {
    let worker = ScriptedWorker::new();
    for id in ["n1", "n2", "n3", "n4"] {
        worker.script(
            id,
            Outcome::Success {
                output: "ok".into(),
                usage: None,
            },
        );
    }
    let d = dag(chain(&["n1", "n2", "n3", "n4"]));
    let mut scheduler = Scheduler::new(d, Box::new(worker.clone()), Box::new(VecSink::new()), 4);

    let report = scheduler
        .run_with_budget(
            false,
            BudgetLimits {
                max_model_calls: Some(2),
                max_tokens: None,
            },
        )
        .await
        .expect("run");

    assert!(report.breach.is_some(), "expected a breach: {report:?}");
    let dispatched: Vec<String> = worker.calls().into_iter().map(|(id, ..)| id).collect();
    assert!(
        !dispatched.contains(&"n3".to_string()) && !dispatched.contains(&"n4".to_string()),
        "no node may be dispatched after the breach: {dispatched:?}"
    );
}

// ============================================================================
// B5b: test_breach_exit_status_is_distinct
// ============================================================================
#[test]
fn test_breach_exit_status_is_distinct() {
    // ---- Budget breach: exit status must be 3. ----
    let dir = scratch("b5b-breach");
    write_mock_config(&dir);

    let mut n1 = llm_node("n1");
    n1.models = vec![model("mock-model")];
    let mut n2 = llm_node("n2");
    n2.models = vec![model("mock-model")];
    n2.depends_on = vec!["n1".to_string()];
    let d = dag(vec![n1, n2]);
    let dag_path = dir.join("dag.json");
    std::fs::write(&dag_path, serde_json::to_string(&d).unwrap()).unwrap();
    let vault_path = dir.join(".pidag/pidag.redb");

    let bin = env!("CARGO_BIN_EXE_pidag");
    let breach_output = Command::new(bin)
        .args([
            "run",
            dag_path.to_str().unwrap(),
            "--max-model-calls",
            "1",
            "--vault",
            vault_path.to_str().unwrap(),
        ])
        .output()
        .expect("spawn pidag");
    let breach_stderr = String::from_utf8_lossy(&breach_output.stderr);

    assert_eq!(
        breach_output.status.code(),
        Some(3),
        "a budget breach must exit with a distinct status (3): stderr={breach_stderr}"
    );
    assert!(
        breach_stderr.contains("budget ceiling breached"),
        "stderr={breach_stderr}"
    );

    // ---- Ordinary node failure: exit status must be 1, NOT 3. ----
    let dir2 = scratch("b5b-fail");
    let mut fail_node = base_node("f");
    fail_node.node_type = Some("shell".to_string());
    fail_node.prompt = "exit 1".to_string();
    let fail_dag = dag(vec![fail_node]);
    let fail_dag_path = dir2.join("dag.json");
    std::fs::write(&fail_dag_path, serde_json::to_string(&fail_dag).unwrap()).unwrap();
    let fail_vault = dir2.join(".pidag/pidag.redb");

    let fail_output = Command::new(bin)
        .args([
            "run",
            fail_dag_path.to_str().unwrap(),
            "--vault",
            fail_vault.to_str().unwrap(),
        ])
        .output()
        .expect("spawn pidag");

    assert_eq!(
        fail_output.status.code(),
        Some(1),
        "an ordinary node failure must exit 1, distinct from a budget breach's 3: stderr={}",
        String::from_utf8_lossy(&fail_output.stderr)
    );
    assert_ne!(
        breach_output.status.code(),
        fail_output.status.code(),
        "a budget breach and an ordinary node failure must be distinguishable by exit status \
         alone -- they demand different operator responses (raise-and-resume vs \
         fix-and-resume)"
    );
}

// ============================================================================
// B6: test_run_resumable_after_breach
// ============================================================================
#[tokio::test]
async fn test_run_resumable_after_breach() {
    let dir = scratch("b6");
    let vault_path = dir.join("vault.redb");

    let worker = ScriptedWorker::new();
    for id in ["n1", "n2", "n3"] {
        worker.script(
            id,
            Outcome::Success {
                output: format!("{id}-out"),
                usage: None,
            },
        );
    }
    let dag_v = dag(chain(&["n1", "n2", "n3"]));
    let run_id = "b6-run".to_string();

    // ---- First run: breaches after n1 (max_model_calls = 1). ----
    {
        let store: Arc<dyn Store> = Arc::new(RedbStore::open(&vault_path).expect("open vault"));
        store
            .put_run(&RunMeta {
                run_id: run_id.clone(),
                dag_json: serde_json::to_string(&dag_v).unwrap(),
                started_at: chrono::Utc::now().to_rfc3339(),
                completed_at: None,
                successful_nodes: 0,
                failed_nodes: 0,
            })
            .await
            .expect("put_run");

        let sink = Box::new(RedbSink::new(Arc::clone(&store), run_id.clone()));
        let mut scheduler = Scheduler::new(dag_v.clone(), Box::new(worker.clone()), sink, 1);
        let report = scheduler
            .run_with_budget(
                false,
                BudgetLimits {
                    max_model_calls: Some(1),
                    max_tokens: None,
                },
            )
            .await
            .expect("run 1");

        assert!(report.breach.is_some(), "first run must breach: {report:?}");
        assert_eq!(
            worker.call_count("n2"),
            0,
            "n2 must never dispatch in the first run"
        );
        assert_eq!(worker.call_count("n3"), 0);

        let run = store
            .get_run(&run_id)
            .await
            .expect("get_run")
            .expect("run exists");
        assert!(
            run.completed_at.is_none(),
            "a breached run must NOT be marked completed_at -- a resume must see it as \
             incomplete, not AlreadyDone (B6)"
        );
    }
    // `store` (and the redb file handle inside it) dropped here -- the next
    // block reopens the vault from scratch, as a resumed `pidag run
    // --resume` process would.

    // ---- Resume with a raised ceiling. ----
    {
        let store: Arc<dyn Store> = Arc::new(RedbStore::open(&vault_path).expect("reopen vault"));
        let decision = load_checkpoint(store.as_ref(), &run_id, false)
            .await
            .expect("load_checkpoint");
        let checkpoint: Checkpoint = match decision {
            ResumeDecision::Resume { checkpoint } => checkpoint,
            other => panic!("expected ResumeDecision::Resume, got {other:?}"),
        };
        assert!(
            checkpoint.completed_nodes.contains("n1"),
            "n1 must be carried forward as completed: {checkpoint:?}"
        );

        let sink = Box::new(RedbSink::new(Arc::clone(&store), run_id.clone()));
        let mut scheduler = Scheduler::with_checkpoint(
            dag_v.clone(),
            Box::new(worker.clone()),
            sink,
            1,
            checkpoint,
        );
        let report = scheduler
            .run_with_budget(
                false,
                BudgetLimits {
                    max_model_calls: Some(10),
                    max_tokens: None,
                },
            )
            .await
            .expect("run 2");

        assert!(
            report.breach.is_none(),
            "resumed run with a raised ceiling must complete: {report:?}"
        );
        assert!(
            report.failed.is_empty(),
            "resumed run must have no failures: {report:?}"
        );
        assert_eq!(
            worker.call_count("n1"),
            1,
            "n1 must NOT be re-dispatched on resume -- no completed work is redone"
        );
        assert_eq!(worker.call_count("n2"), 1);
        assert_eq!(worker.call_count("n3"), 1);
    }
}

// ============================================================================
// B7: test_counters_accumulate_across_resume
// ============================================================================
#[tokio::test]
async fn test_counters_accumulate_across_resume() {
    let dir = scratch("b7");
    let vault_path = dir.join("vault.redb");

    let usage100 = TokenUsage {
        input_tokens: 60,
        output_tokens: 40,
        total_tokens: 100,
    };
    let worker = ScriptedWorker::new();
    for id in ["n1", "n2", "n3"] {
        worker.script(
            id,
            Outcome::Success {
                output: format!("{id}-out"),
                usage: Some(usage100.clone()),
            },
        );
    }
    let dag_v = dag(chain(&["n1", "n2", "n3"]));
    let run_id = "b7-run".to_string();

    // ---- First run: n1+n2 complete (200 tok total). n3 is refused purely
    // by --max-model-calls=2 -- the SEPARATE --max-tokens=250 ceiling never
    // actually trips (200 <= 250). This isolates what B7 is actually
    // testing: the PERSISTED counters carried into a resume, not a token
    // breach's own bookkeeping.
    {
        let store: Arc<dyn Store> = Arc::new(RedbStore::open(&vault_path).expect("open vault"));
        store
            .put_run(&RunMeta {
                run_id: run_id.clone(),
                dag_json: serde_json::to_string(&dag_v).unwrap(),
                started_at: chrono::Utc::now().to_rfc3339(),
                completed_at: None,
                successful_nodes: 0,
                failed_nodes: 0,
            })
            .await
            .expect("put_run");

        let sink = Box::new(RedbSink::new(Arc::clone(&store), run_id.clone()));
        let mut scheduler = Scheduler::new(dag_v.clone(), Box::new(worker.clone()), sink, 1);
        let report = scheduler
            .run_with_budget(
                false,
                BudgetLimits {
                    max_model_calls: Some(2),
                    max_tokens: Some(250),
                },
            )
            .await
            .expect("run 1");

        assert!(
            report.breach.is_some(),
            "first run must breach on max-model-calls: {report:?}"
        );
        assert_eq!(report.model_calls, 2);
        assert_eq!(report.total_tokens, 200);

        let persisted = store.get_budget(&run_id).await.expect("get_budget");
        assert_eq!(
            persisted,
            BudgetCounters {
                model_calls: 2,
                total_tokens: 200
            },
            "counters must be PERSISTED IN THE VAULT, not only held in the scheduler's own \
             process memory (B7, G7)"
        );
    }

    // ---- Resume: raise BOTH ceilings generously. A counter that reset to
    // zero on resume would let this run spend far more than the operator
    // actually raised the ceiling to -- the assertions below are exactly
    // what such a reset would falsify.
    {
        let store: Arc<dyn Store> = Arc::new(RedbStore::open(&vault_path).expect("reopen vault"));
        let decision = load_checkpoint(store.as_ref(), &run_id, false)
            .await
            .expect("load_checkpoint");
        let checkpoint: Checkpoint = match decision {
            ResumeDecision::Resume { checkpoint } => checkpoint,
            other => panic!("expected ResumeDecision::Resume, got {other:?}"),
        };
        assert_eq!(
            checkpoint.budget,
            BudgetCounters {
                model_calls: 2,
                total_tokens: 200
            },
            "the checkpoint loaded for resume must carry the prior run's counters forward, not \
             reset to zero (B7). A reset would bound nothing."
        );

        let sink = Box::new(RedbSink::new(Arc::clone(&store), run_id.clone()));
        let mut scheduler = Scheduler::with_checkpoint(
            dag_v.clone(),
            Box::new(worker.clone()),
            sink,
            1,
            checkpoint,
        );
        let report = scheduler
            .run_with_budget(
                false,
                BudgetLimits {
                    max_model_calls: Some(10),
                    max_tokens: Some(1_000_000),
                },
            )
            .await
            .expect("run 2");

        assert!(
            report.breach.is_none(),
            "resumed run must complete: {report:?}"
        );
        assert_eq!(
            report.model_calls, 3,
            "2 carried forward + 1 (n3, newly dispatched) = 3, not 1 (which is what an \
             in-memory-only counter would report)"
        );
        assert_eq!(
            report.total_tokens, 300,
            "200 carried forward + 100 (n3) = 300, not 100"
        );
    }
}

// ============================================================================
// B8a: test_critic_dispatch_counts  -- THE ACCEPTANCE TEST (Exit Criterion 3)
// ============================================================================
#[tokio::test]
async fn test_critic_dispatch_counts() {
    let worker = ScriptedWorker::new();
    worker.script(
        "producer",
        Outcome::Success {
            output: "producer output".into(),
            usage: None,
        },
    );
    worker.script(
        "producer::verify",
        Outcome::Success {
            output: "PASS - fine".into(),
            usage: None,
        },
    );

    let mut node = llm_node("producer");
    node.verify = Some(Verify::Critic {
        prompt: "Review: {{producer.output}}".to_string(),
        models: vec![model("critic-model")],
    });
    let d = dag(vec![node]);
    let mut scheduler = Scheduler::new(d, Box::new(worker.clone()), Box::new(VecSink::new()), 1);

    let report = scheduler
        .run_with_budget(
            false,
            BudgetLimits {
                max_model_calls: Some(1),
                max_tokens: None,
            },
        )
        .await
        .expect("run");

    assert!(
        report.breach.is_some(),
        "a Verify::Critic dispatch is a model call and must count toward --max-model-calls -- \
         an accumulator wired only into the OUTER ready-loop's own dispatch decision would \
         never see this call at all, because a critic sub-dispatch never passes through the \
         ready queue (B8a, the acceptance test): report={report:?}"
    );
    assert_eq!(
        worker.call_count("producer::verify"),
        0,
        "once the ceiling is already spent by the producer's own call, the critic must never \
         actually be dispatched through the worker: {:?}",
        worker.calls()
    );
}

// ============================================================================
// B8b: test_for_each_children_count
// ============================================================================
#[tokio::test]
async fn test_for_each_children_count() {
    let worker = ScriptedWorker::new();

    let mut node = base_node("gen");
    node.for_each = Some(vec![
        "model-a".to_string(),
        "model-b".to_string(),
        "model-c".to_string(),
    ]);
    node.models = vec![ModelRef {
        name: "{{item}}".to_string(),
        paid: false,
    }];
    let d = dag(vec![node]).expand().expect("expand for_each");
    assert_eq!(
        d.nodes.len(),
        3,
        "sanity: for_each over 3 items expands to 3 nodes"
    );

    for n in &d.nodes {
        worker.script(
            &n.id,
            Outcome::Success {
                output: format!("{}-out", n.id),
                usage: None,
            },
        );
    }

    let mut scheduler = Scheduler::new(d, Box::new(worker.clone()), Box::new(VecSink::new()), 3);
    let report = scheduler
        .run_with_budget(
            false,
            BudgetLimits {
                max_model_calls: Some(2),
                max_tokens: None,
            },
        )
        .await
        .expect("run");

    assert!(report.breach.is_some(), "expected a breach: {report:?}");
    assert_eq!(
        worker.calls().len(),
        2,
        "only 2 of the 3 for_each children may ever be dispatched -- the third is never \
         dispatched: {:?}",
        worker.calls()
    );
}

// ============================================================================
// B9: test_report_shows_counters_without_a_ceiling
// ============================================================================
#[tokio::test]
async fn test_report_shows_counters_without_a_ceiling() {
    let worker = ScriptedWorker::new();
    let usage = TokenUsage {
        input_tokens: 30,
        output_tokens: 20,
        total_tokens: 50,
    };
    worker.script(
        "n1",
        Outcome::Success {
            output: "a".into(),
            usage: Some(usage.clone()),
        },
    );
    worker.script(
        "n2",
        Outcome::Success {
            output: "b".into(),
            usage: Some(usage.clone()),
        },
    );

    let d = dag(chain(&["n1", "n2"]));
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 2);
    // No ceiling flags at all: run(), not run_with_budget().
    let report = scheduler.run(false).await.expect("run");

    assert!(report.breach.is_none());
    assert_eq!(
        report.model_calls, 2,
        "cumulative model calls must be visible in the report even with no ceiling set (B9)"
    );
    assert_eq!(
        report.total_tokens, 100,
        "cumulative tokens must be visible in the report even with no ceiling set (B9)"
    );
}

// ============================================================================
// B10: test_no_flags_is_unchanged
// ============================================================================
#[tokio::test]
async fn test_no_flags_is_unchanged() {
    let worker = ScriptedWorker::new();
    worker.script(
        "n1",
        Outcome::Success {
            output: "ok".into(),
            usage: None,
        },
    );
    worker.script("n2", Outcome::Failure("boom".into()));

    let d = dag(chain(&["n1", "n2"]));
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 2);
    let report = scheduler.run(false).await.expect("run");

    // Pre-spec-39 behaviour, exactly: no budget ceiling exists, so a node
    // failure is reported exactly as before -- `failed` is populated,
    // `breach` is None (N1, B10), and nothing aborts early on account of a
    // budget that was never set.
    assert!(report.breach.is_none());
    assert_eq!(report.failed, vec!["n2".to_string()]);
    assert_eq!(state_of(&report, "n1").state, NodeStatus::Done);
}
