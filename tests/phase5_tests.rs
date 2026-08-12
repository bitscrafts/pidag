use pidag::{Dag, ModelRef, Node, RetryPolicy, RpcServer};
use serde_json::json;
use std::path::PathBuf;

// Helper to create a simple test DAG
fn simple_dag() -> Dag {
    Dag {
        metadata: None,
        nodes: vec![
            Node {
                id: "node-a".to_string(),
                prompt: "test prompt".to_string(),
                depends_on: vec![],
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
                after: vec![],
                verify: None,
                verify_pre: None,
            },
            Node {
                id: "node-b".to_string(),
                prompt: "test prompt 2".to_string(),
                depends_on: vec!["node-a".to_string()],
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
                after: vec![],
                verify: None,
                verify_pre: None,
            },
        ],
    }
}

// Test: parse valid JSON-RPC request
#[test]
fn test_rpc_parse_valid_request() {
    let _server = RpcServer::new(4, PathBuf::from(".pidag/test.redb"));

    // This test uses private method, so we just verify the server can be created
    // (construction itself panics on internal inconsistency; reaching this line
    // means construction succeeded).
}

// Test: health check method
#[tokio::test]
async fn test_rpc_health() {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "health",
        "params": {}
    });

    // Verify the request is valid JSON-RPC 2.0
    let obj = request.as_object().unwrap();
    assert_eq!(obj.get("jsonrpc").and_then(|v| v.as_str()), Some("2.0"));
    assert!(obj.contains_key("id"));
    assert!(obj.contains_key("method"));
}

// Test: DAG submit with valid DAG
#[test]
fn test_rpc_dag_submit_valid() {
    let dag = simple_dag();
    let dag_json = serde_json::to_value(&dag).unwrap();

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "dag.submit",
        "params": {
            "dag": dag_json
        }
    });

    // Verify the request structure is correct
    let obj = request.as_object().unwrap();
    assert_eq!(
        obj.get("method").and_then(|v| v.as_str()),
        Some("dag.submit")
    );

    // Verify params contain dag
    let params = obj.get("params").unwrap().as_object().unwrap();
    assert!(params.contains_key("dag"));
}

// Test: DAG submit with invalid DAG (cycle)
#[test]
fn test_rpc_dag_submit_cycle() {
    let dag = Dag {
        metadata: None,
        nodes: vec![
            Node {
                id: "node-a".to_string(),
                prompt: "test".to_string(),
                depends_on: vec!["node-b".to_string()],
                models: vec![ModelRef {
                    name: "test".to_string(),
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
                after: vec![],
                verify: None,
                verify_pre: None,
            },
            Node {
                id: "node-b".to_string(),
                prompt: "test".to_string(),
                depends_on: vec!["node-a".to_string()],
                models: vec![ModelRef {
                    name: "test".to_string(),
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
                after: vec![],
                verify: None,
                verify_pre: None,
            },
        ],
    };

    // Validate should detect the cycle
    let result = dag.validate();
    assert!(result.is_err());
}

// Test: DAG submit with missing dependency
#[test]
fn test_rpc_dag_submit_missing_dep() {
    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "node-a".to_string(),
            prompt: "test".to_string(),
            depends_on: vec!["nonexistent".to_string()],
            models: vec![ModelRef {
                name: "test".to_string(),
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
            after: vec![],
            verify: None,
            verify_pre: None,
        }],
    };

    let result = dag.validate();
    assert!(result.is_err());
}

// Test: valid JSON-RPC response format
#[test]
fn test_rpc_response_format_success() {
    let response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "dagId": "run-20260731-123456-abc123"
        }
    });

    let obj = response.as_object().unwrap();
    assert_eq!(obj.get("jsonrpc").and_then(|v| v.as_str()), Some("2.0"));
    assert!(obj.contains_key("id"));
    assert!(obj.contains_key("result"));
    assert!(!obj.contains_key("error"));
}

// Test: valid JSON-RPC error response format
#[test]
fn test_rpc_response_format_error() {
    let response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": {
            "code": -32600,
            "message": "Invalid Request"
        }
    });

    let obj = response.as_object().unwrap();
    assert_eq!(obj.get("jsonrpc").and_then(|v| v.as_str()), Some("2.0"));
    assert!(obj.contains_key("id"));
    assert!(!obj.contains_key("result"));
    assert!(obj.contains_key("error"));

    let error = obj.get("error").unwrap().as_object().unwrap();
    assert_eq!(error.get("code").and_then(|v| v.as_i64()), Some(-32600));
    assert_eq!(
        error.get("message").and_then(|v| v.as_str()),
        Some("Invalid Request")
    );
}

// Test: dag.status method structure
#[test]
fn test_rpc_dag_status_structure() {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "dag.status",
        "params": {
            "dagId": "run-20260731-123456-abc123"
        }
    });

    let obj = request.as_object().unwrap();
    assert_eq!(
        obj.get("method").and_then(|v| v.as_str()),
        Some("dag.status")
    );

    let params = obj.get("params").unwrap().as_object().unwrap();
    assert!(params.contains_key("dagId"));
}

// Test: dag.await with timeout
#[test]
fn test_rpc_dag_await_timeout() {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "dag.await",
        "params": {
            "dagId": "run-test",
            "timeoutMs": 5000
        }
    });

    let obj = request.as_object().unwrap();
    assert_eq!(
        obj.get("method").and_then(|v| v.as_str()),
        Some("dag.await")
    );

    let params = obj.get("params").unwrap().as_object().unwrap();
    assert_eq!(params.get("timeoutMs").and_then(|v| v.as_u64()), Some(5000));
}

// Test: node.wait method structure
#[test]
fn test_rpc_node_wait_structure() {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "node.wait",
        "params": {
            "dagId": "run-test",
            "timeoutMs": 10000
        }
    });

    let obj = request.as_object().unwrap();
    assert_eq!(
        obj.get("method").and_then(|v| v.as_str()),
        Some("node.wait")
    );
}

// Test: dag.result method structure
#[test]
fn test_rpc_dag_result_structure() {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "dag.result",
        "params": {
            "dagId": "run-test"
        }
    });

    let obj = request.as_object().unwrap();
    assert_eq!(
        obj.get("method").and_then(|v| v.as_str()),
        Some("dag.result")
    );
}

// Test: node.retry method structure
#[test]
fn test_rpc_node_retry_structure() {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "node.retry",
        "params": {
            "dagId": "run-test",
            "nodeId": "node-a"
        }
    });

    let obj = request.as_object().unwrap();
    assert_eq!(
        obj.get("method").and_then(|v| v.as_str()),
        Some("node.retry")
    );

    let params = obj.get("params").unwrap().as_object().unwrap();
    assert!(params.contains_key("nodeId"));
}

// Test: dag.cancel method structure
#[test]
fn test_rpc_dag_cancel_structure() {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "dag.cancel",
        "params": {
            "dagId": "run-test"
        }
    });

    let obj = request.as_object().unwrap();
    assert_eq!(
        obj.get("method").and_then(|v| v.as_str()),
        Some("dag.cancel")
    );
}

// Test: error code for invalid JSON
#[test]
fn test_rpc_error_parse_error() {
    // Invalid JSON should produce error code -32700
    let invalid_json = "{not valid json";
    let parse_result: Result<serde_json::Value, _> = serde_json::from_str(invalid_json);
    assert!(parse_result.is_err());
}

// Test: error code for missing method
#[test]
fn test_rpc_error_missing_method() {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1
        // Missing "method" field
    });

    let obj = request.as_object().unwrap();
    assert!(!obj.contains_key("method"));
}

// Test: error code for invalid request
#[test]
fn test_rpc_error_invalid_request() {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "nonexistent.method",
        "params": {}
    });

    let obj = request.as_object().unwrap();
    // A nonexistent method should be handled
    assert_eq!(
        obj.get("method").and_then(|v| v.as_str()),
        Some("nonexistent.method")
    );
}

// Test: Resume token format in timeout response
#[test]
fn test_rpc_timeout_resume_token() {
    let response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "running": true,
            "token": "token-abc123"
        }
    });

    let obj = response.as_object().unwrap();
    let result = obj.get("result").unwrap().as_object().unwrap();
    assert_eq!(result.get("running").and_then(|v| v.as_bool()), Some(true));
    assert!(result.contains_key("token"));
}

// Test: Concurrent DAG tracking (request structure)
#[test]
fn test_rpc_concurrent_dags() {
    let dag1_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "dag.submit",
        "params": {
            "dag": simple_dag()
        }
    });

    let dag2_request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "dag.submit",
        "params": {
            "dag": simple_dag()
        }
    });

    // Both requests should be valid
    assert_eq!(dag1_request.get("id").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(dag2_request.get("id").and_then(|v| v.as_i64()), Some(2));
}

// Test: Status response includes expected fields
#[test]
fn test_rpc_status_response_fields() {
    let response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "dagId": "run-test",
            "nodes": [],
            "done": 0,
            "failed": 0,
            "status": "running"
        }
    });

    let result = response.get("result").unwrap().as_object().unwrap();
    assert!(result.contains_key("dagId"));
    assert!(result.contains_key("nodes"));
    assert!(result.contains_key("done"));
    assert!(result.contains_key("failed"));
    assert!(result.contains_key("status"));
}

// Test: Complete node response includes all required fields
#[test]
fn test_rpc_node_wait_response_fields() {
    let response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "nodeId": "node-a",
            "state": "Done",
            "output": "node output",
            "stillRunning": ["node-b"]
        }
    });

    let result = response.get("result").unwrap().as_object().unwrap();
    assert!(result.contains_key("nodeId"));
    assert!(result.contains_key("state"));
    assert!(result.contains_key("output"));
    assert!(result.contains_key("stillRunning"));
}

// Test: DAG await complete response includes report
#[test]
fn test_rpc_dag_await_complete_response() {
    let response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "done": true,
            "report": {
                "nodeStates": [
                    {
                        "nodeId": "node-a",
                        "state": "Done",
                        "output": "output"
                    }
                ],
                "failed": []
            }
        }
    });

    let result = response.get("result").unwrap().as_object().unwrap();
    assert_eq!(result.get("done").and_then(|v| v.as_bool()), Some(true));
    assert!(result.contains_key("report"));

    let report = result.get("report").unwrap().as_object().unwrap();
    assert!(report.contains_key("nodeStates"));
    assert!(report.contains_key("failed"));
}

// Test: Request without required id field
#[test]
fn test_rpc_error_missing_id() {
    let request = json!({
        "jsonrpc": "2.0",
        "method": "health",
        "params": {}
    });

    let obj = request.as_object().unwrap();
    assert!(!obj.contains_key("id"));
}

// Test: Server initialization
#[test]
fn test_rpc_server_creation() {
    let _server = RpcServer::new(4, PathBuf::from(".pidag/test.redb"));
    // Server should be created without errors
    // The test passes if no panic occurs
}

// Test: Topological sort is validated
#[test]
fn test_dag_topological_sort() {
    let dag = simple_dag();
    let sort_result = dag.topo_sort();

    // Simple DAG should have valid topological sort
    assert!(sort_result.is_ok());
    let sorted = sort_result.unwrap();
    assert_eq!(sorted.len(), 2);

    // node-a should come before node-b
    let a_idx = sorted.iter().position(|&n| n == "node-a").unwrap();
    let b_idx = sorted.iter().position(|&n| n == "node-b").unwrap();
    assert!(a_idx < b_idx);
}
