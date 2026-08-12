use crate::core::dag::{Dag, Node};
use std::collections::HashMap;

/// Sanitize a node id into a mermaid-safe identifier.
///
/// Per spec `specs/94-dag-mermaid-describe.md` R5: replace every non-alphanumeric
/// character with `_`, then prefix with `n_`. Example: `my-node.v2` →
/// `n_my_node_v2`. The original id stays in the user-visible label; only
/// the mermaid identifier is sanitized.
fn sanitize_mermaid_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len() + 2);
    out.push_str("n_");
    for c in id.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    out
}

/// Pick the subgraph key for a node's `node_type`.
///
/// Per spec R6: `Some("shell")` → `shell`, `Some("llm")` → `llm`, anything
/// else (`None` or unrecognized) → `other`. Mirrors `TypeDispatchWorker`'s
/// default-to-piworker convention for unknown node types.
fn subgraph_key(node_type: &Option<String>) -> &str {
    match node_type.as_deref() {
        Some("shell") => "shell",
        Some("llm") => "llm",
        _ => "other",
    }
}

/// Render a DAG as a markdown document with an embedded mermaid flowchart.
///
/// Pure function: no I/O, no side effects. Returns a markdown string with
/// three sections (header summary, mermaid flowchart, node details table).
/// See spec `specs/94-dag-mermaid-describe.md` (R1-R9).
pub fn render_dag_mermaid(dag: &Dag) -> String {
    let mut out = String::new();
    let total_nodes = dag.nodes.len();

    // Count edges = sum of depends_on and after across all nodes.
    let total_edges: usize = dag
        .nodes
        .iter()
        .map(|n| n.depends_on.len() + n.after.len())
        .sum();

    // Header section: # DAG: <filename> is added by the CLI wrapper (which
    // knows the path); the pure function here starts at the summary line so
    // it has no path dependency. Caller prepends the `# DAG: ...` line.
    let mut type_counts: HashMap<&str, usize> = HashMap::new();
    for node in &dag.nodes {
        *type_counts
            .entry(subgraph_key(&node.node_type))
            .or_default() += 1;
    }
    let mut type_keys: Vec<&str> = type_counts.keys().copied().collect();
    type_keys.sort(); // deterministic order: llm, other, shell
    let types_str = if type_keys.is_empty() {
        String::new()
    } else {
        let parts: Vec<String> = type_keys
            .iter()
            .map(|k| format!("{} ({})", k, type_counts[k]))
            .collect();
        parts.join(", ")
    };
    out.push_str(&format!(
        "**Nodes:** {} · **Edges:** {}",
        total_nodes, total_edges
    ));
    if !type_keys.is_empty() {
        out.push_str(&format!(" · **Types:** {}", types_str));
    }
    out.push_str("\n\n");

    // Mermaid flowchart section.
    out.push_str("```mermaid\n");
    out.push_str("flowchart TD\n");
    // Group nodes by subgraph in deterministic key order (llm, other, shell).
    for key in &type_keys {
        out.push_str(&format!("    subgraph {}[\"{}\"]\n", key, key));
        for node in &dag.nodes {
            if subgraph_key(&node.node_type) == *key {
                let m_id = sanitize_mermaid_id(&node.id);
                let label = if let Some(first_model) = node.models.first() {
                    format!("{}<br/>model: {}", node.id, first_model.name)
                } else {
                    node.id.clone()
                };
                out.push_str(&format!("        {}[\"{}\"]\n", m_id, label));
            }
        }
        out.push_str("    end\n");
    }
    // Edges: one line per (dependency, node) pair.
    // depends_on edges are solid lines (-->), after edges are dashed (-.->).
    for node in &dag.nodes {
        let to_id = sanitize_mermaid_id(&node.id);
        for dep in &node.depends_on {
            let from_id = sanitize_mermaid_id(dep);
            out.push_str(&format!("    {} --> {}\n", from_id, to_id));
        }
        for after in &node.after {
            let from_id = sanitize_mermaid_id(after);
            out.push_str(&format!("    {} -.-> {}\n", from_id, to_id));
        }
    }
    out.push_str("```\n\n");

    // Node details table.
    out.push_str("## Node Details\n\n");
    out.push_str("| Node | Type | Model | Depends On | After | Retry | Validate |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    // Spec R4: rows in topological order. topo_sort returns `&str` ids.
    let order: Vec<&str> = match dag.topo_sort() {
        Ok(order) => order,
        // Cycle or other validation error: fall back to declared order so the
        // function stays total (no panic) — caller can validate separately.
        Err(_) => dag.nodes.iter().map(|n| n.id.as_str()).collect(),
    };
    let by_id: HashMap<&str, &Node> = dag.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    for id in &order {
        let node = match by_id.get(id) {
            Some(n) => *n,
            None => continue,
        };
        let node_type_str = node.node_type.clone().unwrap_or_else(|| "—".to_string());
        let model_str = node
            .models
            .first()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| "—".to_string());
        let deps_str = if node.depends_on.is_empty() {
            "—".to_string()
        } else {
            node.depends_on.join(", ")
        };
        let after_str = if node.after.is_empty() {
            "—".to_string()
        } else {
            node.after.join(", ")
        };
        let retry_str = format!("{}×", node.retry.attempts);
        let validate_str = node.validate.clone().unwrap_or_else(|| "—".to_string());
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            node.id, node_type_str, model_str, deps_str, after_str, retry_str, validate_str
        ));
    }

    out
}

/// Render the DAG status in incident-encoded GraphMD format.
///
/// Returns a string with:
/// - Header line: `run <id> · <done>/<total> done · <failed> failed`
/// - One line per node in topological order: `<short_id> <glyph> <state> <model?> →deps: <deps>`
///
/// Glyphs: `✓ done`, `⏳ running`, `⛔ blocked`, `✗ failed`, `· pending`
/// Nodes without state in the map render as `pending`.
pub fn render_status(dag: &Dag, states: &HashMap<String, (String, Option<String>)>) -> String {
    let mut output = String::new();

    // Count states for header
    let total_nodes = dag.nodes.len();
    let done_count = states.values().filter(|(state, _)| state == "Done").count();
    let failed_count = states
        .values()
        .filter(|(state, _)| state == "Failed")
        .count();

    // Header line
    output.push_str(&format!(
        "run <id> · {}/{} done · {} failed\n",
        done_count, total_nodes, failed_count
    ));

    // Topo sort to emit nodes in order
    let topo_order = dag.topo_sort().unwrap_or_default();

    for node_id in topo_order {
        let state = states
            .get(node_id)
            .map(|(s, _)| s.clone())
            .unwrap_or_else(|| "Pending".to_string());

        let model = states.get(node_id).and_then(|(_, m)| m.clone());

        let glyph = match state.as_str() {
            "Done" => "✓",
            "Running" => "⏳",
            "Blocked" => "⛔",
            "Failed" => "✗",
            "Skipped" => "⊘",
            _ => "·", // pending
        };

        // Find this node in the DAG
        let node = dag.nodes.iter().find(|n| n.id == node_id);
        let deps_str = if let Some(node) = node {
            if node.depends_on.is_empty() {
                String::new()
            } else {
                format!(" →deps: {}", node.depends_on.join(","))
            }
        } else {
            String::new()
        };

        let model_str = model.map(|m| format!(" {}", m)).unwrap_or_default();

        output.push_str(&format!(
            "{} {} {} {}{}",
            node_id, glyph, state, model_str, deps_str
        ));
        output.push('\n');
    }

    output
}
