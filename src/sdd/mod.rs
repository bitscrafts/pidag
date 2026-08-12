pub mod resume;
pub use resume::{Checkpoint, ResumeDecision, load_checkpoint, run_id_for_spec};

use crate::core::config::{ModelsConfig, SddConfig};
use crate::core::dag::Dag;
use crate::core::error::PidagError;
use crate::workflow::{TemplateContext, WorkflowEngine};
use std::collections::HashMap;
use std::path::Path;

/// Errors produced by spec-name validation (R5 numbered-spec enforcement).
///
/// The TDD contract expects `Err(InvalidName)` for specs that fail the
/// `NN-<slug>.md` pattern. Each variant carries a human-readable message so
/// the CLI can fail fast with a clear, actionable error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    #[error("Invalid spec name: {0}")]
    InvalidName(String),
}

/// Validate that a spec filename follows the numbered `NN-<slug>.md` pattern
/// (R5 / guardrail "Do NOT allow specs without NN- numeric prefix").
///
/// The pattern is exactly two leading digits (`01`-`99`), a hyphen, a
/// lowercase-alphanumeric slug (with embedded single hyphens), and the `.md`
/// extension. No path separators, `.`, or `..` are allowed (path-traversal
/// guardrail). Fails fast rather than scanning further.
pub fn validate_spec_name(name: &str) -> Result<(), SpecError> {
    let err = || {
        SpecError::InvalidName(
            "Spec must be named NN-<slug>.md (e.g., 01-my-feature.md): exactly two leading \
             digits, a hyphen, a lowercase slug, and a .md extension"
                .to_string(),
        )
    };

    // Reject path traversal / empty.
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(err());
    }

    let Some(stem) = name.strip_suffix(".md") else {
        return Err(err());
    };

    // Split "NN-slug" on the first hyphen into the numeric prefix + slug.
    let Some((prefix, slug)) = stem.split_once('-') else {
        return Err(err());
    };

    // Exactly two ASCII digits in 01..=99.
    if prefix.len() != 2
        || !prefix.chars().all(|c| c.is_ascii_digit())
        || !(1..=99).contains(&prefix.parse::<u8>().unwrap_or(0))
    {
        return Err(err());
    }

    // Slug: non-empty, lowercase-alphanumeric, embedded single hyphens only.
    if slug.is_empty()
        || !slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || slug.starts_with('-')
        || slug.ends_with('-')
        || slug.contains("--")
    {
        return Err(err());
    }

    Ok(())
}

/// SDD (Spec-Driven Development) DAG generator.
/// Reads a spec.md file and generates a DAG using a workflow template.
/// Default template is "sdd" (iterate → quality-gate → validate loop).
pub struct SddGenerator;

impl SddGenerator {
    /// Generate an SDD DAG from a spec file and project root, using the given
    /// model chain for LLM nodes and `SddConfig` for script paths. Stamps the
    /// spec file stem into `dag.metadata["spec"]` so the trace UI can link
    /// runs back to the phase (spec) that produced them. The spec file's
    /// absolute path is threaded into the validate-node prompts so the
    /// `validate-exit-criteria.sh` script can find the spec regardless of
    /// the shell node's cwd.
    ///
    /// Uses the default "sdd" workflow template. To use a different workflow,
    /// call `from_spec_with_workflow`.
    pub fn from_spec(
        spec_path: &Path,
        project_root: &Path,
        models: &ModelsConfig,
        sdd: &SddConfig,
    ) -> Result<Dag, PidagError> {
        Self::from_spec_with_workflow(spec_path, project_root, models, sdd, "sdd", None)
    }

    /// Generate an SDD DAG using a specified workflow template.
    ///
    /// `workflow_name` defaults to "sdd". `iterations_override` allows CLI
    /// `--iterations` to take precedence over config and template.
    pub fn from_spec_with_workflow(
        spec_path: &Path,
        project_root: &Path,
        models: &ModelsConfig,
        sdd: &SddConfig,
        workflow_name: &str,
        iterations_override: Option<usize>,
    ) -> Result<Dag, PidagError> {
        let spec_content = std::fs::read_to_string(spec_path)
            .map_err(|e| PidagError::Parse(format!("Failed to read spec: {}", e)))?;
        let spec_name = spec_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        // Resolve the spec file to an absolute path so the validate shell
        // nodes (which run with cwd = project_root) can always find it.
        let spec_file = spec_path
            .canonicalize()
            .or_else(|_| std::env::current_dir().map(|d| d.join(spec_path)))
            .map(|p| p.display().to_string())
            .ok();

        Self::from_spec_content_with_workflow(
            &spec_content,
            project_root,
            models,
            sdd,
            Some(spec_name),
            spec_file,
            workflow_name,
            iterations_override,
        )
    }

    /// Generate DAG from spec content string using the default "sdd" workflow.
    /// When `spec_name` is `Some`, it is stamped into `dag.metadata["spec"]` for trace-UI provenance.
    /// `spec_file` is the absolute path used in validate-node prompts; when
    /// `None`, falls back to `"spec.md"` (legacy behaviour).
    pub fn from_spec_content(
        spec: &str,
        project_root: &Path,
        models: &ModelsConfig,
        sdd: &SddConfig,
        spec_name: Option<String>,
        spec_file: Option<String>,
    ) -> Result<Dag, PidagError> {
        Self::from_spec_content_with_workflow(
            spec,
            project_root,
            models,
            sdd,
            spec_name,
            spec_file,
            "sdd",
            None,
        )
    }

    /// Generate DAG from spec content using a specified workflow template.
    #[allow(clippy::too_many_arguments)]
    pub fn from_spec_content_with_workflow(
        spec: &str,
        project_root: &Path,
        models: &ModelsConfig,
        sdd: &SddConfig,
        spec_name: Option<String>,
        spec_file: Option<String>,
        workflow_name: &str,
        iterations_override: Option<usize>,
    ) -> Result<Dag, PidagError> {
        // Parse spec sections (TDD Contract, Exit Criteria, Guardrails, etc.)
        let sections = Self::parse_spec(spec)?;

        // Load the workflow template
        let template = WorkflowEngine::load_template(workflow_name, Some(project_root))?;

        // Resolve iteration count: CLI override > config > template
        let config_iterations = if sdd.max_iterations > 0 {
            Some(sdd.max_iterations)
        } else {
            None
        };
        let iterations = iterations_override
            .or(config_iterations)
            .or(template.iterations)
            .unwrap_or(3); // Final fallback to 3

        // Build template context with spec sections as prompts
        let mut prompts = HashMap::new();
        prompts.insert(
            "tdd_contract".to_string(),
            sections
                .get("tdd_contract")
                .map(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        );
        prompts.insert(
            "architecture".to_string(),
            sections
                .get("architecture")
                .map(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        );
        prompts.insert(
            "guardrails".to_string(),
            sections
                .get("guardrails")
                .map(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        );
        prompts.insert("research".to_string(), "Research topic".to_string());

        let spec_for_validate = spec_file.as_deref().unwrap_or("spec.md");
        let project_root_str = project_root.display().to_string();

        let context = TemplateContext {
            n: 0,
            spec_path: spec_for_validate.to_string(),
            project_root: project_root_str,
            validate_script: sdd.validate_script.display().to_string(),
            quality_gate_script: sdd.quality_gate_script.display().to_string(),
            prompts,
            models_config: models.clone(),
        };

        // Expand the template
        let mut dag = WorkflowEngine::expand(&template, iterations, context)?;

        // Stamp spec metadata if provided
        if let Some(name) = spec_name {
            if dag.metadata.is_none() {
                dag.metadata = Some(HashMap::new());
            }
            if let Some(meta) = &mut dag.metadata {
                meta.insert("spec".to_string(), name);
            }
        }

        Ok(dag)
    }

    /// Parse spec into sections. Returns a map of section name -> content.
    fn parse_spec(spec: &str) -> Result<std::collections::HashMap<String, String>, PidagError> {
        let mut sections = std::collections::HashMap::new();

        // Extract TDD Contract section
        if let Some(start) = spec.find("## TDD Contract") {
            if let Some(next_section) = spec[start + 14..].find("##").map(|i| i + start + 14) {
                sections.insert(
                    "tdd_contract".to_string(),
                    spec[start..next_section].trim().to_string(),
                );
            } else {
                sections.insert("tdd_contract".to_string(), spec[start..].trim().to_string());
            }
        }

        // Extract Architecture section
        if let Some(start) = spec.find("## Architecture") {
            if let Some(next_section) = spec[start + 14..].find("##").map(|i| i + start + 14) {
                sections.insert(
                    "architecture".to_string(),
                    spec[start..next_section].trim().to_string(),
                );
            } else {
                sections.insert("architecture".to_string(), spec[start..].trim().to_string());
            }
        }

        // Extract Exit Criteria section
        if let Some(start) = spec.find("## Exit Criteria") {
            if let Some(next_section) = spec[start + 15..].find("##").map(|i| i + start + 15) {
                sections.insert(
                    "exit_criteria".to_string(),
                    spec[start..next_section].trim().to_string(),
                );
            } else {
                sections.insert(
                    "exit_criteria".to_string(),
                    spec[start..].trim().to_string(),
                );
            }
        }

        // Extract Guardrails section
        if let Some(start) = spec.find("## Guardrails") {
            if let Some(next_section) = spec[start + 13..].find("##").map(|i| i + start + 13) {
                sections.insert(
                    "guardrails".to_string(),
                    spec[start..next_section].trim().to_string(),
                );
            } else {
                sections.insert("guardrails".to_string(), spec[start..].trim().to_string());
            }
        }

        Ok(sections)
    }
}

// Tests are in tests/sdd_tests.rs (external test file)
