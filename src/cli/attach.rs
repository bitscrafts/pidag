//! `pidag attach` command - initialize pidag in a project

use std::path::PathBuf;

pub async fn attach(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut project_root = PathBuf::from(".");

    // Parse optional arguments
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project-root" => {
                if i + 1 < args.len() {
                    project_root = PathBuf::from(&args[i + 1]);
                    i += 2;
                } else {
                    eprintln!("error: --project-root requires an argument");
                    std::process::exit(1);
                }
            }
            _ => {
                eprintln!("error: unknown option: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Create .pidag directory
    let pidag_dir = project_root.join(".pidag");
    std::fs::create_dir_all(&pidag_dir)
        .map_err(|e| format!("Failed to create .pidag directory: {}", e))?;

    // Create config.toml if it doesn't exist
    let config_path = pidag_dir.join("config.toml");
    if !config_path.exists() {
        let config_content = crate::Config::default_config_toml(&project_root);

        std::fs::write(&config_path, config_content)
            .map_err(|e| format!("Failed to write config.toml: {}", e))?;
    }

    println!("Initialized pidag in {}", pidag_dir.display());
    println!("Config: {}", config_path.display());

    Ok(())
}
