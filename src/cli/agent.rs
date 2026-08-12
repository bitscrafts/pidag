//! `pidag auto` — the autonomous driver subcommand.
//!
//! Picks a project (explicit `--project-root`, or workspace discovery from
//! `--workspace`), reads its `HANDOFF.md`, commits-before-modify, drives a
//! pidag DAG to implement the next target (spec or handoff work-item), updates
//! the handoff, and reports the outcome. Safe to invoke repeatedly from a cron
//! job (every half hour, for example).

use std::path::PathBuf;
use std::time::Duration;

use crate::agent::{AutoOptions, auto_drive};

/// `pidag auto [--workspace <dir>] [--project-root <dir>] [--model <m>]
///               [--timeout <secs>]`
pub async fn auto(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut opts = AutoOptions::default();
    let mut workspace: Option<PathBuf> = None;
    let mut project_root: Option<PathBuf> = None;
    let mut lock_flag = false;
    let mut lock_path: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--workspace" | "-w" => {
                if i + 1 < args.len() {
                    workspace = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                } else {
                    eprintln!("error: --workspace requires an argument");
                    std::process::exit(1);
                }
            }
            "--project-root" | "-p" => {
                if i + 1 < args.len() {
                    project_root = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                } else {
                    eprintln!("error: --project-root requires an argument");
                    std::process::exit(1);
                }
            }
            "--model" | "-m" => {
                if i + 1 < args.len() {
                    opts.model = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("error: --model requires an argument");
                    std::process::exit(1);
                }
            }
            "--timeout" => {
                if i + 1 < args.len() {
                    let secs: u64 = args[i + 1].parse().unwrap_or(600);
                    opts.agent_timeout = Duration::from_secs(secs);
                    i += 2;
                } else {
                    eprintln!("error: --timeout requires an argument");
                    std::process::exit(1);
                }
            }
            "--lock" => {
                lock_flag = true;
                // Optional value: `--lock <path>` or bare `--lock` (default path).
                if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    lock_path = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                } else {
                    i += 1;
                }
            }
            _ => {
                eprintln!("error: unknown option: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    if workspace.is_none() && project_root.is_none() {
        eprintln!("error: pidag auto requires --workspace <dir> or --project-root <dir>");
        std::process::exit(1);
    }

    // Resolve the default lock path under the target root when `--lock` is bare.
    if lock_flag
        && lock_path.is_none()
        && let Some(root) = workspace.as_ref().or(project_root.as_ref())
    {
        lock_path = Some(root.join(".pidag").join("auto.lock"));
    }

    opts.workspace = workspace;
    opts.project_root = project_root;
    opts.pidlock = lock_path;

    match auto_drive(&opts).await {
        Ok(outcome) => {
            let status = if outcome.success { "OK" } else { "FAILED" };
            println!(
                "[auto] status={status} project={} target={} (spec={}) snapshot={} detail={}",
                outcome.project_root.display(),
                outcome.target,
                outcome.is_spec,
                if outcome.snapshot_sha.is_empty() {
                    "-"
                } else {
                    &outcome.snapshot_sha
                },
                outcome.detail
            );
            if outcome.success {
                Ok(())
            } else {
                Err(outcome.detail.into())
            }
        }
        Err(e) => {
            eprintln!("error: auto failed: {e}");
            Err(e.into())
        }
    }
}
