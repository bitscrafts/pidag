//! `pidag sdd` command - generate SDD loop DAG from spec

use std::path::{Path, PathBuf};

pub async fn sdd(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    // --help / -h short-circuits BEFORE the spec-name validation so
    // `pidag sdd --help` does not get misparsed as a malformed spec filename.
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!(
            "pidag sdd - Generate a DAG from a spec using a workflow template.\n\
             \nUSAGE:\n  pidag sdd <NN-spec.md> [--run] [--resume] [--fresh]\n\
                     [--retry-failed] [--allow-paid] [--model NAME] [--provider NAME]\n\
                     [--concurrency N] [--project-root PATH] [--workflow NAME] [--iterations N]\n\n\
             The spec must be named NN-<slug>.md (e.g., 01-my-feature.md).\n\
             --provider sets the LLM provider explicitly (e.g. google, nvidia,\n\
             deepseek); when a model string already carries a provider/model\n\
             prefix the worker forwards it automatically (spec-13).\n\
             --resume re-enters a previously-interrupted run using a\n\
             deterministic run-id derived from the spec path.\n\
             --workflow selects a workflow template (default: sdd; built-in: research).\n\
             --iterations sets iteration count (overrides template and config).\n"
        );
        return Ok(());
    }
    if args.is_empty() {
        eprintln!("error: 'sdd' requires a <spec.md> argument");
        std::process::exit(1);
    }

    let spec_path = &args[0];
    let mut run = false;
    let mut allow_paid = false;
    let mut concurrency = 4usize;
    let mut project_root = PathBuf::from(".");
    let mut cli_model: Option<String> = None;
    let mut workflow_name = "sdd".to_string();
    let mut iterations_override: Option<usize> = None;
    // Resume flags (Spec-08). Forwarded to `pidag run` along with a
    // deterministic `--run-id` derived from the spec so an interrupted run
    // can be resumed by re-running the same `pidag sdd <spec> --run --resume`.
    let mut resume = false;
    let mut fresh = false;
    let mut retry_failed = false;

    // Parse optional arguments
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--run" => {
                run = true;
                i += 1;
            }
            "--allow-paid" => {
                allow_paid = true;
                i += 1;
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
            "--concurrency" => {
                if i + 1 < args.len() {
                    concurrency = args[i + 1].parse().unwrap_or(4);
                    i += 2;
                } else {
                    eprintln!("error: --concurrency requires an argument");
                    std::process::exit(1);
                }
            }
            "--project-root" => {
                if i + 1 < args.len() {
                    project_root = PathBuf::from(&args[i + 1]);
                    i += 2;
                } else {
                    eprintln!("error: --project-root requires an argument");
                    std::process::exit(1);
                }
            }
            "--model" => {
                if i + 1 < args.len() {
                    cli_model = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("error: --model requires an argument");
                    std::process::exit(1);
                }
            }
            "--workflow" => {
                if i + 1 < args.len() {
                    workflow_name = args[i + 1].clone();
                    i += 2;
                } else {
                    eprintln!("error: --workflow requires an argument");
                    std::process::exit(1);
                }
            }
            "--iterations" => {
                if i + 1 < args.len() {
                    iterations_override = args[i + 1].parse().ok();
                    if iterations_override.is_none() {
                        eprintln!("error: --iterations requires a positive integer");
                        std::process::exit(1);
                    }
                    i += 2;
                } else {
                    eprintln!("error: --iterations requires an argument");
                    std::process::exit(1);
                }
            }
            _ => {
                eprintln!("error: unknown option: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // R5 numbered-spec enforcement: the spec filename MUST follow the
    // `NN-<slug>.md` pattern (e.g., 01-fibonacci.md). Specs without a numeric
    // prefix are rejected with a clear, actionable error before any work is
    // done. This is the CLI gateway for the shared `validate_spec_name` rule.
    let spec_file_name = std::path::Path::new(spec_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if let Err(e) = crate::validate_spec_name(&spec_file_name) {
        eprintln!("error: {} (got: {})", e, spec_file_name);
        eprintln!(
            "Please rename the spec to a numbered form, e.g. 'specs/01-{}.md'.",
            spec_file_name.trim_end_matches(".md")
        );
        std::process::exit(1);
    }

    // Load config from .pidag/config.toml (relative to project_root) or use defaults.
    let config_path = project_root.join(".pidag").join("config.toml");
    let config =
        crate::Config::load(&config_path).map_err(|e| format!("Failed to load config: {}", e))?;
    let models = config.resolve_models(cli_model.as_deref());

    // Generate DAG from spec using the specified workflow template
    let dag = crate::SddGenerator::from_spec_with_workflow(
        &PathBuf::from(spec_path),
        &project_root,
        &models,
        &config.sdd,
        &workflow_name,
        iterations_override,
    )
    .map_err(|e| format!("Failed to generate DAG: {}", e))?;

    // Serialize to JSON and print
    let dag_json = serde_json::to_string_pretty(&dag)
        .map_err(|e| format!("Failed to serialize DAG: {}", e))?;

    if !run {
        // Just print the DAG JSON
        println!("{}", dag_json);
        return Ok(());
    }

    // --run: execute the DAG
    println!("Generated SDD DAG with {} nodes", dag.nodes.len());
    println!(
        "Executing with concurrency={}, allow_paid={}",
        concurrency, allow_paid
    );

    // Write DAG to .pidag/<spec-stem>.json for execution
    // R6/R10 from spec-03: DAG is named after the spec file stem (e.g.,
    // specs/01-fibonacci.md -> .pidag/01-fibonacci.json).
    let spec_stem = std::path::Path::new(spec_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let dag_path = PathBuf::from(format!(".pidag/{}.json", spec_stem));
    std::fs::create_dir_all(".pidag").ok();
    std::fs::write(&dag_path, &dag_json).map_err(|e| format!("Failed to write DAG: {}", e))?;

    // Derive a deterministic run_id from the spec path + content so an
    // interrupted `pidag sdd <spec> --run` can be resumed by re-running the
    // same command with `--resume` (Spec-08). Use the canonicalized spec
    // path so a relative vs absolute invocation maps to the same run_id.
    let run_id = {
        let abs_spec = if Path::new(spec_path).is_absolute() {
            PathBuf::from(spec_path)
        } else {
            project_root.join(spec_path)
        };
        let content = std::fs::read_to_string(&abs_spec)
            .map_err(|e| format!("Failed to read spec for run_id derivation: {}", e))?;
        crate::sdd::run_id_for_spec(&abs_spec, &content)
    };

    // Run the DAG via `pidag run`. Forward the deterministic --run-id plus
    // the resume flags so the run subcommand can load/apply a checkpoint.
    // `--fresh` is forwarded but takes precedence over `--resume` on the
    // run side (run.rs skips load_checkpoint when --fresh is set).
    let mut cmd = std::process::Command::new(crate::core::selfexe::self_exe());
    cmd.arg("run")
        .arg(dag_path.to_str().unwrap_or(".pidag/unknown.json"))
        .arg("--concurrency")
        .arg(concurrency.to_string())
        .arg("--vault")
        .arg(".pidag/pidag.redb")
        .arg("--run-id")
        .arg(&run_id);
    if allow_paid {
        cmd.arg("--allow-paid");
    }
    if resume {
        cmd.arg("--resume");
    }
    if fresh {
        cmd.arg("--fresh");
    }
    if retry_failed {
        cmd.arg("--retry-failed");
    }
    cmd.current_dir(&project_root);
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to execute pidag run: {}", e))?;

    println!("{}", String::from_utf8_lossy(&output.stdout));
    if !output.stderr.is_empty() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }

    std::process::exit(output.status.code().unwrap_or(1));
}
