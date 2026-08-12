//! TDD Contract for spec-38 ("`for_each` fan-out and `quorum` adjudication").
//!
//! See `specs/38-foreach-and-quorum.md` for the full contract; test ids
//! below (`F1`, `F2a`, ...) match its TDD Contract table.

use async_trait::async_trait;
use pidag::{
    Dag, ModelRef, Node, NodeStatus, PidagError, QuorumConfig, RetryPolicy, Scheduler, VecSink,
    Worker, WorkerOutput,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
        models: vec![model("test-model")],
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

fn dag(nodes: Vec<Node>) -> Dag {
    Dag {
        nodes,
        metadata: None,
    }
}

/// A `for_each` node fanning out over `items`.
fn fanned_node(id: &str, items: &[&str]) -> Node {
    let mut n = base_node(id);
    n.for_each = Some(items.iter().map(|s| s.to_string()).collect());
    n
}

/// A `node_type = "quorum"` node counting `of`.
fn quorum_node(id: &str, of: &[&str], min_pass: usize) -> Node {
    let mut n = base_node(id);
    n.node_type = Some("quorum".to_string());
    n.quorum = Some(QuorumConfig {
        of: of.iter().map(|s| s.to_string()).collect(),
        min_pass,
    });
    n
}

/// What a scripted worker call returns for a given node id.
#[derive(Clone, Debug)]
enum Outcome {
    Success(String),
    Failure(String),
}

/// Records every dispatch (node_id, prompt, model, attempt) and returns a
/// scripted outcome keyed by node_id. Never spawns a subprocess -- any test
/// that only ever touches `ScriptedWorker` and still gets a correct quorum
/// tally is proof the quorum path never dispatched it (F7a, Exit Criterion 4).
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
            Some(Outcome::Success(s)) => Ok(WorkerOutput {
                success: true,
                output: s,
                retryable: false,
            }),
            Some(Outcome::Failure(s)) => Ok(WorkerOutput {
                success: false,
                output: s,
                retryable: false,
            }),
            None => Ok(WorkerOutput {
                success: true,
                output: format!("default output for {node_id}"),
                retryable: false,
            }),
        }
    }
}

fn state_of<'a>(report: &'a pidag::RunReport, id: &str) -> &'a pidag::NodeState {
    report
        .node_states
        .iter()
        .find(|s| s.node_id == id)
        .unwrap_or_else(|| panic!("no state recorded for node '{id}', report={report:?}"))
}

// ============================================================================
// F1: test_for_each_expands_to_one_node_per_item
// ============================================================================
#[test]
fn test_for_each_expands_to_one_node_per_item() {
    let d = dag(vec![fanned_node("critic", &["m1", "m2", "m3"])]);
    let expanded = d.expand().expect("expand");

    assert_eq!(
        expanded.nodes.len(),
        3,
        "3 items must expand to 3 nodes, got {:?}",
        expanded.nodes.iter().map(|n| &n.id).collect::<Vec<_>>()
    );
    assert!(
        expanded.get_node("critic").is_none(),
        "the un-expanded parent id must be absent post-expansion"
    );
}

// ============================================================================
// F2a: test_item_substituted_in_prompt_and_models
// ============================================================================
#[test]
fn test_item_substituted_in_prompt_and_models() {
    let mut n = fanned_node("critic", &["gpt-4o", "claude"]);
    n.prompt = "review with {{item}}".to_string();
    n.models = vec![model("provider/{{item}}")];
    let d = dag(vec![n]);
    let expanded = d.expand().expect("expand");

    let gpt = expanded
        .nodes
        .iter()
        .find(|n| n.prompt.contains("gpt-4o"))
        .expect("a child prompt must mention gpt-4o");
    assert_eq!(gpt.models[0].name, "provider/gpt-4o");

    let claude = expanded
        .nodes
        .iter()
        .find(|n| n.prompt.contains("claude"))
        .expect("a child prompt must mention claude");
    assert_eq!(claude.models[0].name, "provider/claude");
}

// ============================================================================
// F2b: test_no_item_placeholder_survives_expansion
// ============================================================================
#[test]
fn test_no_item_placeholder_survives_expansion() {
    let mut n = fanned_node("critic", &["m1", "m2"]);
    n.prompt = "review with {{item}}".to_string();
    n.models = vec![model("{{item}}")];
    n.gate = Some("{{item}}:fail".to_string());
    let d = dag(vec![n]);
    let expanded = d.expand().expect("expand");

    for child in &expanded.nodes {
        assert!(
            !child.prompt.contains("{{item}}"),
            "prompt still has {{{{item}}}}: {}",
            child.prompt
        );
        assert!(
            child.models.iter().all(|m| !m.name.contains("{{item}}")),
            "a model name still has {{{{item}}}}: {:?}",
            child.models
        );
        assert!(
            child
                .gate
                .as_deref()
                .is_none_or(|g| !g.contains("{{item}}")),
            "gate still has {{{{item}}}}: {:?}",
            child.gate
        );
    }
}

// ============================================================================
// F3a: test_child_ids_are_slugified
// ============================================================================
#[test]
fn test_child_ids_are_slugified() {
    let d = dag(vec![fanned_node("n", &["GPT-4o", "claude 5"])]);
    let expanded = d.expand().expect("expand");
    let ids: Vec<&str> = expanded.nodes.iter().map(|n| n.id.as_str()).collect();

    assert!(ids.contains(&"n-gpt-4o"), "got ids: {ids:?}");
    assert!(ids.contains(&"n-claude-5"), "got ids: {ids:?}");
}

// ============================================================================
// F3b: test_child_ids_are_deterministic
// ============================================================================
#[test]
fn test_child_ids_are_deterministic() {
    let d = dag(vec![fanned_node("n", &["a", "b", "c"])]);
    let e1 = d.clone().expand().expect("expand 1");
    let e2 = d.expand().expect("expand 2");

    let ids1: Vec<&str> = e1.nodes.iter().map(|n| n.id.as_str()).collect();
    let ids2: Vec<&str> = e2.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids1, ids2, "expanding twice must yield identical ids/order");
}

// ============================================================================
// F3c: test_id_collision_falls_back_to_index
// ============================================================================
#[test]
fn test_id_collision_falls_back_to_index() {
    // "a!" and "a?" both slugify to "a-" -- a collision must fall back to
    // the index rather than silently dropping a node or panicking.
    let d = dag(vec![fanned_node("n", &["a!", "a?"])]);
    let expanded = d.expand().expect("expand must not panic on a collision");

    assert_eq!(expanded.nodes.len(), 2, "both items must produce a node");
    let ids: Vec<&str> = expanded.nodes.iter().map(|n| n.id.as_str()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), 2, "ids must be distinct, got {ids:?}");
}

// ============================================================================
// F4a: test_depends_on_parent_expands_to_children
// ============================================================================
#[test]
fn test_depends_on_parent_expands_to_children() {
    let critic = fanned_node("critic", &["m1", "m2", "m3"]);
    let mut adjudicate = base_node("adjudicate");
    adjudicate.after = vec!["critic".to_string()];
    let d = dag(vec![critic, adjudicate]);
    let expanded = d.expand().expect("expand");

    let adj = expanded.get_node("adjudicate").expect("adjudicate node");
    let expected: std::collections::HashSet<&str> = ["critic-m1", "critic-m2", "critic-m3"]
        .into_iter()
        .collect();
    let got: std::collections::HashSet<&str> = adj.after.iter().map(|s| s.as_str()).collect();
    assert_eq!(got, expected, "after must expand to all 3 children");
}

// ============================================================================
// F4b: test_output_reference_to_parent_expands
// ============================================================================
#[test]
fn test_output_reference_to_parent_expands() {
    let critic = fanned_node("critic", &["m1", "m2"]);
    let mut downstream = base_node("downstream");
    downstream.after = vec!["critic".to_string()];
    downstream.prompt = "verdicts: {{critic.output}}".to_string();
    let d = dag(vec![critic, downstream]);
    let expanded = d.expand().expect("expand");

    let ds = expanded.get_node("downstream").expect("downstream node");
    assert!(
        !ds.prompt.contains("{{critic.output}}"),
        "the parent-id placeholder must not survive: {}",
        ds.prompt
    );
    assert!(
        ds.prompt.contains("{{critic-m1.output}}"),
        "must resolve to child critic-m1: {}",
        ds.prompt
    );
    assert!(
        ds.prompt.contains("{{critic-m2.output}}"),
        "must resolve to child critic-m2: {}",
        ds.prompt
    );
}

// ============================================================================
// F4': test_gate_on_foreach_parent_is_an_error
// ============================================================================
#[test]
fn test_gate_on_foreach_parent_is_an_error() {
    // F4' / G9': `gate: "critic:fail"` where `critic` fans out over three
    // models must be a validation error -- never silently rewritten to an
    // arbitrary child (e.g. "critic-model-a:fail"), which would gate on one
    // arbitrary member of the ensemble while reading, to anyone scanning the
    // DAG, as though it gated on all of them.
    let critic = fanned_node("critic", &["model-a", "model-b", "model-c"]);
    let mut repair = base_node("repair");
    repair.gate = Some("critic:fail".to_string());
    let d = dag(vec![critic, repair]);

    let result = d.expand();

    let err = match result {
        Err(e) => e,
        Ok(expanded) => panic!(
            "gate on a for_each parent must be a validation error, not a \
             successful expansion: repair.gate = {:?}",
            expanded.get_node("repair").and_then(|n| n.gate.clone())
        ),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("repair"),
        "error must name the gating node 'repair': {msg}"
    );
    assert!(
        msg.contains("critic"),
        "error must name the referenced parent 'critic': {msg}"
    );
    assert!(
        msg.to_lowercase().contains("quorum"),
        "error must direct the author to a quorum node: {msg}"
    );
    assert!(
        !msg.contains("critic-model-a:fail"),
        "must never be silently rewritten to one arbitrary child: {msg}"
    );
}

// ============================================================================
// F5a: test_expansion_precedes_validation
// ============================================================================
#[test]
fn test_expansion_precedes_validation() {
    // "r" for_each's over ["a", "b"]. Its OWN depends_on forward-references
    // the id its "b" child will get once expanded ("r-b") -- a reference
    // that does not exist as a node before expansion at all, and becomes a
    // self-cycle (r-b depends on r-b) only once expansion has run. If
    // validation ran on the un-expanded graph, "r-b" wouldn't even resolve
    // (dangling), and the self-cycle would never be seen. This is the
    // premise F5 protects: cycle/dangling checks must see the real,
    // expanded, executed graph.
    let mut r = fanned_node("r", &["a", "b"]);
    r.depends_on = vec!["r-b".to_string()];
    let d = dag(vec![r]);

    let expanded = d.expand().expect("expansion itself must succeed");
    let result = expanded.validate();
    assert!(
        result.is_err(),
        "the self-cycle introduced by expansion must be caught by validate()"
    );
}

// ============================================================================
// F5b: test_empty_for_each_is_an_error
// ============================================================================
#[test]
fn test_empty_for_each_is_an_error() {
    let empty: &[&str] = &[];
    let d = dag(vec![fanned_node("n", empty)]);
    let result = d.expand();
    assert!(
        result.is_err(),
        "an empty for_each list must be a validation error, not a vanishing node"
    );
}

// ============================================================================
// F6: test_vault_stores_expanded_dag
// ============================================================================
#[tokio::test]
async fn test_vault_stores_expanded_dag() {
    use pidag::{RedbSink, RunMeta, Store};

    let critic = fanned_node("critic", &["m1", "m2"]);
    let d = dag(vec![critic]);
    let expanded = d.expand().expect("expand");
    let expanded_json = serde_json::to_string(&expanded).expect("serialize expanded dag");

    // As `pidag run` does (spec-38): pre-seed RunMeta with the EXPANDED
    // dag_json before the scheduler runs.
    let tmpdir = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/_tmp"))
        .join("foreach_quorum_tests")
        .join("test_vault_stores_expanded_dag");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let store: Arc<dyn Store> =
        Arc::new(pidag::RedbStore::open(&tmpdir.join("pidag.redb")).expect("open vault"));
    let run_id = "test-run-foreach".to_string();
    store
        .put_run(&RunMeta {
            run_id: run_id.clone(),
            dag_json: expanded_json,
            started_at: "now".to_string(),
            completed_at: None,
            successful_nodes: 0,
            failed_nodes: 0,
        })
        .await
        .expect("pre-seed run");

    // Actually run the expanded graph through the real scheduler, sinking
    // events into the same vault (mirroring the real pipeline).
    let worker = ScriptedWorker::new();
    let sink = RedbSink::new(Arc::clone(&store), run_id.clone());
    let mut scheduler = Scheduler::new(expanded, Box::new(worker), Box::new(sink), 2);
    let _ = scheduler.run(false).await.expect("run");

    let stored = store
        .get_run(&run_id)
        .await
        .expect("get_run")
        .expect("run must exist");
    assert!(
        stored.dag_json.contains("critic-m1") && stored.dag_json.contains("critic-m2"),
        "dag_json must contain child ids: {}",
        stored.dag_json
    );
    let parsed: serde_json::Value = serde_json::from_str(&stored.dag_json).unwrap();
    let ids: Vec<&str> = parsed["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert!(
        !ids.contains(&"critic"),
        "the unexpanded parent id must not appear in dag_json: {ids:?}"
    );
}

// ============================================================================
// F7a: test_quorum_dispatches_no_worker
// ============================================================================
#[tokio::test]
async fn test_quorum_dispatches_no_worker() {
    let worker = ScriptedWorker::new();
    worker.script("critic-a", Outcome::Success("PASS - fine".into()));
    worker.script("critic-b", Outcome::Success("PASS - fine".into()));

    let critic_a = base_node("critic-a");
    let critic_b = base_node("critic-b");
    let q = quorum_node("adjudicate", &["critic-a", "critic-b"], 1);
    let d = dag(vec![critic_a, critic_b, q]);

    let mut scheduler = Scheduler::new(d, Box::new(worker.clone()), Box::new(VecSink::new()), 2);
    let report = scheduler.run(false).await.expect("run");

    assert_eq!(
        worker.call_count("adjudicate"),
        0,
        "quorum must never be dispatched through the worker: {:?}",
        worker.calls()
    );
    let adj = state_of(&report, "adjudicate");
    assert_eq!(adj.state, NodeStatus::Done);
}

// ============================================================================
// F7b: test_quorum_passes_at_threshold
// ============================================================================
#[tokio::test]
async fn test_quorum_passes_at_threshold() {
    let worker = ScriptedWorker::new();
    worker.script("c1", Outcome::Success("PASS - ok".into()));
    worker.script("c2", Outcome::Success("PASS - ok".into()));
    worker.script("c3", Outcome::Success("FAIL - nope".into()));

    let d = dag(vec![
        base_node("c1"),
        base_node("c2"),
        base_node("c3"),
        quorum_node("adjudicate", &["c1", "c2", "c3"], 2),
    ]);
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 3);
    let report = scheduler.run(false).await.expect("run");

    assert_eq!(state_of(&report, "adjudicate").state, NodeStatus::Done);
}

// ============================================================================
// F7c: test_quorum_fails_below_threshold
// ============================================================================
#[tokio::test]
async fn test_quorum_fails_below_threshold() {
    let worker = ScriptedWorker::new();
    worker.script("c1", Outcome::Success("PASS - ok".into()));
    worker.script("c2", Outcome::Success("FAIL - nope".into()));
    worker.script("c3", Outcome::Success("FAIL - nope".into()));

    let d = dag(vec![
        base_node("c1"),
        base_node("c2"),
        base_node("c3"),
        quorum_node("adjudicate", &["c1", "c2", "c3"], 2),
    ]);
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 3);
    let report = scheduler.run(false).await.expect("run");

    assert_eq!(state_of(&report, "adjudicate").state, NodeStatus::Failed);
}

// ============================================================================
// F8a: test_quorum_uses_shared_verdict_parser
// ============================================================================
#[test]
fn test_quorum_uses_shared_verdict_parser() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/scheduler/execute.rs"
    ))
    .expect("read execute.rs");

    let def_count = src.matches("fn parse_critic_verdict").count();
    assert_eq!(
        def_count, 1,
        "exactly one definition of parse_critic_verdict must exist, found {def_count}"
    );

    let call_count = src.matches("Self::parse_critic_verdict(").count();
    assert!(
        call_count >= 2,
        "parse_critic_verdict must be called from both the critic path (eval_critic) \
         and quorum (compute_quorum), found {call_count} call site(s)"
    );
}

// ============================================================================
// F8b: test_quorum_unparseable_verdict_counts_as_fail
// ============================================================================
#[tokio::test]
async fn test_quorum_unparseable_verdict_counts_as_fail() {
    let worker = ScriptedWorker::new();
    worker.script("c1", Outcome::Success("hmm".into())); // unparseable -> FAIL
    worker.script("c2", Outcome::Success("PASS - ok".into()));
    worker.script("c3", Outcome::Success("PASS - ok".into()));

    let d = dag(vec![
        base_node("c1"),
        base_node("c2"),
        base_node("c3"),
        quorum_node("adjudicate", &["c1", "c2", "c3"], 3),
    ]);
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 3);
    let report = scheduler.run(false).await.expect("run");

    let adj = state_of(&report, "adjudicate");
    assert_eq!(
        adj.state,
        NodeStatus::Failed,
        "an unparseable verdict must fail closed, not pass: {adj:?}"
    );
    let output = adj.output.as_deref().unwrap_or("");
    assert!(
        output.contains("2/3 passed"),
        "expected 2/3 passed (fail-closed on the unparseable one), got: {output}"
    );
}

// ============================================================================
// F9a: test_quorum_of_ids_added_to_after
// ============================================================================
#[test]
fn test_quorum_of_ids_added_to_after() {
    let d = dag(vec![
        base_node("a"),
        base_node("b"),
        quorum_node("adjudicate", &["a", "b"], 1),
    ]);
    let expanded = d.expand().expect("expand");
    let adj = expanded.get_node("adjudicate").expect("adjudicate node");

    assert!(
        adj.after.contains(&"a".to_string()),
        "after={:?}",
        adj.after
    );
    assert!(
        adj.after.contains(&"b".to_string()),
        "after={:?}",
        adj.after
    );
    assert!(
        adj.depends_on.is_empty(),
        "quorum must never be wired through depends_on: {:?}",
        adj.depends_on
    );
}

// ============================================================================
// F9b: test_quorum_counts_failed_critics  -- THE ACCEPTANCE TEST
// ============================================================================
//
// All 3 critics FAIL. Because `of` is wired into `after` (never
// `depends_on` -- F9, G7), the adjudicator must still RUN and report 0
// passed, not go Blocked. This is the one the obvious (depends_on) wiring
// gets wrong: see Exit Criterion 2 for the before/after proof.
#[tokio::test]
async fn test_quorum_counts_failed_critics() {
    let worker = ScriptedWorker::new();
    worker.script("critic-a", Outcome::Failure("FAIL - bug found".into()));
    worker.script("critic-b", Outcome::Failure("FAIL - bug found".into()));
    worker.script("critic-c", Outcome::Failure("FAIL - bug found".into()));

    let d = dag(vec![
        base_node("critic-a"),
        base_node("critic-b"),
        base_node("critic-c"),
        quorum_node("adjudicate", &["critic-a", "critic-b", "critic-c"], 2),
    ]);
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 3);
    let report = scheduler.run(false).await.expect("run");

    for id in ["critic-a", "critic-b", "critic-c"] {
        assert_eq!(state_of(&report, id).state, NodeStatus::Failed, "{id}");
    }
    let adj = state_of(&report, "adjudicate");
    assert_ne!(
        adj.state,
        NodeStatus::Blocked,
        "quorum must run when its critics fail, not be Blocked -- the premise-2 defect"
    );
    assert_eq!(adj.state, NodeStatus::Failed, "0 of 3 passed, min_pass=2");
    let output = adj.output.as_deref().unwrap_or("");
    assert!(
        output.contains("0/3 passed"),
        "must report 0 passed, got: {output}"
    );
}

// ============================================================================
// F10: test_quorum_output_lists_each_verdict
// ============================================================================
#[tokio::test]
async fn test_quorum_output_lists_each_verdict() {
    let worker = ScriptedWorker::new();
    worker.script("c1", Outcome::Success("PASS - looks good".into()));
    worker.script("c2", Outcome::Failure("FAIL - missing tests".into()));

    let d = dag(vec![
        base_node("c1"),
        base_node("c2"),
        quorum_node("adjudicate", &["c1", "c2"], 1),
    ]);
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 2);
    let report = scheduler.run(false).await.expect("run");

    let adj = state_of(&report, "adjudicate");
    let output = adj.output.as_deref().unwrap_or("");
    assert!(output.contains("c1"), "must name c1: {output}");
    assert!(output.contains("c2"), "must name c2: {output}");
    assert!(output.contains("PASS"), "must show c1's verdict: {output}");
    assert!(output.contains("FAIL"), "must show c2's verdict: {output}");
    assert!(
        output.contains("missing tests"),
        "must carry the reason so a repair node can interpolate why: {output}"
    );
}

// ============================================================================
// F11: test_min_pass_bounds_are_validated
// ============================================================================
#[test]
fn test_min_pass_bounds_are_validated() {
    let zero = dag(vec![base_node("a"), quorum_node("adjudicate", &["a"], 0)]);
    let zero_result = zero.expand().and_then(|d| d.validate());
    assert!(
        zero_result.is_err(),
        "min_pass=0 must be a validation error"
    );

    let too_high = dag(vec![
        base_node("a"),
        base_node("b"),
        base_node("c"),
        quorum_node("adjudicate", &["a", "b", "c"], 4),
    ]);
    let too_high_result = too_high.expand().and_then(|d| d.validate());
    assert!(
        too_high_result.is_err(),
        "min_pass=4 of 3 must be a validation error"
    );
}

// ============================================================================
// N1: test_dag_without_for_each_is_unchanged
// ============================================================================
#[test]
fn test_dag_without_for_each_is_unchanged() {
    let d = dag(vec![base_node("a"), {
        let mut b = base_node("b");
        b.depends_on = vec!["a".to_string()];
        b
    }]);
    let before = serde_json::to_string(&d).expect("serialize before");
    let expanded = d.expand().expect("expand");
    let after = serde_json::to_string(&expanded).expect("serialize after");

    assert_eq!(
        before, after,
        "a DAG with no for_each/quorum must expand byte-identically"
    );
}
