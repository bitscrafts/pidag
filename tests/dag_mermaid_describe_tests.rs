//! Tests for the `pidag describe` subcommand + `render_dag_mermaid` pure
//! function. See spec `specs/94-dag-mermaid-describe.md` (R1-R9,
//! T1-T8).
//!
//! T1-T7 are unit tests on `render_dag_mermaid(&dag) -> String`.
//! T8 is an integration test that spawns the `pidag` binary via
//! `env!("CARGO_BIN_EXE_pidag")` and verifies `--output <path>` writes
//! the document to disk.

use pidag::{Dag, ModelRef, Node, RetryPolicy, render_dag_mermaid};
use std::path::PathBuf;
use std::process::Command;

/// Build a single node with sensible defaults; override only the fields the
/// test cares about via the closure pattern. Keeps fixtures terse and
/// readable.
fn node(id: &str, f: impl FnOnce(&mut Node)) -> Node {
    let mut n = Node {
        id: id.to_string(),
        prompt: String::new(),
        depends_on: Vec::new(),
        models: Vec::new(),
        retry: RetryPolicy {
            attempts: 1,
            backoff_ms: 0,
        },
        validate: None,
        node_type: None,
        gate: None,
        timeout: None,
        mcp_call: None,
        after: vec![],
        verify: None,
        verify_pre: None,
        for_each: None,
        quorum: None,
    };
    f(&mut n);
    n
}

fn model(name: &str) -> ModelRef {
    ModelRef {
        name: name.to_string(),
        paid: false,
    }
}

#[test]
fn test_mermaid_empty_dag() {
    // T1: an empty DAG (zero nodes) must produce a valid document with no
    // panic. Header shows zero nodes / zero edges; the mermaid block has the
    // `flowchart TD` header but no subgraphs or edges; the details table has
    // only the column-header rows.
    let dag = Dag {
        metadata: None,
        nodes: vec![],
    };
    let doc = render_dag_mermaid(&dag);

    assert!(doc.contains("**Nodes:** 0 · **Edges:** 0"));
    // No `**Types:**` section when there are zero nodes.
    assert!(!doc.contains("**Types:**"));
    assert!(doc.contains("```mermaid\nflowchart TD\n"));
    // No subgraph blocks for an empty DAG.
    assert!(!doc.contains("subgraph "));
    // Table header is always present.
    assert!(doc.contains("| Node | Type | Model | Depends On | After | Retry | Validate |"));
    // No data rows.
    assert!(!doc.contains("| solo |"));
}

#[test]
fn test_mermaid_single_node() {
    // T2: a single LLM node with one model lands in the `llm` subgraph with a
    // `model:` label line, no edges, and a single details-table row.
    let dag = Dag {
        metadata: None,
        nodes: vec![node("solo", |n| {
            n.node_type = Some("llm".to_string());
            n.models = vec![model("m1")];
        })],
    };
    let doc = render_dag_mermaid(&dag);

    assert!(doc.contains("**Nodes:** 1 · **Edges:** 0"));
    assert!(doc.contains("**Types:** llm (1)"));
    assert!(doc.contains("    subgraph llm[\"llm\"]"));
    assert!(doc.contains("        n_solo[\"solo<br/>model: m1\"]"));
    // No edges for a single-node DAG.
    assert!(!doc.contains("-->"));
    assert!(doc.contains("| solo | llm | m1 |"));
}

#[test]
fn test_mermaid_linear_chain() {
    // T3: A -> B -> C produces two edges in dependency order.
    let dag = Dag {
        metadata: None,
        nodes: vec![
            node("A", |n| {
                n.depends_on = vec![];
            }),
            node("B", |n| {
                n.depends_on = vec!["A".to_string()];
            }),
            node("C", |n| {
                n.depends_on = vec!["B".to_string()];
            }),
        ],
    };
    let doc = render_dag_mermaid(&dag);

    assert!(doc.contains("**Nodes:** 3 · **Edges:** 2"));
    assert!(doc.contains("    n_A --> n_B\n"));
    assert!(doc.contains("    n_B --> n_C\n"));
    // No phantom self-loops or back-edges.
    assert!(!doc.contains("n_A --> n_C"));
}

#[test]
fn test_mermaid_diamond() {
    // T4: A -> {B, C} -> D produces four edges: A->B, A->C, B->D, C->D.
    let dag = Dag {
        metadata: None,
        nodes: vec![
            node("A", |n| {
                n.depends_on = vec![];
            }),
            node("B", |n| {
                n.depends_on = vec!["A".to_string()];
            }),
            node("C", |n| {
                n.depends_on = vec!["A".to_string()];
            }),
            node("D", |n| {
                n.depends_on = vec!["B".to_string(), "C".to_string()];
            }),
        ],
    };
    let doc = render_dag_mermaid(&dag);

    assert!(doc.contains("**Nodes:** 4 · **Edges:** 4"));
    assert!(doc.contains("    n_A --> n_B\n"));
    assert!(doc.contains("    n_A --> n_C\n"));
    assert!(doc.contains("    n_B --> n_D\n"));
    assert!(doc.contains("    n_C --> n_D\n"));
    // D's details row lists both deps in declared order. All diamond nodes
    // have `node_type: None` in this fixture (the table renders `None`
    // `node_type` as the em-dash placeholder; the `other` grouping is a
    // mermaid-subgraph-only normalization — see render.rs `subgraph_key` vs
    // the table's `unwrap_or_else(|| "—".to_string())`).
    assert!(doc.contains("| D | — | — | B, C |"));
}

#[test]
fn test_mermaid_subgraph_grouping() {
    // T5: mixed `shell` and `llm` nodes are grouped into two subgraphs; each
    // node appears in exactly the subgraph matching its `node_type`.
    let dag = Dag {
        metadata: None,
        nodes: vec![
            node("sh1", |n| {
                n.node_type = Some("shell".to_string());
            }),
            node("llm1", |n| {
                n.node_type = Some("llm".to_string());
                n.models = vec![model("z-ai/glm-5.2")];
            }),
            node("unk1", |n| {
                // Unrecognized `node_type` -> `other` subgraph.
                n.node_type = Some("rust".to_string());
            }),
            node("null1", |n| {
                // `None` `node_type` -> `other` subgraph.
                n.node_type = None;
            }),
        ],
    };
    let doc = render_dag_mermaid(&dag);

    assert!(doc.contains("**Types:** llm (1), other (2), shell (1)"));
    assert!(doc.contains("    subgraph shell[\"shell\"]"));
    assert!(doc.contains("        n_sh1[\"sh1\"]\n"));
    assert!(doc.contains("    subgraph llm[\"llm\"]"));
    assert!(doc.contains("        n_llm1[\"llm1<br/>model: z-ai/glm-5.2\"]"));
    assert!(doc.contains("    subgraph other[\"other\"]"));
    assert!(doc.contains("        n_unk1[\"unk1\"]"));
    assert!(doc.contains("        n_null1[\"null1\"]"));
}

#[test]
fn test_mermaid_node_without_models() {
    // T6: a node with `models: []` has no `model:` line in its mermaid label
    // and the table Model column shows the em-dash placeholder.
    let dag = Dag {
        metadata: None,
        nodes: vec![node("nogood", |n| {
            n.node_type = Some("shell".to_string());
        })],
    };
    let doc = render_dag_mermaid(&dag);

    assert!(doc.contains("        n_nogood[\"nogood\"]\n"));
    // No `model:` line.
    assert!(!doc.contains("model: "));
    // Table Model column shows the em-dash.
    assert!(doc.contains("| nogood | shell | — |"));
}

#[test]
fn test_mermaid_id_sanitization() {
    // T7: node id `my-node.v2` -> mermaid identifier `n_my_node_v2`; the
    // original id is preserved in the label and the details table.
    let dag = Dag {
        metadata: None,
        nodes: vec![node("my-node.v2", |n| {
            n.node_type = Some("llm".to_string());
            n.models = vec![model("m1")];
        })],
    };
    let doc = render_dag_mermaid(&dag);

    // Sanitized mermaid identifier.
    assert!(doc.contains("        n_my_node_v2[\"my-node.v2<br/>model: m1\"]"));
    // Original id in the details table.
    assert!(doc.contains("| my-node.v2 | llm | m1 |"));
    // The literal sanitized id must never leak into a label.
    assert!(!doc.contains("\"n_my_node_v2<br/>"));
}

#[test]
fn test_describe_writes_to_output_file() {
    // T8: `pidag describe <dag.json> --output <path>` writes the markdown
    // document to the file at `<path>` (not stdout). Verifies the CLI
    // integration: argument parsing, DAG parse, render, mkdir -p parent,
    // write.
    //
    // Uses `_tmp/` per workspace convention. Run serially (no other test in
    // this file touches this path) and clean up at the end.
    let dag = Dag {
        metadata: None,
        nodes: vec![
            node("validate", |n| {
                n.node_type = Some("shell".to_string());
            }),
            node("implement", |n| {
                n.node_type = Some("llm".to_string());
                n.models = vec![model("z-ai/glm-5.2")];
                n.depends_on = vec!["validate".to_string()];
            }),
        ],
    };
    let dag_json = serde_json::to_string_pretty(&dag).expect("serialize DAG");

    // Work under `_tmp/` in the repo root (the test runs from the crate dir).
    let mut tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    tmp.push("_tmp");
    tmp.push("dag-mermaid-describe");
    std::fs::create_dir_all(&tmp).expect("create _tmp dir");

    let dag_path = tmp.join("diamond-dag.json");
    std::fs::write(&dag_path, &dag_json).expect("write DAG JSON");

    let out_path = tmp.join("diamond-doc.md");
    // Remove a stale file from a previous run so the test is repeatable.
    let _ = std::fs::remove_file(&out_path);

    let bin = env!("CARGO_BIN_EXE_pidag");
    let output = Command::new(bin)
        .args(["describe", dag_path.to_str().unwrap(), "--output"])
        .arg(out_path.to_str().unwrap())
        .output()
        .expect("spawn pidag");

    assert!(
        output.status.success(),
        "pidag describe failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        out_path.exists(),
        "expected --output file at {} to exist",
        out_path.display()
    );

    let doc = std::fs::read_to_string(&out_path).expect("read output md");

    // Header has the basename-derived title.
    assert!(doc.contains("# DAG: diamond-dag.json"));
    // Summary line.
    assert!(doc.contains("**Nodes:** 2 · **Edges:** 1"));
    assert!(doc.contains("**Types:** llm (1), shell (1)"));
    // Mermaid structure: edge from validate -> implement.
    assert!(doc.contains("    n_validate --> n_implement\n"));
    // Details table.
    assert!(doc.contains("| validate | shell | — | — | — | 1× | — |"));
    assert!(doc.contains("| implement | llm | z-ai/glm-5.2 | validate | — | 1× | — |"));

    // Clean up so the workspace stays tidy.
    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_file(&dag_path);
}
