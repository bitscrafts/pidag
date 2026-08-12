//! [`A2aWorker`] -- A2A protocol worker that dispatches DAG `llm` nodes to
//! any A2A-compliant remote agent (Gemini, Claude, Hermes, Orki, ...) via the
//! `tasks/send` JSON-RPC protocol, using `curl` as the HTTP client.
//!
//! # Routing
//!
//! When a `ModelRef.name` starts with `http://` or `https://`,
//! [`crate::worker::TypeDispatchWorker`] sends the node here instead of
//! [`crate::worker::PiPrintWorker`]. The URL fragment (`#skill`) optionally selects
//! an A2A skill via the `skillId` JSON-RPC param.
//!
//! # Design goals
//!
//! - No new Rust dependencies -- `curl` is a system binary shelled out via
//!   `tokio::process::Command`; `serde_json` is already a crate dep.
//! - Reuses the existing [`crate::worker::classify_retryable`] helper so 429/503/quota
//!   HTTP failures flow through the same scheduler 429-failover path as
//!   [`crate::worker::PiPrintWorker`].
//! - `Send + Sync` (required by the [`crate::worker::Worker`] trait): all fields are
//!   `HashMap<String, String>`, `Duration`, `String`, `Vec<String>`.
//!
//! # Architecture
//!
//! [`A2aWorker::run`] POSTs a `tasks/send` JSON-RPC body to
//! `<endpoint>/v1/tasks/send`. If the response `result.state.state` is
//! `"working"`, it polls `tasks/get` with `result.id` every `poll_interval`
//! until terminal (`completed`/`failed`) or the overall `timeout` elapses.

use crate::core::dag::Dag;
use crate::core::error::PidagError;
use crate::worker::{Worker, WorkerOutput, classify_retryable};
use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Returns `true` when `model` names an A2A endpoint -- i.e. starts with
/// `http://` or `https://` (case-sensitive). This is the routing predicate
/// used by [`crate::worker::TypeDispatchWorker`]: a URL model is dispatched to
/// [`A2aWorker`], anything else to [`crate::worker::PiPrintWorker`].
///
/// # Examples
///
/// ```
/// # use pidag::is_a2a_endpoint;
/// assert!(is_a2a_endpoint("https://${DEPLOY_HOST_NAME}:7422/agents/hermes"));
/// assert!(is_a2a_endpoint("http://localhost:8080"));
/// assert!(!is_a2a_endpoint("nvidia/z-ai/glm-5.2"));
/// ```
pub fn is_a2a_endpoint(model: &str) -> bool {
    model.starts_with("http://") || model.starts_with("https://")
}

/// Split a `ModelRef.name` URL into `(endpoint, skill_id_option)`.
/// `endpoint` is the URL with any `#fragment` stripped. `skill_id` is the
/// fragment (without the `#`) when present and non-empty, `None` otherwise.
/// For non-URL inputs (no `#`), the whole string is returned as the
/// endpoint and `skill_id` is `None`.
fn split_endpoint_and_skill(model: &str) -> (String, Option<String>) {
    match model.split_once('#') {
        Some((base, frag)) if !frag.is_empty() => (base.to_string(), Some(frag.to_string())),
        _ => (model.to_string(), None),
    }
}

/// Extract the first text `Part` from a `tasks/send` / `tasks/get` response
/// `result.artifacts` array. Returns the `text` field of
/// `artifacts[0].parts[0]` when present, `None` otherwise. Used by
/// [`A2aWorker`] on the `completed` state.
fn extract_text_part(artifacts: &serde_json::Value) -> Option<String> {
    let first_artifact = artifacts.as_array()?.first()?;
    let parts = first_artifact.get("parts")?.as_array()?;
    let first_part = parts.first()?;
    first_part
        .get("type")
        .and_then(|t| t.as_str())
        .filter(|t| *t == "text")?;
    first_part
        .get("text")
        .and_then(|t| t.as_str())
        .map(str::to_string)
}

/// A2A protocol worker -- dispatches DAG `llm` nodes to any A2A-compliant
/// remote agent (Gemini, Claude, Hermes, Orki, ...) via the `tasks/send`
/// JSON-RPC protocol, using `curl` as the HTTP client.
///
/// Routing: when a `ModelRef.name` starts with `http://` or `https://`,
/// [`crate::worker::TypeDispatchWorker`] sends the node here instead of
/// [`crate::worker::PiPrintWorker`]. The URL fragment (`#skill`) optionally selects
/// an A2A skill.
///
/// See `specs/97-a2a-worker.md` for the full spec.
pub struct A2aWorker {
    timeout: Duration,
    poll_interval: Duration,
    program: String,
    extra_args: Vec<String>,
}

impl A2aWorker {
    /// Real worker: spawns `curl` with a 2s default poll interval.
    pub fn new(_dag: &Dag, timeout: Duration, poll_interval: Duration) -> Self {
        Self::with_command(timeout, poll_interval, "curl".to_string(), Vec::new())
    }

    /// Worker with an overridden program/leading args, for test shims.
    ///
    /// Mirrors [`crate::worker::PiPrintWorker::with_command`]: `program` + `extra_args`
    /// replace `curl` so tests can point at a harmless shim (e.g.
    /// `sh -c 'echo ...'`) instead of a real `curl` binary, keeping the
    /// unit suite offline and deterministic. The `run()` method appends
    /// the per-request args (`-sS -f -X POST <url> -H ... -d <body>`) after
    /// `extra_args`.
    pub fn with_command(
        timeout: Duration,
        poll_interval: Duration,
        program: String,
        extra_args: Vec<String>,
    ) -> Self {
        Self {
            timeout,
            poll_interval,
            program,
            extra_args,
        }
    }

    /// Spawn the configured program with the per-request curl-style args,
    /// apply the overall `self.timeout`, and return `(stdout, stderr,
    /// exit_status_success)`. On spawn failure or timeout, returns
    /// `Err(WorkerOutput)` so the caller can short-circuit with the
    /// appropriate error message.
    async fn run_curl(
        &self,
        url: &str,
        body_str: &str,
        start: std::time::Instant,
    ) -> Result<(String, String, bool), WorkerOutput> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.extra_args);
        cmd.arg("-sS");
        cmd.arg("-f");
        cmd.arg("-X");
        cmd.arg("POST");
        cmd.arg(url);
        cmd.arg("-H");
        cmd.arg("content-type: application/json");
        cmd.arg("-d");
        cmd.arg(body_str);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                return Err(WorkerOutput {
                    success: false,
                    output: format!("spawn failed: {err}"),
                    retryable: false,
                });
            }
        };

        // Remaining time budget for this curl call. If the overall
        // timeout has already elapsed (shouldn't normally happen here on
        // the first call, but can during the poll loop), bail out.
        let remaining = self
            .timeout
            .checked_sub(start.elapsed())
            .unwrap_or(Duration::ZERO);
        if remaining.is_zero() {
            return Err(WorkerOutput {
                success: false,
                output: "a2a poll timed out".to_string(),
                retryable: false,
            });
        }

        let awaited = tokio::time::timeout(remaining, child.wait_with_output()).await;
        let output = match awaited {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                return Err(WorkerOutput {
                    success: false,
                    output: format!("process error: {err}"),
                    retryable: false,
                });
            }
            Err(_elapsed) => {
                return Err(WorkerOutput {
                    success: false,
                    output: "a2a request timed out".to_string(),
                    retryable: false,
                });
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Ok((stdout, stderr, output.status.success()))
    }
}

#[async_trait]
impl Worker for A2aWorker {
    async fn run(
        &self,
        _node_id: &str,
        prompt: &str,
        model: &str,
        _attempt: usize,
    ) -> Result<WorkerOutput, PidagError> {
        let (endpoint, skill_id) = split_endpoint_and_skill(model);
        let start = std::time::Instant::now();

        // Build the tasks/send JSON-RPC body. serde_json guarantees prompt
        // text is properly JSON-escaped (no injection via prompt content).
        let mut body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tasks/send",
            "params": {
                "message": {
                    "role": "user",
                    "parts": [{"type": "text", "text": prompt}]
                }
            }
        });
        if let Some(skill) = &skill_id {
            body["params"]["skillId"] = serde_json::Value::String(skill.clone());
        }
        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let send_url = format!("{endpoint}/v1/tasks/send");

        let (stdout, stderr, ok) = match self.run_curl(&send_url, &body_str, start).await {
            Ok(triple) => triple,
            Err(wo) => return Ok(wo),
        };

        if !ok {
            let combined = format!("{stdout}\n{stderr}");
            let retryable = classify_retryable(&combined);
            return Ok(WorkerOutput {
                success: false,
                output: combined,
                retryable,
            });
        }

        let result: serde_json::Value = match serde_json::from_str(&stdout) {
            Ok(v) => v,
            Err(err) => {
                return Ok(WorkerOutput {
                    success: false,
                    output: format!("failed to parse a2a response: {err}"),
                    retryable: false,
                });
            }
        };

        // The A2A JSON-RPC response wraps the task state under `result`.
        let task_obj = result.get("result").unwrap_or(&result);
        let state = task_obj
            .get("state")
            .and_then(|s| s.get("state"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let task_id = task_obj
            .get("id")
            .and_then(|i| i.as_str())
            .map(str::to_string);
        let artifacts = task_obj.get("artifacts");

        match state {
            "completed" => {
                let text = artifacts
                    .and_then(extract_text_part)
                    .unwrap_or_else(|| "(no artifacts)".to_string());
                Ok(WorkerOutput {
                    success: true,
                    output: text,
                    retryable: false,
                })
            }
            "failed" => Ok(WorkerOutput {
                success: false,
                output: "a2a task failed".to_string(),
                retryable: false,
            }),
            "working" => {
                let task_id = match task_id {
                    Some(id) => id,
                    None => {
                        return Ok(WorkerOutput {
                            success: false,
                            output: "a2a working state without task id".to_string(),
                            retryable: false,
                        });
                    }
                };
                let get_body = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "tasks/get",
                    "params": {"id": task_id}
                });
                let get_body_str = serde_json::to_string(&get_body).unwrap_or_default();
                let get_url = format!("{endpoint}/v1/tasks/get");

                loop {
                    if start.elapsed() >= self.timeout {
                        return Ok(WorkerOutput {
                            success: false,
                            output: "a2a poll timed out".to_string(),
                            retryable: false,
                        });
                    }
                    tokio::time::sleep(self.poll_interval).await;

                    let (p_stdout, p_stderr, p_ok) =
                        match self.run_curl(&get_url, &get_body_str, start).await {
                            Ok(triple) => triple,
                            Err(wo) => return Ok(wo),
                        };

                    if !p_ok {
                        let combined = format!("{p_stdout}\n{p_stderr}");
                        let retryable = classify_retryable(&combined);
                        return Ok(WorkerOutput {
                            success: false,
                            output: combined,
                            retryable,
                        });
                    }

                    let p_result: serde_json::Value = match serde_json::from_str(&p_stdout) {
                        Ok(v) => v,
                        Err(err) => {
                            return Ok(WorkerOutput {
                                success: false,
                                output: format!("failed to parse a2a poll response: {err}"),
                                retryable: false,
                            });
                        }
                    };

                    let p_task = p_result.get("result").unwrap_or(&p_result);
                    let p_state = p_task
                        .get("state")
                        .and_then(|s| s.get("state"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    let p_artifacts = p_task.get("artifacts");

                    match p_state {
                        "completed" => {
                            let text = p_artifacts
                                .and_then(extract_text_part)
                                .unwrap_or_else(|| "(no artifacts)".to_string());
                            return Ok(WorkerOutput {
                                success: true,
                                output: text,
                                retryable: false,
                            });
                        }
                        "failed" => {
                            return Ok(WorkerOutput {
                                success: false,
                                output: "a2a task failed".to_string(),
                                retryable: false,
                            });
                        }
                        "working" => {
                            // keep polling
                            continue;
                        }
                        other => {
                            return Ok(WorkerOutput {
                                success: false,
                                output: format!("unknown a2a state: {other}"),
                                retryable: false,
                            });
                        }
                    }
                }
            }
            other => Ok(WorkerOutput {
                success: false,
                output: format!("unknown a2a state: {other}"),
                retryable: false,
            }),
        }
    }
}
