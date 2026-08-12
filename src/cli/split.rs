//! CLI subcommand: pidag split <spec.md> [--into N | --auto | --validate]

use crate::split::*;
use std::path::{Path, PathBuf};

/// Handle `pidag split` subcommand.
pub async fn split(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        eprintln!("Usage: pidag split <spec.md> [--into N | --auto | --validate]");
        eprintln!("       pidag split --help");
        return Err("No spec file provided".into());
    }

    // Check for help
    if args[0] == "--help" || args[0] == "-h" {
        print_help();
        return Ok(());
    }

    // Parse arguments. Flags may appear in any position; the first non-flag
    // argument is the spec path. `--validate` with no spec validates the
    // report at the CWD (falls back to `<cwd>/.pidag/split-coverage.json`).
    let mut spec_path: Option<String> = None;
    let mut mode = SplitMode::Analyze;
    let mut n_value: Option<usize> = None;
    let mut ordered = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--into" => {
                if i + 1 >= args.len() {
                    return Err("--into requires a number".into());
                }
                n_value = Some(args[i + 1].parse()?);
                mode = SplitMode::IntoN(0);
                i += 2;
            }
            "--auto" => {
                mode = SplitMode::Auto;
                i += 1;
            }
            "--validate" => {
                mode = SplitMode::Validate;
                i += 1;
            }
            "--ordered" => {
                ordered = true;
                i += 1;
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            s if s.starts_with('-') => {
                return Err(format!("Unknown option: {}", s).into());
            }
            _ => {
                spec_path = Some(args[i].clone());
                i += 1;
            }
        }
    }

    // Resolve `--into N` mode with the parsed N.
    if let SplitMode::IntoN(_) = mode {
        let n = n_value.ok_or("--into requires a number")?;
        mode = SplitMode::IntoN(n);
    }

    match mode {
        SplitMode::Analyze => {
            let spec = spec_path.ok_or("No spec file provided")?;
            analyze_spec(&spec).await
        }
        SplitMode::IntoN(n) => {
            let spec = spec_path.ok_or("No spec file provided")?;
            split_spec(&spec, n, ordered).await
        }
        SplitMode::Auto => {
            let spec = spec_path.ok_or("No spec file provided")?;
            split_spec_auto(&spec, ordered).await
        }
        // `--validate`: use the spec path if provided, otherwise validate the
        // report at the CWD.
        SplitMode::Validate => validate_spec(spec_path.as_deref()).await,
    }
}

enum SplitMode {
    Analyze,
    IntoN(usize),
    Auto,
    Validate,
}

/// Analyze spec without splitting (print summary and proposal).
async fn analyze_spec(spec_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let spec =
        std::fs::read_to_string(spec_path).map_err(|e| format!("Failed to read spec: {}", e))?;

    let criteria =
        parse_exit_criteria(&spec).map_err(|e| format!("Failed to parse criteria: {}", e))?;
    let tests = parse_tdd_tests(&spec).map_err(|e| format!("Failed to parse tests: {}", e))?;

    let n_proposed = auto_determine_split_count(criteria.len());

    println!("Spec Analysis: {}", spec_path);
    println!("  Exit Criteria: {}", criteria.len());
    println!("  TDD Tests: {}", tests.len());
    println!("  Proposed Split: {} parts", n_proposed);

    if n_proposed == 1 {
        println!("  → No split needed (fits in one spec)");
    } else {
        println!("  → Suggested: pidag split {} --auto", spec_path);
    }

    Ok(())
}

/// Split spec into exactly N parts.
///
/// When `ordered` is true, criteria are assigned to parts sequentially
/// (contiguous ranges preserving the parent's order); otherwise module-grouping
/// heuristics are used to keep related criteria together.
async fn split_spec(
    spec_path: &str,
    n: usize,
    ordered: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let spec =
        std::fs::read_to_string(spec_path).map_err(|e| format!("Failed to read spec: {}", e))?;

    let criteria =
        parse_exit_criteria(&spec).map_err(|e| format!("Failed to parse criteria: {}", e))?;
    let tests = parse_tdd_tests(&spec).map_err(|e| format!("Failed to parse tests: {}", e))?;

    if n > criteria.len() {
        return Err(format!("Cannot split {} criteria into {} parts", criteria.len(), n).into());
    }

    if n == 1 {
        println!("Note: n=1 means no split (returning single spec)");
        return Ok(());
    }

    // Get parent filename stem
    let parent_stem = Path::new(spec_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Invalid spec filename")?;

    let parent_dir = Path::new(spec_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    // Split criteria into N parts (module-grouped by default, or ordered if
    // `--ordered` was requested).
    let parts = if ordered {
        split_into_n_parts_ordered(&criteria, n)
    } else {
        split_into_n_parts(&criteria, &tests, n)
    }
    .map_err(|e| format!("Failed to split: {}", e))?;

    // Generate child specs
    let mut child_specs = Vec::new();
    for (part_num, indices) in parts.iter().enumerate() {
        let part_num_1 = part_num + 1;
        let child_name = child_spec_filename(parent_stem, part_num_1);
        let child_path = parent_dir.join(&child_name);

        // Generate content
        let child_content = generate_child_spec_content(
            &spec,
            parent_stem,
            part_num_1,
            n,
            indices,
            &criteria,
            &parts,
        )
        .map_err(|e| format!("Failed to generate child spec: {}", e))?;

        // Write to temp file first
        let temp_path = child_path.with_extension("md.tmp");
        std::fs::write(&temp_path, &child_content)
            .map_err(|e| format!("Failed to write temp file: {}", e))?;

        // Atomic rename
        std::fs::rename(&temp_path, &child_path)
            .map_err(|e| format!("Failed to rename child spec: {}", e))?;

        // Find and associate tests with this part
        let part_tests = find_tests_for_criteria(&criteria, indices, &tests);

        child_specs.push((child_name.clone(), indices.clone(), part_tests));
        println!("Created: {} ({} criteria)", child_name, indices.len());
    }

    // Build and write coverage report
    let coverage = build_coverage_report(
        &format!("specs/{}.md", parent_stem),
        &child_specs,
        criteria.len(),
    );

    if !coverage.is_complete() {
        return Err(format!(
            "Coverage incomplete: {} orphaned criteria",
            coverage.coverage.orphaned_criteria.len()
        )
        .into());
    }

    // Write coverage JSON to <project_root>/.pidag/split-coverage.json. Project
    // root is derived from the spec path (parent dir, or one level up when the
    // spec lives under a `specs/` directory).
    let project_root = project_root_for(spec_path, parent_dir);
    let coverage_path = project_root.join(".pidag").join("split-coverage.json");
    write_coverage_json(&coverage, &coverage_path)
        .map_err(|e| format!("Failed to write coverage: {}", e))?;

    println!("\nCoverage: {}/{} (100%)", criteria.len(), criteria.len());
    println!("Report: {}", coverage_path.display());

    Ok(())
}

/// Derive the project root directory from a spec path.
///
/// pidag specs conventionally live under a `specs/` directory, so when the
/// spec's parent directory is named `specs`, the project root is one level up.
/// Otherwise the spec's own directory is used as the project root.
fn project_root_for(_spec_path: &str, parent_dir: &Path) -> PathBuf {
    if parent_dir.file_name().and_then(|n| n.to_str()) == Some("specs") {
        parent_dir
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| {
                // relative `specs/...` path: fall back to the CWD
                PathBuf::from(".")
            })
    } else {
        parent_dir.to_path_buf()
    }
}

/// Split spec automatically (determine N based on criteria count).
async fn split_spec_auto(spec_path: &str, ordered: bool) -> Result<(), Box<dyn std::error::Error>> {
    let spec =
        std::fs::read_to_string(spec_path).map_err(|e| format!("Failed to read spec: {}", e))?;

    let criteria =
        parse_exit_criteria(&spec).map_err(|e| format!("Failed to parse criteria: {}", e))?;

    let n = auto_determine_split_count(criteria.len());

    if n == 1 {
        println!(
            "No split needed ({} criteria, fits in one spec)",
            criteria.len()
        );
        return Ok(());
    }

    println!(
        "Auto-determined: {} parts (target 5-7 criteria each{})",
        n,
        if ordered { ", order-preserving" } else { "" }
    );
    split_spec(spec_path, n, ordered).await
}

/// Validate coverage report (check for orphaned criteria).
///
/// When a spec path is provided, the coverage report is located relative to the
/// spec's project root (so the report is found even when not run from the
/// project root). Without a spec, falls back to `<cwd>/.pidag/split-coverage.json`.
async fn validate_spec(spec_path: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    // Resolve the coverage report path. With a spec, derive its project root;
    // otherwise default to the current directory.
    let coverage_path = match spec_path {
        Some(path) => {
            let parent = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
            let root = project_root_for(path, parent);
            root.join(".pidag").join("split-coverage.json")
        }
        None => PathBuf::from(".pidag/split-coverage.json"),
    };

    if !coverage_path.exists() {
        return Err(format!("Coverage report not found ({})", coverage_path.display()).into());
    }

    let coverage = validate_coverage(&coverage_path)
        .map_err(|e| format!("Coverage validation failed: {}", e))?;

    println!(
        "Coverage validation: {}/{} (100%)",
        coverage.coverage.covered_criteria, coverage.coverage.total_parent_criteria
    );

    for child in &coverage.children {
        println!(
            "  {} ({} criteria)",
            child.spec,
            child.exit_criteria_indices.len()
        );
    }

    println!("Status: PASS");
    Ok(())
}

/// Find tests associated with a set of criteria.
fn find_tests_for_criteria(
    all_criteria: &[ExitCriterion],
    criteria_indices: &[usize],
    all_tests: &[String],
) -> Vec<String> {
    let mut result = Vec::new();

    for idx in criteria_indices {
        if let Some(criterion) = all_criteria.iter().find(|c| c.index == *idx) {
            let related = find_tdd_tests(criterion, all_tests);
            for test in related {
                if !result.contains(&test) {
                    result.push(test);
                }
            }
        }
    }

    result
}

fn print_help() {
    eprintln!(
        r#"pidag split — automatic spec splitting with coverage validation

USAGE:
    pidag split <spec.md>                 Analyze spec (propose split)
    pidag split <spec.md> --into N        Split into exactly N child specs
    pidag split <spec.md> --auto          Auto-determine N (target 5-7 criteria per child)
    pidag split <spec.md> --ordered       Order-preserving split (criteria stay in parent order)
    pidag split --validate                Validate coverage report
    pidag split --help                    Show this message

DESCRIPTION:
    Splits large specs (>7 exit criteria, >10 tests, >5 files) into smaller,
    manageable child specs. Maintains 100% coverage of all original criteria.

    Critical invariant: NO exit criteria may be orphaned in the split.
    A coverage report (.pidag/split-coverage.json) proves 100% coverage.

OPTIONS:
    --into N      Split into exactly N child specs
    --auto        Auto-determine N (recommended)
    --ordered     Join criteria to parts sequentially (preserves dependency order)
    --validate    Check coverage report for orphaned criteria
    --help        Show this message

EXAMPLES:
    # Analyze a large spec
    pidag split specs/01-big-feature.md

    # Split into 3 parts
    pidag split specs/01-big-feature.md --into 3

    # Auto-split (recommended)
    pidag split specs/01-big-feature.md --auto

    # Order-preserving auto-split (keeps criteria in parent order)
    pidag split specs/01-big-feature.md --auto --ordered

    # Validate coverage
    pidag split --validate
    pidag split specs/01-big-feature.md --validate

OUTPUT:
    Child specs are named NN-<parent>-partM.md and include Parent-Spec metadata.
    Coverage report: .pidag/split-coverage.json (JSON format)
"#
    );
}
