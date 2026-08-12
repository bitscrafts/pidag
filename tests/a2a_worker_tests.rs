//! A2A Worker tests — TDD contract from `specs/97-a2a-worker.md`.
//!
//! T1-T8 exercise the A2A protocol worker with shell shims that replace the
//! `curl` binary, emitting canned JSON-RPC responses. This keeps the suite
//! fully offline and deterministic — same pattern as `PiPrintWorker::with_command`
//! tests in `phase2_tests.rs`.
//!
//! The shim is passed as the script argument to `sh -c <script> <curl-args>`.
//! The A2aWorker appends curl-style args (`-sS -f -X POST <url> -H ... -d
//! <body>`) after the script, so they become positional parameters (`$0`,
//! `$1`, ...) in the script. This lets tests inspect the request URL (`$4`)
//! and body (`$8`) when needed.

use pidag::{A2aWorker, TypeDispatchWorker, Worker, is_a2a_endpoint};
use std::time::Duration;

/// Build a single-node DAG with a given prompt (llm node, no node_type).
fn single_node_dag(id: &str, prompt: &str) -> pidag::Dag {
    let json = format!(
        r#"{{
            "nodes": [
                {{
                    "id": "{id}",
                    "prompt": "{prompt}",
                    "depends_on": [],
                    "models": [{{"name": "https://agent.example", "paid": false}}],
                    "retry": {{"attempts": 1, "backoff_ms": 0}}
                }}
            ]
        }}"#
    );
    serde_json::from_str(&json).expect("valid single-node dag fixture")
}

// ============================================================================
// T1: is_a2a_endpoint URL detection (pure unit test, no DAG)
// ============================================================================

/// T1: `is_a2a_endpoint` returns `true` for URLs, `false` for plain model names.
#[test]
fn test_a2a_url_detection() {
    assert!(is_a2a_endpoint(
        "https://agents.example.com:7422/agents/hermes"
    ));
    assert!(is_a2a_endpoint("http://localhost:8080"));
    assert!(!is_a2a_endpoint("nvidia/z-ai/glm-5.2"));
    assert!(!is_a2a_endpoint("phi-4"));
    assert!(!is_a2a_endpoint(""));
    // Case-sensitive: HTTP:// is NOT an A2A endpoint.
    assert!(!is_a2a_endpoint("HTTP://localhost:8080"));
}

// ============================================================================
// T2: A2aWorker success — shim echoes a completed response
// ============================================================================

/// T2: Shim returns a completed response with a text artifact. The worker
/// extracts the text and returns `success: true`.
#[tokio::test]
async fn test_a2a_worker_success() {
    let dag = single_node_dag("node1", "hello prompt");
    // The script is passed to `sh -c <script> <args>`. The `\"` inside
    // double quotes is an escaped quote (literal ") in the shell, so the
    // echo output is valid JSON with unescaped double quotes.
    let shim = r#"echo "{\"result\":{\"state\":{\"state\":\"completed\"},\"artifacts\":[{\"parts\":[{\"type\":\"text\",\"text\":\"hello\"}]}]}}""#;
    let worker = A2aWorker::with_command(
        Duration::from_secs(5),
        Duration::from_secs(2),
        "sh".to_string(),
        vec!["-c".to_string(), shim.to_string()],
    );

    let result = worker
        .run("node1", "test prompt", "https://agent.example", 1)
        .await
        .expect("worker must return WorkerOutput, not Err");

    assert!(
        result.success,
        "completed a2a response must be success: {:?}",
        result
    );
    assert_eq!(result.output, "hello");
    assert!(!result.retryable);
}

// ============================================================================
// T3: A2aWorker polling — shim returns working then completed
// ============================================================================

/// T3: Shim returns `working` on the first call, `completed` on the second.
/// The worker polls and succeeds after the second call.
#[tokio::test]
async fn test_a2a_worker_polling() {
    let count_file = "_tmp/a2a_poll_count_t3";
    // Clean up before the test in case a previous run left a stale file.
    let _ = std::fs::remove_file(count_file);

    let dag = single_node_dag("node1", "poll prompt");
    // The shim increments a counter file; first call returns "working",
    // second call returns "completed" with a text artifact.
    let shim = format!(
        r#"COUNT_FILE={cf}; mkdir -p _tmp; COUNT=$(cat $COUNT_FILE 2>/dev/null || echo 0); COUNT=$((COUNT+1)); echo $COUNT > $COUNT_FILE; if [ $COUNT -eq 1 ]; then echo "{{\"result\":{{\"state\":{{\"state\":\"working\"}},\"id\":\"task-123\"}}}}"; else echo "{{\"result\":{{\"state\":{{\"state\":\"completed\"}},\"artifacts\":[{{\"parts\":[{{\"type\":\"text\",\"text\":\"polled\"}}]}}]}}}}"; fi"#,
        cf = count_file
    );
    let worker = A2aWorker::with_command(
        Duration::from_secs(5),
        Duration::from_millis(50),
        "sh".to_string(),
        vec!["-c".to_string(), shim],
    );

    let result = worker
        .run("node1", "test prompt", "https://agent.example", 1)
        .await
        .expect("worker must return WorkerOutput, not Err");

    assert!(
        result.success,
        "polled a2a response must be success: {:?}",
        result
    );
    assert_eq!(result.output, "polled");

    // Clean up after the test.
    let _ = std::fs::remove_file(count_file);
}

// ============================================================================
// T4: A2aWorker failure — shim returns a failed state
// ============================================================================

/// T4: Shim returns `{"result":{"state":{"state":"failed"}}}`. The worker
/// returns `success: false`.
#[tokio::test]
async fn test_a2a_worker_failure() {
    let dag = single_node_dag("node1", "fail prompt");
    let shim = r#"echo "{\"result\":{\"state\":{\"state\":\"failed\"}}}""#;
    let worker = A2aWorker::with_command(
        Duration::from_secs(5),
        Duration::from_secs(2),
        "sh".to_string(),
        vec!["-c".to_string(), shim.to_string()],
    );

    let result = worker
        .run("node1", "test prompt", "https://agent.example", 1)
        .await
        .expect("worker must return WorkerOutput, not Err");

    assert!(!result.success, "failed a2a state must be success:false");
    assert!(!result.retryable, "a2a task failure is not retryable");
}

// ============================================================================
// T5: A2aWorker 429 retryable — shim exits 1 with 429 on stderr
// ============================================================================

/// T5: Shim exits 1 with "HTTP 429 too many requests" on stderr. The worker
/// classifies this as retryable (so the scheduler can failover to the next
/// model).
#[tokio::test]
async fn test_a2a_worker_429_retryable() {
    let dag = single_node_dag("node1", "rate limited prompt");
    let shim = r#"echo "HTTP 429 too many requests" >&2; exit 1"#;
    let worker = A2aWorker::with_command(
        Duration::from_secs(5),
        Duration::from_secs(2),
        "sh".to_string(),
        vec!["-c".to_string(), shim.to_string()],
    );

    let result = worker
        .run("node1", "test prompt", "https://agent.example", 1)
        .await
        .expect("worker must return WorkerOutput, not Err");

    assert!(!result.success, "curl exit 1 must be success:false");
    assert!(
        result.retryable,
        "429 must be classified as retryable: {:?}",
        result.output
    );
}

// ============================================================================
// T6: A2aWorker timeout — shim always returns working, timeout fires
// ============================================================================

/// T6: Shim always returns `working` with a task id. With a 200ms overall
/// timeout and 50ms poll interval, the poll loop hits the timeout.
#[tokio::test]
async fn test_a2a_worker_timeout() {
    let dag = single_node_dag("node1", "timeout prompt");
    let shim = r#"echo "{\"result\":{\"state\":{\"state\":\"working\"},\"id\":\"task-456\"}}""#;
    let worker = A2aWorker::with_command(
        Duration::from_millis(200),
        Duration::from_millis(50),
        "sh".to_string(),
        vec!["-c".to_string(), shim.to_string()],
    );

    let result = worker
        .run("node1", "test prompt", "https://agent.example", 1)
        .await
        .expect("worker must return WorkerOutput, not Err");

    assert!(!result.success, "timeout must be success:false");
    let lower = result.output.to_ascii_lowercase();
    assert!(
        lower.contains("timed out"),
        "output should mention timeout, got: {:?}",
        result.output
    );
    assert!(!result.retryable, "timeout is not retryable");
}

// ============================================================================
// T7: TypeDispatchWorker routes URL to A2aWorker (not PiPrintWorker)
// ============================================================================

/// T7: Build a DAG with an llm node, construct `TypeDispatchWorker` with
/// both pi and a2a shims. The a2a shim returns a distinctive output
/// "from-a2a-shim" proving the A2A branch was taken (not pi).
#[tokio::test]
async fn test_type_dispatch_routes_a2a_url() {
    let json = r#"
    {
        "nodes": [
            {
                "id": "node1",
                "prompt": "route me to a2a",
                "depends_on": [],
                "models": [{"name": "https://agent.example", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0},
                "node_type": "llm"
            }
        ]
    }
    "#;
    let dag: pidag::Dag = serde_json::from_str(json).expect("valid llm node dag");

    // Pi shim would echo "from-pi-shim" — but we should NOT see this.
    let pi_shim = r#"echo from-pi-shim"#;
    // A2A shim echoes a completed response with "from-a2a-shim".
    let a2a_shim = r#"echo "{\"result\":{\"state\":{\"state\":\"completed\"},\"artifacts\":[{\"parts\":[{\"type\":\"text\",\"text\":\"from-a2a-shim\"}]}]}}""#;

    let worker = TypeDispatchWorker::with_pi_and_a2a_command(
        &dag,
        Duration::from_secs(5),
        "sh".to_string(),
        vec!["-c".to_string(), pi_shim.to_string()],
        "sh".to_string(),
        vec!["-c".to_string(), a2a_shim.to_string()],
        Duration::from_secs(2),
    );

    let result = worker
        .run("node1", "test prompt", "https://agent.example", 1)
        .await
        .expect("worker must return WorkerOutput, not Err");

    assert!(
        result.success,
        "a2a-routed call should succeed: {:?}",
        result
    );
    assert_eq!(
        result.output, "from-a2a-shim",
        "output should come from the a2a shim, not the pi shim"
    );
}

// ============================================================================
// T8: A2aWorker extracts #skill fragment into skillId
// ============================================================================

/// T8: Shim captures the `-d` body arg (positional `$8` in `sh -c script $0
/// $1 ... $8`) and the URL (`$4`), then returns a completed response. The
/// test verifies:
/// 1. `body["params"]["skillId"] == "research"` (from `#research` fragment)
/// 2. `body["params"]["message"]["parts"][0]["text"] == <prompt>`
/// 3. The endpoint URL (`$4`) is `https://agent.example/v1/tasks/send`
///    (no `#research`).
#[tokio::test]
async fn test_a2a_skill_fragment_extracted() {
    let body_file = "_tmp/a2a_body_t8.json";
    let url_file = "_tmp/a2a_url_t8.txt";
    let _ = std::fs::remove_file(body_file);
    let _ = std::fs::remove_file(url_file);

    let dag = single_node_dag("node1", "test-prompt-t8");
    // With `sh -c script $0 $1 ... $8`, $4 is the URL and $8 is the body.
    // The `\"` inside double quotes are escaped quotes (literal ").
    let shim = format!(
        r#"printf '%s' "$4" > {uf}; printf '%s' "$8" > {bf}; echo "{{\"result\":{{\"state\":{{\"state\":\"completed\"}},\"artifacts\":[{{\"parts\":[{{\"type\":\"text\",\"text\":\"ok\"}}]}}]}}}}""#,
        uf = url_file,
        bf = body_file
    );
    let worker = A2aWorker::with_command(
        Duration::from_secs(5),
        Duration::from_secs(2),
        "sh".to_string(),
        vec!["-c".to_string(), shim],
    );

    let result = worker
        .run(
            "node1",
            "test-prompt-t8",
            "https://agent.example#research",
            1,
        )
        .await
        .expect("worker must return WorkerOutput, not Err");

    assert!(result.success, "shim should return completed: {:?}", result);
    assert_eq!(result.output, "ok");

    // Verify the captured URL does NOT contain the #research fragment.
    let captured_url =
        std::fs::read_to_string(url_file).expect("url file should exist after shim runs");
    assert_eq!(
        captured_url, "https://agent.example/v1/tasks/send",
        "endpoint URL must not contain the #research fragment"
    );

    // Verify the captured body contains the skillId and prompt.
    let captured_body =
        std::fs::read_to_string(body_file).expect("body file should exist after shim runs");
    let body_json: serde_json::Value =
        serde_json::from_str(&captured_body).expect("captured body must be valid JSON");

    assert_eq!(
        body_json["params"]["skillId"].as_str(),
        Some("research"),
        "skillId must be extracted from the #research fragment"
    );
    assert_eq!(
        body_json["params"]["message"]["parts"][0]["text"].as_str(),
        Some("test-prompt-t8"),
        "message text must be the node prompt"
    );
    assert_eq!(
        body_json["method"].as_str(),
        Some("tasks/send"),
        "method must be tasks/send"
    );

    // Clean up.
    let _ = std::fs::remove_file(body_file);
    let _ = std::fs::remove_file(url_file);
}
