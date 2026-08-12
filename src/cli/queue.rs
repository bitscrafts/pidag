//! CLI subcommand: `pidag queue <--status|--run|--reset|--retry-failed|--round-robin|--daemon>`
//!
//! Single-project queue + multi-project round-robin carousel (spec
//! `specs/09-carousel-queue.md`).

use crate::queue::{
    RunOutcome, backup_queue_if_needed, check_dry_run_done, discover_projects, discover_specs,
    merge_queues, read_project_queue, render_status_table, reset_all_to_pending, retry_failed_only,
    run_queue, state_file_path, write_project_queue,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Handle `pidag queue` subcommand.
pub async fn queue(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return Ok(());
    }

    let mut mode: Option<&str> = None;
    let mut project_root: Option<String> = None;
    let mut workspace: Option<String> = None;
    let mut batch: usize = 5;
    let mut dry_run = false;
    let mut stop_on_failure = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let (key, inline_val) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(k, v)| (k, Some(v.to_string())));
        match key {
            "--status" => mode = Some("status"),
            "--run" => mode = Some("run"),
            "--reset" => mode = Some("reset"),
            "--retry-failed" => mode = Some("retry-failed"),
            "--round-robin" => mode = Some("round-robin"),
            "--daemon" => mode = Some("daemon"),
            "--dry-run" => dry_run = true,
            "--stop-on-failure" => stop_on_failure = true,
            "--project-root" => {
                project_root = Some(match inline_val {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i)
                            .cloned()
                            .ok_or("--project-root requires a path")?
                    }
                });
            }
            "--workspace" => {
                workspace = Some(match inline_val {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i).cloned().ok_or("--workspace requires a path")?
                    }
                });
            }
            "--batch" => {
                let v = match inline_val {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i).cloned().ok_or("--batch requires a number")?
                    }
                };
                batch = v.parse::<usize>()?;
            }
            other => {
                return Err(format!("Unknown queue option: {}", other).into());
            }
        }
        i += 1;
    }

    let cwd = std::env::current_dir()?;
    let root: PathBuf = match project_root {
        Some(p) => PathBuf::from(p),
        None => cwd.clone(),
    };

    match mode {
        Some("status") => cmd_status(&root),
        Some("reset") => cmd_reset(&root),
        Some("retry-failed") => cmd_retry_failed(&root),
        Some("run") => {
            cmd_run(&root, workspace.as_deref(), dry_run, stop_on_failure).await
        }
        Some("round-robin") => {
            cmd_run(&root, workspace.as_deref(), true, stop_on_failure).await
        }
        Some("daemon") => {
            cmd_daemon(&root, workspace.as_deref(), batch, dry_run, stop_on_failure).await
        }
        _ => Err("No queue mode given (use --status, --run, --reset, --retry-failed, --round-robin, or --daemon)".into()),
    }
}

fn cmd_status(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let discovered = discover_specs(root);
    let cached = read_project_queue(root)?;
    let merged = match cached {
        Some(c) => merge_queues(&c, &discovered, root),
        None => crate::queue::ProjectQueue {
            project_root: root.to_string_lossy().to_string(),
            entries: discovered,
            updated_at: crate::queue::now_iso(),
            weight: 1.0,
        },
    };
    print!("{}", render_status_table(&merged.entries));
    Ok(())
}

fn cmd_reset(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let discovered = discover_specs(root);
    if discovered.is_empty() {
        eprintln!("No specs found under {}/specs", root.display());
        return Ok(());
    }
    let cached = read_project_queue(root)?;
    let mut merged = match cached {
        Some(c) => merge_queues(&c, &discovered, root),
        None => crate::queue::ProjectQueue {
            project_root: root.to_string_lossy().to_string(),
            entries: discovered,
            updated_at: crate::queue::now_iso(),
            weight: 1.0,
        },
    };
    reset_all_to_pending(&mut merged);
    write_project_queue(root, &merged)?;
    let sp = state_file_path(root);
    println!("Reset non-Done specs to Pending -> {}", sp.display());
    Ok(())
}

fn cmd_retry_failed(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let discovered = discover_specs(root);
    let cached = read_project_queue(root)?;
    let mut merged = match cached {
        Some(c) => merge_queues(&c, &discovered, root),
        None => crate::queue::ProjectQueue {
            project_root: root.to_string_lossy().to_string(),
            entries: discovered,
            updated_at: crate::queue::now_iso(),
            weight: 1.0,
        },
    };
    retry_failed_only(&mut merged);
    write_project_queue(root, &merged)?;
    let sp = state_file_path(root);
    println!("Re-queued Failed specs as Pending -> {}", sp.display());
    Ok(())
}

/// Run (or dry-run) the queue, single-project or workspace carousel.
async fn cmd_run(
    root: &Path,
    workspace: Option<&str>,
    dry_run: bool,
    stop_on_failure: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(ws) = workspace {
        return run_workspace(Path::new(ws), dry_run, stop_on_failure).await;
    }

    // Single project.
    let discovered = discover_specs(root);
    if dry_run {
        let cached = read_project_queue(root)?;
        let state = match &cached {
            Some(c) => merge_queues(c, &discovered, root),
            None => crate::queue::ProjectQueue {
                project_root: root.to_string_lossy().to_string(),
                entries: discovered.clone(),
                updated_at: crate::queue::now_iso(),
                weight: 1.0,
            },
        };
        println!(
            "Order: {}",
            state
                .entries
                .iter()
                .filter(|e| e.state == crate::queue::SpecState::Pending)
                .map(|e| e.spec_file.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(());
    }

    return run_single(root, dry_run, stop_on_failure).await;
}

async fn run_single(
    root: &Path,
    _dry_run: bool,
    stop_on_failure: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    backup_queue_if_needed(root);
    let discovered = discover_specs(root);
    let cached = read_project_queue(root)?;
    let mut state = match cached {
        Some(c) => merge_queues(&c, &discovered, root),
        None => crate::queue::ProjectQueue {
            project_root: root.to_string_lossy().to_string(),
            entries: discovered,
            updated_at: crate::queue::now_iso(),
            weight: 1.0,
        },
    };

    let pending: Vec<_> = state
        .entries
        .iter()
        .filter(|e| e.state == crate::queue::SpecState::Pending)
        .cloned()
        .collect();

    if pending.is_empty() {
        println!("No pending specs in {}", root.display());
        return Ok(());
    }

    let outcome: RunOutcome = run_queue(root, &pending, &mut state, false, stop_on_failure).await?;
    println!(
        "Completed: {} | Failed: {} | Skipped: {}",
        outcome.completed, outcome.failed, outcome.skipped
    );
    Ok(())
}

/// Workspace carousel run: round-robin across all projects.
async fn run_workspace(
    ws: &Path,
    dry_run: bool,
    stop_on_failure: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let projects = discover_projects(ws)?;
    let mut pending_by_project: Vec<Vec<crate::queue::QueueEntry>> = Vec::new();
    let mut state_map: HashMap<String, crate::queue::ProjectQueue> = HashMap::new();
    let mut roots: HashMap<String, PathBuf> = HashMap::new();

    for p in &projects {
        let label = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&p.to_string_lossy())
            .to_string();
        let discovered = discover_specs(p);
        let cached = read_project_queue(p)?;
        let state = match cached {
            Some(c) => merge_queues(&c, &discovered, p),
            None => crate::queue::ProjectQueue {
                project_root: p.to_string_lossy().to_string(),
                entries: discovered.clone(),
                updated_at: crate::queue::now_iso(),
                weight: 1.0,
            },
        };
        let pending: Vec<_> = state
            .entries
            .iter()
            .filter(|e| e.state == crate::queue::SpecState::Pending)
            .cloned()
            .collect();
        pending_by_project.push(pending);
        state_map.insert(label.clone(), state);
        roots.insert(label, p.clone());
    }

    if dry_run {
        let interleaved = crate::queue::carousel_interleave(pending_by_project);
        println!(
            "round-robin: {}",
            interleaved
                .iter()
                .map(|e| e.spec_file.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(());
    }

    // Round-robin order across labeled project pending lists.
    let labeled = crate::queue::round_robin_order(
        projects
            .iter()
            .map(|p| {
                let label = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&p.to_string_lossy())
                    .to_string();
                let disc = discover_specs(p);
                let pending: Vec<_> = disc
                    .into_iter()
                    .filter(|e| e.state == crate::queue::SpecState::Pending)
                    .collect();
                (label, pending)
            })
            .collect(),
    );

    for (label, entry) in &labeled {
        if let (Some(state), Some(root)) = (state_map.get_mut(label), roots.get(label)) {
            let _ = crate::queue::execute_entry(root, state, entry, false).await;
            if stop_on_failure
                && state.entries.iter().any(|e| {
                    e.spec_file == entry.spec_file && e.state == crate::queue::SpecState::Failed
                })
            {
                break;
            }
        }
    }

    println!(
        "Workspace carousel pass complete over {} projects.",
        projects.len()
    );
    Ok(())
}

/// Daemon bounded-batch pass.
async fn cmd_daemon(
    root: &Path,
    workspace: Option<&str>,
    batch: usize,
    dry_run: bool,
    stop_on_failure: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = match workspace {
        Some(ws) => {
            // Workspace daemon: round-robin bounded batch across projects.
            run_workspace_daemon(Path::new(ws), batch, dry_run, stop_on_failure).await?
        }
        None => check_dry_run_done(root, batch, dry_run, stop_on_failure).await?,
    };
    println!(
        "Daemon pass: executed {}, remaining {}",
        result.executed, result.remaining
    );
    Ok(())
}

async fn run_workspace_daemon(
    ws: &Path,
    batch: usize,
    dry_run: bool,
    stop_on_failure: bool,
) -> Result<crate::queue::ExecResult, Box<dyn std::error::Error>> {
    let b = if batch == 0 { 5 } else { batch };
    let projects = discover_projects(ws)?;

    // Gather (weight, pending) per project together with the per-project
    // state + root, so a single weighted_carousel_bounded call decides the
    // whole batch (spec-11: weights seed the batch budget, hard `b` cap).
    let mut weighted: Vec<(f64, Vec<crate::queue::QueueEntry>)> = Vec::new();
    // Root + mutable state keyed by position in `weighted` so we can map each
    // emitted entry back to its project for execution.
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut states: Vec<crate::queue::ProjectQueue> = Vec::new();
    for p in &projects {
        let discovered = discover_specs(p);
        let state = match read_project_queue(p)? {
            Some(c) => merge_queues(&c, &discovered, p),
            None => crate::queue::ProjectQueue {
                project_root: p.to_string_lossy().to_string(),
                entries: discovered.clone(),
                updated_at: crate::queue::now_iso(),
                weight: 1.0,
            },
        };
        let pending: Vec<_> = state
            .entries
            .iter()
            .filter(|e| e.state == crate::queue::SpecState::Pending)
            .cloned()
            .collect();
        weighted.push((state.weight, pending));
        roots.push(p.clone());
        states.push(state);
    }

    let order = crate::queue::weighted_carousel_bounded(weighted, b);
    let mut total_executed = 0usize;

    for entry in &order {
        if total_executed >= b {
            break;
        }
        // Locate the owning project by matching the entry's spec_file against
        // each project's pending set (entries are unique across projects).
        let owner = states
            .iter_mut()
            .zip(roots.iter())
            .find(|(s, _)| s.entries.iter().any(|e| e.spec_file == entry.spec_file))
            .map(|(s, r)| (s, r.clone()));
        let Some((state, root)) = owner else { continue };

        if dry_run {
            println!("{}", entry.spec_file);
            total_executed += 1;
        } else {
            let ok = crate::queue::execute_entry(&root, state, entry, false).await;
            total_executed += 1;
            if !ok && stop_on_failure {
                break;
            }
        }
    }

    // Persist each project's state (only those we touched).
    for (state, root) in states.iter().zip(roots.iter()) {
        write_project_queue(root, state)?;
    }

    let remaining = discover_projects(ws)?
        .iter()
        .map(|p| discover_specs(p).len())
        .sum::<usize>()
        .saturating_sub(total_executed);

    Ok(crate::queue::ExecResult {
        executed: total_executed,
        remaining,
    })
}

fn print_help() {
    eprintln!(
        r#"pidag queue — carousel priority queue with round-robin scheduling

USAGE:
    pidag queue --status [--project-root PATH]
    pidag queue --run [--project-root PATH] [--stop-on-failure]
    pidag queue --reset [--project-root PATH]
    pidag queue --retry-failed [--project-root PATH]
    pidag queue --round-robin [--dry-run] [--project-root PATH]
    pidag queue --workspace <path> --run [--dry-run]
    pidag queue --daemon [--workspace <path> | --project-root PATH] [--batch N] [--stop-on-failure]

OPTIONS:
    --status             Print a table of all specs with state + priority
    --run                Execute pending specs in priority order
    --reset              Reset all non-Done specs to Pending
    --retry-failed       Re-queue only Failed specs as Pending
    --round-robin        Use round-robin ordering; with a workspace it is implied
    --workspace <path>   Round-robin carousel across projects in a workspace root
    --daemon             One bounded batch (≤N specs, default 5); cron-safe
    --batch N            Max specs per daemon/round-robin pass (default 5)
    --dry-run            Render the order without spawning SDD runs
    --stop-on-failure    Abort the run at the first failure
    --project-root PATH  Override project root (default: current directory)
    --help               Show this message
"#
    );
}
