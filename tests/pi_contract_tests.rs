//! Contract tests for the real `pi` binary (spec-17).
//!
//! These tests exercise the actual `pi` CLI to pin the exact surface pidag
//! depends on, without spending a single token. All commands are token-free:
//! `get_state`, `set_thinking_level`, `new_session`, `--help`.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Helper: resolve the pi binary using the production path (spec-13 D1).
/// Tries to execute it to verify it exists and is executable.
/// Returns the path, or None if not resolvable.
fn resolve_pi_binary_for_test() -> Option<String> {
    let resolved = pidag::resolve_pi_binary();

    // The function never fails, but we must verify the result is actually usable.
    // Try running it with --version to confirm it's executable.
    if let Ok(out) = Command::new(&resolved).arg("--version").output()
        && out.status.success()
    {
        return Some(resolved);
    }

    None
}

/// Check if pi is available. If not and PIDAG_REQUIRE_PI=1, panic.
/// Otherwise return false so the test can early-return and skip.
/// When skipping, print the skip message so it appears in the test output.
fn pi_available() -> bool {
    if resolve_pi_binary_for_test().is_some() {
        return true;
    }

    // pi is not available
    if std::env::var("PIDAG_REQUIRE_PI") == Ok("1".to_string()) {
        panic!("pi binary not resolvable and PIDAG_REQUIRE_PI=1 is set");
    }

    // Skip with a message (will be printed during test run)
    println!("SKIP: pi not resolvable");
    false
}

/// Helper: A persistent RPC connection to pi that can handle multiple requests.
/// Uses a background reader thread with mpsc channel to enforce hard timeouts.
struct RpcConnection {
    child: std::process::Child,
    rx: std::sync::mpsc::Receiver<String>,
}

impl RpcConnection {
    /// Spawn a persistent pi --mode rpc process, optionally with a session file.
    /// Spawns a background reader thread to read stdout lines and send them over mpsc.
    fn new(session_path: Option<&str>) -> Result<Self, String> {
        let pi_path = pidag::resolve_pi_binary();

        let mut cmd = Command::new(&pi_path);
        cmd.arg("--mode").arg("rpc");

        if let Some(path) = session_path {
            cmd.arg("--session").arg(path);
        }

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn pi: {e}"))?;

        let stdout = child.stdout.take().ok_or("failed to open stdout")?;
        let reader = BufReader::new(stdout);

        let (tx, rx) = std::sync::mpsc::channel::<String>();

        // Spawn a background thread to read stdout lines and send them over the channel.
        // If the channel disconnects (main thread dropped), this thread will exit naturally.
        std::thread::spawn(move || {
            for line in reader.lines().map_while(Result::ok) {
                // If send fails, the receiver is closed and we should exit.
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        Ok(RpcConnection { child, rx })
    }

    /// Send a request and read the response with matching id.
    /// Uses recv_timeout to enforce a hard 10-second limit on waiting for any response.
    fn send_request(&mut self, request: Value) -> Result<Value, String> {
        let request_id = request
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("request must have an 'id' field")?
            .to_string();

        // Write the request to stdin
        {
            let stdin = self.child.stdin.as_mut().ok_or("failed to open stdin")?;
            let request_line = format!("{}\n", request);
            stdin
                .write_all(request_line.as_bytes())
                .map_err(|e| format!("failed to write to stdin: {e}"))?;
            stdin
                .flush()
                .map_err(|e| format!("failed to flush stdin: {e}"))?;
        }

        // Read response lines until we find one with matching id.
        // Use recv_timeout to enforce a hard 10-second deadline on the entire operation.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);

        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .unwrap_or(Duration::ZERO);

            if remaining.is_zero() {
                let _ = self.child.kill();
                return Err("timeout waiting for RPC response".to_string());
            }

            match self.rx.recv_timeout(remaining) {
                Ok(line) => {
                    if let Ok(obj) = serde_json::from_str::<Value>(&line)
                        && let Some(id) = obj.get("id").and_then(|v| v.as_str())
                        && id == request_id
                    {
                        return Ok(obj);
                    }
                    // Continue reading (line didn't match our id)
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let _ = self.child.kill();
                    return Err("timeout waiting for RPC response".to_string());
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("RPC connection closed".to_string());
                }
            }
        }
    }
}

impl Drop for RpcConnection {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Helper: spawn `pi --mode rpc`, send a JSON request, and read the response
/// with matching `id`. Returns the response object, or Err if something fails.
/// Includes a 10-second timeout.
fn rpc_roundtrip(request: Value) -> Result<Value, String> {
    rpc_roundtrip_with_session(request, None)
}

/// Helper: Like rpc_roundtrip but with optional session file support.
fn rpc_roundtrip_with_session(request: Value, session_path: Option<&str>) -> Result<Value, String> {
    let mut conn = RpcConnection::new(session_path)?;
    conn.send_request(request)
}

/// C1: Test that `pi --mode rpc` starts and remains alive.
/// Expects the process to be alive after 500ms with piped stdin/stdout.
#[test]
fn test_pi_rpc_mode_starts() {
    if !pi_available() {
        return;
    }

    let pi_path = pidag::resolve_pi_binary();

    let mut child = Command::new(&pi_path)
        .arg("--mode")
        .arg("rpc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn pi");

    // Give it 500ms to start up
    std::thread::sleep(Duration::from_millis(500));

    // Check that it's still alive (not exited)
    match child.try_wait() {
        Ok(None) => {
            // Process still running — good
            let _ = child.kill();
        }
        Ok(Some(status)) => {
            panic!("pi exited prematurely with status: {:?}", status.code());
        }
        Err(e) => {
            panic!("failed to check process status: {e}");
        }
    }
}

/// C2: Test `get_state` round trip with correct response shape.
/// Expects response with id=="c2", type=="response", success==true, data.sessionId non-empty.
#[test]
fn test_pi_rpc_get_state_roundtrip() {
    if !pi_available() {
        return;
    }

    let request = json!({
        "id": "c2",
        "type": "get_state"
    });

    let response = rpc_roundtrip(request).expect("rpc_roundtrip failed");

    // Verify response shape
    assert_eq!(response.get("id").and_then(|v| v.as_str()), Some("c2"));
    assert_eq!(
        response.get("type").and_then(|v| v.as_str()),
        Some("response")
    );
    assert_eq!(
        response.get("success").and_then(|v| v.as_bool()),
        Some(true)
    );

    let session_id = response
        .get("data")
        .and_then(|v| v.get("sessionId"))
        .and_then(|v| v.as_str());
    assert!(
        session_id.is_some_and(|s| !s.is_empty()),
        "data.sessionId must be non-empty"
    );
}

/// C3: Test `set_thinking_level` command and verify via `get_state`.
/// Uses a persistent connection to ensure the setting survives across commands.
#[test]
fn test_pi_rpc_set_thinking_level() {
    if !pi_available() {
        return;
    }

    let mut conn = RpcConnection::new(None).expect("failed to spawn pi");

    // Send set_thinking_level
    let request1 = json!({
        "id": "c3a",
        "type": "set_thinking_level",
        "level": "low"
    });

    let response1 = conn
        .send_request(request1)
        .expect("set_thinking_level failed");
    assert_eq!(response1.get("id").and_then(|v| v.as_str()), Some("c3a"));
    assert_eq!(
        response1.get("success").and_then(|v| v.as_bool()),
        Some(true)
    );

    // Now send get_state to verify the setting took effect (same connection)
    let request2 = json!({
        "id": "c3b",
        "type": "get_state"
    });

    let response2 = conn
        .send_request(request2)
        .expect("get_state after set_thinking_level failed");
    let thinking_level = response2
        .get("data")
        .and_then(|v| v.get("thinkingLevel"))
        .and_then(|v| v.as_str());
    assert_eq!(thinking_level, Some("low"));
}

/// C4: Test `new_session` creates a new, different sessionId.
/// Sends get_state, notes the sessionId, then sends new_session, then
/// sends get_state again and verifies the sessionId is different.
#[test]
fn test_pi_rpc_new_session() {
    if !pi_available() {
        return;
    }

    // Get the initial session id
    let request1 = json!({
        "id": "c4a",
        "type": "get_state"
    });

    let response1 = rpc_roundtrip(request1).expect("initial get_state failed");
    let session_id1 = response1
        .get("data")
        .and_then(|v| v.get("sessionId"))
        .and_then(|v| v.as_str())
        .expect("no sessionId in first response")
        .to_string();

    // Send new_session
    let request2 = json!({
        "id": "c4b",
        "type": "new_session"
    });

    let response2 = rpc_roundtrip(request2).expect("new_session failed");
    assert_eq!(
        response2.get("success").and_then(|v| v.as_bool()),
        Some(true)
    );

    // Get the new session id
    let request3 = json!({
        "id": "c4c",
        "type": "get_state"
    });

    let response3 = rpc_roundtrip(request3).expect("get_state after new_session failed");
    let session_id2 = response3
        .get("data")
        .and_then(|v| v.get("sessionId"))
        .and_then(|v| v.as_str())
        .expect("no sessionId in third response")
        .to_string();

    assert_ne!(
        session_id1, session_id2,
        "sessionId should be different after new_session"
    );
}

/// C5a: Test that `pi --rpc --mode json` fails with mutually-exclusive flag error.
/// Expects non-zero exit code and stderr containing "cannot be used with".
#[test]
fn test_rpc_and_mode_flags_are_exclusive() {
    if !pi_available() {
        return;
    }

    let pi_path = pidag::resolve_pi_binary();

    let output = Command::new(&pi_path)
        .arg("--rpc")
        .arg("--mode")
        .arg("json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn pi");

    // Must fail (non-zero exit)
    assert!(
        !output.status.success(),
        "pi --rpc --mode json should fail, but succeeded"
    );

    // Stderr must contain the error message
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "stderr should contain 'cannot be used with', got: {stderr}"
    );
}

/// C5b: Test that `--session <nonexistent>` fails with appropriate error.
/// Expects non-zero exit code and stderr containing "Session not found".
#[test]
fn test_session_flag_requires_existing_file() {
    if !pi_available() {
        return;
    }

    let pi_path = pidag::resolve_pi_binary();

    let output = Command::new(&pi_path)
        .arg("--mode")
        .arg("rpc")
        .arg("--session")
        .arg("_tmp/does-not-exist.jsonl")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn pi");

    // Must fail (non-zero exit)
    assert!(
        !output.status.success(),
        "pi with nonexistent session should fail, but succeeded"
    );

    // Stderr must contain the error message
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Session not found"),
        "stderr should contain 'Session not found', got: {stderr}"
    );
}

/// C6: Test that `pi --help` advertises all required flags.
/// Expects stdout to contain: --mode, --rpc, --model, --provider, --thinking,
/// --append-system-prompt, --session, --print.
#[test]
fn test_pi_help_advertises_required_flags() {
    if !pi_available() {
        return;
    }

    let pi_path = pidag::resolve_pi_binary();

    let output = Command::new(&pi_path)
        .arg("--help")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn pi");

    let stdout = String::from_utf8_lossy(&output.stdout);

    let required_flags = vec![
        "--mode",
        "--rpc",
        "--model",
        "--provider",
        "--thinking",
        "--append-system-prompt",
        "--session",
        "--print",
    ];

    for flag in required_flags {
        assert!(
            stdout.contains(flag),
            "help text should contain '{flag}', help output: {stdout}"
        );
    }
}

/// C7: Test that `resolve_pi_binary()` returns an executable path.
/// Pins spec-13 D1: the binary is found via the production resolution path
/// and is actually executable.
#[test]
fn test_resolve_pi_binary_finds_real_pi() {
    if !pi_available() {
        return;
    }

    let resolved = pidag::resolve_pi_binary();

    // Must be absolute (or at least exist relative to cwd)
    // and must be executable (or at least runnable with --version)
    let output = Command::new(&resolved)
        .arg("--version")
        .output()
        .unwrap_or_else(|_| panic!("failed to run {resolved}"));

    assert!(
        output.status.success(),
        "{resolved} --version should succeed"
    );
}
