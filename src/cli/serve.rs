//! `pidag serve` command - start JSON-RPC 2.0 stdio server

use std::path::PathBuf;

pub async fn serve(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut vault_path = PathBuf::from(".pidag/pidag.redb");
    let mut concurrency = 4usize;

    // Parse optional arguments
    let mut i = 0;
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
            "--concurrency" => {
                if i + 1 < args.len() {
                    concurrency = args[i + 1].parse().unwrap_or(4);
                    i += 2;
                } else {
                    eprintln!("error: --concurrency requires an argument");
                    std::process::exit(1);
                }
            }
            _ => {
                eprintln!("error: unknown option: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    eprintln!("[rpc] starting JSON-RPC server on stdin/stdout");
    eprintln!("[rpc] vault: {}", vault_path.display());
    eprintln!("[rpc] concurrency: {}", concurrency);

    // Create vault directory if needed
    if let Some(parent) = vault_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create vault directory: {}", e))?;
    }

    let mut server = crate::RpcServer::new(concurrency, vault_path);
    server
        .run()
        .await
        .map_err(|e| format!("Server error: {}", e).into())
}
