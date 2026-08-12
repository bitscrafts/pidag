use super::error::PidagError;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dag {
    pub nodes: Vec<Node>,
    /// Free-form provenance metadata. The SDD generator stamps `spec` (the
    /// spec file stem) and `spec_title` here so the trace UI can link a run
    /// back to the phase (spec) that produced it without a separate store
    /// table. `#[serde(default)]` keeps old DAG JSON (no metadata) parsing.
    #[serde(default)]
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub prompt: String,
    pub depends_on: Vec<String>,
    pub models: Vec<ModelRef>,
    pub retry: RetryPolicy,
    pub validate: Option<String>,
    #[serde(default)]
    pub node_type: Option<String>,
    #[serde(default)]
    pub gate: Option<String>,
    /// Per-node wall-clock deadline for a single `Worker::run` invocation in
    /// `dispatch_node`. `None` (the default) means "no scheduler-level
    /// timeout — the worker is trusted to bound itself" (pre-existing
    /// behaviour). When `Some`, the scheduler wraps each `worker.run` in
    /// `tokio::time::timeout` and treats an elapsed deadline as a
    /// non-retryable hard failure. Defense-in-depth so a `Worker` impl that
    /// omits its own timeout (or a future A2A worker over a wedged network)
    /// cannot block the dispatch forever. See HANDOFF 2026-08-02 audit P1 #5.
    #[serde(default)]
    pub timeout: Option<Duration>,
    /// MCP (Model Context Protocol) call configuration. When present,
    /// this node invokes an external MCP server tool instead of using the
    /// `prompt` and `models` fields. Enables pidag to act as an MCP client
    /// for tool discovery and execution.
    #[serde(default)]
    pub mcp_call: Option<McpCallConfig>,
    /// Ordering-only edges: nodes that must be terminal (in any state) before
    /// this node can dispatch. Unlike `depends_on`, an `after` edge's outcome
    /// never blocks and never propagates failure. Serde defaults to empty list
    /// so existing DAG JSON parses and behaves identically.
    #[serde(default)]
    pub after: Vec<String>,
    /// Shell command to verify the worker's claim of success. When present, a
    /// node is `Done` only if the worker succeeded AND this command exits 0.
    /// Worker success with `verify` failing ⇒ node is `Failed` with verify
    /// output as the artifact. Runs in the DAG's project_root with the same
    /// cwd/env/timeout discipline as a `shell` node. Defaults to absent;
    /// DAGs without it behave exactly as today (N1).
    #[serde(default)]
    pub verify: Option<String>,
    /// Optional pre-flight baseline command. When present, the scheduler runs
    /// this command before dispatching the node, captures its stdout (trimmed
    /// of trailing whitespace and capped at 4 KB), and exposes it to the
    /// `verify` command via the PIDAG_VERIFY_PRE environment variable.
    /// If verify_pre exits non-zero, the node fails immediately before the
    /// worker is invoked (R5.4). Defaults to absent; nodes without it omit
    /// PIDAG_VERIFY_PRE (backward compatible, R5.5).
    #[serde(default)]
    pub verify_pre: Option<String>,
}

/// Configuration for a node that calls an external MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpCallConfig {
    /// MCP server endpoint: "http://localhost:7421/mcp" or "stdio://path/to/bin"
    pub server: String,
    /// Transport type: "http" or "stdio"
    #[serde(default = "default_transport")]
    pub transport: String,
    /// MCP tool name to invoke (e.g., "search_memory", "store_insight")
    pub tool: String,
    /// Tool arguments as a JSON object, may contain templates like "{{input}}"
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

fn default_transport() -> String {
    "http".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRef {
    pub name: String,
    pub paid: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub attempts: usize,
    pub backoff_ms: u64,
}

impl Dag {
    /// Validate the DAG: check for cycles and dangling dependencies
    pub fn validate(&self) -> Result<(), PidagError> {
        // Y5: Reject node ids containing `:` (reserved delimiter in gate syntax)
        for node in &self.nodes {
            if node.id.contains(':') {
                return Err(PidagError::Validation(format!(
                    "node id '{}' contains reserved delimiter ':' (reserved for gate syntax)",
                    node.id
                )));
            }
        }

        // Check for dangling dependencies (both depends_on and after)
        let node_ids: HashSet<_> = self.nodes.iter().map(|n| &n.id).collect();
        for node in &self.nodes {
            for dep in &node.depends_on {
                if !node_ids.contains(dep) {
                    return Err(PidagError::UnknownDependency);
                }
            }
            for after in &node.after {
                if !node_ids.contains(after) {
                    return Err(PidagError::UnknownDependency);
                }
            }
        }

        // Check for cycles using DFS (including after edges)
        if self.has_cycle() {
            return Err(PidagError::Cycle);
        }

        // Validate output interpolation references in prompts (I3, I4, I5)
        for node in &self.nodes {
            Self::validate_output_references(node, &node_ids)?;
        }

        Ok(())
    }

    /// Validate {{X.output}} references in a node's prompt (I3, I4, I5).
    /// - I3: referenced node must exist
    /// - I4: referenced node must be in depends_on or after
    /// - I5: format must be exactly `<node_id>.output`, nothing else inside {{ }}
    fn validate_output_references(
        node: &Node,
        all_node_ids: &HashSet<&String>,
    ) -> Result<(), PidagError> {
        let prompt = &node.prompt;

        // Find all {{ }} patterns
        let mut i = 0;
        while let Some(start) = prompt[i..].find("{{") {
            let start = i + start;
            let after_open = start + 2;

            // Find the closing }}
            if let Some(offset) = prompt[after_open..].find("}}") {
                let end = after_open + offset;
                let content = &prompt[after_open..end];

                // I5: validate the format is exactly <node_id>.output
                // Check if it contains nested braces or invalid patterns
                if content.contains('{') || content.contains('}') {
                    return Err(PidagError::Validation(format!(
                        "node {}: invalid placeholder format {{{{{}}}}}, nested braces not allowed",
                        node.id, content
                    )));
                }

                // Must be of the form "<node_id>.output"
                if let Some(dot_pos) = content.find('.') {
                    let node_ref = &content[..dot_pos];
                    let suffix = &content[dot_pos + 1..];

                    if suffix != "output" {
                        return Err(PidagError::Validation(format!(
                            "node {}: invalid placeholder {{{{{}}}}}, only .output is supported",
                            node.id, content
                        )));
                    }

                    // I3: check that the referenced node exists
                    let node_ref_owned = node_ref.to_string();
                    if !all_node_ids.contains(&node_ref_owned) {
                        return Err(PidagError::Validation(format!(
                            "node {}: references unknown node {} in {{{{{}}}}}",
                            node.id, node_ref, content
                        )));
                    }

                    // I4: check that the referenced node is in depends_on or after
                    if !node.depends_on.contains(&node_ref.to_string())
                        && !node.after.contains(&node_ref.to_string())
                    {
                        return Err(PidagError::Validation(format!(
                            "node {}: references {{{{{}}}}} but {} is not in depends_on or after",
                            node.id, content, node_ref
                        )));
                    }
                } else {
                    // No dot found, invalid format
                    return Err(PidagError::Validation(format!(
                        "node {}: invalid placeholder {{{{{}}}}}, must be of form <node_id>.output",
                        node.id, content
                    )));
                }

                i = end + 2;
            } else {
                // Unclosed {{, skip to avoid infinite loop
                break;
            }
        }

        Ok(())
    }

    fn has_cycle(&self) -> bool {
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();

        for node in &self.nodes {
            if !visited.contains(&node.id)
                && self.has_cycle_dfs(&node.id, &mut visited, &mut rec_stack)
            {
                return true;
            }
        }
        false
    }

    fn has_cycle_dfs(
        &self,
        node_id: &str,
        visited: &mut HashSet<String>,
        rec_stack: &mut HashSet<String>,
    ) -> bool {
        visited.insert(node_id.to_string());
        rec_stack.insert(node_id.to_string());

        if let Some(node) = self.nodes.iter().find(|n| n.id == node_id) {
            // Check depends_on edges
            for dep in &node.depends_on {
                if !visited.contains(dep) {
                    if self.has_cycle_dfs(dep, visited, rec_stack) {
                        return true;
                    }
                } else if rec_stack.contains(dep) {
                    return true;
                }
            }
            // Check after edges
            for after in &node.after {
                if !visited.contains(after) {
                    if self.has_cycle_dfs(after, visited, rec_stack) {
                        return true;
                    }
                } else if rec_stack.contains(after) {
                    return true;
                }
            }
        }

        rec_stack.remove(node_id);
        false
    }

    /// Topological sort using Kahn's algorithm
    pub fn topo_sort(&self) -> Result<Vec<&str>, PidagError> {
        let mut in_degree = HashMap::new();
        let mut adjacency = HashMap::new();

        // Initialize in-degree and adjacency
        for node in &self.nodes {
            in_degree.insert(node.id.as_str(), 0);
            adjacency.insert(node.id.as_str(), Vec::new());
        }

        // Build adjacency list and in-degrees
        for node in &self.nodes {
            for dep in &node.depends_on {
                adjacency
                    .entry(dep.as_str())
                    .or_insert_with(Vec::new)
                    .push(node.id.as_str());
                *in_degree.entry(node.id.as_str()).or_insert(0) += 1;
            }
        }

        // Queue all nodes with in-degree 0
        let mut queue: VecDeque<_> = in_degree
            .iter()
            .filter(|&(_, degree)| *degree == 0)
            .map(|(id, _)| *id)
            .collect();

        let mut result = Vec::new();

        while let Some(node_id) = queue.pop_front() {
            result.push(node_id);

            // Process all dependents
            if let Some(dependents) = adjacency.get(node_id) {
                for &dependent in dependents {
                    if let Some(degree) = in_degree.get_mut(dependent) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(dependent);
                        }
                    }
                }
            }
        }

        if result.len() == self.nodes.len() {
            Ok(result)
        } else {
            Err(PidagError::Cycle)
        }
    }

    /// Get ready nodes (in-degree 0)
    pub fn ready_nodes(&self) -> Result<Vec<String>, PidagError> {
        let mut in_degree = HashMap::new();

        for node in &self.nodes {
            in_degree.insert(node.id.clone(), 0);
        }

        for node in &self.nodes {
            for _dep in &node.depends_on {
                *in_degree.entry(node.id.clone()).or_insert(0) += 1;
            }
        }

        Ok(in_degree
            .iter()
            .filter(|&(_, degree)| *degree == 0)
            .map(|(id, _)| id.clone())
            .collect())
    }

    pub fn get_node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, depends_on: &[&str]) -> Node {
        Node {
            id: id.to_string(),
            prompt: format!("prompt for {id}"),
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
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
            after: Vec::new(),
            verify: None,
            verify_pre: None,
        }
    }

    fn dag(nodes: Vec<Node>) -> Dag {
        Dag {
            metadata: None,
            nodes,
        }
    }

    fn as_str_set(v: Vec<String>) -> std::collections::HashSet<String> {
        v.into_iter().collect()
    }

    #[test]
    fn test_ready_nodes_returns_roots_not_leaves() {
        // T1: A -> B  (B depends on A). ready_nodes() must return ["A"].
        let d = dag(vec![node("A", &[]), node("B", &["A"])]);

        let ready = d.ready_nodes().expect("ready_nodes should succeed");

        assert!(
            ready.iter().any(|n| n == "A"),
            "root A must be ready, got {ready:?}"
        );
        assert!(
            !ready.iter().any(|n| n == "B"),
            "leaf B must NOT be ready, got {ready:?}"
        );
        assert_eq!(
            ready.len(),
            1,
            "only the root should be ready, got {ready:?}"
        );
    }

    #[test]
    fn test_ready_nodes_diamond_returns_single_root() {
        // T3: diamond A -> {B, C} -> D. ready_nodes() must return ["A"] only.
        let d = dag(vec![
            node("A", &[]),
            node("B", &["A"]),
            node("C", &["A"]),
            node("D", &["B", "C"]),
        ]);

        let ready = d.ready_nodes().expect("ready_nodes should succeed");

        assert_eq!(as_str_set(ready), as_str_set(vec!["A".to_string()]));
    }
}
