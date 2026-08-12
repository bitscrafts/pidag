//! [`ShellWorker`] and [`RealShellWorker`] — `bash -c` execution for
//! `node_type == "shell"` DAG nodes.

use super::{Worker, WorkerOutput};
use crate::core::dag::Dag;
use crate::core::error::PidagError;
use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Shell worker: base struct for shell command execution.
/// Note: This worker alone cannot execute without the DAG node prompts.
/// Use `RealShellWorker` instead, which carries the prompt lookup.
#[allow(dead_code)]
pub struct ShellWorker {
    timeout: Duration,
}

impl ShellWorker {
    #[allow(dead_code)]
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

/// Real shell worker: executes bash commands from DAG node prompts.
/// The prompt is passed directly by the scheduler, not looked up from a stored map.
pub struct RealShellWorker {
    timeout: Duration,
}

impl RealShellWorker {
    pub fn new(_dag: &Dag, timeout: Duration) -> Self {
        Self { timeout }
    }
}

#[async_trait]
impl Worker for RealShellWorker {
    async fn run(
        &self,
        _node_id: &str,
        prompt: &str,
        _model: &str,
        _attempt: usize,
    ) -> Result<WorkerOutput, PidagError> {
        let mut cmd = Command::new("bash");
        cmd.arg("-c");
        cmd.arg(prompt);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                return Ok(WorkerOutput {
                    success: false,
                    output: format!("spawn failed: {err}"),
                    retryable: false,
                });
            }
        };

        let awaited = tokio::time::timeout(self.timeout, child.wait_with_output()).await;

        let output = match awaited {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                return Ok(WorkerOutput {
                    success: false,
                    output: format!("process error: {err}"),
                    retryable: false,
                });
            }
            Err(_elapsed) => {
                return Ok(WorkerOutput {
                    success: false,
                    output: "timed out waiting for shell command".to_string(),
                    retryable: false,
                });
            }
        };

        let success = output.status.success();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let combined_output = if stderr.is_empty() {
            stdout
        } else if stdout.is_empty() {
            stderr
        } else {
            format!("{}\n{}", stdout, stderr)
        };

        Ok(WorkerOutput {
            success,
            output: combined_output,
            retryable: false,
        })
    }
}
