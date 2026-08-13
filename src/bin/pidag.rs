//! pidag CLI -- deterministic, resilient multi-node LLM DAG executor.
//!
//! This is the minimal entry point for the pidag binary. All subcommands
//! are implemented in the `pidag::cli` module and called from here.

use pidag::cli;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_help();
        return Ok(());
    }

    match args[1].as_str() {
        "run" => cli::run(&args[2..]).await,
        "show" => cli::show(&args[2..]).await,
        "list" => cli::list(&args[2..]).await,
        "attach" => cli::attach(&args[2..]).await,
        "sdd" => cli::sdd(&args[2..]).await,
        "split" => cli::split(&args[2..]).await,
        "queue" => cli::queue(&args[2..]).await,
        "auto" => cli::auto(&args[2..]).await,
        "serve" => cli::serve(&args[2..]).await,
        "mcp" => cli::mcp(&args[2..]).await,
        "ui" => cli::ui(&args[2..]).await,
        "describe" => cli::describe(&args[2..]).await,
        "workflows" => cli::workflows(&args[2..]).await,
        "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "--version" | "-v" => {
            println!("pidag 0.1.0");
            Ok(())
        }
        _ => {
            eprintln!("unknown subcommand: {}", args[1]);
            print_help();
            std::process::exit(1);
        }
    }
}

fn print_help() {
    eprintln!(
        r#"pidag -- deterministic, resilient multi-node LLM DAG executor

USAGE:
    pidag run <dag.json> [--concurrency N] [--allow-paid] [--vault PATH] [--run-id ID] [--resume] [--fresh] [--retry-failed] [--max-model-calls N] [--max-tokens N]
    pidag show <run_id> [--vault PATH]
    pidag list [--vault PATH]
    pidag attach [--project-root PATH]
    pidag sdd <spec.md> [--run] [--resume] [--fresh] [--retry-failed] [--allow-paid] [--concurrency N] [--model <model>] [--project-root PATH] [--workflow <name>] [--iterations N]
    pidag split <spec.md> [--into N | --auto | --validate]
    pidag queue [--status|--round-robin|--reset|--retry-failed] [--workspace PATH]
    pidag auto [--workspace PATH | --project-root PATH] [--model M] [--timeout SECS]
    pidag serve [--vault PATH] [--concurrency N]
    pidag mcp [--vault PATH] [--concurrency N]
    pidag ui [--port 4600] [--host 127.0.0.1] [--vault PATH]
    pidag describe <dag.json> [--output PATH]
    pidag workflows [show <name>]
    pidag --help
    pidag --version

SUBCOMMANDS:
    run <dag.json>          Execute a DAG from a JSON file
    show <run_id>           Display results of a completed run
    list                    List all stored runs
    attach                  Initialize pidag in a project
    sdd <spec.md>           Generate DAG from spec using a workflow template
    split <spec.md>         Split spec into child specs with coverage validation
    queue                   Carousel priority queue with round-robin scheduling
    auto                    Autonomous driver: read handoff, dispatch next spec/DAG
    serve                   Start JSON-RPC 2.0 stdio server
    mcp                     Start MCP (Model Context Protocol) stdio server
    ui                      Start the trace UI web server (local-only by default)
    describe <dag.json>     Render the DAG as a markdown doc with a mermaid flowchart
    workflows               List available workflows; workflows show <name> renders a DAG
    --help                  Show this message
    --version               Show version

OPTIONS:
    --concurrency N         Max parallel nodes (default: 4)
    --allow-paid            Allow paid model calls
    --vault PATH            Path to vault database (default: .pidag/pidag.redb)
    --run-id ID             Deterministic run id (run/sdd; sdd derives from spec hash)
    --resume                Resume an interrupted run from checkpoint (sdd + run)
    --fresh                 Force clean start, ignoring any existing checkpoint
    --retry-failed          Retry nodes that failed in a previous run (use with --resume)
    --max-model-calls N     Abort the run once model-consuming dispatches would exceed N
                            (shell/quorum nodes don't count). Counters persist in the vault
                            and accumulate across --resume, so a raised ceiling picks up
                            from where the prior run stopped, not from zero. Nodes already
                            in flight when the ceiling trips are not cancelled (pidag cannot
                            cancel mid-call), so a run may OVERSHOOT the ceiling by at most
                            the in-flight set -- i.e. up to --concurrency extra dispatches.
                            On breach the run exits with status 3 (distinct from an ordinary
                            node failure's status 1); resume with --resume --run-id after
                            raising the ceiling.
    --max-tokens N          Abort the run once cumulative reported tokens would exceed N.
                            STARTUP ERROR if the configured backend does not report token
                            usage (the default `pi -p` print-mode path never does) -- pidag
                            refuses to start rather than silently not enforcing the ceiling.
                            Use --max-model-calls on backends that can't report tokens.
                            Same overshoot and persistence behaviour as --max-model-calls.
    --project-root PATH     Project root for attach (default: current directory)
    --run                   Execute DAG immediately (for sdd)
    --resume                Resume an interrupted run from checkpoint (sdd only)
    --fresh                 Force clean start, ignoring any existing checkpoint (sdd only)
    --retry-failed          Retry nodes that failed in a previous run (sdd only, use with --resume)
    --model <model>         Override default model chain (sdd only; takes precedence over
                            PIDAG_DEFAULT_MODEL and .pidag/config.toml [models])
    --workflow <name>       Select workflow template (sdd only; default: sdd; built-in: research)
    --iterations N          Set iteration count (sdd only; overrides config and template)
    --into N                Split into exactly N child specs (split only)
    --auto                  Auto-determine split count (split only)
    --validate              Validate coverage report (split only)
    --port <N>              Trace UI port (default: 4600)
    --host <ADDR>           Trace UI bind address (default: 127.0.0.1)
    --output PATH           Write describe output to PATH instead of stdout (describe only)
"#
    );
}
