/// Generate the legacy `dag_json` fixture for spec-37 C8 (wire-compatibility
/// guard for `Node.verify: Option<String>` -> `Option<Verify>`).
///
/// The JSON below is a **hand-written literal**, not produced by calling
/// `serde_json::to_string` on the current `Node` type. That is deliberate:
/// if this generator derived the fixture from the live struct, a change to
/// `Node`/`Verify` could silently reshape the "pre-spec-37" fixture into
/// whatever the new code produces, and the compatibility guard it exists to
/// prove would stop proving anything -- the exact failure mode spec-34 had
/// and spec-36 fixed for the vault fixture (see `tests/gen_legacy_vault.rs`,
/// `docs/FINDINGS.md`).
///
/// IGNORED ON PURPOSE -- run only via
/// `cargo test --test gen_legacy_dag -- --ignored`. Never run this to
/// "refresh" the fixture after a `Node`/`Verify` change; that would defeat
/// the guard the same way described above.
use std::path::PathBuf;

const LEGACY_DAG_JSON: &str = r#"{"nodes":[{"id":"build","prompt":"cargo build","depends_on":[],"models":[],"retry":{"attempts":1,"backoff_ms":0},"validate":null,"node_type":"shell","gate":null,"timeout":null,"mcp_call":null,"after":[],"verify":"test -f out.txt","verify_pre":null},{"id":"report","prompt":"echo done","depends_on":["build"],"models":[],"retry":{"attempts":1,"backoff_ms":0},"validate":null,"node_type":"shell","gate":null,"timeout":null,"mcp_call":null,"after":[],"verify":null,"verify_pre":null}],"metadata":null}"#;

#[test]
#[ignore]
fn gen_legacy_dag_fixture() {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy_dag");
    std::fs::create_dir_all(&fixture_dir).expect("create fixture dir");

    let fixture_path = fixture_dir.join("legacy_dag.json");
    std::fs::write(&fixture_path, LEGACY_DAG_JSON).expect("write legacy_dag.json");

    println!("Generated legacy dag_json fixture at {:?}", fixture_path);
    println!(
        "  Two nodes: build (verify: bare string \"test -f out.txt\"), report (verify: null)."
    );
    println!("  This fixture is the compatibility guard for spec-37 C8/C2.");
    println!("  Do NOT regenerate after a Node/Verify change -- commit this fixture first!");
}
