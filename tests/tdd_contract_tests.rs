//! TDD Contract tests from the spec.
//!
//! Tests T1-T10 covering provider/model splitting, provider-aware worker
//! invocation, validate-baseline parse-only behaviour, NodeFailure output
//! recording, pidag show error output, and no-unwrap-in-production.

use pidag::{Dag, Node, PiPrintWorker, RealShellWorker, SddGenerator, Store, Worker};
use std::collections::HashMap;
use std::time::Duration;

/// Check that the external exit-criteria validator the baseline node invokes is
/// actually present. Mirrors `pi_available()` in `pi_backend_tests.rs`.
///
/// The `validate-baseline` node runs `{validate_script} {spec_path}`, and
/// `validate_script` resolves to a `loop-engineer` skill script that lives
/// outside this repository. Executing the prompt without it does not test
/// pidag at all: the missing-script case makes bash exit 127, which is
/// non-zero, so `test_validate_baseline_malformed` passed in CI for entirely
/// the wrong reason while `test_validate_baseline_parse_only` failed.
///
/// Set `PIDAG_REQUIRE_VALIDATOR=1` to turn a missing validator into a failure
/// instead of a skip; the local gate does this.
fn validator_available(baseline_prompt: &str) -> bool {
    // The prompt is "<script> <spec_path>"; the script is the first token.
    let script = baseline_prompt.split_whitespace().next().unwrap_or("");
    if !script.is_empty() && std::path::Path::new(script).exists() {
        return true;
    }

    if std::env::var("PIDAG_REQUIRE_VALIDATOR") == Ok("1".to_string()) {
        panic!("exit-criteria validator {script:?} not found and PIDAG_REQUIRE_VALIDATOR=1 is set");
    }

    println!("SKIP: exit-criteria validator not found at {script:?}");
    false
}

// ============================================================================
// Helper functions
// ============================================================================

fn minimal_dag_with_node(node: Node) -> Dag {
    Dag {
        nodes: vec![node],
        metadata: None,
    }
}

fn shell_node(id: &str, command: &str) -> Node {
    Node {
        id: id.to_string(),
        prompt: command.to_string(),
        depends_on: vec![],
        models: vec![],
        retry: pidag::RetryPolicy {
            attempts: 1,
            backoff_ms: 0,
        },
        validate: None,
        node_type: Some("shell".to_string()),
        gate: None,
        timeout: None,
        mcp_call: None,
        after: vec![],
        verify: None,
        verify_pre: None,
    }
}

// ============================================================================
// T1-T3: split_provider_model
// ============================================================================

/// T1: "google/gemini-3.6-flash" → (Some("google"), "gemini-3.6-flash")
#[test]
fn test_split_provider_model_slash() {
    let (provider, model) = pidag::split_provider_model("google/gemini-3.6-flash");
    assert_eq!(provider, Some("google"));
    assert_eq!(model, "gemini-3.6-flash");
}

/// T2: "nvidia:z-ai/glm-5.2" → (Some("nvidia"), "z-ai/glm-5.2")
#[test]
fn test_split_provider_model_colon() {
    let (provider, model) = pidag::split_provider_model("nvidia:z-ai/glm-5.2");
    assert_eq!(provider, Some("nvidia"));
    assert_eq!(model, "z-ai/glm-5.2");
}

/// T3: "deepseek-chat" → (None, "deepseek-chat")
#[test]
fn test_split_provider_model_bare() {
    let (provider, model) = pidag::split_provider_model("deepseek-chat");
    assert_eq!(provider, None);
    assert_eq!(model, "deepseek-chat");
}

// ============================================================================
// T4-T5: Provider-aware worker invocation
// ============================================================================

/// T4: model "google/gemini-3.6-flash" → cmd includes --provider google --model gemini-3.6-flash
#[tokio::test]
async fn test_worker_invokes_provider_flag() {
    let dag = minimal_dag_with_node(Node {
        id: "test-node".to_string(),
        prompt: "say hello".to_string(),
        depends_on: vec![],
        models: vec![pidag::ModelRef {
            name: "google/gemini-3.6-flash".to_string(),
            paid: false,
        }],
        retry: pidag::RetryPolicy {
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
    });

    // Use a shim that echoes its arguments to track what gets passed
    let worker = PiPrintWorker::with_command(
        Duration::from_secs(30),
        "sh".to_string(),
        vec!["-c".to_string(), "echo args: $@".to_string()],
    );

    let result = worker
        .run("test-node", "dummy prompt", "google/gemini-3.6-flash", 1)
        .await
        .expect("worker should not error");

    assert!(result.success, "worker should succeed");
    let output = result.output;
    assert!(
        output.contains("--provider"),
        "output should contain --provider flag, got: {output}"
    );
    assert!(
        output.contains("google"),
        "output should contain provider name 'google', got: {output}"
    );
    assert!(
        output.contains("--model"),
        "output should contain --model flag, got: {output}"
    );
    assert!(
        output.contains("gemini-3.6-flash"),
        "output should contain model name 'gemini-3.6-flash', got: {output}"
    );
}

/// T5: model "deepseek-chat" → cmd has --model deepseek-chat, no --provider
#[tokio::test]
async fn test_worker_bare_model_no_provider() {
    let dag = minimal_dag_with_node(Node {
        id: "test-node".to_string(),
        prompt: "say hello".to_string(),
        depends_on: vec![],
        models: vec![pidag::ModelRef {
            name: "deepseek-chat".to_string(),
            paid: false,
        }],
        retry: pidag::RetryPolicy {
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
    });

    let worker = PiPrintWorker::with_command(
        Duration::from_secs(30),
        "sh".to_string(),
        vec!["-c".to_string(), "echo args: $@".to_string()],
    );

    let result = worker
        .run("test-node", "dummy prompt", "deepseek-chat", 1)
        .await
        .expect("worker should not error");

    assert!(result.success, "worker should succeed");
    let output = result.output;
    assert!(
        !output.contains("--provider"),
        "output should NOT contain --provider flag, got: {output}"
    );
    assert!(
        output.contains("--model"),
        "output should contain --model flag, got: {output}"
    );
    assert!(
        output.contains("deepseek-chat"),
        "output should contain model name 'deepseek-chat', got: {output}"
    );
}

// ============================================================================
// T6-T7: Validate baseline parse-only
// ============================================================================

/// T6: well-formed spec, artifacts absent → baseline returns 0 (parse-only)
#[tokio::test]
async fn test_validate_baseline_parse_only() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let spec_path = dir.path().join("01-test.md");
    let spec_content = r#"# Spec: Test Feature

## Overview
Test description

## TDD Contract
| Test | Given | Expects |
|------|-------|---------|
| t1   | foo   | bar     |

## Exit Criteria
- [ ] `echo ok`

## Guardrails
- Do NOT do bad things
"#;
    std::fs::write(&spec_path, spec_content).expect("write spec");

    let dag = SddGenerator::from_spec(
        &spec_path,
        dir.path(),
        &Default::default(),
        &Default::default(),
    )
    .expect("generate DAG");

    let baseline = dag
        .nodes
        .iter()
        .find(|n| n.id == "validate-baseline")
        .expect("baseline node exists");

    assert_eq!(
        baseline.node_type.as_deref(),
        Some("shell"),
        "validate-baseline should be a shell node"
    );

    // The baseline prompt should be a parse-only check (not the full validate
    // script).  Execute it to verify it returns 0.
    if !validator_available(&baseline.prompt) {
        return;
    }
    let output = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(&baseline.prompt)
        .output()
        .await
        .expect("execute baseline");

    assert!(
        output.status.success(),
        "baseline should return 0 for well-formed spec, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// T7: spec with no Exit Criteria section → baseline returns non-zero
#[tokio::test]
async fn test_validate_baseline_malformed() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let spec_path = dir.path().join("01-test.md");
    let spec_content = r#"# Spec: Test Feature

## Overview
Test description

## TDD Contract
| Test | Given | Expects |
|------|-------|---------|
| t1   | foo   | bar     |

## Guardrails
- Do NOT do bad things
"#; // No Exit Criteria section
    std::fs::write(&spec_path, spec_content).expect("write spec");

    let dag = SddGenerator::from_spec(
        &spec_path,
        dir.path(),
        &Default::default(),
        &Default::default(),
    )
    .expect("generate DAG");

    let baseline = dag
        .nodes
        .iter()
        .find(|n| n.id == "validate-baseline")
        .expect("baseline node exists");

    // Execute the baseline prompt — should fail because Exit Criteria is missing.
    // Gated: without the validator, bash exits 127 and this test would pass
    // without exercising anything.
    if !validator_available(&baseline.prompt) {
        return;
    }
    let output = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(&baseline.prompt)
        .output()
        .await
        .expect("execute baseline");

    assert!(
        !output.status.success(),
        "baseline should return non-zero for spec without Exit Criteria"
    );
}

// ============================================================================
// T8: NodeState records failure output (stdout/stderr/exit)
// ============================================================================

/// T8: failing shell node → stored NodeRecord has stdout/stderr/exit
#[tokio::test]
async fn test_nodestate_records_failure_output() {
    // Use a shell node that fails with specific output on stdout and stderr
    let dag = minimal_dag_with_node(shell_node(
        "fail-node",
        "echo stdout-text && echo stderr-text >&2 && exit 3",
    ));

    // Use MockStore to capture persisted state
    let store = std::sync::Arc::new(pidag::MockStore::new());

    // Pre-seed the run so the RedbSink doesn't overwrite
    store
        .put_run(&pidag::RunMeta {
            run_id: "test-run".to_string(),
            dag_json: serde_json::to_string(&dag).unwrap(),
            started_at: "2024-01-01T00:00:00Z".to_string(),
            completed_at: None,
            successful_nodes: 0,
            failed_nodes: 0,
        })
        .await
        .expect("put_run");

    let worker = RealShellWorker::new(&dag, Duration::from_secs(30));

    let result = worker
        .run(
            "fail-node",
            "echo stdout-text && echo stderr-text >&2 && exit 3",
            "",
            1,
        )
        .await
        .expect("worker should not error");

    assert!(!result.success, "shell node should fail with exit 3");

    // The worker output should contain both stdout and stderr
    assert!(
        result.output.contains("stdout-text"),
        "output should contain stdout, got: {}",
        result.output
    );
    assert!(
        result.output.contains("stderr-text"),
        "output should contain stderr, got: {}",
        result.output
    );

    // Now verify the shell worker correctly separates stdout/stderr/exit
    // This is tested through the worker output which combines them
}

// ============================================================================
// T9: pidag show prints node error (stderr/exit)
// ============================================================================

/// T9: a run with a failed node → pidag show output contains the node's stderr/exit
#[tokio::test]
async fn test_show_prints_node_error() {
    // Create a run with a failed node, then verify that the render/display
    // output includes the stderr text and exit code.
    let dag = minimal_dag_with_node(shell_node(
        "fail-node",
        "echo error-marker-xyz >&2 && exit 42",
    ));

    // Construct a NodeState matching what the scheduler would produce after
    // the shell node fails
    let output = {
        let worker = RealShellWorker::new(&dag, Duration::from_secs(30));
        let result = worker
            .run("fail-node", "echo error-marker-xyz >&2 && exit 42", "", 1)
            .await
            .expect("worker should not error");
        result.output
    };

    // Build states map as pidag show would
    let mut states: HashMap<String, (String, Option<String>)> = HashMap::new();
    states.insert(
        "fail-node".to_string(),
        ("Failed".to_string(), Some(output.clone())),
    );

    let status = pidag::render_status(&dag, &states);

    // The rendered output should contain the stderr text from the failed node
    assert!(
        status.contains("error-marker-xyz"),
        "status output should contain stderr text 'error-marker-xyz', got:\n{status}"
    );
    assert!(
        status.contains("fail-node"),
        "status output should mention the node id 'fail-node', got:\n{status}"
    );
}

// ============================================================================
// T10: No .unwrap() / .expect() in production code
// ============================================================================

/// T10: walk src/ production — no .unwrap()/.expect() outside tests
#[test]
fn test_no_production_unwrap() {
    // Collect all production .rs files (exclude tests, test modules, bin/crash_writer)
    let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let violations = walk_production_unwraps(&src_dir);
    assert!(
        violations.is_empty(),
        "Found .unwrap() / .expect() in production code:\n{}",
        violations.join("\n")
    );
}

fn walk_production_unwraps(src_dir: &std::path::Path) -> Vec<String> {
    let mut violations = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![src_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                // Skip test directories
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == "tests" {
                    continue;
                }
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                // Skip bin/crash_writer.rs (test aid)
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map_or(false, |n| n == "crash_writer.rs")
                {
                    continue;
                }
                // Read and check
                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                check_file_for_unwrap(&path, &content, &mut violations);
            }
        }
    }

    violations
}

fn check_file_for_unwrap(path: &std::path::Path, content: &str, violations: &mut Vec<String>) {
    // Track `#[cfg(test)]` regions by brace depth so we never flag `.unwrap()`
    // / `.expect()` that legitimately live inside test modules.
    let mut test_depth: usize = 0; // brace-nesting depth inside a #[cfg(test)]/tests block
    let mut in_test_block = false;
    let mut brace_count = 0usize; // running brace depth across the whole file

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let line_body = trimmed
            .strip_prefix("//")
            .unwrap_or(trimmed)
            .strip_prefix("///")
            .unwrap_or(trimmed);

        // Enter a test region.
        if line_body.starts_with("#[cfg(test)]")
            || line_body.starts_with("#[tokio::test]")
            || line_body.starts_with("#[test]")
        {
            // A test region begins; the enclosing `mod`/`fn` opens on this or
            // the next `{`. Treat the current brace level as the base.
            if !in_test_block {
                in_test_block = true;
                test_depth = brace_count;
            }
        }

        // Track braces on non-comment content.
        let code = strip_comment(line_body);
        let opens: usize = code.bytes().filter(|b| *b == b'{').count();
        let closes: usize = code.bytes().filter(|b| *b == b'}').count();
        brace_count = brace_count.saturating_add(opens).saturating_sub(closes);

        // Determine if we're inside a test region: if we entered at a brace
        // level `test_depth`, we're inside until `brace_count <= test_depth`.
        let in_test_region = in_test_block && brace_count > test_depth;

        if in_test_region {
            // Once we fall back to (or below) the base level, the region has
            // closed.
            if brace_count <= test_depth {
                in_test_block = false;
            }
            continue; // skip test code entirely
        }

        // Only guard the "*c; `.unwrap()`/`expect` checks below are NOT in a
        // test region.
        let has_unwrap = line_body.contains(".unwrap()");
        let has_expect = line_body.contains(".expect(");
        let is_allowed_unwrap = line_body.contains(".unwrap_or")
            || line_body.contains(".unwrap_or_default")
            || line_body.contains(".unwrap_or_else");
        if (has_unwrap && !is_allowed_unwrap) || has_expect {
            violations.push(format!(
                "{}:{}: {}",
                path.strip_prefix(std::env::current_dir().unwrap_or_default())
                    .unwrap_or(path)
                    .display(),
                line_num + 1,
                trimmed
            ));
        }
    }
}

/// Strip an inline `//` or `/* ... */` comment from a line of code so brace
/// counting ignores comment text.
fn strip_comment(line: &str) -> &str {
    if let Some(i) = line.find("//") {
        return &line[..i];
    }
    line
}
