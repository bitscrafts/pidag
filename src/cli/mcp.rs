//! `pidag mcp` command - start MCP (Model Context Protocol) stdio server

use std::path::PathBuf;

pub async fn mcp(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
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

    crate::mcp::run_mcp_server(vault_path, concurrency)
        .await
        .map_err(|e| format!("MCP Server error: {}", e).into())
}
