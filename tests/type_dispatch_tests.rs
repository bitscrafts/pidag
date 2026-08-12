use pidag::{Dag, TypeDispatchWorker, Worker};
use std::sync::Mutex;
use std::time::Duration;

/// Serializes env-var tests (spec-15 S3) so PI_NO_SESSION doesn't race S1/S2.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Create a DAG with a single node of the specified type.
fn single_typed_node_dag(id: &str, prompt: &str, node_type: Option<&str>) -> Dag {
    let node_type_json = match node_type {
        Some(t) => format!(r#""node_type": "{t}","#),
        None => String::new(),
    };
    let json = format!(
        r#"{{
            "nodes": [
                {{
                    "id": "{id}",
                    "prompt": "{prompt}",
                    "depends_on": [],
                    {node_type_json}
                    "models": [{{"name": "nvidia", "paid": false}}],
                    "retry": {{"attempts": 1, "backoff_ms": 0}}
                }}
            ]
        }}"#
    );
    serde_json::from_str(&json).expect("valid single-node dag fixture")
}

/// Create a DAG with mixed node types for routing tests.
fn mixed_dag() -> Dag {
    let json = r#"
    {
        "nodes": [
            {
                "id": "shell_node",
                "prompt": "echo shell",
                "depends_on": [],
                "node_type": "shell",
                "models": [{"name": "nvidia", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            },
            {
                "id": "llm_node",
                "prompt": "llm prompt",
                "depends_on": [],
                "node_type": "llm",
                "models": [{"name": "nvidia", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            },
            {
                "id": "default_node",
                "prompt": "default prompt",
                "depends_on": [],
                "models": [{"name": "nvidia", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            }
        ]
    }
    "#;
    serde_json::from_str(json).expect("valid mixed dag fixture")
}

// ============================================================================
// TypeDispatchWorker tests
//
// T1-T5 from the TDD contract in the spec.
// ============================================================================

/// T1: Shell nodes are routed to RealShellWorker (verifiable by output).
#[tokio::test]
async fn test_type_dispatch_routes_shell_to_shellworker() {
    let dag = single_typed_node_dag("test", "echo routed_to_shell", Some("shell"));
    let worker = TypeDispatchWorker::new(&dag, Duration::from_secs(5));

    let result = worker
        .run("test", "echo routed_to_shell", "nvidia", 1)
        .await
        .expect("worker must return WorkerOutput, not Err");

    // RealShellWorker executes bash -c, so "echo routed_to_shell" produces output.
    assert!(result.success, "shell node should succeed: {:?}", result);
    assert!(
        result.output.contains("routed_to_shell"),
        "output should contain echo result: {:?}",
        result.output
    );
}

/// T2: LLM nodes are routed to PiPrintWorker.
/// We use with_pi_command to shim the pi binary with a shell command that
/// proves we went through PiPrintWorker (which adds -p --output-format json).
#[tokio::test]
async fn test_type_dispatch_routes_llm_to_piworker() {
    let dag = single_typed_node_dag("test", "llm prompt", Some("llm"));
    // Override pi binary with a shim that echoes a marker proving we're in
    // PiPrintWorker's code path (which invokes the program with -p --output-format json).
    let worker = TypeDispatchWorker::with_pi_command(
        &dag,
        Duration::from_secs(5),
        "sh".to_string(),
        vec![
            "-c".to_string(),
            // Echo a marker; PiPrintWorker expects plain text or JSON envelope.
            "echo piworker_marker".to_string(),
        ],
    );

    let result = worker
        .run("test", "test prompt", "nvidia", 1)
        .await
        .expect("worker must return WorkerOutput, not Err");

    // The shim ran through PiPrintWorker's command (not bash -c from RealShellWorker).
    assert!(result.success, "llm node should succeed: {:?}", result);
    assert!(
        result.output.contains("piworker_marker"),
        "output should contain piworker marker: {:?}",
        result.output
    );
}

/// T3: Nodes with node_type=None are routed to PiPrintWorker (default).
#[tokio::test]
async fn test_type_dispatch_routes_none_to_piworker() {
    let dag = single_typed_node_dag("test", "default prompt", None);
    let worker = TypeDispatchWorker::with_pi_command(
        &dag,
        Duration::from_secs(5),
        "sh".to_string(),
        vec!["-c".to_string(), "echo piworker_default".to_string()],
    );

    let result = worker
        .run("test", "test prompt", "nvidia", 1)
        .await
        .expect("worker must return WorkerOutput, not Err");

    assert!(result.success, "default node should succeed: {:?}", result);
    assert!(
        result.output.contains("piworker_default"),
        "output should contain piworker marker: {:?}",
        result.output
    );
}

/// T4: Shell nodes actually execute bash commands (end-to-end).
#[tokio::test]
async fn test_shell_node_executes_bash_command() {
    let dag = single_typed_node_dag("test", "echo hello", Some("shell"));
    let worker = TypeDispatchWorker::new(&dag, Duration::from_secs(5));

    let result = worker
        .run("test", "echo hello", "nvidia", 1)
        .await
        .expect("worker must return WorkerOutput, not Err");

    assert!(result.success, "shell echo should succeed: {:?}", result);
    assert!(
        result.output.contains("hello"),
        "output should contain 'hello': {:?}",
        result.output
    );
}

/// T5: Mixed DAG routes each node to the correct worker based on node_type.
#[tokio::test]
async fn test_mixed_dag_routes_correctly() {
    let dag = mixed_dag();
    // For the piworker override, we use a shim that outputs a unique marker.
    let worker = TypeDispatchWorker::with_pi_command(
        &dag,
        Duration::from_secs(5),
        "sh".to_string(),
        vec!["-c".to_string(), "echo llm_via_piworker".to_string()],
    );

    // Shell node: should execute bash -c "echo shell" -> contains "shell"
    let shell_result = worker
        .run("shell_node", "echo routed_to_shell", "nvidia", 1)
        .await
        .expect("shell_node must return WorkerOutput");
    assert!(
        shell_result.success,
        "shell_node should succeed: {:?}",
        shell_result
    );
    assert!(
        shell_result.output.contains("shell"),
        "shell_node output should contain 'shell': {:?}",
        shell_result.output
    );

    // LLM node: should go through PiPrintWorker shim -> contains "llm_via_piworker"
    let llm_result = worker
        .run("llm_node", "test prompt", "nvidia", 1)
        .await
        .expect("llm_node must return WorkerOutput");
    assert!(
        llm_result.success,
        "llm_node should succeed: {:?}",
        llm_result
    );
    assert!(
        llm_result.output.contains("llm_via_piworker"),
        "llm_node output should contain piworker marker: {:?}",
        llm_result.output
    );

    // Default node (node_type=None): should also go through PiPrintWorker shim
    let default_result = worker
        .run("default_node", "test prompt", "nvidia", 1)
        .await
        .expect("default_node must return WorkerOutput");
    assert!(
        default_result.success,
        "default_node should succeed: {:?}",
        default_result
    );
    assert!(
        default_result.output.contains("llm_via_piworker"),
        "default_node output should contain piworker marker: {:?}",
        default_result.output
    );
}

/// Additional test: unknown node_type values fall back to PiPrintWorker.
#[tokio::test]
async fn test_unknown_node_type_falls_back_to_piworker() {
    let dag = single_typed_node_dag("test", "unknown type prompt", Some("custom_type"));
    let worker = TypeDispatchWorker::with_pi_command(
        &dag,
        Duration::from_secs(5),
        "sh".to_string(),
        vec!["-c".to_string(), "echo fallback_marker".to_string()],
    );

    let result = worker
        .run("test", "test prompt", "nvidia", 1)
        .await
        .expect("worker must return WorkerOutput, not Err");

    assert!(
        result.success,
        "unknown type should succeed via fallback: {:?}",
        result
    );
    assert!(
        result.output.contains("fallback_marker"),
        "unknown type should fall back to piworker: {:?}",
        result.output
    );
}

/// Test shell node with empty models array dispatched via TypeDispatchWorker.
/// This verifies the dispatch fix works end-to-end with TypeDispatchWorker.
#[tokio::test]
async fn test_shell_node_empty_models_via_type_dispatch() {
    let json = r#"
    {
        "nodes": [
            {
                "id": "shell-test",
                "prompt": "echo 'Hello from shell node' && exit 0",
                "depends_on": [],
                "models": [],
                "retry": {"attempts": 1, "backoff_ms": 0},
                "node_type": "shell"
            }
        ]
    }
    "#;
    let dag: Dag = serde_json::from_str(json).expect("valid shell node dag");
    let worker = TypeDispatchWorker::new(&dag, Duration::from_secs(5));

    let result = worker
        .run(
            "shell-test",
            "echo 'Hello from shell node' && exit 0",
            "",
            1,
        )
        .await
        .expect("worker must return WorkerOutput");

    assert!(
        result.success,
        "shell node with empty models should succeed: {:?}",
        result
    );
    assert!(
        result.output.contains("Hello from shell node"),
        "shell output should contain expected message: {:?}",
        result.output
    );
}

// ============================================================================
// spec-15: session-backed worker (--session / -c flags)
// ============================================================================

/// S1: implement-iter1 (first iteration) gets `--session <path>` but NOT `-c`.
#[tokio::test]
async fn test_spec15_iter1_gets_session_no_continue() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dag = single_typed_node_dag("implement-iter1", "do work", None);
    let args_file = std::env::temp_dir().join("spec15_s1_args.txt");
    let _ = std::fs::remove_file(&args_file);
    let shim = format!("echo \"$@\" > {}", args_file.display());
    let worker = TypeDispatchWorker::with_pi_command(
        &dag,
        Duration::from_secs(5),
        "sh".to_string(),
        vec!["-c".to_string(), shim],
    );
    let _ = worker
        .run("implement-iter1", "test prompt", "model1", 1)
        .await;
    let args = std::fs::read_to_string(&args_file).unwrap_or_default();
    let _ = std::fs::remove_file(&args_file);
    assert!(
        args.contains("--session"),
        "iter1 must have --session: {args}"
    );
    assert!(
        !args.contains(" -c "),
        "iter1 must NOT have -c (continue): {args}"
    );
}

/// S2: implement-iter2 (fix iteration) gets both `--session <path>` AND `-c`.
#[tokio::test]
async fn test_spec15_iter2_gets_session_and_continue() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dag = single_typed_node_dag("implement-iter2", "fix it", None);
    let args_file = std::env::temp_dir().join("spec15_s2_args.txt");
    let _ = std::fs::remove_file(&args_file);
    let shim = format!("echo \"$@\" > {}", args_file.display());
    let worker = TypeDispatchWorker::with_pi_command(
        &dag,
        Duration::from_secs(5),
        "sh".to_string(),
        vec!["-c".to_string(), shim],
    );
    let _ = worker
        .run("implement-iter2", "test prompt", "model1", 1)
        .await;
    let args = std::fs::read_to_string(&args_file).unwrap_or_default();
    let _ = std::fs::remove_file(&args_file);
    assert!(
        args.contains("--session"),
        "iter2 must have --session: {args}"
    );
    assert!(
        args.contains(" -c "),
        "iter2 must have -c (continue): {args}"
    );
}

/// S3: PI_NO_SESSION=1 disables session flags entirely.
#[tokio::test]
async fn test_spec15_no_session_env_disables() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("PI_NO_SESSION", "1");
    }
    unsafe {
        std::env::set_var("PI_NO_SESSION", "1");
    }
    let dag = single_typed_node_dag("implement-iter1", "do work", None);
    let args_file = std::env::temp_dir().join("spec15_s3_args.txt");
    let _ = std::fs::remove_file(&args_file);
    let shim = format!("echo \"$@\" > {}", args_file.display());
    let worker = TypeDispatchWorker::with_pi_command(
        &dag,
        Duration::from_secs(5),
        "sh".to_string(),
        vec!["-c".to_string(), shim],
    );
    let _ = worker
        .run("implement-iter1", "test prompt", "model1", 1)
        .await;
    let args = std::fs::read_to_string(&args_file).unwrap_or_default();
    let _ = std::fs::remove_file(&args_file);
    unsafe {
        std::env::remove_var("PI_NO_SESSION");
    }
    assert!(
        !args.contains("--session"),
        "PI_NO_SESSION must suppress --session: {args}"
    );
    assert!(
        !args.contains(" -c "),
        "PI_NO_SESSION must suppress -c: {args}"
    );
}
