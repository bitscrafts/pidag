//! `auto_drive`: the autonomous driver orchestration loop.
//!
//! For one selected target (a pending spec or a handoff-nominated work item):
//!
//! 1. Read `HANDOFF.md`.
//! 2. Choose the next target via [`select`].
//! 3. Commit-before-modify (git snapshot) so every change is rollbackable.
//! 4. Dispatch a detached `pi -p --mode json` agent whose prompt tells it to
//!    read the spec + handoff, implement/research the target, satisfy any exit
//!    criteria, commit its changes, and update `HANDOFF.md`.
//! 5. Record the outcome (`Done`/`Failed`) in the queue state.
//! 6. Return so the caller can report and exit (cron loop).

use super::git;
use super::handoff::{self, WorkKind, WorkPlan, spec_nominated_by_handoff};
use super::select::{select_from_workspace, select_spec_for_project};
use super::splitter::{AutoSplitter, SpecSplitter};
use crate::queue::{
    SpecState, discover_specs, merge_queues, read_project_queue, write_project_queue,
};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Result of one autonomous drive pass.
#[derive(Debug, Clone)]
pub struct AutoOutcome {
    /// Project root that was worked on.
    pub project_root: PathBuf,
    /// Target selected (spec file or free-form work item).
    pub target: String,
    /// Whether the target is a pending spec (else a handoff work item).
    pub is_spec: bool,
    /// Whether the drive succeeded.
    pub success: bool,
    /// The pre-work git snapshot SHA (empty if unavailable).
    pub snapshot_sha: String,
    /// Human-readable detail (e.g. error or success note).
    pub detail: String,
}

/// Options for the autonomous driver.
#[derive(Debug, Clone)]
pub struct AutoOptions {
    pub workspace: Option<PathBuf>,
    pub project_root: Option<PathBuf>,
    pub model: Option<String>,
    pub agent_timeout: Duration,
    /// Optional pid-lock path. When set, `auto_drive` refuses to run while
    /// another live `pidag auto` holds the same lock (cron single-flight).
    pub pidlock: Option<PathBuf>,
    /// Strategy used to split large specs before running them (free-tier 429
    /// safety). Defaults to the auto-split heuristic.
    pub splitter: Box<dyn SpecSplitter>,
}

impl Default for AutoOptions {
    fn default() -> Self {
        Self {
            workspace: None,
            project_root: None,
            model: None,
            agent_timeout: Duration::from_secs(600),
            pidlock: None,
            splitter: AutoSplitter::boxed(),
        }
    }
}

/// Read a project's handoff via our own helper (nowhere-dependent closure).
fn read_handoff_opt(root: &Path) -> Option<String> {
    handoff::read_handoff(root).ok().flatten()
}

/// Run one autonomous drive pass.
pub async fn auto_drive(opts: &AutoOptions) -> Result<AutoOutcome, String> {
    // 0. Single-flight pid-lock: refuse to run when another live pass holds it.
    let _lock_guard = if let Some(lock_path) = &opts.pidlock {
        match super::lock::PidLock::acquire(lock_path)
            .map_err(|e| format!("pid-lock acquire failed: {e}"))?
        {
            Some(g) => Some(g),
            None => {
                return Ok(AutoOutcome {
                    project_root: PathBuf::new(),
                    target: "<locked>".to_string(),
                    is_spec: false,
                    success: false,
                    snapshot_sha: String::new(),
                    detail: format!(
                        "another pidag auto holds {}; skipping this cron pass",
                        lock_path.display()
                    ),
                });
            }
        }
    } else {
        None
    };

    // 1. Resolve the project root (workspace discovery or explicit path).
    let root = match (&opts.workspace, &opts.project_root) {
        (Some(ws), _) => {
            let picked = select_from_workspace(ws, &read_handoff_opt)
                .map_err(|e| format!("workspace selection failed: {e}"))?;
            match picked {
                Some(p) => p.root.clone(),
                None => {
                    // Nothing pending anywhere — report an empty pass.
                    return Ok(AutoOutcome {
                        project_root: ws.clone(),
                        target: "<none>".to_string(),
                        is_spec: false,
                        success: true,
                        snapshot_sha: String::new(),
                        detail: "no pending work in workspace".to_string(),
                    });
                }
            }
        }
        (None, Some(pr)) => pr.clone(),
        (None, None) => std::env::current_dir().map_err(|e| e.to_string())?,
    };

    // 2. Read the handoff for context + any declared Work Direction.
    let handoff_text = read_handoff_opt(&root);
    let direction = handoff::work_direction(handoff_text.as_deref());

    // 3. Choose the target.
    //    - If the handoff declares an explicit work direction with a target,
    //      use it directly (mode shapes the DAG flavour).
    //    - Else if the handoff nominates a mode but no target, pick a pending
    //      spec (or fall back to a research item).
    //    - Else the default rule: resume/failed first, then pending spec,
    //      else a handoff work item.
    if let Some(plan) = direction.clone().filter(|p| p.target.is_some()) {
        let target_text = plan_target_spec_path(&root, plan.target.as_deref().unwrap_or_default());
        return dispatch_plan(&root, plan, target_text, opts).await;
    }
    if let Some(plan) = direction.clone() {
        return dispatch_plan_unset_target(&root, plan, handoff_text.as_deref(), opts).await;
    }

    let selected = select_spec_for_project(&root, &read_handoff_opt)
        .map_err(|e| format!("selection failed: {e}"))?;

    let (target, is_spec, kind) = match selected {
        Some(spec_file) => (spec_file, true, WorkKind::Implement),
        None => {
            // No pending spec; try a handoff work item as a research/improve target.
            match spec_nominated_by_handoff(handoff_text.as_deref()) {
                Some(item) => (item, false, WorkKind::Research),
                None => {
                    return Ok(AutoOutcome {
                        project_root: root,
                        target: "<none>".to_string(),
                        is_spec: false,
                        success: true,
                        snapshot_sha: String::new(),
                        detail: "no pending spec and no handoff work item".to_string(),
                    });
                }
            }
        }
    };

    // 4. Commit-before-modify: snapshot the working tree.
    let git_state = git::pre_work_snapshot(&root, &target)
        .map_err(|e| format!("pre-work git snapshot failed: {e}"))?;

    // 5. Dispatch the agent with the chosen mode.
    let outcome = dispatch_kind(&root, &target, kind, opts).await;

    // 6. Record queue state (only meaningful for spec targets with a state entry).
    record_queue_state(&root, &target, is_spec, outcome.success)?;

    Ok(AutoOutcome {
        project_root: root,
        target,
        is_spec,
        success: outcome.success,
        snapshot_sha: git_state.snapshot_sha,
        detail: outcome.detail,
    })
}

/// Normalise a plan target that names a spec so it dispatches via `sdd --run`
/// when it is a real spec file, otherwise as a flexible-DAG research item.
fn plan_target_spec_path(_root: &Path, target: &str) -> String {
    // Both branches surfaced the same string; collapse to a single return
    // (clippy `if-same-then`). The distinction is documented for callers.
    target.to_string()
}

/// Dispatch a work-direction plan whose target is known.
async fn dispatch_plan(
    root: &Path,
    plan: WorkPlan,
    target_text: String,
    opts: &AutoOptions,
) -> Result<AutoOutcome, String> {
    let is_spec = target_text.ends_with(".md") && target_text.contains("specs/");
    let git_state = git::pre_work_snapshot(root, &target_text)
        .map_err(|e| format!("pre-work git snapshot failed: {e}"))?;
    let outcome = dispatch_kind(root, &target_text, plan.kind, opts).await;
    record_queue_state(root, &target_text, is_spec, outcome.success)?;
    Ok(AutoOutcome {
        project_root: root.to_path_buf(),
        target: target_text,
        is_spec,
        success: outcome.success,
        snapshot_sha: git_state.snapshot_sha,
        detail: outcome.detail,
    })
}

/// Dispatch a work-direction plan whose mode is set but target was not:
/// pick a pending spec when the mode implies implementation, else research.
async fn dispatch_plan_unset_target(
    root: &Path,
    plan: WorkPlan,
    _handoff: Option<&str>,
    opts: &AutoOptions,
) -> Result<AutoOutcome, String> {
    if plan.kind == WorkKind::Implement
        && let Some(spec) =
            select_spec_for_project(root, &|p| handoff::read_handoff(p).ok().flatten())?
    {
        return dispatch_plan(root, plan, spec, opts).await;
    }
    let target_text = plan
        .target
        .clone()
        .unwrap_or_else(|| "auto: research-directed work item".to_string());
    dispatch_plan(root, plan, target_text, opts).await
}

/// Dispatch work by [`WorkKind`]: implement (single spec) vs the flexible-DAG
/// flavours for research/debug/refactor.
async fn dispatch_kind(
    root: &Path,
    target: &str,
    kind: WorkKind,
    opts: &AutoOptions,
) -> AgentResult {
    match kind {
        WorkKind::Implement => dispatch_implement(root, target, opts).await,
        _ => dispatch_flexible_dag(root, target, kind, opts).await,
    }
}

/// Dispatch an `Implement` target, splitting first when the spec is large so
/// each SDD sub-run stays small (free-tier 429 safety).
///
/// - Not a spec file under `specs/`: run it directly.
/// - Spec fits one run: run the parent directly.
/// - Spec is large: split into ordered child specs, run each child in order,
///   and succeed only when every child run succeeds.
async fn dispatch_implement(root: &Path, target: &str, opts: &AutoOptions) -> AgentResult {
    // Only consider splitting a real spec file. Relative spec paths are resolved
    // against the project root (matching how `pidag sdd` resolves them).
    let is_spec = target.ends_with(".md") && target.contains("specs");
    if !is_spec {
        return dispatch_sdd(root, target, opts).await;
    }
    let abs = if Path::new(target).is_absolute() {
        target.to_string()
    } else {
        root.join(target).to_string_lossy().to_string()
    };

    let children = match opts.splitter.split_if_large(Path::new(&abs), root) {
        Ok(children) => children,
        Err(e) => {
            // A failed split should not silently drop work: fall back to running
            // the parent whole rather than losing the target.
            eprintln!("[auto] splitter warning: {e}; running spec whole");
            None
        }
    };

    let Some(children) = children else {
        // No split produced (small spec) — run the parent directly.
        return dispatch_sdd(root, target, opts).await;
    };

    if children.is_empty() {
        return dispatch_sdd(root, target, opts).await;
    }

    // Split happened: run each child in dependency order.
    let mut success = true;
    let mut detail = String::new();
    for child in &children {
        let child_target = child.path.to_string_lossy().to_string();
        let r = dispatch_sdd(root, &child_target, opts).await;
        if !r.success {
            success = false;
            detail = format!("child {} failed: {}", child.name, r.detail);
            break;
        }
    }
    if success {
        AgentResult {
            success: true,
            detail: format!(
                "split {} into {} children and ran all successfully",
                target,
                children.len()
            ),
        }
    } else {
        AgentResult {
            success: false,
            detail,
        }
    }
}

/// Result of dispatching the work via the pidag scheduler (subprocess).
struct AgentResult {
    success: bool,
    detail: String,
}

/// Subprocess to `pidag sdd <spec.md> --run`.
async fn dispatch_sdd(root: &Path, target: &str, opts: &AutoOptions) -> AgentResult {
    let mut cmd = Command::new(crate::core::selfexe::self_exe());
    cmd.arg("sdd");
    cmd.arg(target);
    cmd.arg("--run");
    // Resume by default: if a prior interrupted run for this spec exists in
    // the vault, continue from the last completed node instead of restarting
    // (Spec-08 + auto-driver design req #2). A first-run (no prior run_id in
    // the vault) is a no-op Fresh path on the run side.
    cmd.arg("--resume");
    if let Some(model) = &opts.model {
        cmd.arg("--model");
        cmd.arg(model);
    }
    cmd.current_dir(root);
    cmd.stdin(Stdio::null());

    let awaited = tokio::time::timeout(opts.agent_timeout, cmd.status()).await;
    match awaited {
        Ok(Ok(status)) => AgentResult {
            success: status.success(),
            detail: if status.success() {
                format!("sdd run finished for {target}")
            } else {
                format!("sdd run failed for {target} (exit {status:?})")
            },
        },
        Ok(Err(e)) => AgentResult {
            success: false,
            detail: format!("failed to spawn pidag sdd: {e}"),
        },
        Err(_elapsed) => AgentResult {
            success: false,
            detail: "pidag sdd timed out".to_string(),
        },
    }
}

/// Build a flexible DAG for a research/improvement work item and run it via
/// `pidag run <dag.json>`. The DAG is a single LLM analyse node chained into a
/// "apply" shell, but the key point is it is executed by pidag's scheduler so
/// free-tier 429s are handled with retry/backoff/fallback.
async fn dispatch_flexible_dag(
    root: &Path,
    target: &str,
    kind: WorkKind,
    opts: &AutoOptions,
) -> AgentResult {
    let dag_dir = root.join(".pidag");
    std::fs::create_dir_all(&dag_dir)
        .map_err(|e| e.to_string())
        .ok();
    let dag_path = dag_dir.join("auto-flex-dag.json");

    let model = opts
        .model
        .clone()
        .unwrap_or_else(|| "deepseek-chat".to_string());
    let instruction = match kind {
        WorkKind::Research => format!(
            "Research the HANDOFF.md work item: {target}. Read the repo + HANDOFF.md, form a concrete implementation/improvement plan, and perform the change. When done, git add -A && git commit."
        ),
        WorkKind::Debug => format!(
            "DEBUG task: {target}. Reproduce the failing behaviour, locate the root cause, fix it, and add a regression test. Run the relevant tests to confirm. When done, git add -A && git commit."
        ),
        WorkKind::Refactor => format!(
            "REFACTOR task: {target}. Analyse the code, plan the refactor, keep/add tests, then apply the change. When done, git add -A && git commit."
        ),
        WorkKind::Implement => format!(
            "IMPLEMENT task: {target}. Form a plan, implement, satisfy any exit criteria, update HANDOFF.md. When done, git add -A && git commit."
        ),
    };
    let dag = serde_json::json!({
        "name": "auto-flex",
        "nodes": [
            {
                "id": "research",
                "text": instruction,
                "models": [ { "name": model, "paid": false } ],
                "retry": { "attempts": 3, "backoff_ms": 5000 }
            }
        ]
    });

    match serde_json::to_string_pretty(&dag) {
        Ok(js) => {
            if std::fs::write(&dag_path, js).is_err() {
                return AgentResult {
                    success: false,
                    detail: "failed to write flexible DAG".to_string(),
                };
            }
        }
        Err(e) => {
            return AgentResult {
                success: false,
                detail: format!("failed to serialize flexible DAG: {e}"),
            };
        }
    }

    let mut cmd = Command::new(crate::core::selfexe::self_exe());
    cmd.arg("run");
    cmd.arg(&dag_path);
    cmd.current_dir(root);
    cmd.stdin(Stdio::null());

    let awaited = tokio::time::timeout(opts.agent_timeout, cmd.status()).await;
    match awaited {
        Ok(Ok(status)) => AgentResult {
            success: status.success(),
            detail: format!(
                "flexible DAG {} {}",
                dag_path.display(),
                if status.success() { "OK" } else { "FAILED" }
            ),
        },
        Ok(Err(e)) => AgentResult {
            success: false,
            detail: format!("failed to spawn pidag run: {e}"),
        },
        Err(_elapsed) => AgentResult {
            success: false,
            detail: "pidag run timed out".to_string(),
        },
    }
}

/// Update the queue state entry for a spec target to Done/Failed.
fn record_queue_state(
    root: &Path,
    target: &str,
    is_spec: bool,
    success: bool,
) -> Result<(), String> {
    if !is_spec {
        return Ok(()); // handoff work items aren't queue specs
    }
    let discovered = discover_specs(root);
    if discovered.is_empty() {
        return Ok(());
    }
    let cached = read_project_queue(root).unwrap_or(None);
    let mut state = match cached {
        Some(c) => merge_queues(&c, &discovered, root),
        None => crate::queue::ProjectQueue {
            project_root: root.to_string_lossy().to_string(),
            entries: discovered,
            updated_at: crate::queue::now_iso(),
            weight: 1.0,
        },
    };
    let stem = target.trim_start_matches("specs/").trim_end_matches(".md");
    if let Some(e) = state.entries.iter_mut().find(|e| e.spec_name == stem) {
        e.state = if success {
            SpecState::Done
        } else {
            SpecState::Failed
        };
        if !success {
            e.error = Some("agent run failed".to_string());
        }
    }
    write_project_queue(root, &state).map_err(|e| e.to_string())
}
