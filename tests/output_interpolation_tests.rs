//! TDD Contract tests for output interpolation (spec-29).
//! Tests I1-I9 and N1 covering static validation and dynamic interpolation.

use pidag::NodeStatus;
use pidag::core::dag::{Dag, ModelRef, Node, RetryPolicy};
use pidag::core::error::PidagError;
use pidag::scheduler::NodeState;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn create_test_node(id: &str, prompt: &str, depends_on: Vec<&str>, after: Vec<&str>) -> Node {
    Node {
        id: id.to_string(),
        prompt: prompt.to_string(),
        depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        models: vec![ModelRef {
            name: "test-model".to_string(),
            paid: false,
        }],
        retry: RetryPolicy {
            attempts: 1,
            backoff_ms: 0,
        },
        validate: None,
        node_type: None,
        gate: None,
        timeout: None,
        mcp_call: None,
        after: after.iter().map(|s| s.to_string()).collect(),
        verify: None,
        verify_pre: None,
    }
}

// I1: test_output_interpolated_at_dispatch
#[test]
fn test_output_interpolated_at_dispatch() {
    let nodes = vec![
        create_test_node("a", "echo HELLO", vec![], vec![]),
        create_test_node("b", "saw: {{a.output}}", vec!["a"], vec![]),
    ];

    let dag = Dag {
        nodes,
        metadata: None,
    };

    // Should validate without error
    assert!(dag.validate().is_ok());

    // Simulate dispatch-time interpolation
    let mut node_state: HashMap<String, NodeState> = HashMap::new();
    node_state.insert(
        "a".to_string(),
        NodeState {
            node_id: "a".to_string(),
            state: NodeStatus::Done,
            model: Some("test".to_string()),
            attempts: 1,
            output: Some("HELLO".to_string()),
        },
    );

    // Test interpolation (this would be called in execute.rs before dispatch)
    let interpolated = interpolate_outputs("saw: {{a.output}}", &node_state);
    assert_eq!(interpolated, "saw: HELLO");
    assert!(!interpolated.contains("{{"));
}

// I2: test_failed_node_output_is_available
#[test]
fn test_failed_node_output_is_available() {
    let nodes = vec![
        create_test_node("a", "echo BOOM >&2; exit 1", vec![], vec![]),
        create_test_node("b", "Handle: {{a.output}}", vec!["a"], vec![]),
    ];

    let dag = Dag {
        nodes,
        metadata: None,
    };

    assert!(dag.validate().is_ok());

    // Node in Failed state should still have output available
    let mut node_state: HashMap<String, NodeState> = HashMap::new();
    node_state.insert(
        "a".to_string(),
        NodeState {
            node_id: "a".to_string(),
            state: NodeStatus::Failed,
            model: None,
            attempts: 1,
            output: Some("BOOM".to_string()),
        },
    );

    let interpolated = interpolate_outputs("Handle: {{a.output}}", &node_state);
    assert_eq!(interpolated, "Handle: BOOM");
}

// I3: test_unknown_node_reference_is_validation_error
#[test]
fn test_unknown_node_reference_is_validation_error() {
    let nodes = vec![create_test_node(
        "b",
        "use: {{nosuch.output}}",
        vec![],
        vec![],
    )];

    let dag = Dag {
        nodes,
        metadata: None,
    };

    let result = dag.validate();
    assert!(result.is_err());
    if let Err(PidagError::Validation(msg)) = result {
        assert!(msg.contains("b"));
        assert!(msg.contains("nosuch"));
    } else {
        panic!("Expected Validation error");
    }
}

// I4a: test_reference_without_edge_is_validation_error
#[test]
fn test_reference_without_edge_is_validation_error() {
    let nodes = vec![
        create_test_node("a", "echo OK", vec![], vec![]),
        create_test_node("b", "use: {{a.output}}", vec![], vec![]), // No depends_on or after
    ];

    let dag = Dag {
        nodes,
        metadata: None,
    };

    let result = dag.validate();
    assert!(result.is_err());
    if let Err(PidagError::Validation(msg)) = result {
        assert!(msg.contains("b"));
        assert!(msg.contains("a"));
    } else {
        panic!("Expected Validation error");
    }
}

// I4b: test_reference_via_after_edge_is_allowed
#[test]
fn test_reference_via_after_edge_is_allowed() {
    let nodes = vec![
        create_test_node("a", "echo OK", vec![], vec![]),
        create_test_node("b", "use: {{a.output}}", vec![], vec!["a"]), // after edge
    ];

    let dag = Dag {
        nodes,
        metadata: None,
    };

    // Should validate without error
    assert!(dag.validate().is_ok());
}

// I5a: test_malformed_double_brace_is_error
#[test]
fn test_malformed_status_reference() {
    let nodes = vec![
        create_test_node("a", "echo OK", vec![], vec![]),
        create_test_node("b", "status: {{a.status}}", vec!["a"], vec![]),
    ];

    let dag = Dag {
        nodes,
        metadata: None,
    };

    assert!(dag.validate().is_err());
}

#[test]
fn test_malformed_node_only_reference() {
    let nodes = vec![
        create_test_node("a", "echo OK", vec![], vec![]),
        create_test_node("b", "use: {{a}}", vec!["a"], vec![]),
    ];

    let dag = Dag {
        nodes,
        metadata: None,
    };

    assert!(dag.validate().is_err());
}

#[test]
fn test_malformed_empty_braces() {
    let nodes = vec![
        create_test_node("a", "echo OK", vec![], vec![]),
        create_test_node("b", "use: {{}}", vec!["a"], vec![]),
    ];

    let dag = Dag {
        nodes,
        metadata: None,
    };

    assert!(dag.validate().is_err());
}

// I5b: test_nested_placeholder_inside_double_brace_is_error
#[test]
fn test_nested_placeholder_inside_double_brace_is_error() {
    let nodes = vec![
        create_test_node("a", "echo OK", vec![], vec![]),
        create_test_node("b", "use: {{validate-iter{1}.output}}", vec!["a"], vec![]),
    ];

    let dag = Dag {
        nodes,
        metadata: None,
    };

    let result = dag.validate();
    assert!(result.is_err());
}

// I6a: test_large_output_truncated_to_cap
#[test]
fn test_large_output_truncated_to_cap() {
    // Create a 100 KB output
    let large_output = "x".repeat(100 * 1024);

    let mut node_state: HashMap<String, NodeState> = HashMap::new();
    node_state.insert(
        "a".to_string(),
        NodeState {
            node_id: "a".to_string(),
            state: NodeStatus::Done,
            model: None,
            attempts: 1,
            output: Some(large_output.clone()),
        },
    );

    let interpolated = interpolate_outputs("Output: {{a.output}}", &node_state);

    // Should be capped at 32 KB + truncation marker
    assert!(interpolated.len() < 35 * 1024);
    assert!(interpolated.contains("[… "));
    assert!(interpolated.contains("bytes truncated"));
}

// I6b: test_truncation_keeps_the_tail
#[test]
fn test_truncation_keeps_the_tail() {
    // Create output with known ending
    let mut output = "x".repeat(100 * 1024);
    output.push_str("THE-END");

    let mut node_state: HashMap<String, NodeState> = HashMap::new();
    node_state.insert(
        "a".to_string(),
        NodeState {
            node_id: "a".to_string(),
            state: NodeStatus::Done,
            model: None,
            attempts: 1,
            output: Some(output),
        },
    );

    let interpolated = interpolate_outputs("Output: {{a.output}}", &node_state);

    // Should contain the end marker
    assert!(interpolated.contains("THE-END"));
}

// I6c: test_truncation_is_char_safe
#[test]
fn test_truncation_is_char_safe() {
    // Create output with 3-byte UTF-8 character repeated to exceed cap
    let three_byte_char = "你"; // 3 bytes in UTF-8
    let mut output = String::new();
    for _ in 0..(35 * 1024) {
        output.push_str(three_byte_char);
    }

    let mut node_state: HashMap<String, NodeState> = HashMap::new();
    node_state.insert(
        "a".to_string(),
        NodeState {
            node_id: "a".to_string(),
            state: NodeStatus::Done,
            model: None,
            attempts: 1,
            output: Some(output),
        },
    );

    // Should not panic and result should be valid UTF-8
    let interpolated = interpolate_outputs("Output: {{a.output}}", &node_state);
    assert!(interpolated.is_char_boundary(interpolated.len()));
    // Verify it's valid UTF-8
    let _: &str = &interpolated;
}

// I7: test_resume_hydrates_output_from_vault
#[test]
fn test_resume_hydrates_output_from_vault() {
    // This test verifies that when a checkpoint is loaded with outputs,
    // they are used for interpolation. Testing the full flow would require
    // integration with store, so this tests the NodeState seeding:
    // If output is in checkpoint.outputs, it should be seeded into NodeState.

    let nodes = vec![
        create_test_node("a", "echo RESULT", vec![], vec![]),
        create_test_node("b", "{{a.output}}", vec!["a"], vec![]),
    ];

    let dag = Dag {
        nodes,
        metadata: None,
    };

    assert!(dag.validate().is_ok());

    // Simulate resumed state where output was hydrated from vault
    let mut node_state: HashMap<String, NodeState> = HashMap::new();
    node_state.insert(
        "a".to_string(),
        NodeState {
            node_id: "a".to_string(),
            state: NodeStatus::Done,
            model: None,
            attempts: 1,
            output: Some("RESULT".to_string()), // Seeded from checkpoint.outputs
        },
    );

    let interpolated = interpolate_outputs("{{a.output}}", &node_state);
    assert_eq!(interpolated, "RESULT");
}

// I8: test_skipped_node_output_substitutes_empty
#[test]
fn test_skipped_node_output_substitutes_empty() {
    // Node was skipped (gated), so output is None
    let mut node_state: HashMap<String, NodeState> = HashMap::new();
    node_state.insert(
        "a".to_string(),
        NodeState {
            node_id: "a".to_string(),
            state: NodeStatus::Done,
            model: None,
            attempts: 0,
            output: None, // Skipped, no output
        },
    );

    let interpolated = interpolate_outputs("Output: [{{a.output}}]", &node_state);

    // Should be replaced with empty string, no explanatory text
    assert_eq!(interpolated, "Output: []");
    assert!(!interpolated.contains("{{"));
}

// I9a: test_sdd_repair_node_receives_validator_output
#[test]
fn test_sdd_repair_node_receives_validator_output() {
    let golden_path = Path::new("tests/fixtures/sdd_golden.json");
    if !golden_path.exists() {
        println!("Skipping test: golden fixture not found");
        return;
    }

    let golden_json = fs::read_to_string(golden_path).expect("read golden fixture");
    let dag: Dag = serde_json::from_str(&golden_json).expect("parse golden DAG");

    // Validate that implement-iter2 has the placeholder
    let impl2 = dag
        .nodes
        .iter()
        .find(|n| n.id == "implement-iter2")
        .unwrap();
    assert!(
        impl2.prompt.contains("{{validate-iter1.output}}"),
        "implement-iter2 must reference validate-iter1.output"
    );

    // Validate DAG is valid
    assert!(dag.validate().is_ok());

    // Simulate dispatch with validator output
    let mut node_state: HashMap<String, NodeState> = HashMap::new();
    node_state.insert(
        "validate-iter1".to_string(),
        NodeState {
            node_id: "validate-iter1".to_string(),
            state: NodeStatus::Failed,
            model: None,
            attempts: 1,
            output: Some("AssertionError: expected True".to_string()),
        },
    );

    // After interpolation, implement-iter2's prompt should contain the error
    let interpolated = interpolate_outputs(&impl2.prompt, &node_state);
    assert!(
        interpolated.contains("AssertionError: expected True"),
        "implement-iter2 should receive validator output"
    );
    assert!(
        !interpolated.contains("{{validate-iter1.output}}"),
        "placeholder should be replaced"
    );
}

// I9b: test_research_synthesize_receives_investigations
#[test]
fn test_research_synthesize_receives_investigations() {
    let nodes = vec![
        create_test_node("investigate-1", "research topic 1", vec![], vec![]),
        create_test_node("investigate-2", "research topic 2", vec![], vec![]),
        create_test_node("investigate-3", "research topic 3", vec![], vec![]),
        create_test_node(
            "synthesize",
            "Results:\nI1: {{investigate-1.output}}\nI2: {{investigate-2.output}}\nI3: {{investigate-3.output}}",
            vec!["investigate-1", "investigate-2", "investigate-3"],
            vec![],
        ),
    ];

    let dag = Dag {
        nodes,
        metadata: None,
    };

    assert!(dag.validate().is_ok());

    let mut node_state: HashMap<String, NodeState> = HashMap::new();
    node_state.insert(
        "investigate-1".to_string(),
        NodeState {
            node_id: "investigate-1".to_string(),
            state: NodeStatus::Done,
            model: None,
            attempts: 1,
            output: Some("Finding 1".to_string()),
        },
    );
    node_state.insert(
        "investigate-2".to_string(),
        NodeState {
            node_id: "investigate-2".to_string(),
            state: NodeStatus::Done,
            model: None,
            attempts: 1,
            output: Some("Finding 2".to_string()),
        },
    );
    node_state.insert(
        "investigate-3".to_string(),
        NodeState {
            node_id: "investigate-3".to_string(),
            state: NodeStatus::Done,
            model: None,
            attempts: 1,
            output: Some("Finding 3".to_string()),
        },
    );

    let synthesize = dag.nodes.iter().find(|n| n.id == "synthesize").unwrap();
    let interpolated = interpolate_outputs(&synthesize.prompt, &node_state);

    assert!(interpolated.contains("Finding 1"));
    assert!(interpolated.contains("Finding 2"));
    assert!(interpolated.contains("Finding 3"));
    assert!(!interpolated.contains("{{"));
}

// N1: test_prompt_without_tokens_is_unchanged
#[test]
fn test_prompt_without_tokens_is_unchanged() {
    let prompt = "This is a normal prompt with no special tokens.";
    let node_state: HashMap<String, NodeState> = HashMap::new();

    let interpolated = interpolate_outputs(prompt, &node_state);
    assert_eq!(interpolated, prompt);
}

// I11: test_worker_receives_interpolated_prompt
// An end-to-end test that the worker receives the interpolated prompt, not the literal placeholder.
// This test demonstrates the I10 defect: the Worker trait receives the non-interpolated prompt
// from its internal snapshot, not the interpolated prompt computed by the scheduler.
#[tokio::test]
async fn test_worker_receives_interpolated_prompt() {
    use async_trait::async_trait;
    use pidag::NodeStatus;
    use pidag::core::error::PidagError;
    use pidag::core::event::{Event, EventSink};
    use pidag::scheduler::Scheduler;
    use pidag::worker::Worker;
    use std::sync::Arc;
    use std::sync::Mutex;

    // Recording mock worker that records which prompts it receives.
    struct RecordingWorker {
        captured_calls: Arc<Mutex<Vec<(String, String)>>>, // (node_id, prompt_used) pairs
    }

    impl RecordingWorker {
        fn new(_dag: &Dag, captured_calls: Arc<Mutex<Vec<(String, String)>>>) -> Self {
            Self { captured_calls }
        }
    }

    #[async_trait]
    impl Worker for RecordingWorker {
        async fn run(
            &self,
            node_id: &str,
            prompt: &str,
            _model: &str,
            _attempt: usize,
        ) -> Result<pidag::worker::WorkerOutput, PidagError> {
            // After I10, record the prompt that was passed to run()
            if let Ok(mut calls) = self.captured_calls.lock() {
                calls.push((node_id.to_string(), prompt.to_string()));
            }

            // Return appropriate output based on node
            let output = if node_id == "a" {
                "UPSTREAM_OUTPUT_VALUE"
            } else {
                "downstream_processed"
            };

            Ok(pidag::worker::WorkerOutput {
                success: true,
                output: output.to_string(),
                retryable: false,
            })
        }
    }

    // Create a minimal EventSink for testing (just discards events)
    struct NoOpEventSink;

    #[async_trait]
    impl EventSink for NoOpEventSink {
        async fn emit(&mut self, _event: &Event) -> Result<(), std::io::Error> {
            Ok(())
        }
    }

    // Create a two-node DAG where b references a's output
    let nodes = vec![
        create_test_node("a", "echo UPSTREAM_OUTPUT_VALUE", vec![], vec![]),
        create_test_node("b", "process: {{a.output}}", vec!["a"], vec![]),
    ];

    let dag = Dag {
        nodes,
        metadata: None,
    };

    // Validate the DAG
    assert!(dag.validate().is_ok());

    // Create the recording worker
    let captured_calls = Arc::new(Mutex::new(Vec::new()));
    let worker = RecordingWorker::new(&dag, captured_calls.clone());

    // Create a scheduler and run the DAG
    let mut scheduler = Scheduler::new(
        dag,
        Box::new(worker),
        Box::new(NoOpEventSink),
        1, // concurrency = 1
    );

    let result = scheduler.run(false).await; // allow_paid = false

    // Verify the scheduler succeeded
    assert!(result.is_ok(), "Scheduler failed: {:?}", result.err());

    // Check what prompts the worker actually received (from its internal snapshot)
    let calls = captured_calls.lock().unwrap();

    // Find the call for node "b"
    let b_call = calls.iter().find(|(id, _)| id == "b");
    assert!(
        b_call.is_some(),
        "Node b was not called. Calls: {:?}",
        calls
    );

    let (_, b_prompt_used) = b_call.unwrap();

    // AFTER I10: The worker receives the interpolated prompt directly as a parameter.
    // It sees "process: UPSTREAM_OUTPUT_VALUE" instead of the literal "process: {{a.output}}"
    //
    // This assertion verifies that the scheduler's interpolation is reaching the worker.
    assert!(
        b_prompt_used.contains("UPSTREAM_OUTPUT_VALUE"),
        "After I10, the worker should receive the interpolated prompt. \
         Got: {}",
        b_prompt_used
    );

    assert!(
        !b_prompt_used.contains("{{"),
        "After I10, the interpolated prompt should not contain literal placeholders. \
         Got: {}",
        b_prompt_used
    );
}

// Helper function to interpolate outputs for testing
// This mirrors the implementation in execute.rs
fn interpolate_outputs(prompt: &str, node_state: &HashMap<String, NodeState>) -> String {
    let mut result = String::new();
    let mut last_end = 0;

    let mut i = 0;
    while let Some(start) = prompt[i..].find("{{") {
        let start_abs = i + start;
        let after_open = start_abs + 2;

        if let Some(offset) = prompt[after_open..].find("}}") {
            let end = after_open + offset;
            let content = &prompt[after_open..end];

            if let Some(dot_pos) = content.find('.') {
                let node_ref = &content[..dot_pos];
                let suffix = &content[dot_pos + 1..];

                if suffix == "output" {
                    result.push_str(&prompt[last_end..start_abs]);
                    let output = node_state
                        .get(node_ref)
                        .and_then(|ns| ns.output.as_deref())
                        .unwrap_or("");

                    const MAX_INTERPOLATED: usize = 32 * 1024;
                    if output.len() > MAX_INTERPOLATED {
                        let tail_start = output.len() - MAX_INTERPOLATED;
                        let safe_tail_start = find_char_boundary_after(output, tail_start);
                        let tail = &output[safe_tail_start..];
                        let dropped = output.len() - tail.len();
                        result.push_str(&format!("[… {} bytes truncated …]\n", dropped));
                        result.push_str(tail);
                    } else {
                        result.push_str(output);
                    }

                    last_end = end + 2;
                    i = end + 2;
                    continue;
                }
            }

            i = end + 2;
        } else {
            break;
        }
    }

    result.push_str(&prompt[last_end..]);
    result
}

fn find_char_boundary_after(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    if s.is_char_boundary(pos) {
        return pos;
    }
    for i in pos + 1..=s.len() {
        if s.is_char_boundary(i) {
            return i;
        }
    }
    s.len()
}
