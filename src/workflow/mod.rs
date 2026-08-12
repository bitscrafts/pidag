//! Declarative workflow templates for pidag
//!
//! This module implements the template system for workflow definitions,
//! allowing pidag to run any DAG topology, not just the hardcoded SDD loop.
//! Templates are TOML files with substitution support for `{n}`, `{n-1}`,
//! and configuration values.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::core::dag::{Dag, Node, RetryPolicy, Verify};
use crate::core::error::PidagError;

/// A workflow template that can be expanded into a DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub description: String,
    /// Number of iterations (can be overridden by config or --iterations flag)
    #[serde(default)]
    pub iterations: Option<usize>,
    /// Static nodes (not repeated)
    #[serde(default)]
    pub nodes: Vec<TemplateNode>,
    /// Repeated sections: nodes that are expanded for each applicable iteration
    #[serde(default)]
    pub repeat: Vec<RepeatSection>,
}

/// A node definition in a template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: Option<String>,
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub models: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub gate: Option<String>,
    /// Authored as a bare shell-command string in TOML; `expand_node` wraps
    /// it into `Verify::Shell` when building the `Node` (spec-37, C2b).
    /// Authoring a `Critic`/`All` verify from a template is out of scope
    /// (G8) — a template can only produce a shell verify today.
    #[serde(default)]
    pub verify: Option<String>,
    #[serde(default)]
    pub verify_pre: Option<String>,
    /// spec-38 F1: templates may author a `for_each` fan-out. Passed
    /// through to `Node.for_each` verbatim (items are literal data, G9 --
    /// not substituted against `{n}`/`{n-1}`/etc, since the template
    /// engine's iteration variables and `for_each`'s `{{item}}` are
    /// separate mechanisms). `WorkflowEngine::expand` runs `Dag::expand`
    /// on the assembled DAG before its own `dag.validate()` (F5).
    #[serde(default)]
    pub for_each: Option<Vec<String>>,
}

/// Nodes to be repeated for each iteration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepeatSection {
    /// First iteration this section applies to (default 1)
    #[serde(default = "default_start")]
    pub start: usize,
    pub nodes: Vec<TemplateNode>,
}

fn default_start() -> usize {
    1
}

/// Context for template substitution
pub struct TemplateContext {
    pub n: usize, // current iteration
    pub spec_path: String,
    pub project_root: String,
    pub validate_script: String,
    pub quality_gate_script: String,
    pub prompts: HashMap<String, String>,
    pub models_config: crate::core::config::ModelsConfig,
}

/// Loader and expander for workflow templates
pub struct WorkflowEngine;

impl WorkflowEngine {
    /// Validate raw template placeholders before substitution.
    /// Checks that all {...} placeholders (excluding {{...}}) are in the known vocabulary.
    fn validate_raw_placeholders(
        template_name: &str,
        node_id: &str,
        field_value: &str,
    ) -> Result<(), PidagError> {
        // Use char_indices to iterate consistently over characters
        let chars: Vec<(usize, char)> = field_value.char_indices().collect();
        let mut i = 0;

        while i < chars.len() {
            // Check for {{ pattern
            if chars[i].1 == '{' && i + 1 < chars.len() && chars[i + 1].1 == '{' {
                // Found {{ - skip until we find }}
                i += 2;
                while i + 1 < chars.len() {
                    if chars[i].1 == '}' && chars[i + 1].1 == '}' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                // If we didn't find }}, we skip to the end
                if i < chars.len() && !(i > 0 && chars[i - 1].1 == '}') {
                    while i < chars.len() {
                        i += 1;
                    }
                }
            } else if chars[i].1 == '{' {
                // Found a single { - extract the placeholder
                let start_idx = i;
                i += 1;
                while i < chars.len() && chars[i].1 != '}' {
                    i += 1;
                }

                // Extract the placeholder string
                let placeholder = if i < chars.len() {
                    // Include the closing }
                    let start_byte = chars[start_idx].0;
                    let end_byte = if i + 1 < chars.len() {
                        chars[i + 1].0
                    } else {
                        field_value.len()
                    };
                    &field_value[start_byte..end_byte]
                } else {
                    // Unclosed brace
                    let start_byte = chars[start_idx].0;
                    &field_value[start_byte..]
                };

                // Validate against known vocabulary
                if !Self::is_known_placeholder(placeholder) {
                    return Err(PidagError::Parse(format!(
                        "Template {}: node {}: unknown placeholder {}",
                        template_name, node_id, placeholder
                    )));
                }

                if i < chars.len() {
                    i += 1; // consume the closing }
                }
            } else {
                i += 1;
            }
        }

        Ok(())
    }

    /// Check if a placeholder is in the known vocabulary
    fn is_known_placeholder(placeholder: &str) -> bool {
        // Exact matches
        if matches!(
            placeholder,
            "{n}"
                | "{n-1}"
                | "{spec_path}"
                | "{project_root}"
                | "{validate_script}"
                | "{quality_gate_script}"
        ) {
            return true;
        }

        // Prefix matches: {prompt.<anything>}
        if placeholder.starts_with("{prompt.") && placeholder.ends_with("}") {
            return true;
        }

        // Prefix matches: {models.worker(...)}
        if placeholder.starts_with("{models.worker(") && placeholder.ends_with(")}") {
            return true;
        }

        false
    }

    /// Validate all placeholders in a template node's raw fields
    fn validate_node_placeholders(
        template_name: &str,
        node: &TemplateNode,
    ) -> Result<(), PidagError> {
        // Validate id
        Self::validate_raw_placeholders(template_name, &node.id, &node.id)?;

        // Validate command and prompt
        if let Some(ref cmd) = node.command {
            Self::validate_raw_placeholders(template_name, &node.id, cmd)?;
        }
        if let Some(ref prompt) = node.prompt {
            Self::validate_raw_placeholders(template_name, &node.id, prompt)?;
        }

        // Validate models
        if let Some(ref models) = node.models {
            Self::validate_raw_placeholders(template_name, &node.id, models)?;
        }

        // Validate gate
        if let Some(ref gate) = node.gate {
            Self::validate_raw_placeholders(template_name, &node.id, gate)?;
        }

        // Validate verify
        if let Some(ref verify) = node.verify {
            Self::validate_raw_placeholders(template_name, &node.id, verify)?;
        }

        // Validate verify_pre
        if let Some(ref verify_pre) = node.verify_pre {
            Self::validate_raw_placeholders(template_name, &node.id, verify_pre)?;
        }

        // Validate depends_on entries
        for dep in &node.depends_on {
            Self::validate_raw_placeholders(template_name, &node.id, dep)?;
        }

        // Validate after entries
        for a in &node.after {
            Self::validate_raw_placeholders(template_name, &node.id, a)?;
        }

        // Validate for_each items (spec-38 F1)
        if let Some(items) = &node.for_each {
            for item in items {
                Self::validate_raw_placeholders(template_name, &node.id, item)?;
            }
        }

        Ok(())
    }

    /// Load a template from a file path, or return a built-in if not found.
    pub fn load_template(name: &str, project_root: Option<&Path>) -> Result<Template, PidagError> {
        // Try to load from project directory first
        if let Some(root) = project_root {
            let project_path = root
                .join(".pidag")
                .join("workflows")
                .join(format!("{}.toml", name));
            if project_path.exists() {
                let content = std::fs::read_to_string(&project_path).map_err(|e| {
                    PidagError::Parse(format!("Failed to read workflow {}: {}", name, e))
                })?;
                return Self::parse_template(&content, name);
            }
        }

        // Fall back to built-in templates
        Self::load_builtin(name)
    }

    /// Load a built-in template embedded in the binary
    fn load_builtin(name: &str) -> Result<Template, PidagError> {
        let content = match name {
            "sdd" => include_str!("templates/sdd.toml"),
            "research" => include_str!("templates/research.toml"),
            _ => {
                return Err(PidagError::Parse(format!(
                    "Unknown workflow: {} (built-in workflows: sdd, research)",
                    name
                )));
            }
        };
        Self::parse_template(content, name)
    }

    /// Parse TOML template content
    fn parse_template(content: &str, name: &str) -> Result<Template, PidagError> {
        toml::from_str(content)
            .map_err(|e| PidagError::Parse(format!("Failed to parse workflow {}: {}", name, e)))
    }

    /// Expand a template into a DAG
    pub fn expand(
        template: &Template,
        iterations: usize,
        context: TemplateContext,
    ) -> Result<Dag, PidagError> {
        // Validate raw placeholders in all template nodes BEFORE any substitution
        for node in &template.nodes {
            Self::validate_node_placeholders(&template.name, node)?;
        }

        for section in &template.repeat {
            for node in &section.nodes {
                Self::validate_node_placeholders(&template.name, node)?;
            }
        }

        let mut nodes = Vec::new();

        // Expand static nodes first
        for node in &template.nodes {
            let expanded = Self::expand_node(node, &context, 0)?;
            nodes.push(expanded);
        }

        // Expand repeated sections for applicable iterations, organized by iteration
        for n in 1..=iterations {
            for section in &template.repeat {
                if n >= section.start {
                    for repeat_node in &section.nodes {
                        let mut tn = repeat_node.clone();
                        // Prune raw template text BEFORE substitution
                        if n == 1 {
                            tn.depends_on.retain(|d| !d.contains("{n-1}"));
                            tn.after.retain(|a| !a.contains("{n-1}"));
                            if tn.gate.as_deref().is_some_and(|g| g.contains("{n-1}")) {
                                tn.gate = None;
                            }
                        }
                        nodes.push(Self::expand_node(&tn, &context, n)?);
                    }
                }
            }
        }

        // Validate the generated DAG
        let dag = Dag {
            nodes,
            metadata: Some(
                [("workflow".to_string(), template.name.clone())]
                    .iter()
                    .cloned()
                    .collect(),
            ),
        };

        // spec-38 F5: expand for_each (a template node may carry one, F1)
        // before validating, so validation sees the real executed graph.
        // A no-op for templates without for_each (N1).
        let dag = dag.expand().map_err(|e| {
            PidagError::Parse(format!(
                "Template {} for_each expansion failed: {}",
                template.name, e
            ))
        })?;

        dag.validate().map_err(|e| {
            PidagError::Parse(format!(
                "Template {} produced invalid DAG: {}",
                template.name, e
            ))
        })?;

        Ok(dag)
    }

    /// Expand a single node template, substituting {n}, {n-1}, and config values
    fn expand_node(
        node: &TemplateNode,
        context: &TemplateContext,
        n: usize,
    ) -> Result<Node, PidagError> {
        let n_str = n.to_string();
        let n_minus_1_str = if n > 0 {
            (n - 1).to_string()
        } else {
            String::new()
        };

        // Helper to substitute placeholders
        let substitute = |s: &str| -> String {
            let mut result = s.to_string();
            result = result.replace("{n}", &n_str);
            result = result.replace("{n-1}", &n_minus_1_str);
            result = result.replace("{spec_path}", &context.spec_path);
            result = result.replace("{project_root}", &context.project_root);
            result = result.replace("{validate_script}", &context.validate_script);
            result = result.replace("{quality_gate_script}", &context.quality_gate_script);

            // Replace {prompt.<key>} with actual prompt content
            for (key, value) in &context.prompts {
                result = result.replace(&format!("{{prompt.{}}}", key), value);
            }

            // Replace {models.worker(n)} with actual model chain
            if result.contains("{models.worker") {
                let models_str = serde_json::to_string(&context.models_config.models_for_iter(n))
                    .unwrap_or_default();
                result = result.replace(&format!("{{models.worker({})}}", n), &models_str);
            }

            result
        };

        // Build the prompt: use command for shell nodes, prompt for LLM nodes
        let prompt = if let Some(cmd) = &node.command {
            substitute(cmd)
        } else if let Some(p) = &node.prompt {
            substitute(p)
        } else {
            String::new()
        };

        // Parse models field (JSON array of ModelRef or {models.worker(n)})
        let models = if let Some(m) = &node.models {
            let substituted = substitute(m);
            // If it still contains {models.worker(...)}, use the actual config
            if substituted.contains("worker") {
                context.models_config.models_for_iter(n)
            } else {
                // Try to parse as JSON array
                serde_json::from_str(&substituted).unwrap_or_default()
            }
        } else {
            vec![]
        };

        // Expand edge lists
        let depends_on: Vec<String> = node.depends_on.iter().map(|d| substitute(d)).collect();
        let after: Vec<String> = node.after.iter().map(|a| substitute(a)).collect();

        let id = substitute(&node.id);
        let gate = node.gate.as_ref().map(|g| substitute(g));
        // spec-37 (C2b): a template still authors verify as a bare string;
        // wrap it into the widened `Verify` shape here so `Node.verify`
        // (now `Option<Verify>`) is produced identically to before.
        let verify = node.verify.as_ref().map(|v| Verify::Shell(substitute(v)));
        let verify_pre = node.verify_pre.as_ref().map(|v| substitute(v));

        Ok(Node {
            id,
            prompt,
            depends_on,
            models,
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: node.node_type.clone(),
            gate,
            timeout: None,
            mcp_call: None,
            after,
            verify,
            verify_pre,
            for_each: node.for_each.clone(),
            quorum: None,
        })
    }

    /// List all available workflows (built-in + project)
    pub fn list_workflows(project_root: Option<&Path>) -> Result<Vec<String>, PidagError> {
        let mut workflows = vec!["sdd".to_string(), "research".to_string()];

        // Add project workflows
        if let Some(root) = project_root {
            let workflows_dir = root.join(".pidag").join("workflows");
            if let Ok(entries) = std::fs::read_dir(&workflows_dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str().filter(|n| n.ends_with(".toml"))
                    {
                        workflows.push(name.trim_end_matches(".toml").to_string());
                    }
                }
            }
        }

        workflows.sort();
        workflows.dedup();
        Ok(workflows)
    }
}
