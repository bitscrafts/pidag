//! `pidag ui` command - start the trace UI web server

use crate::Store;
use std::path::PathBuf;
use std::sync::Arc;

pub async fn ui(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut port: u16 = 4600;
    let mut host: String = "127.0.0.1".to_string();
    let mut vault: Option<String> = None;
    let mut project_root: Option<String> = None;
    let mut workspace: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                if i + 1 < args.len() {
                    port = args[i + 1].parse().unwrap_or(4600);
                    i += 2;
                } else {
                    eprintln!("error: --port requires an argument");
                    std::process::exit(1);
                }
            }
            "--host" => {
                if i + 1 < args.len() {
                    host = args[i + 1].clone();
                    i += 2;
                } else {
                    eprintln!("error: --host requires an argument");
                    std::process::exit(1);
                }
            }
            "--vault" => {
                if i + 1 < args.len() {
                    vault = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("error: --vault requires an argument");
                    std::process::exit(1);
                }
            }
            "--project-root" => {
                if i + 1 < args.len() {
                    project_root = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("error: --project-root requires an argument");
                    std::process::exit(1);
                }
            }
            "--workspace" => {
                if i + 1 < args.len() {
                    workspace = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("error: --workspace requires an argument");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                eprintln!(
                    "Usage: pidag ui [--port 4600] [--host 127.0.0.1] [--vault PATH] [--project-root PATH] [--workspace PATH]"
                );
                eprintln!();
                eprintln!("Start the trace UI web server (local-only by default).");
                eprintln!();
                eprintln!("OPTIONS:");
                eprintln!("    --port <N>          Port to listen on (default: 4600)");
                eprintln!("    --host <ADDR>       Address to bind (default: 127.0.0.1)");
                eprintln!(
                    "    --vault <PATH>      Path to the pidag vault (default: .pidag/pidag.redb)"
                );
                eprintln!("    --project-root <P>  Project root for the Project Overview view");
                eprintln!("                         (default: current dir; enables #/project)");
                eprintln!(
                    "    --workspace <P>     Workspace root for multi-project mode: scan P for"
                );
                eprintln!(
                    "                         projects (specs/, Cargo.toml, pyproject.toml) and show"
                );
                eprintln!(
                    "                         them as cards; enables #/ and #/project/:name. The"
                );
                eprintln!(
                    "                         workspace vault is created at P/.pidag/pidag.redb."
                );
                return Ok(());
            }
            _ => {
                eprintln!("error: unknown option: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Workspace mode: scan a directory tree for projects and serve the
    // multi-project landing page (#/ + project cards). Mutually exclusive
    // with single-project --project-root mode (guardrail: don't break it).
    if let Some(ws) = &workspace {
        let ws_root = PathBuf::from(ws);
        if !ws_root.is_dir() {
            eprintln!("error: --workspace path is not a directory: {}", ws);
            std::process::exit(1);
        }

        // R4 / NF2: the workspace vault is created automatically on first
        // `pidag ui --workspace` invocation at <ws_root>/.pidag/pidag.redb.
        // Registering the vault via RedbStorePool::new creates the file lazily;
        // we touch the .pidag dir now so the vault path is guaranteed present.
        let vault_path = crate::RedbStorePool::new(ws_root.join(".pidag").join("pidag.redb"));
        // Force creation of the workspace vault so `.pidag/pidag.redb` exists
        // immediately after the first --workspace launch (NF2 / T10). We are
        // already inside the tokio runtime here, so await directly rather than
        // blocking on the current handle (which panics: "Cannot start a
        // runtime from within a runtime").
        let _ = vault_path.list_runs().await;

        let ws_vault_path = ws_root.join(".pidag").join("pidag.redb");
        eprintln!("pidag ui: workspace = {}", ws_root.display());
        eprintln!("pidag ui: workspace vault = {}", ws_vault_path.display());
        let store: Arc<dyn Store> = Arc::new(crate::RedbStorePool::new(ws_vault_path.clone()));
        return crate::ui::serve_workspace(store, ws_root, ws_vault_path, &host, port)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>);
    }

    let cwd = std::env::current_dir()?;
    let vault_path = vault
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::RedbStore::default_path(&cwd));
    eprintln!("pidag ui: vault = {}", vault_path.display());

    let root = project_root.map(PathBuf::from).or_else(|| {
        // Default to the current working directory so `pidag ui` launched
        // inside a project shows its specs without requiring --project-root.
        Some(cwd.clone())
    });
    if let Some(r) = &root {
        eprintln!("pidag ui: project_root = {}", r.display());
    }

    // Use `RedbStorePool` so the UI opens the vault per-request, releasing
    // the exclusive lock between polls. While an SDD run (via `pidag run`)
    // holds a persistent `RedbStore` lock, the UI's per-request opens will
    // retry on lock contention, which is acceptable latency for a 2-second
    // polling loop.
    let store = Arc::new(crate::RedbStorePool::new(vault_path));
    crate::ui::serve(store, root, &host, port).await?;
    Ok(())
}
