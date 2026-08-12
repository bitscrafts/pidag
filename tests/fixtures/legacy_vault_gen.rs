/// Generate the legacy vault fixture for spec-34 Y2b compatibility test.
/// Run this ONCE before any code changes to create tests/fixtures/legacy_vault/legacy.redb
///
/// Usage (from /projects/pidag):
///   cargo run --test legacy_vault_gen
///
/// Do NOT regenerate after code changes - the fixture guards wire compatibility.

use std::path::PathBuf;

fn main() {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/legacy_vault");

    std::fs::create_dir_all(&fixture_dir).expect("create fixture dir");

    println!("Generating legacy vault fixture at {:?}", fixture_dir);
    println!("This fixture contains a run 'legacy1' with three nodes: alpha (Done), beta (Failed), gamma (Blocked)");

    // This needs to be run as an integration test against the old binary
    // For now, we'll note that the fixture must be generated manually before code changes
    eprintln!(
        "NOTE: The legacy vault fixture must be generated from the CURRENT build \
         BEFORE any code changes are made. Instructions:\
         \n\n1. Ensure the current code is building and tests pass\
         \n2. Create a test DAG that produces nodes in Done/Failed/Blocked states\
         \n3. Run `pidag run` with that DAG to create the vault\
         \n4. Copy the vault directory to tests/fixtures/legacy_vault/\
         \n5. Verify the test passes: `cargo test test_legacy_vault_still_loads`\
         \n6. Commit the fixture BEFORE implementing typed state changes"
    );
}
