//! `pidag run` command - execute a DAG from JSON

use crate::{
    CompositeSink, Dag, JsonlSink, NodeStatus, RedbSink, RedbStore, Scheduler, Store, VecSink,
};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub async fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        eprintln!("error: 'run' requires a <dag.json> argument");
        std::process::exit(1);
    }

    let dag_path = &args[0];
    let mut concurrency = 4usize;
    let mut allow_paid = false;
    let mut vault_path = PathBuf::from(".pidag/pidag.redb");
    // Resume flags (Spec-08). `--run-id` opts into the deterministic-run_id
    // path (used by `pidag sdd --run`); `--resume` loads the checkpoint;
    // `--fresh` ignores any existing checkpoint; `--retry-failed` resets
    // Failed checkpoint nodes to Pending for retry.
    let mut run_id_override: Option<String> = None;
    let mut resume = false;
    let mut fresh = false;
    let mut retry_failed = false;

    // Parse optional arguments
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--concurrency" => {
                if i + 1 < args.len() {
                    concurrency = args[i + 1].parse().unwrap_or(4);
                    i += 2;
                } else {
                    eprintln!("error: --concurrency requires an argument");
                    std::process::exit(1);
                }
            }
            "--allow-paid" => {
                allow_paid = true;
                i += 1;
            }
            "--vault" => {
                if i + 1 < args.len() {
                    vault_path = PathBuf::from(&args[i + 1]);
                    i += 2;
                } else {
                    eprintln!("error: --vault requires an argument");
                    std::process::exit(1);
                }
            }
            "--run-id" => {
                if i + 1 < args.len() {
                    run_id_override = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("error: --run-id requires an argument");
                    std::process::exit(1);
                }
            }
            "--resume" => {
                resume = true;
                i += 1;
            }
            "--fresh" => {
                fresh = true;
                i += 1;
            }
            "--retry-failed" => {
                retry_failed = true;
                i += 1;
            }
            _ => {
                eprintln!("error: unknown option: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Read and parse DAG JSON
    let dag_json =
        std::fs::read_to_string(dag_path).map_err(|e| format!("Failed to read DAG file: {}", e))?;
    let dag: Dag =
        serde_json::from_str(&dag_json).map_err(|e| format!("Failed to parse DAG JSON: {}", e))?;

    // Validate DAG
    dag.validate()
        .map_err(|e| format!("DAG validation failed: {}", e))?;

    // Open vault store. Use `RedbStore` directly (persistent lock for the
    // entire run duration) to avoid the per-operation open/close race
    // condition that corrupts dag_json to "{}". The lock is held for minutes
    // (the run duration), and the UI can retry on lock contention.
    let store: Arc<dyn Store> =
        Arc::new(RedbStore::open(&vault_path).map_err(|e| format!("Failed to open vault: {}", e))?);

    // Determine run_id + optional checkpoint (Spec-08).
    //
    // `--run-id` opts into the deterministic-run_id path (used by `pidag sdd
    // --run`, which derives it from `run_id_for_spec`). Without it we keep the
    // historical timestamp id (byte-identical backward compat for direct
    // `pidag run` users).
    //
    // `--resume` (requires `--run-id`) loads the checkpoint from the vault:
    //   - AlreadyDone -> print the cached report and exit 0 (no re-execution).
    //   - Resume      -> Scheduler::with_checkpoint (skip completed prefix).
    //   - Fresh       -> normal new run with the given run_id.
    // `--fresh` skips load_checkpoint entirely (still a Fresh/new run using
    // the given run_id, so it OVERWRITES the prior interrupted run record).
    let checkpoint: Option<crate::scheduler::Checkpoint> =
        if resume && let Some(rid) = run_id_override.as_ref() {
            // --resume without --fresh: consult the vault.
            match crate::sdd::load_checkpoint(store.as_ref(), rid, retry_failed).await {
                Ok(crate::sdd::ResumeDecision::AlreadyDone { run_id, report }) => {
                    println!(
                        "Run {} already completed (resumed from checkpoint):",
                        run_id
                    );
                    for ns in &report.node_states {
                        println!("  {}: {}", ns.node_id, ns.state);
                    }
                    if !report.failed.is_empty() {
                        eprintln!("failed nodes: {:?}", report.failed);
                        std::process::exit(1);
                    }
                    std::process::exit(0);
                }
                Ok(crate::sdd::ResumeDecision::Resume { checkpoint }) => {
                    println!(
                        "Resuming run {} ({} completed, {} stale-running, {} failed, {} blocked)",
                        checkpoint.run_id,
                        checkpoint.completed_nodes.len(),
                        checkpoint.stale_running.len(),
                        checkpoint.failed_nodes.len(),
                        checkpoint.blocked_nodes.len(),
                    );
                    Some(checkpoint)
                }
                Ok(crate::sdd::ResumeDecision::Fresh { run_id: _ }) => None,
                Err(e) => {
                    eprintln!("error: failed to load checkpoint: {}", e);
                    std::process::exit(1);
                }
            }
        } else {
            // No --resume (or --fresh): a brand-new run, no checkpoint applied.
            None
        };
    // fresh==true implies no checkpoint load happened above (Fresh path is the
    // fallthrough); `retry_failed` is only consulted by load_checkpoint and is
    // a no-op when resume is false. Both flags are intentionally swallowed
    // here when their precondition (--resume + --run-id) is not met.
    let _ = (fresh, retry_failed);

    let run_id = run_id_override.unwrap_or_else(generate_run_id);

    // R4: Acquire per-run-id lock to fence concurrent runs.
    // Lock path is <vault_dir>/.locks/<run_id>.pid (per run_id, not per vault).
    let lock_path = run_lock_path(&vault_path, &run_id);
    let _lock = match crate::agent::lock::PidLock::acquire(&lock_path) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            // Another process holds the lock. Read the PID and report it.
            if let Some(holder_pid) = read_lock_pid(&lock_path) {
                eprintln!(
                    "error: run {} is already executing (held by PID {})",
                    run_id, holder_pid
                );
            } else {
                eprintln!("error: run {} is already executing", run_id);
            }
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("error: failed to acquire run lock: {}", e);
            std::process::exit(1);
        }
    };

    // Pre-seed the store with DAG JSON
    let started_at = chrono::Utc::now().to_rfc3339();
    let run_meta = crate::RunMeta {
        run_id: run_id.clone(),
        dag_json: dag_json.clone(),
        started_at: started_at.clone(),
        completed_at: None,
        successful_nodes: 0,
        failed_nodes: 0,
    };
    store
        .put_run(&run_meta)
        .await
        .map_err(|e| format!("Failed to store run metadata: {}", e))?;

    // Create event sinks
    let write_failures = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let redb_sink = Box::new(RedbSink::new_with_failures(
        Arc::clone(&store),
        run_id.clone(),
        Arc::clone(&write_failures),
    ));

    // Optionally create JSONL sink for events
    let jsonl_file_path = PathBuf::from(".pidag").join(format!("{}.events.jsonl", run_id));
    let jsonl_sink: Box<dyn crate::EventSink> =
        if let Ok(file) = std::fs::File::create(&jsonl_file_path) {
            Box::new(JsonlSink::new(Arc::new(std::sync::Mutex::new(
                Box::new(file) as Box<dyn Write + Send>,
            ))))
        } else {
            // Fall back to no JSONL sink if file creation fails
            Box::new(VecSink::new())
        };

    let composite_sink = Box::new(CompositeSink::new(vec![redb_sink, jsonl_sink]));

    // Create worker (type-dispatching: shell nodes -> bash, llm nodes -> pi).
    // Load timeout and backend config from `.pidag/config.toml` if present;
    // fall back to 60s timeout and no backend. Without this, LLM nodes that need more than
    // 60s (e.g. deepseek-v4-flash writing a full Rust module) time out.
    let (config, worker_timeout) = {
        let config_path = vault_path
            .parent()
            .map(|p| p.join("config.toml"))
            .unwrap_or_else(|| PathBuf::from(".pidag/config.toml"));
        match crate::Config::load(&config_path) {
            Ok(c) => (c.clone(), Duration::from_secs(c.worker.timeout_secs)),
            Err(_) => {
                // Fall back to default config with 60s timeout
                (
                    crate::Config {
                        project: crate::core::config::ProjectConfig {
                            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                        },
                        worker: crate::core::config::WorkerConfig {
                            default_model: "nvidia/z-ai/glm-5.2".to_string(),
                            timeout_secs: 60,
                            thinking_level: "low".to_string(),
                            provider: None,
                        },
                        sdd: crate::core::config::SddConfig::default(),
                        models: crate::core::config::ModelsConfig::default(),
                        agent: crate::core::config::AgentConfig::default(),
                        rpc: crate::core::config::RpcConfig::default(),
                    },
                    Duration::from_secs(60),
                )
            }
        }
    };
    let worker: Box<dyn crate::Worker> =
        crate::worker::build_worker(&dag, worker_timeout, &config, concurrency)
            .map_err(|e| format!("Failed to build worker: {}", e))?;

    // Create scheduler — `with_checkpoint` when a checkpoint was loaded
    // above (resumes the prior run), else plain `new` (fresh run).
    let mut scheduler = match checkpoint {
        Some(cp) => {
            Scheduler::with_checkpoint(dag.clone(), worker, composite_sink, concurrency, cp)
        }
        None => Scheduler::new(dag.clone(), worker, composite_sink, concurrency),
    };

    // Run the DAG
    let mut report = match scheduler.run(allow_paid).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error during DAG execution: {}", e);
            std::process::exit(1);
        }
    };
    // Update the report with the actual write failure count from RedbSink
    report.store_write_failures = write_failures.load(std::sync::atomic::Ordering::SeqCst);

    // T2b: a run whose vault rejected writes is DEGRADED, not successful. Say so
    // loudly. `.pidag/` is the only record of a run, and the whole defect this
    // replaces was that a failed vault write was indistinguishable from a clean
    // one -- the run completed, reported success, and left no trace of the gap.
    if report.store_write_failures > 0 {
        eprintln!(
            "warning: run completed with {} vault write failure(s) -- this run's record is \
             INCOMPLETE. Node states, artifacts or timings are missing from the vault, so \
             `pidag show {}` and any resume from this run will be unreliable.",
            report.store_write_failures, run_id
        );
    }

    // Build states map from report for rendering
    let mut states: HashMap<String, (String, Option<String>)> = HashMap::new();
    for node_state in &report.node_states {
        states.insert(
            node_state.node_id.clone(),
            (
                node_state.state.as_str().to_string(),
                node_state.output.clone(),
            ),
        );
    }

    // Update run metadata with completion info
    let completed_at = chrono::Utc::now().to_rfc3339();
    let final_run_meta = crate::RunMeta {
        run_id: run_id.clone(),
        dag_json,
        started_at,
        completed_at: Some(completed_at),
        successful_nodes: report
            .node_states
            .iter()
            .filter(|n| n.state == NodeStatus::Done)
            .count(),
        failed_nodes: report.failed.len(),
    };
    store
        .put_run(&final_run_meta)
        .await
        .map_err(|e| format!("Failed to update run metadata: {}", e))?;

    // Render and print status
    let status = crate::render_status(&dag, &states);
    println!("\n{}", status);

    // Print summary
    println!("Run ID: {} (vault: {})", run_id, vault_path.display());
    println!(
        "Show results with: pidag show {} --vault {}",
        run_id,
        vault_path.display()
    );

    // Exit with appropriate code
    if report.failed.is_empty() {
        Ok(())
    } else {
        eprintln!("failed nodes: {:?}", report.failed);
        Err("one or more nodes failed".into())
    }
}

/// Compute the lock path for a given vault and run_id.
///
/// The lock is per run_id, not per vault, to allow:
/// - Multiple different runs in the same project (different run_ids)
/// - The UI serving while a run executes (per-run-id locking)
///
/// Lock path: `<vault_dir>/.locks/<run_id>.pid`
fn run_lock_path(vault_path: &Path, run_id: &str) -> PathBuf {
    let vault_dir = vault_path.parent().unwrap_or_else(|| Path::new(".pidag"));
    vault_dir.join(".locks").join(format!("{}.pid", run_id))
}

/// Read the PID from a lock file (if any).
fn read_lock_pid(lock_path: &Path) -> Option<u32> {
    let content = std::fs::read_to_string(lock_path).ok()?;
    content.trim().parse::<u32>().ok()
}

fn generate_run_id() -> String {
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");

    // Simple random suffix: use system time nanoseconds for pseudo-randomness
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = now.subsec_nanos();
    let rand_suffix = format!("{:06x}", nanos % 0xffffff);

    format!("run-{}-{}", timestamp, rand_suffix)
}
