//! `pidag list` command - list all stored runs

use crate::RedbStore;
use std::path::PathBuf;

pub async fn list(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut vault_path = PathBuf::from(".pidag/pidag.redb");

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
            _ => {
                eprintln!("error: unknown option: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Check if vault exists
    if !vault_path.exists() {
        println!("No runs found (vault does not exist yet)");
        return Ok(());
    }

    // Open vault store
    let _store =
        RedbStore::open(&vault_path).map_err(|e| format!("Failed to open vault: {}", e))?;

    // For now, we don't have a list_runs method in the Store trait,
    // so we'll just indicate that the vault exists.
    // In a full implementation, the Store would expose all run_ids.
    println!("Vault: {}", vault_path.display());
    println!("Note: full run enumeration not yet implemented in Store trait");
    println!("Use 'pidag show <run_id>' to view a specific run");

    Ok(())
}
