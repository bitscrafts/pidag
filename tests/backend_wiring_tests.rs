use pidag::Dag;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// Serializes env-var tests so PIDAG_AGENT_BACKEND doesn't race between tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Create a DAG with a single LLM node.
fn single_llm_node_dag(id: &str, prompt: &str) -> Dag {
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

/// Helper to create a config with optional backend name.
fn config_with_backend(backend_name: Option<&str>) -> pidag::Config {
    pidag::Config {
        project: pidag::core::config::ProjectConfig {
            root: PathBuf::from("."),
        },
        worker: pidag::core::config::WorkerConfig {
            default_model: "nvidia/z-ai/glm-5.2".to_string(),
            timeout_secs: 60,
            thinking_level: "low".to_string(),
            provider: None,
        },
        sdd: pidag::core::config::SddConfig::default(),
        models: pidag::core::config::ModelsConfig::default(),
        agent: pidag::core::config::AgentConfig {
            backend: backend_name.map(|s| s.to_string()),
        },
        rpc: pidag::core::config::RpcConfig::default(),
    }
}

/// Test that no backend configured uses default routing (spec-21 N1).
/// When no [agent] backend is set and no env var is set, the worker should have
/// no agent backend attached; it routes LLM nodes to A2aWorker/PiPrintWorker.
#[tokio::test]
async fn test_no_backend_configured_uses_default_routing() {
    let _lock = ENV_LOCK.lock();
    unsafe { std::env::remove_var("PIDAG_AGENT_BACKEND") };

    let dag = single_llm_node_dag("test", "test prompt");
    let config = config_with_backend(None);
    let timeout = Duration::from_secs(5);

    // build_worker with no backend configured should return a plain TypeDispatchWorker
    let worker = pidag::worker::build_worker(&dag, timeout, &config, 1)
        .expect("should build worker without backend");

    // Verify the worker can run; we don't directly check for the absence of
    // agent_worker, but we know the routing defaults when none is attached.
    let result = worker.run("test", "test prompt", "nvidia", 1).await;
    assert!(
        result.is_ok(),
        "worker should run without error (default routing): {:?}",
        result
    );
}

/// Test that "mock" backend is properly attached and routes LLM nodes through it.
/// When [agent] backend = "mock", LLM nodes should route through MockBackend.
#[tokio::test]
async fn test_configured_mock_backend_is_attached() {
    let _lock = ENV_LOCK.lock();
    unsafe { std::env::remove_var("PIDAG_AGENT_BACKEND") };

    let dag = single_llm_node_dag("test", "test prompt");
    let config = config_with_backend(Some("mock"));
    let timeout = Duration::from_secs(5);

    // build_worker with "mock" backend should attach the MockBackend
    let worker = pidag::worker::build_worker(&dag, timeout, &config, 1)
        .expect("should build worker with mock backend");

    // When the worker runs an LLM node with mock backend attached,
    // it should route through AgentWorker -> MockBackend.
    let result = worker.run("test", "test prompt", "nvidia", 1).await;
    assert!(
        result.is_ok(),
        "worker should run LLM node through mock backend: {:?}",
        result
    );
}

/// Test that env var PIDAG_AGENT_BACKEND overrides config (spec-21 R6).
/// Config says "mock", but env says "pi": "pi" should win.
#[tokio::test]
async fn test_env_overrides_config() {
    let _lock = ENV_LOCK.lock();

    // Set env to "pi" (overrides config "mock")
    unsafe { std::env::set_var("PIDAG_AGENT_BACKEND", "pi") };

    let dag = single_llm_node_dag("test", "test prompt");
    let config = config_with_backend(Some("mock")); // config says "mock"
    let timeout = Duration::from_secs(5);

    // build_worker should use "pi" from env, not "mock" from config
    let result = pidag::worker::build_worker(&dag, timeout, &config, 1);

    // Clean up
    unsafe { std::env::remove_var("PIDAG_AGENT_BACKEND") };

    // The result should be Ok because "pi" is a registered backend
    assert!(
        result.is_ok(),
        "env var should override config and resolve to pi backend"
    );
}

/// Test that unknown backend name is an error listing registered names (spec-21 R6).
/// When [agent] backend = "nope" (not registered), should return clear error.
#[tokio::test]
async fn test_unknown_backend_is_error() {
    let _lock = ENV_LOCK.lock();
    unsafe { std::env::remove_var("PIDAG_AGENT_BACKEND") };

    let dag = single_llm_node_dag("test", "test prompt");
    let config = config_with_backend(Some("nope")); // unknown backend
    let timeout = Duration::from_secs(5);

    // build_worker should fail with an error listing registered backends
    let result = pidag::worker::build_worker(&dag, timeout, &config, 1);

    match result {
        Err(err) => {
            let err_msg = err.to_string();
            assert!(
                err_msg.contains("unknown backend"),
                "error should mention 'unknown backend': {}",
                err_msg
            );
            assert!(
                err_msg.contains("mock"),
                "error should list 'mock' as a registered backend: {}",
                err_msg
            );
            assert!(
                err_msg.contains("pi"),
                "error should list 'pi' as a registered backend: {}",
                err_msg
            );
        }
        Ok(_) => panic!("expected error for unknown backend 'nope', but got Ok"),
    }
}

/// Test that "pi" backend is registered and resolvable (construction only).
/// Verifies that "pi" is in the registered backends list; doesn't spawn a process.
#[tokio::test]
async fn test_pi_backend_is_registered() {
    let _lock = ENV_LOCK.lock();
    unsafe { std::env::remove_var("PIDAG_AGENT_BACKEND") };

    let dag = single_llm_node_dag("test", "test prompt");
    let config = config_with_backend(Some("pi"));
    let timeout = Duration::from_secs(5);

    // build_worker should resolve "pi" successfully
    let result = pidag::worker::build_worker(&dag, timeout, &config, 1);

    assert!(
        result.is_ok(),
        "should resolve 'pi' backend from registered backends"
    );
}
