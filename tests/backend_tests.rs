//! Backend module tests (G1-G9, G11).
//! Tests for the pluggable agent backend abstraction (spec-21).

use pidag::{
    AgentBackend, BackendRegistry, Dag, MockBackend, MockCapabilities, NodeStatus, Scheduler,
    SessionSpec, ThinkingLevel, TypeDispatchWorker, VecSink, Worker,
};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

/// Serializes env-var tests so PIDAG_AGENT_BACKEND doesn't race.
static ENV_LOCK: Mutex<()> = Mutex::new(());

// Helper to create a simple single-node DAG
fn single_node_dag(id: &str, prompt: &str) -> Dag {
    let json = format!(
        r#"{{
            "nodes": [
                {{
                    "id": "{id}",
                    "prompt": "{prompt}",
                    "depends_on": [],
                    "models": [{{"name": "nvidia/test", "paid": false}}],
                    "retry": {{"attempts": 1, "backoff_ms": 0}}
                }}
            ]
        }}"#
    );
    serde_json::from_str(&json).expect("valid single-node dag fixture")
}

// Helper to create a 3-node DAG for G11
fn three_node_dag() -> Dag {
    let json = r#"
    {
        "nodes": [
            {
                "id": "node1",
                "prompt": "First prompt",
                "depends_on": [],
                "models": [{"name": "nvidia/test", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            },
            {
                "id": "node2",
                "prompt": "Second prompt",
                "depends_on": ["node1"],
                "models": [{"name": "nvidia/test", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            },
            {
                "id": "node3",
                "prompt": "Third prompt",
                "depends_on": ["node2"],
                "models": [{"name": "nvidia/test", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            }
        ]
    }
    "#;
    serde_json::from_str(json).expect("valid three-node dag fixture")
}

// G1: Registry resolves configured backend by name
#[tokio::test]
async fn test_registry_resolves_configured_backend() {
    let registry = BackendRegistry::default();
    let backend = registry.get("mock");
    assert!(backend.is_some(), "MockBackend should be registered");
    assert_eq!(backend.unwrap().name(), "mock");
}

// G2: Unknown backend is a startup error listing registered names
#[tokio::test]
async fn test_unknown_backend_is_startup_error() {
    let registry = BackendRegistry::default();
    let result = registry.resolve("nope");
    assert!(result.is_err(), "Unknown backend should return error");
    if let Err(err) = result {
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("unknown backend"),
            "Error should mention unknown backend"
        );
        assert!(
            err_msg.contains("mock"),
            "Error should list registered backends"
        );
    }
}

// G3: Env override (PIDAG_AGENT_BACKEND) beats config
#[tokio::test]
async fn test_env_overrides_config_backend() {
    let _guard = ENV_LOCK.lock().unwrap();

    // Simulate config saying "pi"
    let config_backend = Some("pi");

    // Set env to "mock"
    unsafe {
        std::env::set_var("PIDAG_AGENT_BACKEND", "mock");
    }

    let resolved = pidag::backend::registry::resolve_backend_name(config_backend);
    assert_eq!(resolved, Some("mock".to_string()));

    unsafe {
        std::env::remove_var("PIDAG_AGENT_BACKEND");
    }
}

// G4: Capabilities are honest — undeclared capability returns Unsupported
#[tokio::test]
async fn test_capabilities_are_honest() {
    // Create a MockBackend with fork capability false
    let backend = MockBackend::with_capabilities(MockCapabilities {
        fork: false,
        ..Default::default()
    });

    assert!(
        !backend.capabilities().fork,
        "Backend should declare fork: false"
    );

    // Open a session and verify fork returns Unsupported
    let spec = SessionSpec {
        model: pidag::backend::ModelRef {
            name: "test".to_string(),
            paid: false,
        },
        thinking_level: ThinkingLevel::Low,
    };

    let mut session = backend.open_session(spec).await.expect("session opens");
    let result = session.fork().await;
    assert!(result.is_err(), "fork should return Unsupported");
}

// G5: Degrade no model_switch — close session, open new one with target
#[tokio::test]
async fn test_degrade_no_model_switch() {
    let backend = MockBackend::with_capabilities(MockCapabilities {
        model_switch: false,
        ..Default::default()
    });

    let spec = SessionSpec {
        model: pidag::backend::ModelRef {
            name: "model1".to_string(),
            paid: false,
        },
        thinking_level: ThinkingLevel::Low,
    };

    let mut session = backend.open_session(spec).await.expect("session opens");

    // Try to switch model on a backend without model_switch capability
    let new_model = pidag::backend::ModelRef {
        name: "model2".to_string(),
        paid: false,
    };

    let result = session.set_model(&new_model).await;
    assert!(result.is_err(), "set_model should fail without capability");

    // In a real DAG execution, pidag would close this session and open a new one.
    // For now, just verify the capability gating works.
}

// G6: Degrade no fork — open independent sessions for parallel nodes
#[tokio::test]
async fn test_degrade_no_fork_opens_fresh_session() {
    // Create a backend without fork capability
    let backend = MockBackend::with_capabilities(MockCapabilities {
        fork: false,
        ..Default::default()
    });

    let spec = SessionSpec {
        model: pidag::backend::ModelRef {
            name: "test".to_string(),
            paid: false,
        },
        thinking_level: ThinkingLevel::Low,
    };

    // Open two independent sessions (as pidag would do without fork)
    let session1 = backend
        .open_session(spec.clone())
        .await
        .expect("session1 opens");
    let session2 = backend.open_session(spec).await.expect("session2 opens");

    // Both should succeed independently
    assert_ne!(
        session1.as_ref() as *const _,
        session2.as_ref() as *const _,
        "Sessions should be distinct objects"
    );
}

// G7: AgentWorker maps node to prompt
#[tokio::test]
async fn test_agent_worker_maps_node_to_prompt() {
    let dag = single_node_dag("test_node", "What is 2+2?");
    let backend = Arc::new(MockBackend::new());
    let worker = pidag::AgentWorker::new(backend, &dag, Duration::from_secs(10));

    let result = worker
        .run("test_node", "What is 2+2?", "nvidia/test", 1)
        .await
        .expect("worker runs");
    assert!(result.success);
    assert!(
        result.output.contains("2+2"),
        "Output should reference the prompt"
    );
}

// G8: AgentWorker error handling - session prompt failure maps to WorkerOutput
#[tokio::test]
async fn test_agent_worker_error_maps_to_worker_output() {
    let dag = single_node_dag("test_node", "What happens on transport error?");
    let backend = Arc::new(MockBackend::new().with_prompt_failure());
    let worker = pidag::AgentWorker::new(backend, &dag, Duration::from_secs(10));

    // Prompt failure should return Ok(WorkerOutput { success: false, retryable: true })
    let result = worker
        .run(
            "test_node",
            "What happens on transport error?",
            "nvidia/test",
            1,
        )
        .await;
    match result {
        Ok(output) => {
            assert!(!output.success, "Should report failure when prompt fails");
            assert!(
                output.retryable,
                "429 error should be classified as retryable"
            );
            assert!(
                output.output.contains("429") || output.output.contains("Prompt failed"),
                "Output should describe the error"
            );
        }
        Err(e) => {
            panic!(
                "AgentWorker should return Ok(WorkerOutput {{ success: false, .. }}) not Err: {}",
                e
            );
        }
    }
}

// G9: No vendor types in backend module (source scan)
//
// Spec-18 (R13) + Spec-21 (R5): The seam must be vendor-neutral. Vendor types
// (pi::sdk, pi_agent_rust, PI_[A-Z_]+ env vars) are allowed ONLY in src/backend/pi.rs.
// Every other backend file MUST be free of vendor imports and references.
//
// The internal crate module path (pub use pi::PiBackend; in mod.rs) is legitimate
// and must NOT be flagged as a vendor leak.
#[test]
fn test_no_vendor_types_in_backend_module() {
    // Read the vendor-neutral files (excluding src/backend/pi.rs, which IS allowed vendor types)
    let backend_mod =
        std::fs::read_to_string("src/backend/mod.rs").expect("can read backend/mod.rs");
    let backend_capabilities = std::fs::read_to_string("src/backend/capabilities.rs")
        .expect("can read backend/capabilities.rs");
    let backend_registry =
        std::fs::read_to_string("src/backend/registry.rs").expect("can read backend/registry.rs");
    let backend_mock =
        std::fs::read_to_string("src/backend/mock.rs").expect("can read backend/mock.rs");

    let all_content = format!(
        "{}\n{}\n{}\n{}",
        backend_mod, backend_capabilities, backend_registry, backend_mock
    );

    // Vendor SDK imports (pi::sdk::*)
    assert!(
        !all_content.contains("pi::sdk"),
        "backend module should not contain pi::sdk (vendor SDK) imports"
    );

    // Vendor crate name
    assert!(
        !all_content.contains("pi_agent_rust"),
        "backend module should not contain pi_agent_rust (vendor crate) references"
    );

    // Vendor env vars: PI_* (note: PIDAG_* is our own)
    // We check for the pattern by looking for "PI_" followed by uppercase letters and underscores.
    // This catches PI_THINKING, PI_TRANSPORT_URL, etc., but NOT innocent patterns like
    // "PI_" in comments or strings that aren't env var references.
    // To be safe, we check for std::env::var("PI_ pattern which is how vendor env vars are accessed.
    assert!(
        !all_content.contains("std::env::var(\"PI_"),
        "backend module should not access vendor PI_* env vars"
    );
    assert!(
        !all_content.contains("env::var(\"PI_"),
        "backend module should not access vendor PI_* env vars"
    );
}

// G11 (KEYSTONE): Zero-capability backend completes a 3-node DAG via fallbacks
#[tokio::test]
async fn test_zero_capability_backend_completes_a_dag() {
    let dag = three_node_dag();
    let backend = Arc::new(MockBackend::with_capabilities(MockCapabilities::default()));

    // Verify the backend truly has zero capabilities
    let caps = backend.capabilities();
    assert!(
        !caps.sessions
            && !caps.multi_turn
            && !caps.model_switch
            && !caps.thinking_levels
            && !caps.fork
            && !caps.compact
            && !caps.token_usage
            && !caps.cancellation
            && !caps.tool_events,
        "Backend should declare zero capabilities"
    );

    // Build a TypeDispatchWorker that routes LLM nodes through the agent backend
    let dispatcher = TypeDispatchWorker::new(&dag, Duration::from_secs(10));
    let dispatcher = dispatcher.with_backend(backend, &dag, Duration::from_secs(10));

    // Create an in-memory sink to capture events
    let sink = VecSink::new();

    // Create a scheduler and run the DAG
    let mut scheduler =
        Scheduler::new(dag.clone(), Box::new(dispatcher), Box::new(sink.clone()), 1);

    let report = scheduler
        .run(false)
        .await
        .expect("DAG should run successfully");

    // Verify all three nodes reached terminal state
    assert_eq!(
        report.node_states.len(),
        3,
        "Should have exactly 3 node states in report"
    );

    for node_state in &report.node_states {
        assert_eq!(
            node_state.state,
            NodeStatus::Done,
            "Node {} should be in Done state, got: {}",
            node_state.node_id,
            node_state.state
        );
        assert!(
            node_state.output.is_some(),
            "Node {} should have output",
            node_state.node_id
        );
    }
}
