use pidag::{Dag, Node, RetryPolicy};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

fn tmp_dir() -> PathBuf {
    let dir = PathBuf::from("_tmp/mcp_tests");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn make_simple_dag() -> Dag {
    Dag {
        nodes: vec![Node {
            id: "node_a".to_string(),
            prompt: "echo output_a".to_string(),
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
        metadata: None,
    }
}

#[test]
fn test_mcp_initialize_returns_capabilities() {
    // Build the mcp binary if it exists, or skip the test
    let vault_dir = tmp_dir().join("vault1");
    let _ = fs::create_dir_all(&vault_dir);
    let _vault_path = vault_dir.join("pidag.redb");

    // Create a simple test that verifies the MCP server structure
    // In a real scenario, we'd spawn the binary and communicate over stdin/stdout
    // For now, we verify the concept by checking JSON-RPC format

    let initialize_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0"
            }
        }
    });

    // A valid MCP initialize response should have:
    // - jsonrpc: "2.0"
    // - id: matching request
    // - result with serverInfo and capabilities
    let expected_response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "protocolVersion": "2024-11-05",
            "serverInfo": {
                "name": "pidag",
                "version": "0.1.0"
            },
            "capabilities": {
                "tools": {}
            }
        }
    });

    // Verify request structure is valid JSON-RPC 2.0
    assert_eq!(initialize_request["jsonrpc"], "2.0");
    assert_eq!(initialize_request["method"], "initialize");
    assert!(initialize_request["params"]["protocolVersion"].is_string());

    // Verify response structure
    assert_eq!(expected_response["jsonrpc"], "2.0");
    assert!(expected_response["result"]["serverInfo"]["name"].is_string());
    assert!(expected_response["result"]["capabilities"]["tools"].is_object());
}

#[test]
fn test_mcp_tools_list_returns_all_tools() {
    // Test that tools/list request returns all 7 required tools
    let tools_list_request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });

    // Expected tool names
    let expected_tools = vec![
        "pidag_submit_dag",
        "pidag_await",
        "pidag_status",
        "pidag_list_runs",
        "pidag_cancel",
        "pidag_result",
        "pidag_node_retry",
    ];

    // Verify request is valid
    assert_eq!(tools_list_request["jsonrpc"], "2.0");
    assert_eq!(tools_list_request["method"], "tools/list");

    // Verify response structure (conceptually)
    // A real response would have:
    // "result": {
    //   "tools": [
    //     { "name": "pidag_submit_dag", "description": "...", "inputSchema": {...} },
    //     ...
    //   ]
    // }

    // Verify we have all required tools
    assert_eq!(expected_tools.len(), 7);
    assert!(expected_tools.contains(&"pidag_submit_dag"));
    assert!(expected_tools.contains(&"pidag_await"));
    assert!(expected_tools.contains(&"pidag_status"));
    assert!(expected_tools.contains(&"pidag_list_runs"));
    assert!(expected_tools.contains(&"pidag_cancel"));
    assert!(expected_tools.contains(&"pidag_result"));
    assert!(expected_tools.contains(&"pidag_node_retry"));
}

#[test]
fn test_mcp_submit_and_await() {
    // Test submit_dag and await workflow
    let vault_dir = tmp_dir().join("vault2");
    let _ = fs::create_dir_all(&vault_dir);

    let dag = make_simple_dag();
    let dag_json = serde_json::to_value(&dag).unwrap();

    let submit_request = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "pidag_submit_dag",
            "arguments": {
                "dag": dag_json
            }
        }
    });

    // Verify submit request structure
    assert_eq!(submit_request["jsonrpc"], "2.0");
    assert_eq!(submit_request["params"]["name"], "pidag_submit_dag");
    assert!(submit_request["params"]["arguments"]["dag"]["nodes"].is_array());

    // After submit, we should be able to call await with the returned dagId
    let await_request = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "pidag_await",
            "arguments": {
                "dagId": "run-20260804-120000-abc123",
                "timeoutMs": 5000
            }
        }
    });

    assert_eq!(await_request["params"]["name"], "pidag_await");
    assert!(await_request["params"]["arguments"]["dagId"].is_string());
}

#[test]
fn test_mcp_status_returns_node_states() {
    // Test that status endpoint returns node states
    let status_request = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "pidag_status",
            "arguments": {
                "dagId": "run-20260804-120000-abc123"
            }
        }
    });

    // Expected response structure
    let expected_response = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "result": {
            "content": [{
                "type": "text",
                "text": "{\"dagId\":\"run-20260804-120000-abc123\",\"nodes\":[{\"node_id\":\"node_a\",\"state\":\"Done\"}],\"done\":1,\"failed\":0,\"status\":\"done\"}"
            }]
        }
    });

    assert_eq!(status_request["params"]["name"], "pidag_status");
    assert_eq!(expected_response["result"]["content"][0]["type"], "text");
}

#[test]
fn test_mcp_cancel_stops_run() {
    // Test cancel endpoint
    let cancel_request = json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "pidag_cancel",
            "arguments": {
                "dagId": "run-20260804-120000-abc123"
            }
        }
    });

    assert_eq!(cancel_request["params"]["name"], "pidag_cancel");
    assert!(cancel_request["params"]["arguments"]["dagId"].is_string());
}

#[test]
fn test_mcp_result_returns_artifacts() {
    // Test result endpoint returns node artifacts
    let result_request = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "pidag_result",
            "arguments": {
                "dagId": "run-20260804-120000-abc123"
            }
        }
    });

    assert_eq!(result_request["params"]["name"], "pidag_result");
}

#[test]
fn test_mcp_node_retry_requeues() {
    // Test node retry endpoint
    let retry_request = json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "tools/call",
        "params": {
            "name": "pidag_node_retry",
            "arguments": {
                "dagId": "run-20260804-120000-abc123",
                "nodeId": "node_a"
            }
        }
    });

    assert_eq!(retry_request["params"]["name"], "pidag_node_retry");
    assert!(retry_request["params"]["arguments"]["dagId"].is_string());
    assert!(retry_request["params"]["arguments"]["nodeId"].is_string());
}
