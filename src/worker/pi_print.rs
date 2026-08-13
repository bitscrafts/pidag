//! [`PiPrintWorker`] — shells out to the `pi` CLI in non-interactive print
//! mode, plus the [`parse_pi_envelope`] helper that extracts the assistant
//! reply from the `pi -p --mode json` transcript.

use super::{Worker, WorkerOutput, classify_retryable};
use crate::core::dag::Dag;
use crate::core::error::PidagError;
use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Internal anti-narration discipline appended to every worker `pi -p`
/// invocation (spec-14). Keeps the worker from drifting into text-only turns
/// under long context — a cross-model failure mode observed with both
/// deepseek-chat and glm-5.2 cloud. Self-contained: no external file coupling.
const WORKER_ANTI_LOOP_PROMPT: &str = "\nAct, don't narrate (worker anti-loop discipline): To make progress you MUST emit a concrete tool call. NEVER reply with only narration, placeholder text, or a bare intent statement with no tool call attached. Do not pause mid-task for permission on routine steps. If you cannot proceed, say so in ONE line and stop. A turn that produces text but no tool call is a failure.\n";

/// Real `Worker` implementation that shells out to the `pi` CLI in
/// non-interactive print mode: `pi -p --mode json`.
///
/// The prompt is passed directly by the scheduler at dispatch time.
///
/// The underlying program/args are overridable via [`Self::with_command`]
/// so tests can point at a harmless shim (e.g. `sh -c '...'`) instead of a
/// real `pi` binary, keeping the unit suite offline and deterministic.
pub struct PiPrintWorker {
    timeout: Duration,
    program: String,
    extra_args: Vec<String>,
}

impl PiPrintWorker {
    /// Real worker: spawns `pi -p --mode json` using an absolute path to the
    /// `pi` binary.
    ///
    /// The bare `pi` name is NOT enough: pidag runs `pi` in a subprocess, and
    /// that subprocess may not have `/root/.local/bin` (where `pi` lives) on
    /// its `PATH` — observed as `pi: command not found` (exit 127), which
    /// made every LLM node fail with "execution failed" regardless of model.
    /// Resolving the binary to an absolute path here makes the worker robust
    /// to the subprocess environment. (D1 fix, spec-13.)
    pub fn new(_dag: &Dag, timeout: Duration) -> Self {
        Self::with_command(timeout, resolve_pi_binary(), Vec::new())
    }

    /// Worker with an overridden program/leading args, for test shims.
    pub fn with_command(timeout: Duration, program: String, extra_args: Vec<String>) -> Self {
        Self {
            timeout,
            program,
            extra_args,
        }
    }
}

/// Resolve the `pi` binary to an absolute path so subprocess spawns can find
/// it even when the spawned environment's `PATH` is minimal.
///
/// Search order: env `PI_BINARY` (explicit override) → `which pi` → common
/// install locations (`/root/.local/bin/pi`, `~/.local/bin/pi`,
/// `/usr/local/bin/pi`, `/usr/bin/pi`). Falls back to `"pi"` so the old
/// behaviour is preserved when nothing is found (never panics).
pub fn resolve_pi_binary() -> String {
    if let Ok(explicit) = std::env::var("PI_BINARY")
        && !explicit.trim().is_empty()
    {
        return explicit;
    }
    // `which pi` on the current PATH.
    if let Ok(out) = std::process::Command::new("which").arg("pi").output()
        && out.status.success()
    {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() && std::path::Path::new(&s).exists() {
            return s;
        }
    }
    // Common locations.
    let candidates = ["/root/.local/bin/pi", "/usr/local/bin/pi", "/usr/bin/pi"];
    for c in candidates {
        if std::path::Path::new(c).exists() {
            return c.to_string();
        }
    }
    // Fallback: assume `pi` is on PATH (old behaviour).
    "pi".to_string()
}

#[async_trait]
impl Worker for PiPrintWorker {
    async fn run(
        &self,
        node_id: &str,
        prompt: &str,
        model: &str,
        _attempt: usize,
    ) -> Result<WorkerOutput, PidagError> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.extra_args);
        cmd.arg("-p");
        cmd.arg("--mode");
        cmd.arg("json");
        // Forbid runaway extended-thinking (observed: deepseek-v4-flash with
        // `xhigh` thinking never terminates on long implement prompts, causing
        // "execution failed" timeouts). Default to `low`; override via
        // `PI_THINKING` env (off|minimal|low|medium|high|xhigh|max).
        let thinking = std::env::var("PI_THINKING").unwrap_or_else(|_| "low".to_string());
        cmd.arg("--thinking");
        cmd.arg(&thinking);
        // spec-14 (pi anti-loop): force tool-call discipline in the worker
        // subprocess so pidag-spawned `pi -p` nodes never fall into a
        // narration-only loop (observed cross-model under long context). The
        // `.bashrc` alias only covers interactive shells; the worker spawns
        // `pi` directly, so we inject an internal const prompt here (no
        // external file coupling). Override via `PI_NO_WORKER_PROMPT=1`.
        if std::env::var("PI_NO_WORKER_PROMPT").is_err() {
            cmd.arg("--append-system-prompt");
            cmd.arg(WORKER_ANTI_LOOP_PROMPT);
        }
        // spec-15 (session-backed worker): pass `--session <path>` so each
        // `pi -p` call persists its session. Fix iterations (iter2/iter3)
        // pass `-c` (continue) to resume the prior session with full
        // context — what iter1 wrote, tool calls, etc. This gives the
        // worker multi-turn capability with zero architecture change.
        // Override via `PI_NO_SESSION=1` (fallback to ephemeral one-shot).
        // Session file lives under `_tmp/` (intermediate artifacts).
        if std::env::var("PI_NO_SESSION").is_err() {
            let session_path = std::path::Path::new("_tmp").join(".pidag-worker-session.jsonl");
            cmd.arg("--session");
            cmd.arg(&session_path);
            // Fix iterations continue the session (iter1 creates it).
            if node_id.contains("iter2") || node_id.contains("iter3") {
                cmd.arg("-c");
            }
        }
        // Spec-13 (P13): pass `--provider` explicitly when the model string
        // Spec-13 (P13): pass `--provider` explicitly when the model string
        // carries a `provider/model` or `provider:model` prefix. Without this,
        // pi's `PI_PROVIDER` env (e.g. `deepseek`) forces all traffic there and
        // cross-provider models (nvidia/glm, google/gemini) 400.
        let (provider, bare_model) = crate::split_provider_model(model);
        if let Some(p) = provider {
            cmd.arg("--provider");
            cmd.arg(p);
        }
        cmd.arg("--model");
        cmd.arg(bare_model);
        cmd.arg(prompt);
        // Critical: `pi -p` blocks reading stdin if it is inherited from the
        // parent. Closing it is what makes this a non-interactive call.
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // No orphaned `pi` processes: if the timeout below fires and the
        // future wrapping `wait_with_output` is dropped, `kill_on_drop`
        // kills the child instead of leaving it running.
        cmd.kill_on_drop(true);

        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                return Ok(WorkerOutput {
                    success: false,
                    output: format!("spawn failed: {err}"),
                    retryable: false,
                    usage: None,
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
                    usage: None,
                });
            }
            Err(_elapsed) => {
                return Ok(WorkerOutput {
                    success: false,
                    output: "timed out waiting for pi".to_string(),
                    retryable: false,
                    usage: None,
                });
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr_str}");

        if !output.status.success() {
            // Include stderr in the failure message for better diagnostics
            let error_detail = if !stderr_str.is_empty() {
                format!(
                    "pi exited with status: {:?}\nstderr:\n{}",
                    output.status.code(),
                    stderr_str
                )
            } else {
                format!("pi exited with status: {:?}", output.status.code())
            };
            return Ok(WorkerOutput {
                success: false,
                output: error_detail,
                retryable: classify_retryable(&combined),
                usage: None,
            });
        }

        // Try JSON envelope first (pi --mode json); fall back to
        // plain-text stdout (pi 0.1.x emits text even with --mode json).
        let text = parse_pi_envelope(&stdout);
        let plain = stdout.trim();
        match text {
            Some(t) => Ok(WorkerOutput {
                success: true,
                output: t,
                retryable: false,
                usage: None,
            }),
            None if !plain.is_empty() => Ok(WorkerOutput {
                success: true,
                output: plain.to_string(),
                retryable: false,
                usage: None,
            }),
            _ => Ok(WorkerOutput {
                success: false,
                output: "empty output from pi".to_string(),
                retryable: classify_retryable(&combined),
                usage: None,
            }),
        }
    }
}

/// Extract the assistant's reply text from a `pi -p --mode json`
/// transcript.
///
/// Per the pi protocol, the transcript is a JSON array of messages (or an
/// object with a top-level `"messages"` array); the assistant output is the
/// text of the **last** message with `"role": "assistant"`, specifically
/// `content[0].text` — there is no top-level `output` field. Any shape
/// mismatch returns `None` rather than panicking; callers map that to a
/// node failure.
pub fn parse_pi_envelope(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;

    let messages = value
        .as_array()
        .cloned()
        .or_else(|| value.get("messages").and_then(|m| m.as_array()).cloned())?;

    messages
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|content| content.first())
        .and_then(|first| first.get("text"))
        .and_then(|t| t.as_str())
        .map(str::to_string)
}
