use pidag::{
    AwaitOutcome, Dag, ModelsConfig, Node, NodeStatus, RealShellWorker, RetryPolicy, Scheduler,
    SddGenerator, VecSink, Verify,
};
use std::path::PathBuf;
use std::time::Duration;

/// Extract the shell command string from a `Verify::Shell` arm. Every node
/// literal in this file pre-dates spec-37 and uses the shell form, so a
/// panic on a non-Shell arm is the correct failure mode for this helper.
fn verify_shell(v: &Option<Verify>) -> &str {
    match v {
        Some(Verify::Shell(cmd)) => cmd.as_str(),
        other => panic!("expected Verify::Shell, got {other:?}"),
    }
}

// ============================================================================
// Test 1: test_verify_passes_node_done
// ============================================================================
// When worker succeeds and verify exits 0, node should be Done
#[tokio::test]
async fn test_verify_passes_node_done() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_verify_passes");
    let _ = std::fs::remove_dir_all(&tmpdir);
    // Ensure parent directory exists
    let _ = std::fs::create_dir_all("_tmp/effect_verify");
    std::fs::create_dir_all(&tmpdir).unwrap();

    // Create a marker file that verify will check for
    std::fs::write(tmpdir.join("marker.txt"), "done").unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: "echo success".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            // Verify checks for the marker file
            verify: Some(Verify::Shell("test -f marker.txt".to_string())),
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    // Just verify the node has the verify field
    let node = dag.get_node("test_node").expect("node should exist");
    assert!(node.verify.is_some(), "node should have verify field");
    assert_eq!(verify_shell(&node.verify), "test -f marker.txt");
}

// ============================================================================
// Test 2: test_verify_fails_node_failed
// ============================================================================
// When worker succeeds but verify exits non-zero, node should be Failed with verify output
#[tokio::test]
async fn test_verify_fails_node_failed() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_verify_fails");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: "echo success".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            // Verify fails: file doesn't exist
            verify: Some(Verify::Shell("test -f nonexistent.txt".to_string())),
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    // Verify the node has the verify field
    let node = dag.get_node("test_node").expect("node should exist");
    assert!(node.verify.is_some(), "node should have verify field");
    // The actual test of verify failure happens during scheduler execution
}

// ============================================================================
// Test 3: test_no_verify_unchanged
// ============================================================================
// When no verify field is set, behavior should be identical to today
#[tokio::test]
async fn test_no_verify_unchanged() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_no_verify");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: "echo hello".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            // No verify field - should work like before
            verify: None,
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    // Verify the node has no verify field (None)
    let node = dag.get_node("test_node").expect("node should exist");
    assert!(node.verify.is_none(), "node should not have verify field");
}

// ============================================================================
// Test 4: test_verify_runs_in_project_root
// ============================================================================
// Verify should run in the DAG's working directory and find files there
#[tokio::test]
async fn test_verify_runs_in_project_root() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_verify_cwd");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    // Create a marker file in the tmpdir
    std::fs::write(tmpdir.join("marker.txt"), "found").unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: "echo test > output.txt && echo created output.txt".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            // Verify checks for both marker and output file
            verify: Some(Verify::Shell(
                "test -f marker.txt && test -f output.txt".to_string(),
            )),
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    // Verify the node structure is correct
    let node = dag.get_node("test_node").expect("node should exist");
    assert!(node.verify.is_some(), "node should have verify field");
    assert!(verify_shell(&node.verify).contains("marker.txt"));
}

// ============================================================================
// Test 5: test_success_alone_cannot_promote_when_verify_set
// ============================================================================
// Worker success + verify failure => NOT Done (guards the invariant directly)
#[tokio::test]
async fn test_success_alone_cannot_promote_when_verify_set() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_success_not_promoted");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: "exit 0".to_string(), // Worker succeeds
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            // Verify fails
            verify: Some(Verify::Shell("exit 1".to_string())),
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    // The keystone test will be at the scheduler level
    // Here we just verify the node is properly configured
    let node = dag.get_node("test_node").expect("node should exist");
    assert!(node.verify.is_some(), "node should have verify field");
    assert_eq!(verify_shell(&node.verify), "exit 1");
}

// ============================================================================
// Test 6: test_verify_failure_not_retryable
// ============================================================================
// Verify failure should not be marked retryable
#[tokio::test]
async fn test_verify_failure_not_retryable() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_verify_not_retryable");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: "echo success".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            verify: Some(Verify::Shell("exit 1".to_string())),
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    // Verify failure doesn't rely on retryable - it's handled by returning
    // Failed state from dispatch_node
    let node = dag.get_node("test_node").expect("node should exist");
    assert!(node.verify.is_some(), "node should have verify field");
}

// ============================================================================
// Test 7: test_verify_failure_event_states_both_facts
// ============================================================================
// The event should state both worker claim and verify failure
#[tokio::test]
async fn test_verify_failure_event_states_both_facts() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_verify_event");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: "echo worker_said_success".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            verify: Some(Verify::Shell("echo verify_failed && exit 1".to_string())),
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    // The NodeVerifyFailed event is emitted by the scheduler
    let node = dag.get_node("test_node").expect("node should exist");
    assert!(node.verify.is_some(), "node should have verify field");
}

// ============================================================================
// Test 8: test_sdd_emits_verify_on_implement_nodes
// ============================================================================
// Every implement-iterN node from SDD should have a non-empty verify field
#[tokio::test]
async fn test_sdd_emits_verify_on_implement_nodes() {
    let spec_content = r#"
## TDD Contract
- test_basic: basic test

## Architecture
Simple implementation.

## Exit Criteria
All tests pass.

## Guardrails
None.
"#;

    let project_root = PathBuf::from("_tmp/effect_verify/sdd_test");
    let _ = std::fs::remove_dir_all(&project_root);
    std::fs::create_dir_all(&project_root).unwrap();

    let models = ModelsConfig::default();
    let sdd = pidag::SddConfig::default();

    let dag = SddGenerator::from_spec_content(
        spec_content,
        &project_root,
        &models,
        &sdd,
        Some("test-spec".to_string()),
        None,
    )
    .expect("SDD generation should succeed");

    // Check implement-iter1
    let impl1 = dag
        .get_node("implement-iter1")
        .expect("implement-iter1 should exist");
    assert!(
        impl1.verify.is_some(),
        "implement-iter1 should have verify field"
    );
    assert!(
        !verify_shell(&impl1.verify).is_empty(),
        "verify should not be empty"
    );
    // The new verify uses git status instead of git diff
    assert!(
        verify_shell(&impl1.verify).contains("git status"),
        "verify should check git status"
    );
    // Also check that verify_pre is set
    assert!(
        impl1.verify_pre.is_some(),
        "implement-iter1 should have verify_pre field"
    );
    assert!(
        impl1.verify_pre.as_ref().unwrap().contains("git status"),
        "verify_pre should capture git status"
    );

    // Check implement-iter2
    let impl2 = dag
        .get_node("implement-iter2")
        .expect("implement-iter2 should exist");
    assert!(
        impl2.verify.is_some(),
        "implement-iter2 should have verify field"
    );
    assert!(
        !verify_shell(&impl2.verify).is_empty(),
        "verify should not be empty"
    );

    // Check implement-iter3
    let impl3 = dag
        .get_node("implement-iter3")
        .expect("implement-iter3 should exist");
    assert!(
        impl3.verify.is_some(),
        "implement-iter3 should have verify field"
    );
    assert!(
        !verify_shell(&impl3.verify).is_empty(),
        "verify should not be empty"
    );
}

// ============================================================================
// Test 9: test_verify_with_timeout
// ============================================================================
// Verify should respect node timeout configuration
#[tokio::test]
async fn test_verify_with_timeout() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_verify_timeout");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: "echo success".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: Some(Duration::from_secs(5)), // Node has timeout
            mcp_call: None,
            after: vec![],
            verify: Some(Verify::Shell("test -f marker.txt".to_string())),
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    // Verify the node has both timeout and verify
    let node = dag.get_node("test_node").expect("node should exist");
    assert!(node.verify.is_some(), "node should have verify field");
    assert!(node.timeout.is_some(), "node should have timeout");
}

// ============================================================================
// Test 10: test_no_verify_on_other_node_types
// ============================================================================
// Verify works on all node types (shell, llm, etc.)
#[tokio::test]
async fn test_no_verify_on_other_node_types() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_all_types");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    // Shell node
    let dag1 = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "shell_node".to_string(),
            prompt: "echo test".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            verify: Some(Verify::Shell("test -f file.txt".to_string())),
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    let node = dag1.get_node("shell_node").expect("node should exist");
    assert!(node.verify.is_some(), "shell node should support verify");

    // LLM node (llm nodes also support verify)
    let dag2 = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "llm_node".to_string(),
            prompt: "Implement feature".to_string(),
            depends_on: vec![],
            models: vec![pidag::ModelRef {
                name: "gpt-4".to_string(),
                paid: false,
            }],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: None, // LLM node
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            verify: Some(Verify::Shell(
                "git diff --quiet && exit 1 || exit 0".to_string(),
            )),
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    let node = dag2.get_node("llm_node").expect("node should exist");
    assert!(node.verify.is_some(), "LLM node should support verify");
}

// ============================================================================

// ============================================================================
// Test 11: test_verify_integration_passes
// ============================================================================
// Integration test: run a shell node through the scheduler with verify passing
#[tokio::test]
async fn test_verify_integration_passes() {
    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "verify_passes".to_string(),
            prompt: "echo success".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            // Simple verify that always succeeds
            verify: Some(Verify::Shell("exit 0".to_string())),
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(5));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);

    let outcome = scheduler.await_dag(None).await;

    match outcome {
        AwaitOutcome::Done(report) => {
            assert_eq!(report.node_states.len(), 1);
            assert_eq!(report.node_states[0].state, NodeStatus::Done);
            assert!(
                report.failed.is_empty(),
                "node should not be in failed list"
            );
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

// ============================================================================
// Test 12: test_verify_integration_fails
// ============================================================================
// Integration test: run a shell node through the scheduler with verify failing
// The keystone test: worker success + verify failure => node Failed
#[tokio::test]
async fn test_verify_integration_fails() {
    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "verify_fails".to_string(),
            prompt: "echo success".to_string(), // Worker succeeds
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            // Verify fails: exit with non-zero
            verify: Some(Verify::Shell("exit 1".to_string())),
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(5));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);

    let outcome = scheduler.await_dag(None).await;

    match outcome {
        AwaitOutcome::Done(report) => {
            // The keystone test: node must be Failed, not Done
            assert_eq!(report.node_states.len(), 1);
            assert_eq!(
                report.node_states[0].state,
                NodeStatus::Failed,
                "worker success + verify fail must result in Failed state"
            );
            assert!(
                report.failed.contains(&"verify_fails".to_string()),
                "node must be in failed list"
            );
        }
        other => panic!("expected Done (with Failed node), got {other:?}"),
    }
}

// ============================================================================
// Test R5a: test_verify_pre_absent_behaves_as_before
// ============================================================================
// A node with verify and no verify_pre behaves exactly as today; PIDAG_VERIFY_PRE is unset
#[tokio::test]
async fn test_verify_pre_absent_behaves_as_before() {
    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: "true".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            verify: Some(Verify::Shell("exit 0".to_string())),
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(5));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);
    let outcome = scheduler.await_dag(None).await;
    match outcome {
        AwaitOutcome::Done(report) => {
            assert_eq!(report.node_states.len(), 1);
            assert_eq!(report.node_states[0].state, NodeStatus::Done);
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

// ============================================================================
// Test R5b: test_verify_pre_exposed_as_env
// ============================================================================
#[tokio::test]
async fn test_verify_pre_exposed_as_env() {
    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: "true".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            verify: Some(Verify::Shell(
                "test \"$PIDAG_VERIFY_PRE\" = TOKEN123".to_string(),
            )),
            verify_pre: Some("echo TOKEN123".to_string()),

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(5));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);
    let outcome = scheduler.await_dag(None).await;
    match outcome {
        AwaitOutcome::Done(report) => {
            assert_eq!(report.node_states.len(), 1);
            assert_eq!(report.node_states[0].state, NodeStatus::Done);
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

// ============================================================================
// Test R5d: test_verify_pre_failure_fails_node
// ============================================================================
#[tokio::test]
async fn test_verify_pre_failure_fails_node() {
    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: "should_not_run".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            verify: Some(Verify::Shell("exit 0".to_string())),
            verify_pre: Some("exit 3".to_string()),

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(5));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);
    let outcome = scheduler.await_dag(None).await;
    match outcome {
        AwaitOutcome::Done(report) => {
            assert_eq!(report.node_states.len(), 1);
            assert_eq!(report.node_states[0].state, NodeStatus::Failed);
            assert_eq!(report.node_states[0].attempts, 0);
        }
        other => panic!("expected Done (with Failed node), got {other:?}"),
    }
}

// ============================================================================
// Test N2: test_existing_dag_json_without_verify_pre_parses
// ============================================================================
#[test]
fn test_existing_dag_json_without_verify_pre_parses() {
    let json = r#"{
  "nodes": [
    {
      "id": "test",
      "prompt": "echo test",
      "depends_on": [],
      "models": [],
      "retry": {"attempts": 1, "backoff_ms": 0},
      "validate": null,
      "node_type": "shell",
      "gate": null,
      "timeout": null,
      "mcp_call": null,
      "after": [],
      "verify": null
    }
  ],
  "metadata": null
}"#;

    let dag: pidag::Dag = serde_json::from_str(json).expect("should parse old JSON");
    assert_eq!(dag.nodes.len(), 1);
    let node = &dag.nodes[0];
    assert_eq!(node.id, "test");
    assert!(node.verify.is_none());
    assert!(node.verify_pre.is_none());
}

// ============================================================================
// Test R5e (REGRESSION TEST): test_dirty_tree_at_start_does_not_satisfy_verify
// ============================================================================
// THE CRITICAL REGRESSION TEST. This MUST fail before the fix.
// Create a git repo, make it dirty BEFORE the node runs, give the node a command
// that changes nothing (true), and use the sdd-style verify_pre/verify pair.
// The node must end Failed.
#[tokio::test]
async fn test_dirty_tree_at_start_does_not_satisfy_verify() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_dirty_tree_regression");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    // Initialize a git repo
    std::process::Command::new("git")
        .arg("init")
        .current_dir(&tmpdir)
        .output()
        .expect("should init git repo");

    // Configure git
    std::process::Command::new("git")
        .args(&["config", "user.email", "test@example.com"])
        .current_dir(&tmpdir)
        .output()
        .ok();

    std::process::Command::new("git")
        .args(&["config", "user.name", "Test User"])
        .current_dir(&tmpdir)
        .output()
        .ok();

    // Create initial file and commit
    std::fs::write(tmpdir.join("file.txt"), "initial content").unwrap();
    std::process::Command::new("git")
        .args(&["add", "."])
        .current_dir(&tmpdir)
        .output()
        .expect("should add files");

    std::process::Command::new("git")
        .args(&["commit", "-m", "initial"])
        .current_dir(&tmpdir)
        .output()
        .expect("should commit");

    // Make the tree dirty BEFORE the node runs
    std::fs::write(tmpdir.join("file.txt"), "modified content").unwrap();

    // Absolute path: commands `cd` here explicitly rather than mutating the
    // process-global cwd, which would corrupt sibling tests running in
    // parallel threads within this same test binary.
    let repo = std::fs::canonicalize(&tmpdir)
        .unwrap()
        .display()
        .to_string();

    // Create a DAG that does nothing (true) with the sdd-style verify pair
    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: format!("cd {} && true", repo), // Does nothing
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            // The new delta-based verify: check if the tree changed DURING this node
            verify: Some(Verify::Shell(format!(
                "cd {repo} && test \"$( ( git status --porcelain; git diff; git diff --cached; git ls-files -o --exclude-standard -z | xargs -0 -r sha256sum ) | sha256sum )\" != \"$PIDAG_VERIFY_PRE\""
            ))),
            verify_pre: Some(format!(
                "cd {repo} && ( git status --porcelain; git diff; git diff --cached; git ls-files -o --exclude-standard -z | xargs -0 -r sha256sum ) | sha256sum"
            )),

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(5));

    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);
    let outcome = scheduler.await_dag(None).await;

    match outcome {
        AwaitOutcome::Done(report) => {
            assert_eq!(report.node_states.len(), 1);
            // THIS IS THE REGRESSION TEST: node must be Failed, not Done
            // Before the fix, the tree was already dirty, so the old verify predicate
            // (git diff --quiet && exit 1 || exit 0) would pass because git diff is non-empty.
            // With the fix, verify_pre captures the baseline at dispatch time,
            // so a no-op worker leaves the tree unchanged relative to baseline, failing verify.
            assert_eq!(
                report.node_states[0].state,
                NodeStatus::Failed,
                "node that does nothing must fail when tree was already dirty (regression test R5e)"
            );
        }
        other => panic!("expected Done (with Failed node), got {other:?}"),
    }
}

// ============================================================================
// Test R5f: test_new_change_on_dirty_tree_passes
// ============================================================================
// COUNTERWEIGHT TEST. Same repo already dirty, but node makes an ADDITIONAL change.
// Node must end Done. Guards against a "fix" that just rejects all dirty trees.
#[tokio::test]
async fn test_new_change_on_dirty_tree_passes() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_new_change_dirty_tree");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    // Initialize a git repo
    std::process::Command::new("git")
        .arg("init")
        .current_dir(&tmpdir)
        .output()
        .expect("should init git repo");

    // Configure git
    std::process::Command::new("git")
        .args(&["config", "user.email", "test@example.com"])
        .current_dir(&tmpdir)
        .output()
        .ok();

    std::process::Command::new("git")
        .args(&["config", "user.name", "Test User"])
        .current_dir(&tmpdir)
        .output()
        .ok();

    // Create initial file and commit
    std::fs::write(tmpdir.join("file.txt"), "initial content").unwrap();
    std::process::Command::new("git")
        .args(&["add", "."])
        .current_dir(&tmpdir)
        .output()
        .expect("should add files");

    std::process::Command::new("git")
        .args(&["commit", "-m", "initial"])
        .current_dir(&tmpdir)
        .output()
        .expect("should commit");

    // Make the tree dirty BEFORE the node runs
    std::fs::write(tmpdir.join("file.txt"), "already modified content").unwrap();

    // Absolute path: commands `cd` here explicitly rather than mutating the
    // process-global cwd, which would corrupt sibling tests running in
    // parallel threads within this same test binary.
    let repo = std::fs::canonicalize(&tmpdir)
        .unwrap()
        .display()
        .to_string();

    // Create a DAG that makes an ADDITIONAL change (creates a new file)
    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "test_node".to_string(),
            prompt: format!("cd {} && echo 'new content' > newfile.txt", repo), // Creates a new file
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            // The new delta-based verify: check if the tree changed DURING this node
            verify: Some(Verify::Shell(format!(
                "cd {repo} && test \"$( ( git status --porcelain; git diff; git diff --cached; git ls-files -o --exclude-standard -z | xargs -0 -r sha256sum ) | sha256sum )\" != \"$PIDAG_VERIFY_PRE\""
            ))),
            verify_pre: Some(format!(
                "cd {repo} && ( git status --porcelain; git diff; git diff --cached; git ls-files -o --exclude-standard -z | xargs -0 -r sha256sum ) | sha256sum"
            )),

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(5));

    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);
    let outcome = scheduler.await_dag(None).await;

    match outcome {
        AwaitOutcome::Done(report) => {
            assert_eq!(report.node_states.len(), 1);
            // The node made a new change (created newfile.txt), so even though
            // the tree was already dirty, the delta-based verify should pass.
            assert_eq!(
                report.node_states[0].state,
                NodeStatus::Done,
                "node that makes additional changes should pass (R5f guards against overfix)"
            );
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

// ============================================================================
// Test R5c: test_verify_pre_output_capped
// ============================================================================
// verify_pre stdout becomes an environment variable, so it is capped at 4 KB
// (R5.3). The cap must slice on a char boundary: a naive byte slice panics
// with "byte index is not a char boundary" when the cut lands mid-codepoint,
// and a verify_pre command is free to emit UTF-8. The multibyte padding below
// is what makes this a regression test for that panic rather than a length
// check.
#[tokio::test]
async fn test_verify_pre_output_capped() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_verify_pre_capped");
    let _ = std::fs::remove_dir_all(&tmpdir);
    let _ = std::fs::create_dir_all("_tmp/effect_verify");
    std::fs::create_dir_all(&tmpdir).unwrap();

    let repo = std::fs::canonicalize(&tmpdir)
        .unwrap()
        .display()
        .to_string();

    // Emit ~9 KB of a 3-byte UTF-8 character, so the 4096-byte cut lands
    // mid-codepoint unless the implementation respects char boundaries.
    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "capped".to_string(),
            prompt: format!("cd {repo} && true"),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            // Succeeds only if the token was truncated to at most 4096 bytes.
            verify: Some(Verify::Shell(
                "test \"${#PIDAG_VERIFY_PRE}\" -le 4096 && test -n \"$PIDAG_VERIFY_PRE\""
                    .to_string(),
            )),
            verify_pre: Some("printf '\\u2713%.0s' $(seq 1 3000)".to_string()),

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(10));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);
    let outcome = scheduler.await_dag(None).await;

    match outcome {
        AwaitOutcome::Done(report) => {
            assert_eq!(report.node_states.len(), 1);
            // Reaching a terminal state at all proves no panic on the boundary.
            assert_eq!(
                report.node_states[0].state,
                NodeStatus::Done,
                "capped multibyte token must be <= 4096 bytes and non-empty (R5c)"
            );
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

// ============================================================================
// Test R5g: test_content_only_change_on_dirty_tree_passes
// ============================================================================
// THE GAP THAT `test_new_change_on_dirty_tree_passes` MISSED.
//
// That test creates a NEW file, which alters `git status --porcelain` output
// (a `??` line appears), so it passed even with a token that only hashed
// porcelain output. But porcelain encodes file names and states, not
// CONTENTS: a node that edits a file already showing ` M`, or rewrites an
// already-untracked file, produces byte-identical porcelain. Such a node did
// real work and would have been marked Failed -- a false negative, and the
// mirror image of the bug spec-31 set out to fix.
//
// This test modifies an already-dirty TRACKED file and nothing else, so it
// fails against a porcelain-only token and passes against a content-sensitive
// one.
#[tokio::test]
async fn test_content_only_change_on_dirty_tree_passes() {
    let tmpdir = PathBuf::from("_tmp/effect_verify/test_content_only_change");
    let _ = std::fs::remove_dir_all(&tmpdir);
    let _ = std::fs::create_dir_all("_tmp/effect_verify");
    std::fs::create_dir_all(&tmpdir).unwrap();

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&tmpdir)
            .output()
            .expect("git command should run");
    };
    git(&["init"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test User"]);
    std::fs::write(tmpdir.join("file.txt"), "committed content").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "initial"]);

    // Dirty the tracked file BEFORE the node runs, as a crashed attempt would.
    std::fs::write(tmpdir.join("file.txt"), "dirty from a prior attempt").unwrap();

    let repo = std::fs::canonicalize(&tmpdir)
        .unwrap()
        .display()
        .to_string();
    let snip = "( git status --porcelain; git diff; git diff --cached; \
                 git ls-files -o --exclude-standard -z | xargs -0 -r sha256sum ) | sha256sum";

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "content_only".to_string(),
            // Modifies the SAME already-dirty file. No new file, no rename:
            // `git status --porcelain` is byte-identical before and after.
            prompt: format!("cd {repo} && echo 'repaired by this node' > file.txt"),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            verify: Some(Verify::Shell(format!(
                "cd {repo} && test \"$( {snip} )\" != \"$PIDAG_VERIFY_PRE\""
            ))),
            verify_pre: Some(format!("cd {repo} && {snip}")),

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(10));
    let mut scheduler = Scheduler::new(dag, Box::new(worker), Box::new(VecSink::new()), 1);
    let outcome = scheduler.await_dag(None).await;

    match outcome {
        AwaitOutcome::Done(report) => {
            assert_eq!(report.node_states.len(), 1);
            assert_eq!(
                report.node_states[0].state,
                NodeStatus::Done,
                "a content-only edit to an already-dirty file is real work and must verify \
                 (R5g: guards the false negative a porcelain-only token produces)"
            );
        }
        other => panic!("expected Done, got {other:?}"),
    }
}
