//! `pidag describe` command - render DAG as markdown with mermaid flowchart

use std::io::Write;
use std::path::PathBuf;

pub async fn describe(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        eprintln!("error: 'describe' requires a <dag.json> argument");
        eprintln!("Usage: pidag describe <dag.json> [--output PATH]");
        std::process::exit(1);
    }

    let dag_path = &args[0];
    let mut output: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--output" => {
                if i + 1 < args.len() {
                    output = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("error: --output requires an argument");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                eprintln!("Usage: pidag describe <dag.json> [--output PATH]");
                eprintln!();
                eprintln!("Render the DAG as a markdown document with an embedded mermaid");
                eprintln!("flowchart (nodes grouped by node_type, edges from depends_on) and");
                eprintln!("a node details table. Prints to stdout unless --output is given.");
                eprintln!();
                eprintln!("OPTIONS:");
                eprintln!(
                    "    --output <PATH>   Write the markdown document to PATH instead of stdout"
                );
                return Ok(());
            }
            _ => {
                eprintln!("error: unknown option: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    let dag_json =
        std::fs::read_to_string(dag_path).map_err(|e| format!("Failed to read DAG file: {}", e))?;
    let dag: crate::Dag =
        serde_json::from_str(&dag_json).map_err(|e| format!("Failed to parse DAG JSON: {}", e))?;

    // Use the basename (file stem) for the header title so reproducible runs on
    // the same DAG produce the same document regardless of CWD.
    let title = std::path::Path::new(dag_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(dag_path);

    let mut doc = String::new();
    doc.push_str(&format!("# DAG: {}\n\n", title));
    doc.push_str(&crate::render_dag_mermaid(&dag));

    match output {
        Some(path) => {
            let p = PathBuf::from(&path);
            if let Some(parent) = p.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&p, &doc)?;
            eprintln!("pidag describe: wrote {}", p.display());
        }
        None => {
            // Write to stdout atomically via a single write to avoid interleaving.
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(doc.as_bytes())?;
        }
    }
    Ok(())
}
