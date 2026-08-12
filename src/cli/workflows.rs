//! `pidag workflows` command - list and inspect available workflows

use std::path::PathBuf;

/// Result of parsing `pidag workflows` command arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowsCommand {
    /// List all available workflows (no arguments)
    List,
    /// Show help (--help or -h)
    Help,
    /// Show a specific workflow by name
    Show(String),
    /// Unknown subcommand (error case)
    Unknown(String),
}

/// Parse raw command arguments into a WorkflowsCommand.
///
/// Rules:
/// - `[]` (no args) → `List`
/// - `["--help"]` or `["-h"]` → `Help`
/// - `["show", name]` → `Show(name)`
/// - `["show"]` with no name → `Unknown("show")`
/// - anything else → `Unknown(arg)`
pub fn parse_args(args: &[String]) -> WorkflowsCommand {
    if args.is_empty() {
        // R1: Empty args should list, not show help
        return WorkflowsCommand::List;
    }

    match args[0].as_str() {
        "--help" | "-h" => WorkflowsCommand::Help,
        "show" => {
            if args.len() >= 2 {
                WorkflowsCommand::Show(args[1].clone())
            } else {
                // show without a name is an error
                WorkflowsCommand::Unknown("show".to_string())
            }
        }
        unknown => {
            // R1.4: Unknown subcommands are errors, not silently listed
            WorkflowsCommand::Unknown(unknown.to_string())
        }
    }
}

pub async fn workflows(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let command = parse_args(args);

    let project_root = PathBuf::from(".");

    match command {
        WorkflowsCommand::Help => {
            print!(
                "pidag workflows - List and inspect available workflows\n\
                 \nUSAGE:\n  pidag workflows\n  pidag workflows show <name>\n\
                 \nSubcommands:\n  workflows            List all available workflows\n\
                 workflows show <name>  Render an expanded DAG (without running)\n"
            );
            Ok(())
        }
        WorkflowsCommand::List => {
            // List available workflows
            let workflows = crate::workflow::WorkflowEngine::list_workflows(Some(&project_root))
                .map_err(|e| format!("Failed to list workflows: {}", e))?;

            println!("Available workflows:");
            for workflow in workflows {
                let marker = if workflow == "sdd" { " (default)" } else { "" };
                println!("  {}{}", workflow, marker);
            }

            Ok(())
        }
        WorkflowsCommand::Show(workflow_name) => {
            // Load the template
            let template =
                crate::workflow::WorkflowEngine::load_template(&workflow_name, Some(&project_root))
                    .map_err(|e| format!("Failed to load workflow {}: {}", workflow_name, e))?;

            // Create a dummy spec context for expansion
            let config_path = project_root.join(".pidag").join("config.toml");
            let config =
                crate::Config::load(&config_path).unwrap_or_else(|_| crate::Config::default());

            let iterations = template.iterations.unwrap_or(3);

            // Build context
            let mut prompts = std::collections::HashMap::new();
            prompts.insert(
                "tdd_contract".to_string(),
                "TDD Contract section".to_string(),
            );
            prompts.insert(
                "architecture".to_string(),
                "Architecture section".to_string(),
            );
            prompts.insert("guardrails".to_string(), "Guardrails section".to_string());
            prompts.insert("research".to_string(), "Research topic".to_string());

            let context = crate::workflow::TemplateContext {
                n: 0,
                spec_path: "spec.md".to_string(),
                project_root: project_root.display().to_string(),
                validate_script: config.sdd.validate_script.display().to_string(),
                quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
                prompts,
                models_config: config.models,
            };

            // Expand the template
            let dag = crate::workflow::WorkflowEngine::expand(&template, iterations, context)
                .map_err(|e| format!("Failed to expand workflow: {}", e))?;

            // Render as mermaid
            let mermaid = crate::render_dag_mermaid(&dag);
            println!("{}", mermaid);

            Ok(())
        }
        WorkflowsCommand::Unknown(arg) => {
            eprintln!("error: unknown subcommand '{}'", arg);
            std::process::exit(1);
        }
    }
}
