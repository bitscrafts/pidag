//! `pidag show` command - display results of a completed run

use crate::{RedbStore, Store};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub async fn show(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        eprintln!("error: 'show' requires a <run_id> argument");
        std::process::exit(1);
    }

    let run_id = &args[0];
    let mut vault_path = PathBuf::from(".pidag/pidag.redb");

    // Parse optional arguments
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--vault" => {
                if i + 1 < args.len() {
                    vault_path = PathBuf::from(&args[i + 1]);
                    i += 2;
                } else {
                    eprintln!("error: --vault requires an argument");
                    std::process::exit(1);
                }
            }
            _ => {
                eprintln!("error: unknown option: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Open vault store
    let redb_store =
        RedbStore::open(&vault_path).map_err(|e| format!("Failed to open vault: {}", e))?;
    let store: Arc<dyn Store> = Arc::new(redb_store);

    // Load run metadata
    let run = store
        .get_run(run_id)
        .await
        .map_err(|e| format!("Failed to retrieve run: {}", e))?
        .ok_or_else(|| format!("Run not found: {}", run_id))?;

    // Parse stored DAG JSON
    let dag: crate::Dag = serde_json::from_str(&run.dag_json)
        .map_err(|e| format!("Failed to parse stored DAG: {}", e))?;

    // Load all node states
    let node_records = store
        .list_nodes(run_id)
        .await
        .map_err(|e| format!("Failed to list nodes: {}", e))?;

    // Build states map
    let mut states: HashMap<String, (String, Option<String>)> = HashMap::new();
    for record in &node_records {
        // Try to load artifact for this node
        let artifact = store
            .get_artifact(run_id, &record.node_id)
            .await
            .ok()
            .flatten();
        states.insert(
            record.node_id.clone(),
            (record.state.as_str().to_string(), artifact),
        );
    }

    // Render status
    let status = crate::render_status(&dag, &states);
    println!("\n{}", status);

    // Print run metadata
    println!("\nRun metadata:");
    println!("  ID: {}", run.run_id);
    println!("  Started: {}", run.started_at);
    if let Some(completed) = run.completed_at {
        println!("  Completed: {}", completed);
    }
    println!("  Successful: {}", run.successful_nodes);
    println!("  Failed: {}", run.failed_nodes);

    // Print per-node artifacts
    println!("\nNode artifacts:");
    for record in &node_records {
        if let Some(artifact) = states.get(&record.node_id).and_then(|(_, a)| a.clone()) {
            let preview = if artifact.len() > 100 {
                format!("{}...", &artifact[..100])
            } else {
                artifact.clone()
            };
            println!("  {}: {}", record.node_id, preview);
        }
    }

    // Print event timeline (compact log)
    let events = store
        .load_events(run_id)
        .await
        .map_err(|e| format!("Failed to load events: {}", e))?;

    if !events.is_empty() {
        println!("\nEvent log:");
        for (idx, event) in events.iter().enumerate() {
            println!("  [{}] {:?}", idx, event);
        }
    }

    Ok(())
}
