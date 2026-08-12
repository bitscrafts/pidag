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
    /// Verification seam for the worker's claim of success (spec-37). When
    /// present, a node is `Done` only if the worker succeeded AND this
    /// `Verify` passes. Worker success with `verify` failing ⇒ node is
    /// `Failed`, with `"worker: <claim>\nverify: <reason>"` as the artifact.
    /// `Verify::Shell` runs in the DAG's project_root with the same
    /// cwd/env/timeout discipline as a `shell` node, byte-identical to the
    /// pre-spec-37 behaviour (N1). Defaults to absent; DAGs without it
    /// behave exactly as today.
    #[serde(default)]
    pub verify: Option<Verify>,
    /// Optional pre-flight baseline command. When present, the scheduler runs
    /// this command before dispatching the node, captures its stdout (trimmed
    /// of trailing whitespace and capped at 4 KB), and exposes it to the
    /// `verify` command via the PIDAG_VERIFY_PRE environment variable.
    /// If verify_pre exits non-zero, the node fails immediately before the
    /// worker is invoked (R5.4). Defaults to absent; nodes without it omit
    /// PIDAG_VERIFY_PRE (backward compatible, R5.5).
    #[serde(default)]
    pub verify_pre: Option<String>,
    /// spec-38 F1: literal fan-out. A node carrying this expands at DAG
    /// load (before `dag.validate()`) into one node per item, `{{item}}`
    /// substituted in `prompt`/`models`/`verify`/`gate`. The list is
    /// authored data -- never a file read, a model output, or a glob
    /// (G9) -- because a computed width is a runtime topology decision,
    /// which this spec deliberately forbids (G4). Absent (`None`) for
    /// every node that isn't fanning out; `#[serde(default)]` keeps old
    /// DAG JSON parsing unchanged (N1).
    #[serde(default)]
    pub for_each: Option<Vec<String>>,
    /// spec-38 F7: `node_type = "quorum"` configuration -- the ids to
    /// count (`of`) and the pass threshold (`min_pass`). A quorum node
    /// dispatches no worker and no subprocess (F7a, G5); it reads the
    /// already-terminal outputs of `of` (added to `after`, never
    /// `depends_on`, at expansion time -- F9, G7) and counts verdicts
    /// with the same parser `Verify::Critic` uses (F8, G6). `#[serde(default)]`
    /// keeps old DAG JSON parsing unchanged (N1).
    #[serde(default)]
    pub quorum: Option<QuorumConfig>,
}

/// spec-38 F7/F11: quorum node configuration. `of` names the nodes whose
/// recorded outputs are counted; `min_pass` is the pass threshold. Both are
/// validated by `Dag::validate` -- every id in `of` must name a real node
/// (F9) and `0 < min_pass <= of.len()` (F11, a quorum that cannot fail or
/// cannot pass is a configuration mistake).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuorumConfig {
    pub of: Vec<String>,
    pub min_pass: usize,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRef {
    pub name: String,
    pub paid: bool,
}

/// A verification seam for a node's claimed success (spec-37).
///
/// - `Shell(cmd)` is today's behaviour, unchanged: run `cmd` in the DAG's
///   project_root with the same cwd/env/timeout discipline as a `shell`
///   node, gate on its exit status.
/// - `Critic { prompt, models }` dispatches `prompt` through `&dyn Worker`
///   — the same trait, retry and model-fallback machinery a normal node
///   uses, never a subprocess — and parses the reply for a leading `PASS`
///   or `FAIL` token. Anything else (unparseable reply, worker error,
///   timeout, exhausted fallbacks, empty reply) is a FAIL; there is no
///   input that produces a silent pass (C4, G4).
/// - `All(arms)` requires every arm to pass. Evaluation short-circuits on
///   the first failure, in author order (C6).
///
/// `#[serde(untagged)]` is what makes a bare JSON/TOML string deserialize
/// as `Shell` — the exact wire shape `Node.verify: Option<String>` used
/// before spec-37 (C2). This is a requirement, not an accident: `Node` is
/// serialized into `RunMeta.dag_json` in every vault and authored as TOML
/// in `src/workflow/templates/`, so a shape that cannot read the old form
/// breaks every existing vault and template (see `docs/FINDINGS.md`, and
/// `tests/fixtures/legacy_dag/` for the hash-pinned proof, C8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Verify {
    Shell(String),
    Critic {
        prompt: String,
        models: Vec<ModelRef>,
    },
    All(Vec<Verify>),
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

        let node_ids: HashSet<_> = self.nodes.iter().map(|n| &n.id).collect();

        // F9, F11: quorum config -- named node id, `of` membership, `min_pass`
        // bounds. Checked BEFORE the generic dangling-dependency scan below so
        // a misconfigured quorum gets a message naming the node and the exact
        // problem (error-handling expectations), not the generic
        // `PidagError::UnknownDependency`. Runs on the EXPANDED dag (F5): by
        // this point `of` holds real (post-expansion) node ids and F9 has
        // already copied them into `after`.
        for node in &self.nodes {
            if let Some(q) = &node.quorum {
                for of_id in &q.of {
                    if !node_ids.contains(of_id) {
                        return Err(PidagError::Validation(format!(
                            "node '{}': quorum.of references unknown node '{}'",
                            node.id, of_id
                        )));
                    }
                }
                if q.min_pass == 0 {
                    return Err(PidagError::Validation(format!(
                        "node '{}': quorum.min_pass is 0 -- a quorum that cannot fail is a \
                         configuration mistake, not a runtime condition",
                        node.id
                    )));
                }
                if q.min_pass > q.of.len() {
                    return Err(PidagError::Validation(format!(
                        "node '{}': quorum.min_pass ({}) exceeds quorum.of length ({}) -- a \
                         quorum that cannot pass is a configuration mistake, not a runtime \
                         condition",
                        node.id,
                        q.min_pass,
                        q.of.len()
                    )));
                }
            }
        }

        // Check for dangling dependencies (both depends_on and after)
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

    /// spec-38 F1-F5, F9: expand `for_each` fan-out and wire quorum `after`
    /// edges. Runs at DAG load, **before** `dag.validate()` (F5, G4) -- the
    /// scheduler must keep executing a topology it did not choose, so this
    /// is the one place the shape of the graph is decided. Idempotent: a
    /// DAG with no `for_each` node returns byte-identical nodes (N1), so
    /// callers may call it more than once (e.g. once at the real load site
    /// to persist the expanded `dag_json`, and defensively again inside the
    /// scheduler) without changing behaviour.
    ///
    /// Three phases, each O(nodes + total items) -- no quadratic reference
    /// rewriting on large fan-outs (N7):
    /// 1. expand every `for_each` node into one child per item, substituting
    ///    `{{item}}` in `prompt`/`models`/`verify`/`gate` (F1, F2, F3);
    /// 2. rewrite every `depends_on`/`after`/`quorum.of` entry and every
    ///    `{{X.output}}` prompt reference that names a since-removed parent
    ///    id, to the parent's full child set (F4). A `gate` naming a
    ///    since-removed `for_each` parent is NOT rewritten -- `gate` is a
    ///    scalar single-node reference and cannot hold a fan-out's set, so
    ///    this is a validation error instead (F4', G9'), naming the gating
    ///    node, the parent, and directing the author to `quorum`;
    /// 3. add each (now-expanded) `quorum.of` id to that node's `after` set,
    ///    never `depends_on` (F9, G7) -- see spec-38 premise 2: `depends_on`
    ///    would block the adjudicator exactly when its critics disagree,
    ///    which is the only time adjudication matters.
    pub fn expand(mut self) -> Result<Dag, PidagError> {
        let mut parent_to_children: HashMap<String, Vec<String>> = HashMap::new();
        let mut new_nodes: Vec<Node> = Vec::with_capacity(self.nodes.len());

        for node in self.nodes.drain(..) {
            let Some(items) = node.for_each.clone() else {
                new_nodes.push(node);
                continue;
            };
            if items.is_empty() {
                // F5b: an empty for_each is a validation error, not a
                // silently-vanishing node -- fail here, before any node is
                // produced for this id, rather than let it disappear.
                return Err(PidagError::Validation(format!(
                    "node '{}': for_each list is empty",
                    node.id
                )));
            }
            let base_id = node.id.clone();
            let mut used_ids: HashSet<String> = HashSet::new();
            let mut children_ids: Vec<String> = Vec::with_capacity(items.len());
            for (idx, item) in items.iter().enumerate() {
                // F3: `<id>-<item>` slugified, falling back to `<id>-<index>`
                // on a collision or an empty slug.
                let slug = slugify(item);
                let candidate = if slug.is_empty() {
                    format!("{base_id}-{idx}")
                } else {
                    format!("{base_id}-{slug}")
                };
                let child_id = if used_ids.contains(&candidate) {
                    format!("{base_id}-{idx}")
                } else {
                    candidate
                };
                used_ids.insert(child_id.clone());

                let mut child = node.clone();
                child.for_each = None;
                child.id = child_id.clone();
                substitute_item_in_node(&mut child, item);
                new_nodes.push(child);
                children_ids.push(child_id);
            }
            parent_to_children.insert(base_id, children_ids);
        }

        // Phase 2: rewrite references to removed parent ids (F4). `gate` is
        // the one exception -- it is a validation error instead (F4', G9'),
        // handled inline below.
        for node in new_nodes.iter_mut() {
            node.depends_on = expand_ref_list(&node.depends_on, &parent_to_children);
            node.after = expand_ref_list(&node.after, &parent_to_children);
            if let Some(q) = node.quorum.as_mut() {
                q.of = expand_ref_list(&q.of, &parent_to_children);
            }
            node.prompt = expand_output_refs(&node.prompt, &parent_to_children);
            if let Some(g) = &node.gate {
                // F4', G9': a `gate` naming a `for_each` parent is a
                // validation error, not a rewrite -- `gate` is a scalar
                // `"<id>:<suffix>"` reference to exactly one upstream node,
                // it cannot hold a fan-out's whole child set, and picking
                // one child (e.g. the first) would silently gate on an
                // arbitrary member of the ensemble while reading, to anyone
                // scanning the DAG, as though it gated on all of them.
                let id_part = match g.find(':') {
                    Some(colon) => &g[..colon],
                    None => g.as_str(),
                };
                if let Some(children) = parent_to_children.get(id_part) {
                    return Err(PidagError::Validation(format!(
                        "node '{}': gate '{}' references '{}', which is a for_each \
                         fan-out expanded into {} children ({}) -- a gate cannot pick \
                         one arbitrary child of an ensemble to stand for the whole. \
                         Add a `quorum` node that counts '{}'s children and gate on \
                         the quorum node instead.",
                        node.id,
                        g,
                        id_part,
                        children.len(),
                        children.join(", "),
                        id_part
                    )));
                }
            }
        }

        // Phase 3: quorum.of -> after, never depends_on (F9, G7). Must run
        // after phase 2 so `of` already holds expanded child ids.
        for node in new_nodes.iter_mut() {
            if let Some(q) = node.quorum.clone() {
                for id in &q.of {
                    if !node.after.contains(id) {
                        node.after.push(id.clone());
                    }
                }
            }
        }

        Ok(Dag {
            nodes: new_nodes,
            metadata: self.metadata,
        })
    }
}

/// F3: lowercase, non-alphanumerics to `-`. Item values are authored data
/// (model names, labels), so ASCII case mapping is sufficient; there is no
/// requirement (or test) for full Unicode case-folding.
fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// F2: replace the literal `{{item}}` placeholder with the item's raw
/// value (not slugified -- slugification is only for generated ids, F3).
fn substitute_item(s: &str, item: &str) -> String {
    s.replace("{{item}}", item)
}

/// F2: `{{item}}` substitution in `verify`, recursing into `All` arms so a
/// fanned-out node's `Verify::Critic` prompt/models can reference the item
/// too.
fn substitute_item_verify(v: Verify, item: &str) -> Verify {
    match v {
        Verify::Shell(s) => Verify::Shell(substitute_item(&s, item)),
        Verify::Critic { prompt, models } => Verify::Critic {
            prompt: substitute_item(&prompt, item),
            models: models
                .into_iter()
                .map(|mut m| {
                    m.name = substitute_item(&m.name, item);
                    m
                })
                .collect(),
        },
        Verify::All(arms) => Verify::All(
            arms.into_iter()
                .map(|a| substitute_item_verify(a, item))
                .collect(),
        ),
    }
}

/// F2: substitute `{{item}}` in `prompt`, `models`, `verify` and `gate`.
/// The generated child `id` is NOT substituted here -- F3's `<id>-<item>`
/// formula (computed by the caller) is the sole authority on child ids, so
/// a literal `{{item}}` in an authored `id` has no separate effect.
fn substitute_item_in_node(node: &mut Node, item: &str) {
    node.prompt = substitute_item(&node.prompt, item);
    for m in node.models.iter_mut() {
        m.name = substitute_item(&m.name, item);
    }
    if let Some(g) = node.gate.take() {
        node.gate = Some(substitute_item(&g, item));
    }
    if let Some(v) = node.verify.take() {
        node.verify = Some(substitute_item_verify(v, item));
    }
}

/// F4: expand a `depends_on`/`after`/`quorum.of` list -- any entry naming a
/// since-removed `for_each` parent id is replaced by all of that parent's
/// children, in item order. Entries that don't name a parent pass through
/// unchanged. O(list length): one hash lookup per entry, no nested scan.
fn expand_ref_list(
    list: &[String],
    parent_to_children: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut out = Vec::with_capacity(list.len());
    for id in list {
        match parent_to_children.get(id) {
            Some(children) => out.extend(children.iter().cloned()),
            None => out.push(id.clone()),
        }
    }
    out
}

/// F4: rewrite `{{X.output}}` references to a since-removed `for_each`
/// parent into one `{{child.output}}` placeholder per child, so
/// `interpolate_outputs` (unmodified, spec-38 premise 1) resolves each at
/// dispatch time. References that don't name a parent are left verbatim.
fn expand_output_refs(prompt: &str, parent_to_children: &HashMap<String, Vec<String>>) -> String {
    if parent_to_children.is_empty() || !prompt.contains("{{") {
        return prompt.to_string();
    }
    let mut result = String::with_capacity(prompt.len());
    let mut i = 0;
    while let Some(offset) = prompt[i..].find("{{") {
        let start = i + offset;
        result.push_str(&prompt[i..start]);
        let after_open = start + 2;
        let Some(close_offset) = prompt[after_open..].find("}}") else {
            // Unclosed placeholder: copy the rest verbatim and stop.
            result.push_str(&prompt[start..]);
            i = prompt.len();
            break;
        };
        let end = after_open + close_offset;
        let content = &prompt[after_open..end];
        let mut rewritten = false;
        if let Some(dot_pos) = content.find('.') {
            let node_ref = &content[..dot_pos];
            let suffix = &content[dot_pos + 1..];
            if suffix == "output"
                && let Some(children) = parent_to_children.get(node_ref)
            {
                for child in children {
                    result.push_str("{{");
                    result.push_str(child);
                    result.push_str(".output}}");
                }
                rewritten = true;
            }
        }
        if !rewritten {
            result.push_str(&prompt[start..end + 2]);
        }
        i = end + 2;
    }
    result.push_str(&prompt[i..]);
    result
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
            for_each: None,
            quorum: None,
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
