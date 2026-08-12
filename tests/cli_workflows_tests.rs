//! Tests for `pidag workflows` command parsing (R1).
//!
//! Verifies that the workflows command correctly dispatches based on arguments:
//! - Empty args → List (the fix for the bug)
//! - --help / -h → Help
//! - show <name> → Show(name)
//! - show without name → Unknown (not a panic)
//! - Unknown subcommand → Unknown

#[test]
fn test_workflows_no_args_lists() {
    // R1a: Empty argument list should return List, not Help.
    // This test MUST fail against the current code (which returns Help).
    let result = pidag::cli::workflows::parse_args(&[]);
    assert_eq!(
        result,
        pidag::cli::workflows::WorkflowsCommand::List,
        "Empty args should dispatch to List, not Help"
    );
}

#[test]
fn test_workflows_help_flag_is_help() {
    // R1b: Both --help and -h should return Help.
    let result_help = pidag::cli::workflows::parse_args(&["--help".to_string()]);
    assert_eq!(result_help, pidag::cli::workflows::WorkflowsCommand::Help);

    let result_h = pidag::cli::workflows::parse_args(&["-h".to_string()]);
    assert_eq!(result_h, pidag::cli::workflows::WorkflowsCommand::Help);
}

#[test]
fn test_workflows_show_parses_name() {
    // R1c: "show" with a name argument should return Show(name).
    let result = pidag::cli::workflows::parse_args(&["show".to_string(), "sdd".to_string()]);
    assert_eq!(
        result,
        pidag::cli::workflows::WorkflowsCommand::Show("sdd".to_string())
    );
}

#[test]
fn test_workflows_show_without_name_errors() {
    // R1d: "show" without a name should return Unknown, not panic.
    let result = pidag::cli::workflows::parse_args(&["show".to_string()]);
    match result {
        pidag::cli::workflows::WorkflowsCommand::Unknown(_) => {
            // This is the expected case
        }
        _ => {
            panic!(
                "Expected Unknown variant for 'show' without name, got {:?}",
                result
            );
        }
    }
}

#[test]
fn test_workflows_unknown_subcommand_errors() {
    // R1e: An unknown subcommand should return Unknown, NOT List.
    // This fixes the mirror-image bug where any junk argument silently lists.
    let result = pidag::cli::workflows::parse_args(&["bogus".to_string()]);
    assert_eq!(
        result,
        pidag::cli::workflows::WorkflowsCommand::Unknown("bogus".to_string()),
        "Unknown subcommand should be Unknown, not List"
    );
}
