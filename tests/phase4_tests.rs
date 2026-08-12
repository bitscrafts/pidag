use pidag::{
    Config, Dag, ModelsConfig, Node, RealShellWorker, RetryPolicy, SddConfig, SddGenerator, Worker,
};
use std::path::PathBuf;
use std::time::Duration;

// ============================================================================
// Test 1: test_shell_worker_success
// ============================================================================
#[tokio::test]
async fn test_shell_worker_success() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_shell_success");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "echo_test".to_string(),
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
            verify: None,
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(5));
    let result = worker.run("echo_test", "echo hello", "", 1).await.unwrap();

    assert!(result.success);
    assert!(result.output.contains("hello"));
}

// ============================================================================
// Test 2: test_shell_worker_failure
// ============================================================================
#[tokio::test]
async fn test_shell_worker_failure() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_shell_failure");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "exit_fail".to_string(),
            prompt: "exit 1".to_string(),
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
            verify: None,
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(5));
    let result = worker.run("exit_fail", "exit 1", "", 1).await.unwrap();

    assert!(!result.success);
}

// ============================================================================
// Test 3: test_shell_worker_timeout
// ============================================================================
#[tokio::test]
async fn test_shell_worker_timeout() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_shell_timeout");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "sleep_test".to_string(),
            prompt: "sleep 10".to_string(),
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
            verify: None,
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_millis(100));
    let result = worker.run("sleep_test", "sleep 10", "", 1).await.unwrap();

    assert!(!result.success);
    assert!(result.output.contains("timed out") || result.output.is_empty());
}

// ============================================================================
// Test 4: test_shell_worker_captures_stderr
// ============================================================================
#[tokio::test]
async fn test_shell_worker_captures_stderr() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_shell_stderr");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "stderr_test".to_string(),
            prompt: "echo out; echo err >&2".to_string(),
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
            verify: None,
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(5));
    let result = worker
        .run("stderr_test", "echo out; echo err >&2", "", 1)
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("out"));
    assert!(result.output.contains("err"));
}

// ============================================================================
// Test 5: test_sdd_generator_parses_spec
// ============================================================================
#[tokio::test]
async fn test_sdd_generator_parses_spec() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_sdd_parse");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let spec_content = r#"
# Spec

## TDD Contract

| Test | Given | Expected |
|------|-------|----------|
| T1 | input | output |

## Architecture

This is the architecture section.

## Exit Criteria

- Criteria 1
- Criteria 2

## Guardrails

- Guard 1
- Guard 2
"#;

    let spec_path = tmpdir.join("spec.md");
    std::fs::write(&spec_path, spec_content).unwrap();

    let dag = SddGenerator::from_spec(
        &spec_path,
        &tmpdir,
        &ModelsConfig::default(),
        &SddConfig::default(),
    )
    .unwrap();

    assert_eq!(dag.nodes.len(), 10);
    assert_eq!(dag.nodes[0].id, "validate-baseline");
    assert_eq!(dag.nodes[1].id, "implement-iter1");
    assert_eq!(dag.nodes[2].id, "quality-gate-1");
    assert_eq!(dag.nodes[3].id, "validate-iter1");
}

// ============================================================================
// Test 6: test_sdd_generator_prompt_iter1
// ============================================================================
#[tokio::test]
async fn test_sdd_generator_prompt_iter1() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_sdd_prompt_iter1");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let spec_content = r#"
## TDD Contract

| Test | Given | Expected |
|------|-------|----------|
| T1 | input | output |

## Architecture

Build it this way.

## Exit Criteria

- Pass all tests

## Guardrails

- No unwrap in production
"#;

    let spec_path = tmpdir.join("spec.md");
    std::fs::write(&spec_path, spec_content).unwrap();

    let dag = SddGenerator::from_spec(
        &spec_path,
        &tmpdir,
        &ModelsConfig::default(),
        &SddConfig::default(),
    )
    .unwrap();

    let iter1 = &dag.nodes[1];
    assert_eq!(iter1.id, "implement-iter1");
    assert!(iter1.prompt.contains("TDD Contract"));
    assert!(iter1.prompt.contains("Architecture"));
    assert!(iter1.prompt.contains("Guardrails"));
}

// ============================================================================
// Test 7: test_sdd_generator_prompt_iter2_placeholder
// ============================================================================
#[tokio::test]
async fn test_sdd_generator_prompt_iter2_placeholder() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_sdd_prompt_iter2");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let spec_content = r#"
## TDD Contract

Test contract.

## Architecture

Architecture.

## Exit Criteria

Criteria.

## Guardrails

Guard.
"#;

    let spec_path = tmpdir.join("spec.md");
    std::fs::write(&spec_path, spec_content).unwrap();

    let dag = SddGenerator::from_spec(
        &spec_path,
        &tmpdir,
        &ModelsConfig::default(),
        &SddConfig::default(),
    )
    .unwrap();

    let iter2 = &dag.nodes[4]; // implement-iter2
    assert_eq!(iter2.id, "implement-iter2");
    assert!(iter2.prompt.contains("{{validate-iter1.output}}"));
}

// ============================================================================
// Test 8: test_sdd_generator_models_escalation
// ============================================================================
#[tokio::test]
async fn test_sdd_generator_models_escalation() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_sdd_models");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let spec_content = r#"
## TDD Contract
Test.

## Architecture
Arch.

## Exit Criteria
Criteria.

## Guardrails
Guard.
"#;

    let spec_path = tmpdir.join("spec.md");
    std::fs::write(&spec_path, spec_content).unwrap();

    let dag = SddGenerator::from_spec(
        &spec_path,
        &tmpdir,
        &ModelsConfig::default(),
        &SddConfig::default(),
    )
    .unwrap();

    // Iter1 and Iter2: only free model
    let iter1 = &dag.nodes[1];
    assert_eq!(iter1.models.len(), 1);
    assert!(!iter1.models[0].paid);

    let iter2 = &dag.nodes[4];
    assert_eq!(iter2.models.len(), 1);
    assert!(!iter2.models[0].paid);

    // Iter3: free + paid
    let iter3 = &dag.nodes[7];
    assert_eq!(iter3.models.len(), 2);
    assert!(!iter3.models[0].paid);
    assert!(iter3.models[1].paid);
}

// ============================================================================
// Test 9: test_sdd_generator_gate_fields
// ============================================================================
#[tokio::test]
async fn test_sdd_generator_gate_fields() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_sdd_gates");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let spec_content = r#"
## TDD Contract
Test.

## Architecture
Arch.

## Exit Criteria
Criteria.

## Guardrails
Guard.
"#;

    let spec_path = tmpdir.join("spec.md");
    std::fs::write(&spec_path, spec_content).unwrap();

    let dag = SddGenerator::from_spec(
        &spec_path,
        &tmpdir,
        &ModelsConfig::default(),
        &SddConfig::default(),
    )
    .unwrap();

    let iter2 = &dag.nodes[4]; // implement-iter2
    assert_eq!(iter2.gate, Some("validate-iter1:fail".to_string()));

    let iter3 = &dag.nodes[7]; // implement-iter3
    assert_eq!(iter3.gate, Some("validate-iter2:fail".to_string()));
}

// ============================================================================
// Test 10: test_sdd_generator_shell_nodes
// ============================================================================
#[tokio::test]
async fn test_sdd_generator_shell_nodes() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_sdd_shell_nodes");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let spec_content = r#"
## TDD Contract
Test.

## Architecture
Arch.

## Exit Criteria
Criteria.

## Guardrails
Guard.
"#;

    let spec_path = tmpdir.join("spec.md");
    std::fs::write(&spec_path, spec_content).unwrap();

    let dag = SddGenerator::from_spec(
        &spec_path,
        &tmpdir,
        &ModelsConfig::default(),
        &SddConfig::default(),
    )
    .unwrap();

    // Check that validate-* and quality-gate-* nodes have type "shell"
    for node in &dag.nodes {
        if node.id.starts_with("validate-") || node.id.starts_with("quality-gate-") {
            assert_eq!(node.node_type, Some("shell".to_string()));
        }
    }
}

// ============================================================================
// Test 11: test_config_parse_valid
// ============================================================================
#[tokio::test]
async fn test_config_parse_valid() {
    let toml_content = r#"
[project]
root = "/path/to/project"

[worker]
default_model = "google/gemini-pro"
timeout_secs = 300

[sdd]
max_iterations = 5
validate_script = "/path/to/validate.sh"
quality_gate_script = "/path/to/quality.sh"
"#;

    let config = Config::from_toml_str(toml_content).unwrap();

    assert_eq!(config.project.root, PathBuf::from("/path/to/project"));
    assert_eq!(config.worker.default_model, "google/gemini-pro");
    assert_eq!(config.worker.timeout_secs, 300);
    assert_eq!(config.sdd.max_iterations, 5);
}

// ============================================================================
// Test 12: test_config_defaults
// ============================================================================
#[tokio::test]
async fn test_config_defaults() {
    let config = Config::default();

    assert_eq!(config.project.root, PathBuf::from("."));
    assert_eq!(config.worker.default_model, "nvidia/z-ai/glm-5.2");
    assert_eq!(config.worker.timeout_secs, 120);
    assert_eq!(config.sdd.max_iterations, 3);
}

// ============================================================================
// Test 13: test_node_type_field
// ============================================================================
#[tokio::test]
async fn test_node_type_field() {
    let json = r#"
{
  "nodes": [
    {
      "id": "shell_node",
      "prompt": "echo test",
      "depends_on": [],
      "models": [],
      "retry": {"attempts": 1, "backoff_ms": 0},
      "node_type": "shell"
    },
    {
      "id": "llm_node",
      "prompt": "implement",
      "depends_on": [],
      "models": [{"name": "gpt-4", "paid": true}],
      "retry": {"attempts": 2, "backoff_ms": 1000}
    }
  ]
}
"#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    assert_eq!(dag.nodes[0].node_type, Some("shell".to_string()));
    assert_eq!(dag.nodes[1].node_type, None);
}

// ============================================================================
// Test 14: test_gate_field
// ============================================================================
#[tokio::test]
async fn test_gate_field() {
    let json = r#"
{
  "nodes": [
    {
      "id": "validate_node",
      "prompt": "validate",
      "depends_on": [],
      "models": [],
      "retry": {"attempts": 1, "backoff_ms": 0}
    },
    {
      "id": "conditional_node",
      "prompt": "implement",
      "depends_on": ["validate_node"],
      "models": [{"name": "gpt-4", "paid": true}],
      "retry": {"attempts": 1, "backoff_ms": 0},
      "gate": "validate_node:fail"
    }
  ]
}
"#;

    let dag: Dag = serde_json::from_str(json).unwrap();
    assert_eq!(dag.nodes[1].gate, Some("validate_node:fail".to_string()));
}

// ============================================================================
// Test 15: test_sdd_dag_is_valid_pidag_json
// ============================================================================
#[tokio::test]
async fn test_sdd_dag_is_valid_pidag_json() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_sdd_valid");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let spec_content = r#"
## TDD Contract
Test.

## Architecture
Arch.

## Exit Criteria
Criteria.

## Guardrails
Guard.
"#;

    let spec_path = tmpdir.join("spec.md");
    std::fs::write(&spec_path, spec_content).unwrap();

    let dag = SddGenerator::from_spec(
        &spec_path,
        &tmpdir,
        &ModelsConfig::default(),
        &SddConfig::default(),
    )
    .unwrap();

    // Should be valid JSON and pass DAG validation
    let json = serde_json::to_string(&dag).unwrap();
    let deserialized: Dag = serde_json::from_str(&json).unwrap();
    deserialized.validate().unwrap();
}

// ============================================================================
// Test 16: test_sdd_iteration_dependencies
// ============================================================================
#[tokio::test]
async fn test_sdd_iteration_dependencies() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_sdd_deps");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let spec_content = r#"
## TDD Contract
Test.

## Architecture
Arch.

## Exit Criteria
Criteria.

## Guardrails
Guard.
"#;

    let spec_path = tmpdir.join("spec.md");
    std::fs::write(&spec_path, spec_content).unwrap();

    let dag = SddGenerator::from_spec(
        &spec_path,
        &tmpdir,
        &ModelsConfig::default(),
        &SddConfig::default(),
    )
    .unwrap();

    // Check proper dependency chain
    // spec-25 (ordering edges): validate/quality-gate nodes use `after`
    // (ordering-only, never-blocking) edges so validate ALWAYS runs; fix
    // nodes keep depends_on + gate on their validate source.
    assert_eq!(dag.nodes[1].depends_on, Vec::<String>::new()); // implement-iter1 has no deps
    assert_eq!(dag.nodes[2].depends_on, Vec::<String>::new()); // quality-gate-1
    assert_eq!(dag.nodes[2].after, vec!["implement-iter1".to_string()]);
    assert_eq!(dag.nodes[3].depends_on, Vec::<String>::new()); // validate-iter1
    assert_eq!(
        dag.nodes[3].after,
        vec!["implement-iter1".to_string(), "quality-gate-1".to_string()]
    );
    assert_eq!(dag.nodes[4].depends_on, vec!["validate-iter1".to_string()]); // implement-iter2
    assert_eq!(dag.nodes[4].gate.as_deref(), Some("validate-iter1:fail"));
    assert_eq!(dag.nodes[5].depends_on, Vec::<String>::new()); // quality-gate-2
    assert_eq!(dag.nodes[5].after, vec!["implement-iter2".to_string()]);
    assert_eq!(dag.nodes[6].depends_on, Vec::<String>::new()); // validate-iter2
    assert_eq!(
        dag.nodes[6].after,
        vec!["implement-iter2".to_string(), "quality-gate-2".to_string()]
    );
    assert_eq!(dag.nodes[7].depends_on, vec!["validate-iter2".to_string()]); // implement-iter3
    assert_eq!(dag.nodes[7].gate.as_deref(), Some("validate-iter2:fail"));
    assert_eq!(dag.nodes[8].depends_on, Vec::<String>::new()); // quality-gate-3
    assert_eq!(dag.nodes[8].after, vec!["implement-iter3".to_string()]);
    assert_eq!(dag.nodes[9].depends_on, Vec::<String>::new()); // validate-iter3
    assert_eq!(
        dag.nodes[9].after,
        vec!["implement-iter3".to_string(), "quality-gate-3".to_string()]
    );
}

// ============================================================================
// Test 17: test_sdd_node_count
// ============================================================================
#[tokio::test]
async fn test_sdd_node_count() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_sdd_count");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let spec_content = r#"
## TDD Contract
Test.

## Architecture
Arch.

## Exit Criteria
Criteria.

## Guardrails
Guard.
"#;

    let spec_path = tmpdir.join("spec.md");
    std::fs::write(&spec_path, spec_content).unwrap();

    let dag = SddGenerator::from_spec(
        &spec_path,
        &tmpdir,
        &ModelsConfig::default(),
        &SddConfig::default(),
    )
    .unwrap();

    assert_eq!(dag.nodes.len(), 10);
}

// ============================================================================
// Test 18: test_sdd_node_ids
// ============================================================================
#[tokio::test]
async fn test_sdd_node_ids() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_sdd_ids");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let spec_content = r#"
## TDD Contract
Test.

## Architecture
Arch.

## Exit Criteria
Criteria.

## Guardrails
Guard.
"#;

    let spec_path = tmpdir.join("spec.md");
    std::fs::write(&spec_path, spec_content).unwrap();

    let dag = SddGenerator::from_spec(
        &spec_path,
        &tmpdir,
        &ModelsConfig::default(),
        &SddConfig::default(),
    )
    .unwrap();

    let expected_ids = [
        "validate-baseline",
        "implement-iter1",
        "quality-gate-1",
        "validate-iter1",
        "implement-iter2",
        "quality-gate-2",
        "validate-iter2",
        "implement-iter3",
        "quality-gate-3",
        "validate-iter3",
    ];

    for (i, expected_id) in expected_ids.iter().enumerate() {
        assert_eq!(dag.nodes[i].id, expected_id.to_string());
    }
}

// ============================================================================
// Test 19: test_render_status_skipped_state
// ============================================================================
#[tokio::test]
async fn test_render_status_skipped_state() {
    use pidag::render_status;
    use std::collections::HashMap;

    let tmpdir = PathBuf::from("_tmp/phase4/test_render_skipped");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![
            Node {
                id: "node1".to_string(),
                prompt: "prompt1".to_string(),
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
            },
            Node {
                id: "node2".to_string(),
                prompt: "prompt2".to_string(),
                depends_on: vec!["node1".to_string()],
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
            },
        ],
    };

    let mut states: HashMap<String, (String, Option<String>)> = HashMap::new();
    states.insert("node1".to_string(), ("Done".to_string(), None));
    states.insert("node2".to_string(), ("Skipped".to_string(), None));

    let status = render_status(&dag, &states);
    assert!(status.contains("⊘"));
    assert!(status.contains("Skipped"));
}

// ============================================================================
// Test 20: test_shell_worker_command_not_found
// ============================================================================
#[tokio::test]
async fn test_shell_worker_command_not_found() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_shell_notfound");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "notfound".to_string(),
            prompt: "nonexistent_command_12345".to_string(),
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
            verify: None,
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
    };

    let worker = RealShellWorker::new(&dag, Duration::from_secs(5));
    let result = worker
        .run("notfound", "nonexistent_command_12345", "", 1)
        .await
        .unwrap();

    assert!(!result.success);
}

// ============================================================================
// Test 21: test_sdd_spec_with_minimal_content
// ============================================================================
#[tokio::test]
async fn test_sdd_spec_with_minimal_content() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_sdd_minimal");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    // Spec with missing sections should still generate a valid DAG
    let spec_content = r#"
# Minimal Spec

Some intro text.
"#;

    let spec_path = tmpdir.join("spec.md");
    std::fs::write(&spec_path, spec_content).unwrap();

    let dag = SddGenerator::from_spec(
        &spec_path,
        &tmpdir,
        &ModelsConfig::default(),
        &SddConfig::default(),
    )
    .unwrap();

    // Should still generate 10 nodes even with minimal content
    assert_eq!(dag.nodes.len(), 10);
    dag.validate().unwrap(); // Should be valid DAG
}

// ============================================================================
// Test 22: test_config_load_from_nonexistent_file
// ============================================================================
#[tokio::test]
async fn test_config_load_from_nonexistent_file() {
    let tmpdir = PathBuf::from("_tmp/phase4/test_config_missing");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();

    let config_path = tmpdir.join("nonexistent.toml");

    // Should return defaults, not error
    let config = Config::load(&config_path).unwrap();

    assert_eq!(config.project.root, PathBuf::from("."));
    assert_eq!(config.worker.default_model, "nvidia/z-ai/glm-5.2");
    assert_eq!(config.sdd.max_iterations, 3);
}
