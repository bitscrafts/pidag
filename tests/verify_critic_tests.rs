//! TDD Contract for spec-37 ("`verify` becomes a critic").
//!
//! `Node.verify` widens from `Option<String>` to `Option<Verify>` --
//! `Verify::Shell` (today's behaviour, unchanged), `Verify::Critic`
//! (dispatches through `&dyn Worker`, not a subprocess), and `Verify::All`
//! (every arm must pass, short-circuit on the first failure). See
//! `specs/37-verify-critic.md` for the full contract; test ids below
//! (`C1`, `C2a`, ...) match its TDD Contract table.

use async_trait::async_trait;
use pidag::{
    Dag, Event, ModelRef, ModelsConfig, Node, NodeStatus, PidagError, RealShellWorker, RetryPolicy,
    Scheduler, TypeDispatchWorker, VecSink, Verify, Worker, WorkerOutput,
    workflow::{Template, TemplateContext, WorkflowEngine},
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ============================================================================
// Shared test fixtures
// ============================================================================

fn shell_model(name: &str) -> ModelRef {
    ModelRef {
        name: name.to_string(),
        paid: false,
    }
}

fn paid_model(name: &str) -> ModelRef {
    ModelRef {
        name: name.to_string(),
        paid: true,
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

fn dag(nodes: Vec<Node>) -> Dag {
    Dag {
        nodes,
        metadata: None,
    }
}

/// A producer node with a `Verify::Critic` attached. `models` is the
/// producer's own (non-critic) model chain; `critic_models` populate the
/// `Verify::Critic` arm.
fn producer_with_critic(id: &str, critic_prompt: &str, critic_models: Vec<ModelRef>) -> Node {
    let mut n = base_node(id);
    n.models = vec![shell_model("worker-model")];
    n.verify = Some(Verify::Critic {
        prompt: critic_prompt.to_string(),
        models: critic_models,
    });
    n
}

/// What a scripted worker call returns for a given node id.
#[derive(Clone, Debug)]
enum Outcome {
    Success(String),
    Failure(String),
    /// The `Worker::run` trait method itself returns `Err` (as opposed to
    /// `Ok(WorkerOutput{success:false,..})`) -- exercises C4e's "worker
    /// returns Err" path distinctly from a worker-reported failure.
    Error,
}

/// Records every dispatch (node_id, prompt, model, attempt) and returns a
/// scripted outcome keyed by node_id. Never spawns a subprocess -- it is
/// pure Rust, so any test that only ever touches `ScriptedWorker` and still
/// gets a passing/failing critic verdict is proof the critic path never
/// needed a shell command (C3a, Exit Criterion 4).
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
                usage: None,
            }),
            Some(Outcome::Failure(s)) => Ok(WorkerOutput {
                success: false,
                output: s,
                retryable: false,
                usage: None,
            }),
            Some(Outcome::Error) => Err(PidagError::WorkerFailed),
            None => Ok(WorkerOutput {
                success: true,
                output: format!("default output for {node_id}"),
                retryable: false,
                usage: None,
            }),
        }
    }
}

/// Find the `NodeVerifyFailed` event for `node_id`, if any.
fn verify_failed_for(events: &[Event], node_id: &str) -> Option<(String, String)> {
    events.iter().find_map(|e| match e {
        Event::NodeVerifyFailed {
            node_id: id,
            worker_claim,
            verify_output,
        } if id == node_id => Some((worker_claim.clone(), verify_output.clone())),
        _ => None,
    })
}

// ============================================================================
// C1: test_verify_enum_variants_roundtrip
// ============================================================================
#[test]
fn test_verify_enum_variants_roundtrip() {
    let shell = Verify::Shell("test -f x".to_string());
    let critic = Verify::Critic {
        prompt: "check {{n.output}}".to_string(),
        models: vec![shell_model("m1"), paid_model("m2")],
    };
    let all = Verify::All(vec![shell.clone(), critic.clone()]);

    for v in [shell, critic, all] {
        let json = serde_json::to_string(&v).expect("serialize Verify");
        let back: Verify = serde_json::from_str(&json).expect("deserialize Verify");
        assert_eq!(v, back, "roundtrip mismatch via {json}");
    }
}

// ============================================================================
// C2a: test_bare_string_verify_deserializes_as_shell
// ============================================================================
#[test]
fn test_bare_string_verify_deserializes_as_shell() {
    let json = r#"{
        "id": "n", "prompt": "p", "depends_on": [], "models": [],
        "retry": {"attempts": 1, "backoff_ms": 0}, "validate": null,
        "verify": "test -f x"
    }"#;
    let node: Node = serde_json::from_str(json).expect("deserialize node with bare verify");
    assert_eq!(node.verify, Some(Verify::Shell("test -f x".to_string())));
}

// ============================================================================
// C2b: test_toml_string_verify_still_loads
// ============================================================================
#[test]
fn test_toml_string_verify_still_loads() {
    let toml_src = r#"
name = "t37"
description = "spec-37 C2b"
iterations = 1

[[nodes]]
id = "n"
type = "shell"
command = "echo hi"
verify = "test -f x"
"#;
    let template: Template = toml::from_str(toml_src).expect("parse workflow toml");
    let context = TemplateContext {
        n: 0,
        spec_path: "s".to_string(),
        project_root: "p".to_string(),
        validate_script: "v".to_string(),
        quality_gate_script: "q".to_string(),
        prompts: HashMap::new(),
        models_config: ModelsConfig::default(),
    };
    let expanded = WorkflowEngine::expand(&template, 1, context).expect("expand template");
    let node = expanded.get_node("n").expect("node n");
    assert_eq!(
        node.verify,
        Some(Verify::Shell("test -f x".to_string())),
        "a TOML string verify must still load and behave as Verify::Shell"
    );
}

// ============================================================================
// C3a: test_critic_dispatches_through_worker
// ============================================================================
#[tokio::test]
async fn test_critic_dispatches_through_worker() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("producer output".into()));
    worker.script("producer::verify", Outcome::Success("PASS - fine".into()));

    let node = producer_with_critic(
        "producer",
        "Review: {{producer.output}}",
        vec![shell_model("critic-model")],
    );
    let d = dag(vec![node]);
    let mut scheduler = Scheduler::new(d, Box::new(worker.clone()), Box::new(VecSink::new()), 1);
    let report = scheduler.run(false).await.expect("run");

    assert!(
        report.failed.is_empty(),
        "expected producer Done, report={:?}",
        report
    );
    assert_eq!(
        worker.call_count("producer::verify"),
        1,
        "the critic must dispatch exactly once through the worker: {:?}",
        worker.calls()
    );
}

// ============================================================================
// C3b: test_critic_prompt_receives_node_output
// ============================================================================
#[tokio::test]
async fn test_critic_prompt_receives_node_output() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("HELLO".into()));
    worker.script("producer::verify", Outcome::Success("PASS - ok".into()));

    let node = producer_with_critic(
        "producer",
        "Review: {{producer.output}}",
        vec![shell_model("critic-model")],
    );
    let d = dag(vec![node]);
    let mut scheduler = Scheduler::new(d, Box::new(worker.clone()), Box::new(VecSink::new()), 1);
    let _ = scheduler.run(false).await.expect("run");

    let calls = worker.calls();
    let critic_call = calls
        .iter()
        .find(|(id, ..)| id == "producer::verify")
        .expect("critic must dispatch");
    assert!(
        critic_call.1.contains("HELLO"),
        "critic prompt must contain the producing node's output, got: {}",
        critic_call.1
    );
    assert!(
        !critic_call.1.contains("{{producer.output}}"),
        "the placeholder must be substituted, not passed through verbatim, got: {}",
        critic_call.1
    );
}

// ============================================================================
// C4a: test_critic_pass_verdict
// ============================================================================
#[tokio::test]
async fn test_critic_pass_verdict() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("out".into()));
    worker.script(
        "producer::verify",
        Outcome::Success("PASS - looks right".into()),
    );

    let node = producer_with_critic(
        "producer",
        "check {{producer.output}}",
        vec![shell_model("m")],
    );
    let d = dag(vec![node]);
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 1);
    let report = scheduler.run(false).await.expect("run");

    let state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "producer")
        .unwrap();
    assert_eq!(state.state, NodeStatus::Done);
}

// ============================================================================
// C4b: test_critic_fail_verdict
// ============================================================================
#[tokio::test]
async fn test_critic_fail_verdict() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("out".into()));
    worker.script(
        "producer::verify",
        Outcome::Success("FAIL - missing case".into()),
    );

    let node = producer_with_critic(
        "producer",
        "check {{producer.output}}",
        vec![shell_model("m")],
    );
    let d = dag(vec![node]);
    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.expect("run");

    let state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "producer")
        .unwrap();
    assert_eq!(state.state, NodeStatus::Failed);

    let (_, reason) =
        verify_failed_for(&sink.events(), "producer").expect("NodeVerifyFailed event");
    assert_eq!(reason, "missing case");
}

// ============================================================================
// C4c: test_unparseable_verdict_fails_closed
// ============================================================================
#[tokio::test]
async fn test_unparseable_verdict_fails_closed() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("out".into()));
    worker.script("producer::verify", Outcome::Success("I am not sure".into()));

    let node = producer_with_critic(
        "producer",
        "check {{producer.output}}",
        vec![shell_model("m")],
    );
    let d = dag(vec![node]);
    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.expect("run");

    let state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "producer")
        .unwrap();
    assert_eq!(
        state.state,
        NodeStatus::Failed,
        "an unparseable verdict must fail closed, never Done"
    );

    let (_, reason) =
        verify_failed_for(&sink.events(), "producer").expect("NodeVerifyFailed event");
    assert_eq!(
        reason, "I am not sure",
        "the raw reply must be preserved as the reason"
    );
}

// ============================================================================
// C4d: test_verdict_substring_does_not_pass
// ============================================================================
// THE ACCEPTANCE TEST FOR FAIL-CLOSED. A naive `reply.contains("PASS")`
// implementation reads "this does not PASS" as a pass -- see the Exit
// Criterion 2 transcript in the delivery report for this test run against
// that naive implementation (it fails), then against the real
// `parse_critic_verdict` (it passes).
#[tokio::test]
async fn test_verdict_substring_does_not_pass() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("out".into()));
    worker.script(
        "producer::verify",
        Outcome::Success("this does not PASS".into()),
    );

    let node = producer_with_critic(
        "producer",
        "check {{producer.output}}",
        vec![shell_model("m")],
    );
    let d = dag(vec![node]);
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 1);
    let report = scheduler.run(false).await.expect("run");

    let state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "producer")
        .unwrap();
    assert_eq!(
        state.state,
        NodeStatus::Failed,
        "a reply that merely contains the substring PASS must not pass -- \
         the naive `reply.contains(\"PASS\")` implementation gets this wrong"
    );
}

// ============================================================================
// C4e: test_critic_worker_error_fails_closed
// ============================================================================
#[tokio::test]
async fn test_critic_worker_error_fails_closed() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("out".into()));
    worker.script("producer::verify", Outcome::Error);

    let node = producer_with_critic(
        "producer",
        "check {{producer.output}}",
        vec![shell_model("m")],
    );
    let d = dag(vec![node]);
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 1);
    let report = scheduler.run(false).await.expect("run");

    let state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "producer")
        .unwrap();
    assert_eq!(
        state.state,
        NodeStatus::Failed,
        "a critic worker Err must fail closed, never Done"
    );
}

// A distinct C4e path: the worker trait call succeeds but reports a
// worker-attributed failure (`WorkerOutput{success:false,..}`, e.g. an
// exhausted model fallback), as opposed to the trait method itself
// returning `Err`. Both must fail closed.
#[tokio::test]
async fn test_critic_worker_failure_output_fails_closed() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("out".into()));
    worker.script(
        "producer::verify",
        Outcome::Failure("rate limited".to_string()),
    );

    let node = producer_with_critic(
        "producer",
        "check {{producer.output}}",
        vec![shell_model("m")],
    );
    let d = dag(vec![node]);
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 1);
    let report = scheduler.run(false).await.expect("run");

    let state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "producer")
        .unwrap();
    assert_eq!(
        state.state,
        NodeStatus::Failed,
        "an exhausted critic worker (success:false) must fail closed, never Done"
    );
}

// ============================================================================
// C5: test_critic_reason_reaches_repair_prompt
// ============================================================================
#[tokio::test]
async fn test_critic_reason_reaches_repair_prompt() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("bad output".into()));
    worker.script(
        "producer::verify",
        Outcome::Success("FAIL - approach is wrong".into()),
    );
    worker.script("repair", Outcome::Success("repaired".into()));

    let producer = producer_with_critic(
        "producer",
        "check {{producer.output}}",
        vec![shell_model("m")],
    );

    let mut repair = base_node("repair");
    repair.depends_on = vec!["producer".to_string()];
    repair.gate = Some("producer:fail".to_string());
    repair.models = vec![shell_model("worker-model")];
    repair.prompt = "Fix this: {{producer.output}}".to_string();

    let d = dag(vec![producer, repair]);
    let worker_clone = worker.clone();
    let mut scheduler = Scheduler::new(d, Box::new(worker_clone), Box::new(VecSink::new()), 1);
    let report = scheduler.run(false).await.expect("run");

    let repair_state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "repair")
        .unwrap();
    assert_eq!(
        repair_state.state,
        NodeStatus::Done,
        "the repair node must dispatch since producer failed"
    );

    let calls = worker.calls();
    let repair_call = calls
        .iter()
        .find(|(id, ..)| id == "repair")
        .expect("repair must dispatch");
    assert!(
        repair_call.1.contains("approach is wrong"),
        "the repair prompt must interpolate the critic's reason, got: {}",
        repair_call.1
    );
}

// ============================================================================
// C6a: test_all_requires_every_arm
// ============================================================================
#[tokio::test]
async fn test_all_requires_every_arm() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("out".into()));
    worker.script("producer::verify", Outcome::Success("FAIL - bad".into()));

    let mut node = base_node("producer");
    node.models = vec![shell_model("worker-model")];
    node.verify = Some(Verify::All(vec![
        Verify::Shell("exit 0".to_string()),
        Verify::Critic {
            prompt: "check {{producer.output}}".to_string(),
            models: vec![shell_model("m")],
        },
    ]));

    let d = dag(vec![node]);
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 1);
    let report = scheduler.run(false).await.expect("run");

    let state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "producer")
        .unwrap();
    assert_eq!(
        state.state,
        NodeStatus::Failed,
        "All must fail if any arm fails, even when an earlier arm passed"
    );
}

// ============================================================================
// C6b: test_all_short_circuits
// ============================================================================
#[tokio::test]
async fn test_all_short_circuits() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("out".into()));
    // Scripted to PASS -- if this ever dispatches, the test below catches it.
    worker.script(
        "producer::verify",
        Outcome::Success("PASS - would have passed".into()),
    );

    let mut node = base_node("producer");
    node.models = vec![shell_model("worker-model")];
    node.verify = Some(Verify::All(vec![
        Verify::Shell("exit 1".to_string()),
        Verify::Critic {
            prompt: "check {{producer.output}}".to_string(),
            models: vec![shell_model("m")],
        },
    ]));

    let d = dag(vec![node]);
    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(d, Box::new(worker.clone()), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.expect("run");

    let state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "producer")
        .unwrap();
    assert_eq!(state.state, NodeStatus::Failed);

    assert_eq!(
        worker.call_count("producer::verify"),
        0,
        "the critic arm must never dispatch once the shell arm already failed: {:?}",
        worker.calls()
    );

    let (_, reason) =
        verify_failed_for(&sink.events(), "producer").expect("NodeVerifyFailed event");
    assert!(
        reason.contains("shell"),
        "the reason must name the failing arm, got: {reason}"
    );
}

// ============================================================================
// C7: test_paid_critic_respects_allow_paid
// ============================================================================
#[tokio::test]
async fn test_paid_critic_respects_allow_paid() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("out".into()));
    // Scripted to PASS -- must never be reached with allow_paid=false.
    worker.script("producer::verify", Outcome::Success("PASS - fine".into()));

    let node = producer_with_critic(
        "producer",
        "check {{producer.output}}",
        vec![paid_model("expensive-critic")],
    );
    let d = dag(vec![node]);
    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(d, Box::new(worker.clone()), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.expect("run"); // allow_paid = false

    let state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "producer")
        .unwrap();
    assert_eq!(
        state.state,
        NodeStatus::Failed,
        "a critic blocked by allow_paid must not silently pass"
    );

    assert_eq!(
        worker.call_count("producer::verify"),
        0,
        "a paid-only critic must never dispatch when allow_paid=false: {:?}",
        worker.calls()
    );

    let (_, reason) =
        verify_failed_for(&sink.events(), "producer").expect("NodeVerifyFailed event");
    assert!(
        reason.to_lowercase().contains("allow_paid"),
        "the reason must say explicitly this is an allow_paid block, not a generic failure, got: {reason}"
    );
}

// ============================================================================
// C8: test_legacy_dag_json_verify_string
// ============================================================================
// Fixture-based wire-compatibility guard, mirroring the spec-36
// `legacy_vault` pattern: `tests/fixtures/legacy_dag/legacy_dag.json` is a
// hand-written (not live-serialized) pre-spec-37 `dag_json` blob with a bare
// string `verify`. Its generator, `tests/gen_legacy_dag.rs`, is
// `#[ignore]`d so it can never silently regenerate the fixture from the
// current build.
fn legacy_dag_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy_dag/legacy_dag.json")
}

#[test]
fn test_legacy_dag_json_verify_string() {
    let bytes = std::fs::read(legacy_dag_fixture_path()).expect("read legacy_dag.json fixture");

    const PINNED_SHA256: &str = "a04f115d331fe7b618d2fb6827319baf4f7aeb8b897229a49968a05c214efcb6";
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = format!("{:x}", hasher.finalize());
    assert_eq!(
        actual, PINNED_SHA256,
        "tests/fixtures/legacy_dag/legacy_dag.json must stay byte-identical to the pinned \
         pre-spec-37 blob -- if this fails, the fixture was regenerated (spec-37 C8, G9)"
    );

    let content = String::from_utf8(bytes).expect("fixture must be UTF-8");
    let loaded: Dag =
        serde_json::from_str(&content).expect("legacy dag_json must still deserialize (C2/C8)");

    let build = loaded.get_node("build").expect("build node");
    assert_eq!(
        build.verify,
        Some(Verify::Shell("test -f out.txt".to_string())),
        "a pre-spec-37 bare string verify must load and behave as Verify::Shell"
    );

    let report = loaded.get_node("report").expect("report node");
    assert!(report.verify.is_none());
}

#[test]
fn test_legacy_dag_generator_is_ignored() {
    let src = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/gen_legacy_dag.rs"),
    )
    .expect("read gen_legacy_dag.rs");

    let fn_pos = src
        .find("fn gen_legacy_dag_fixture")
        .expect("generator function must exist");
    let before_fn = &src[..fn_pos];
    let ignore_pos = before_fn
        .rfind("#[ignore]")
        .expect("#[ignore] must be present on the generator");
    let between = &before_fn[ignore_pos + "#[ignore]".len()..];
    assert!(
        between.trim().is_empty() || between.trim_start().starts_with("#["),
        "#[ignore] must sit directly on gen_legacy_dag_fixture, not on something else: {:?}",
        between
    );
}

// ============================================================================
// C10: test_critic_emits_events
// ============================================================================
#[tokio::test]
async fn test_critic_emits_events() {
    let worker = ScriptedWorker::new();
    worker.script("producer", Outcome::Success("out".into()));
    worker.script("producer::verify", Outcome::Success("PASS - ok".into()));

    let node = producer_with_critic(
        "producer",
        "check {{producer.output}}",
        vec![shell_model("critic-model")],
    );
    let d = dag(vec![node]);
    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(sink.clone()), 1);
    let _ = scheduler.run(false).await.expect("run");

    let dispatched = sink.events().into_iter().any(|e| {
        matches!(
            e,
            Event::NodeDispatched { node_id, model, .. }
                if node_id == "producer::verify" && model == "critic-model"
        )
    });
    assert!(
        dispatched,
        "a critic dispatch must appear in the event log like any other model call: {:?}",
        sink.events()
    );
}

// ============================================================================
// N1a: test_existing_shell_verify_unchanged
// ============================================================================
#[tokio::test]
async fn test_existing_shell_verify_unchanged() {
    let tmpdir = PathBuf::from("_tmp/verify_critic/n1a");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    std::fs::write(tmpdir.join("marker.txt"), "x").unwrap();

    let mut node = base_node("shell_node");
    node.node_type = Some("shell".to_string());
    node.prompt = "echo ok".to_string();
    node.verify = Some(Verify::Shell(format!(
        "test -f {}/marker.txt",
        tmpdir.display()
    )));

    let d = dag(vec![node]);
    let worker = RealShellWorker::new(&d, Duration::from_secs(5));
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 1);
    let report = scheduler.run(false).await.expect("run");

    let state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "shell_node")
        .unwrap();
    assert_eq!(
        state.state,
        NodeStatus::Done,
        "a bare Verify::Shell must behave exactly as pre-spec-37 (N1)"
    );
}

// ============================================================================
// N1b: test_verify_pre_unchanged
// ============================================================================
#[tokio::test]
async fn test_verify_pre_unchanged() {
    let mut node = base_node("pre_node");
    node.node_type = Some("shell".to_string());
    node.prompt = "echo ok".to_string();
    node.verify_pre = Some("echo TOKEN123".to_string());
    node.verify = Some(Verify::Shell(
        "test \"$PIDAG_VERIFY_PRE\" = TOKEN123".to_string(),
    ));

    let d = dag(vec![node]);
    let worker = RealShellWorker::new(&d, Duration::from_secs(5));
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(VecSink::new()), 1);
    let report = scheduler.run(false).await.expect("run");

    let state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "pre_node")
        .unwrap();
    assert_eq!(
        state.state,
        NodeStatus::Done,
        "verify_pre's PIDAG_VERIFY_PRE contract must be unchanged (C9, N1b)"
    );
}

// ============================================================================
// Exit Criterion 3: a real run.
// ============================================================================
// A two-node DAG: a real `bash -c` shell worker (`RealShellWorker`, via
// `TypeDispatchWorker`) writes a file whose content does NOT satisfy the
// stated intent, and a critic dispatched through the real subprocess-spawn
// path of `PiPrintWorker` (`pi` replaced by a deterministic bash shim --
// this container has no live model credentials, so the shim stands in for
// the judgment a real LLM critic would make, while the wiring across the
// process boundary is exercised for real) catches the mismatch. This is the
// seam-level proof the spec calls for: a passing unit suite is not evidence
// the seam works (docs/FINDINGS.md).
#[tokio::test]
async fn test_real_run_shell_producer_critic_catches_mismatch() {
    let tmpdir = PathBuf::from("_tmp/verify_critic/real_run");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let shim = r#"prompt="${@: -1}"; if echo "$prompt" | grep -q "Actual file contents: hello world"; then echo "PASS - the file matches the required content"; else echo "FAIL - the file does not contain the required content 'hello world'"; fi"#;

    let mut producer = base_node("write_greeting");
    producer.node_type = Some("shell".to_string());
    producer.prompt = format!(
        "printf 'goodbye world' > {dir}/greeting.txt && cat {dir}/greeting.txt",
        dir = tmpdir.display()
    );
    producer.verify = Some(Verify::Critic {
        prompt: "The file must contain exactly 'hello world'. Actual file contents: \
                  {{write_greeting.output}}. Reply with a leading PASS or FAIL token and a \
                  one-line reason."
            .to_string(),
        models: vec![ModelRef {
            name: "stub-critic".to_string(),
            paid: false,
        }],
    });

    let mut downstream = base_node("report");
    downstream.node_type = Some("shell".to_string());
    downstream.depends_on = vec!["write_greeting".to_string()];
    downstream.prompt = "echo done".to_string();

    let d = dag(vec![producer, downstream]);
    let worker = TypeDispatchWorker::with_pi_command(
        &d,
        Duration::from_secs(10),
        "bash".to_string(),
        vec!["-c".to_string(), shim.to_string()],
    );

    let sink = VecSink::new();
    let mut scheduler = Scheduler::new(d, Box::new(worker), Box::new(sink.clone()), 1);
    let report = scheduler.run(false).await.expect("run");

    let producer_state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "write_greeting")
        .unwrap();
    assert_eq!(
        producer_state.state,
        NodeStatus::Failed,
        "the critic must catch the mismatch between stated intent and actual file content"
    );

    let downstream_state = report
        .node_states
        .iter()
        .find(|s| s.node_id == "report")
        .unwrap();
    assert_eq!(
        downstream_state.state,
        NodeStatus::Blocked,
        "a failed producer must block its dependent"
    );

    let (worker_claim, verify_output) = verify_failed_for(&sink.events(), "write_greeting")
        .expect("NodeVerifyFailed event must be present");

    // EXIT CRITERION 3: the real NodeVerifyFailed event, quoted.
    println!(
        "REAL RUN NodeVerifyFailed: node_id=write_greeting worker_claim={:?} verify_output={:?}",
        worker_claim, verify_output
    );

    assert!(worker_claim.contains("goodbye world"));
    assert!(verify_output.contains("does not contain"));
}
