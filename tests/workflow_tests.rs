//! Tests for declarative workflow templates (spec-26)

use pidag::{Config, Dag, ModelsConfig, SddGenerator};
use std::collections::HashMap;
use std::path::PathBuf;

// ============================================================================
// W1: Test project workflow overrides built-in
// ============================================================================
#[test]
fn test_loads_project_template_over_builtin() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/project_override");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    // Create a custom workflow
    let workflows_dir = tmpdir.join(".pidag").join("workflows");
    std::fs::create_dir_all(&workflows_dir).ok();

    let custom_sdd = r#"
name        = "sdd"
description = "Custom SDD template"
iterations  = 2

[[nodes]]
id      = "custom-baseline"
type    = "shell"
command = "echo custom"
"#;

    std::fs::write(workflows_dir.join("sdd.toml"), custom_sdd).ok();

    // Load the template
    let template = pidag::workflow::WorkflowEngine::load_template("sdd", Some(&tmpdir))
        .expect("should load custom template");

    assert_eq!(template.name, "sdd");
    assert_eq!(template.iterations, Some(2));
    assert_eq!(template.nodes[0].id, "custom-baseline");
}

// ============================================================================
// W2: Test built-in SDD template produces parity with golden fixture
// ============================================================================
#[test]
fn test_builtin_sdd_matches_golden() {
    use pidag::workflow::{TemplateContext, WorkflowEngine};

    // Load the built-in sdd template
    let template = WorkflowEngine::load_template("sdd", None).expect("should load built-in sdd");

    // Build a fixed context for reproducibility (from literal values, not Config::default)
    let mut prompts = HashMap::new();
    prompts.insert("tdd_contract".to_string(), "Test requirement".to_string());
    prompts.insert("architecture".to_string(), "Test architecture".to_string());
    prompts.insert("guardrails".to_string(), "Test guardrails".to_string());

    let context = TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: "/project".to_string(),
        validate_script: "/validate.sh".to_string(),
        quality_gate_script: "/quality-gate.sh".to_string(),
        prompts,
        models_config: ModelsConfig::default(),
    };

    // Expand the template at 3 iterations
    let dag = WorkflowEngine::expand(&template, 3, context).expect("should expand sdd template");

    // Load the golden fixture
    let golden_json = std::fs::read_to_string("tests/fixtures/sdd_golden.json")
        .expect("should read golden fixture");
    let golden_dag: Dag = serde_json::from_str(&golden_json).expect("should parse golden fixture");

    // Assert deep equality
    assert_eq!(
        dag.nodes.len(),
        golden_dag.nodes.len(),
        "node count mismatch"
    );

    for (i, (actual, expected)) in dag.nodes.iter().zip(golden_dag.nodes.iter()).enumerate() {
        assert_eq!(
            actual.id, expected.id,
            "node {} id mismatch: {} != {}",
            i, actual.id, expected.id
        );
        assert_eq!(actual.prompt, expected.prompt, "node {} prompt mismatch", i);
        assert_eq!(
            actual.depends_on, expected.depends_on,
            "node {} depends_on mismatch",
            i
        );
        assert_eq!(actual.gate, expected.gate, "node {} gate mismatch", i);
        assert_eq!(actual.after, expected.after, "node {} after mismatch", i);
        assert_eq!(actual.verify, expected.verify, "node {} verify mismatch", i);
        assert_eq!(
            actual.verify_pre, expected.verify_pre,
            "node {} verify_pre mismatch",
            i
        );
    }
}

// ============================================================================
// W3a: Test iterations from config max_iterations
// ============================================================================
#[test]
fn test_iterations_from_config() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/iterations_config");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    // Create spec and config
    let spec_content = r#"## TDD Contract
Test
## Architecture
Test
## Exit Criteria
Test
## Guardrails
Test"#;
    std::fs::write(tmpdir.join("spec.md"), spec_content).ok();

    // Create config with max_iterations = 5
    let config_toml = r#"[project]
root = "."

[worker]
default_model = "test"
timeout_secs = 300
thinking_level = "low"

[sdd]
max_iterations = 5
validate_script = "/tmp/validate.sh"
quality_gate_script = "/tmp/qg.sh"
"#;
    std::fs::create_dir_all(tmpdir.join(".pidag")).ok();
    std::fs::write(tmpdir.join(".pidag/config.toml"), config_toml).ok();

    // Load config and generate DAG
    let config = Config::from_toml_str(config_toml).expect("should load config");
    let dag = SddGenerator::from_spec(
        &tmpdir.join("spec.md"),
        &tmpdir,
        &config.models,
        &config.sdd,
    )
    .expect("should generate DAG");

    // Should have 5 iterations: baseline + 5*(implement+qg+validate) = 16 nodes
    let impl_count = dag
        .nodes
        .iter()
        .filter(|n| n.id.starts_with("implement-iter"))
        .count();
    assert_eq!(
        impl_count, 5,
        "should have 5 implement nodes from config max_iterations"
    );
}

// ============================================================================
// W3b: Test --iterations CLI override
// ============================================================================
#[test]
fn test_iterations_cli_overrides_config() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/iterations_cli");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    let spec_content = r#"## TDD Contract
Test
## Architecture
Test
## Exit Criteria
Test
## Guardrails
Test"#;
    std::fs::write(tmpdir.join("spec.md"), spec_content).ok();

    // Config has 3, CLI override is 2
    let config = Config::default();
    let dag = SddGenerator::from_spec_with_workflow(
        &tmpdir.join("spec.md"),
        &tmpdir,
        &config.models,
        &config.sdd,
        "sdd",
        Some(2), // CLI override
    )
    .expect("should generate DAG");

    let impl_count = dag
        .nodes
        .iter()
        .filter(|n| n.id.starts_with("implement-iter"))
        .count();
    assert_eq!(impl_count, 2, "CLI override should produce 2 iterations");
}

// ============================================================================
// W4: Test no hardcoded node IDs in src/sdd/
// ============================================================================
#[test]
fn test_no_hardcoded_node_ids_in_src() {
    // Verify that src/sdd/*.rs doesn't contain hardcoded implement-iter literals
    let sdd_dir = std::fs::read_dir("src/sdd").expect("should read src/sdd");

    for entry in sdd_dir {
        if let Ok(entry) = entry {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "rs") {
                let content = std::fs::read_to_string(&path).expect("should read file");

                // Look for patterns like implement-iter1, implement-iter2, etc.
                for i in 0..10 {
                    let pattern = format!("implement-iter{}", i);
                    if content.contains(&pattern) {
                        panic!("Found hardcoded '{}' in {}", pattern, path.display());
                    }
                }

                // Also look for quality-gate patterns
                for i in 0..10 {
                    let pattern = format!("quality-gate-{}", i);
                    if content.contains(&pattern) {
                        panic!("Found hardcoded '{}' in {}", pattern, path.display());
                    }
                }

                // And validate-iter patterns
                for i in 0..10 {
                    let pattern = format!("validate-iter{}", i);
                    if content.contains(&pattern) {
                        panic!("Found hardcoded '{}' in {}", pattern, path.display());
                    }
                }
            }
        }
    }
}

// ============================================================================
// W5a: Test first iteration drops {n-1} edges
// ============================================================================
#[test]
fn test_first_iteration_drops_prev_edges() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/iter1_edges");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    let spec_content = r#"## TDD Contract
Test
## Architecture
Test
## Exit Criteria
Test
## Guardrails
Test"#;
    std::fs::write(tmpdir.join("spec.md"), spec_content).ok();

    let config = Config::default();
    let dag = SddGenerator::from_spec(
        &tmpdir.join("spec.md"),
        &tmpdir,
        &config.models,
        &config.sdd,
    )
    .expect("should generate DAG");

    // Check that implement-iter1 has no dependencies
    let impl1 = dag
        .nodes
        .iter()
        .find(|n| n.id == "implement-iter1")
        .unwrap();
    assert!(
        impl1.depends_on.is_empty(),
        "implement-iter1 should have no depends_on edges"
    );
    assert!(impl1.gate.is_none(), "implement-iter1 should have no gate");

    // Check that implement-iter2 has proper dependencies
    let impl2 = dag
        .nodes
        .iter()
        .find(|n| n.id == "implement-iter2")
        .unwrap();
    assert!(
        !impl2.depends_on.is_empty(),
        "implement-iter2 should have depends_on edges"
    );
    assert!(impl2.gate.is_some(), "implement-iter2 should have a gate");
}

// ============================================================================
// W5b: Test template error reporting
// ============================================================================
#[test]
fn test_template_error_names_template_and_node() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/template_error");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    // Create a bad template with dangling references
    let workflows_dir = tmpdir.join(".pidag").join("workflows");
    std::fs::create_dir_all(&workflows_dir).ok();

    let bad_template = r#"
name        = "bad"
description = "Template with dangling ref"
iterations  = 1

[[nodes]]
id      = "test-node"
type    = "shell"
command = "echo test"
after   = ["nonexistent-node"]
"#;

    std::fs::write(workflows_dir.join("bad.toml"), bad_template).ok();

    // Try to use the bad template
    let result =
        pidag::workflow::WorkflowEngine::load_template("bad", Some(&tmpdir)).and_then(|template| {
            let config = Config::default();
            pidag::workflow::WorkflowEngine::expand(
                &template,
                1,
                pidag::workflow::TemplateContext {
                    n: 0,
                    spec_path: "spec.md".to_string(),
                    project_root: tmpdir.display().to_string(),
                    validate_script: config.sdd.validate_script.display().to_string(),
                    quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
                    prompts: HashMap::new(),
                    models_config: config.models,
                },
            )
        });

    // Should fail with an error mentioning the template and node
    assert!(result.is_err(), "should fail with dangling reference");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("bad"),
        "error should mention template name"
    );
}

// ============================================================================
// W6: Test workflows show renders without running
// ============================================================================
#[test]
fn test_workflows_show_renders_without_running() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/workflows_show");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    // Load the built-in SDD template
    let template = pidag::workflow::WorkflowEngine::load_template("sdd", Some(&tmpdir))
        .expect("should load built-in sdd");

    let config = Config::default();
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: {
            let mut m = HashMap::new();
            m.insert("tdd_contract".to_string(), "test".to_string());
            m.insert("architecture".to_string(), "test".to_string());
            m.insert("guardrails".to_string(), "test".to_string());
            m.insert("research".to_string(), "test".to_string());
            m
        },
        models_config: config.models,
    };

    // Expand the template (this is what workflows show does)
    let dag = pidag::workflow::WorkflowEngine::expand(&template, 3, context)
        .expect("should expand template");

    // Render to mermaid (this is also what workflows show does)
    let mermaid = pidag::render_dag_mermaid(&dag);

    // Should render without any node execution
    assert!(
        mermaid.contains("implement-iter1"),
        "should render implement-iter1 in mermaid"
    );
    assert!(mermaid.len() > 0, "mermaid output should not be empty");
}

// ============================================================================
// W7: Test research template expands correctly
// ============================================================================
#[test]
fn test_research_template_expands() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/research_template");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    // Load the built-in research template
    let template = pidag::workflow::WorkflowEngine::load_template("research", Some(&tmpdir))
        .expect("should load built-in research");

    assert_eq!(template.name, "research");

    let config = Config::default();
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: {
            let mut m = HashMap::new();
            m.insert("research".to_string(), "research topic".to_string());
            m
        },
        models_config: config.models,
    };

    // Expand without iterations (research has no iteration loop)
    let dag = pidag::workflow::WorkflowEngine::expand(&template, 1, context)
        .expect("should expand research template");

    // Verify the fan-out/fan-in structure
    assert!(
        dag.nodes.iter().any(|n| n.id == "investigate-1"),
        "should have investigate-1"
    );
    assert!(
        dag.nodes.iter().any(|n| n.id == "investigate-2"),
        "should have investigate-2"
    );
    assert!(
        dag.nodes.iter().any(|n| n.id == "investigate-3"),
        "should have investigate-3"
    );
    assert!(
        dag.nodes.iter().any(|n| n.id == "synthesize"),
        "should have synthesize node"
    );
    assert!(
        dag.nodes.iter().any(|n| n.id == "validate-research"),
        "should have validate-research"
    );

    // Verify the fan-in structure: synthesize depends on all investigations
    let synthesize = dag.nodes.iter().find(|n| n.id == "synthesize").unwrap();
    assert!(synthesize.depends_on.contains(&"investigate-1".to_string()));
    assert!(synthesize.depends_on.contains(&"investigate-2".to_string()));
    assert!(synthesize.depends_on.contains(&"investigate-3".to_string()));
    // W7′ authorized edit: synthesize.after should be empty (uses depends_on instead)
    assert!(
        synthesize.after.is_empty(),
        "synthesize should have no after edges"
    );

    // Verify no iteration loop, no quality gate
    let has_impl_iter = dag.nodes.iter().any(|n| n.id.contains("implement-iter"));
    let has_qg = dag.nodes.iter().any(|n| n.id.contains("quality-gate"));
    assert!(
        !has_impl_iter,
        "research should not have implement-iter nodes"
    );
    assert!(!has_qg, "research should not have quality-gate nodes");
}

// ============================================================================
// W8: Test hand-written DAG unchanged
// ============================================================================
#[test]
fn test_hand_written_dag_unchanged() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/handwritten_dag");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    // Create a hand-written DAG
    let dag_json = r#"{
  "nodes": [
    {
      "id": "node1",
      "prompt": "test",
      "depends_on": [],
      "models": [],
      "retry": {"attempts": 1, "backoff_ms": 0},
      "validate": null,
      "node_type": "shell"
    }
  ],
  "metadata": null
}"#;

    std::fs::write(tmpdir.join("test.json"), dag_json).ok();

    // Parse the DAG
    let parsed: pidag::Dag = serde_json::from_str(dag_json).expect("should parse JSON");

    // Validate it still works
    assert!(
        parsed.validate().is_ok(),
        "hand-written DAG should validate"
    );
    assert_eq!(parsed.nodes.len(), 1);
    assert_eq!(parsed.nodes[0].id, "node1");
}

// ============================================================================
// R1a: Test all builtin templates parse
// ============================================================================
#[test]
fn test_all_builtin_templates_parse() {
    for template_name in &["sdd", "research"] {
        let result = pidag::workflow::WorkflowEngine::load_template(template_name, None);
        assert!(result.is_ok(), "template {} should parse", template_name);
        let template = result.unwrap();
        assert_eq!(template.name, *template_name);
    }
}

// ============================================================================
// R1b: Test no null literal in shipped templates
// ============================================================================
#[test]
fn test_no_null_literal_in_shipped_templates() {
    for file in &[
        "src/workflow/templates/sdd.toml",
        "src/workflow/templates/research.toml",
    ] {
        let content =
            std::fs::read_to_string(file).unwrap_or_else(|_| panic!("should read {}", file));
        // Look for bare "null" token (not inside strings)
        for line in content.lines() {
            if line.trim().starts_with('#') {
                continue;
            }
            // Check for = null pattern (outside of quoted strings)
            if line.contains("= null") || line.contains("= null,") {
                panic!("found 'null' in {}: {}", file, line);
            }
        }
    }
}

// ============================================================================
// R2a: Test prev edges dropped for generic IDs
// ============================================================================
#[test]
fn test_prev_edges_dropped_for_generic_ids() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/generic_ids");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    let workflows_dir = tmpdir.join(".pidag").join("workflows");
    std::fs::create_dir_all(&workflows_dir).ok();

    let template_toml = r#"
name = "generic"
description = "Template with generic IDs"
iterations = 2

[[repeat]]
start = 1

  [[repeat.nodes]]
  id = "review-{n}"
  type = "shell"
  command = "echo review"
  depends_on = ["review-{n-1}"]
"#;

    std::fs::write(workflows_dir.join("generic.toml"), template_toml).ok();

    let template = pidag::workflow::WorkflowEngine::load_template("generic", Some(&tmpdir))
        .expect("should load generic template");

    let config = Config::default();
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models,
    };

    let dag = pidag::workflow::WorkflowEngine::expand(&template, 2, context)
        .expect("should expand generic template");

    // review-1 should have empty depends_on (dropped {n-1})
    let review1 = dag.nodes.iter().find(|n| n.id == "review-1").unwrap();
    assert!(
        review1.depends_on.is_empty(),
        "review-1 should have no depends_on"
    );

    // review-2 should have depends_on = ["review-1"]
    let review2 = dag.nodes.iter().find(|n| n.id == "review-2").unwrap();
    assert!(
        review2.depends_on.contains(&"review-1".to_string()),
        "review-2 should depend on review-1"
    );
}

// ============================================================================
// R2b: Test no iter0 heuristic in source
// ============================================================================
#[test]
fn test_no_iter0_heuristic_in_source() {
    let mod_content =
        std::fs::read_to_string("src/workflow/mod.rs").expect("should read src/workflow/mod.rs");
    assert!(
        !mod_content.contains("\"iter0\""),
        "should not have literal \"iter0\" in src/workflow/mod.rs"
    );
}

// ============================================================================
// R3: Test gate without prev ref survives iter1
// ============================================================================
#[test]
fn test_gate_without_prev_ref_survives_iter1() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/gate_no_prev");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    let workflows_dir = tmpdir.join(".pidag").join("workflows");
    std::fs::create_dir_all(&workflows_dir).ok();

    let template_toml = r#"
name = "gate_test"
description = "Template with gate without {n-1}"
iterations = 2

[[nodes]]
id = "baseline"
type = "shell"
command = "echo base"

[[repeat]]
start = 1

  [[repeat.nodes]]
  id = "process-{n}"
  type = "shell"
  command = "echo process"
  gate = "baseline:success"
"#;

    std::fs::write(workflows_dir.join("gate_test.toml"), template_toml).ok();

    let template = pidag::workflow::WorkflowEngine::load_template("gate_test", Some(&tmpdir))
        .expect("should load gate_test template");

    let config = Config::default();
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models,
    };

    let dag = pidag::workflow::WorkflowEngine::expand(&template, 2, context)
        .expect("should expand gate_test template");

    // process-1 should retain the gate (no {n-1} reference)
    let process1 = dag.nodes.iter().find(|n| n.id == "process-1").unwrap();
    assert_eq!(
        process1.gate,
        Some("baseline:success".to_string()),
        "process-1 should retain gate"
    );
}

// ============================================================================
// R4a: Test research synthesize uses depends_on
// ============================================================================
#[test]
fn test_research_synthesize_uses_depends_on() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/research_depends_on");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    let template = pidag::workflow::WorkflowEngine::load_template("research", Some(&tmpdir))
        .expect("should load research template");

    let config = Config::default();
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: {
            let mut m = HashMap::new();
            m.insert("research".to_string(), "topic".to_string());
            m
        },
        models_config: config.models,
    };

    let dag = pidag::workflow::WorkflowEngine::expand(&template, 1, context)
        .expect("should expand research template");

    let synthesize = dag.nodes.iter().find(|n| n.id == "synthesize").unwrap();
    assert!(
        synthesize.depends_on.contains(&"investigate-1".to_string()),
        "synthesize should depend on investigate-1"
    );
    assert!(
        synthesize.depends_on.contains(&"investigate-2".to_string()),
        "synthesize should depend on investigate-2"
    );
    assert!(
        synthesize.depends_on.contains(&"investigate-3".to_string()),
        "synthesize should depend on investigate-3"
    );
    assert!(
        synthesize.after.is_empty(),
        "synthesize should have no after edges"
    );
}

// ============================================================================
// R4b: Test research validate keeps after
// ============================================================================
#[test]
fn test_research_validate_keeps_after() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/research_validate_after");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    let template = pidag::workflow::WorkflowEngine::load_template("research", Some(&tmpdir))
        .expect("should load research template");

    let config = Config::default();
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: {
            let mut m = HashMap::new();
            m.insert("research".to_string(), "topic".to_string());
            m
        },
        models_config: config.models,
    };

    let dag = pidag::workflow::WorkflowEngine::expand(&template, 1, context)
        .expect("should expand research template");

    let validate = dag
        .nodes
        .iter()
        .find(|n| n.id == "validate-research")
        .unwrap();
    assert_eq!(
        validate.after,
        vec!["synthesize".to_string()],
        "validate-research should have after = [synthesize]"
    );
    assert!(
        validate.depends_on.is_empty(),
        "validate-research should have empty depends_on"
    );
}

// ============================================================================
// R5: Test no output interpolation syntax in research
// ============================================================================
#[test]
fn test_no_output_interpolation_syntax_in_templates() {
    // Spec-29 I9: research.toml has output references on synthesize again, restored
    // after spec-27 R5 stripped them because nothing implemented the interpolation.
    // The assertion is INVERTED, not removed: spec-27 required their absence because
    // they were dead syntax; spec-29 implements them, so their presence is now the
    // requirement. This test still fails if fan-in loses its inputs.
    // Research synthesize should reference investigate outputs for fan-in.
    let content = std::fs::read_to_string("src/workflow/templates/research.toml")
        .expect("should read research.toml");
    // Verify that synthesize references investigate outputs (I9)
    assert!(
        content.contains("investigate-1.output"),
        "research.toml synthesize should reference investigate-1.output"
    );
    assert!(
        content.contains("investigate-2.output"),
        "research.toml synthesize should reference investigate-2.output"
    );
    assert!(
        content.contains("investigate-3.output"),
        "research.toml synthesize should reference investigate-3.output"
    );
}

// ============================================================================
// R6a: Test unknown placeholder is hard error
// ============================================================================
#[test]
fn test_unknown_placeholder_is_hard_error() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/unknown_placeholder");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    let workflows_dir = tmpdir.join(".pidag").join("workflows");
    std::fs::create_dir_all(&workflows_dir).ok();

    let template_toml = r#"
name = "bad_placeholder"
description = "Template with unknown placeholder"
iterations = 1

[[nodes]]
id = "test"
type = "shell"
command = "echo {bogus}"
"#;

    std::fs::write(workflows_dir.join("bad_placeholder.toml"), template_toml).ok();

    let template = pidag::workflow::WorkflowEngine::load_template("bad_placeholder", Some(&tmpdir))
        .expect("should load bad_placeholder template");

    let config = Config::default();
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models,
    };

    let result = pidag::workflow::WorkflowEngine::expand(&template, 1, context);
    assert!(result.is_err(), "should fail with unknown placeholder");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("bogus"),
        "error should mention bogus placeholder"
    );
    // Strengthen the test to verify it names template and node
    assert!(
        err.contains("bad_placeholder"),
        "error should mention template name 'bad_placeholder'"
    );
    assert!(err.contains("test"), "error should mention node id 'test'");
}

// ============================================================================
// R6b: Test clean expansion has no residual braces
// ============================================================================
#[test]
fn test_clean_expansion_has_no_residual_braces() {
    let config = Config::default();

    // Test sdd at 3 iterations
    let sdd_template =
        pidag::workflow::WorkflowEngine::load_template("sdd", None).expect("should load sdd");

    let mut prompts = HashMap::new();
    prompts.insert("tdd_contract".to_string(), "contract".to_string());
    prompts.insert("architecture".to_string(), "arch".to_string());
    prompts.insert("guardrails".to_string(), "guards".to_string());

    let sdd_context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: "/project".to_string(),
        validate_script: "/validate.sh".to_string(),
        quality_gate_script: "/qg.sh".to_string(),
        prompts,
        models_config: config.models.clone(),
    };

    let sdd_dag = pidag::workflow::WorkflowEngine::expand(&sdd_template, 3, sdd_context)
        .expect("should expand sdd");

    // Check sdd nodes for leftover braces (masking {{...}})
    for node in &sdd_dag.nodes {
        let check_field = |field: &str| {
            let mut masked = String::new();
            let mut chars = field.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '{' && chars.peek() == Some(&'{') {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '}' && chars.peek() == Some(&'}') {
                            chars.next();
                            break;
                        }
                    }
                } else {
                    masked.push(ch);
                }
            }
            assert!(
                !masked.contains('{'),
                "node {} has residual braces in: {}",
                node.id,
                field
            );
        };

        check_field(&node.id);
        check_field(&node.prompt);
        if let Some(ref g) = node.gate {
            check_field(g);
        }
        if let Some(ref v) = node.verify {
            check_field(v);
        }
        for dep in &node.depends_on {
            check_field(dep);
        }
        for a in &node.after {
            check_field(a);
        }
    }

    // Test research
    let research_template = pidag::workflow::WorkflowEngine::load_template("research", None)
        .expect("should load research");

    let research_context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: "/project".to_string(),
        validate_script: "/validate.sh".to_string(),
        quality_gate_script: "/qg.sh".to_string(),
        prompts: {
            let mut m = HashMap::new();
            m.insert("research".to_string(), "topic".to_string());
            m
        },
        models_config: config.models,
    };

    let research_dag =
        pidag::workflow::WorkflowEngine::expand(&research_template, 1, research_context)
            .expect("should expand research");

    for node in &research_dag.nodes {
        assert!(
            !node.id.contains('{'),
            "research node {} has braces in id",
            node.id
        );
        // Research nodes may contain {{...}} for output interpolation (I9)
        // but should not have residual braces from template expansion
        let check_field = |field: &str| {
            let mut masked = String::new();
            let mut chars = field.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '{' && chars.peek() == Some(&'{') {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '}' && chars.peek() == Some(&'}') {
                            chars.next();
                            break;
                        }
                    }
                } else {
                    masked.push(ch);
                }
            }
            assert!(
                !masked.contains('{'),
                "research node {} has residual braces in: {}",
                node.id,
                field
            );
        };
        check_field(&node.prompt);
    }
}

// ============================================================================
// R8a: Test implement_iter1 says from scratch
// ============================================================================
#[test]
fn test_implement_iter1_says_from_scratch() {
    let config = Config::default();
    let template =
        pidag::workflow::WorkflowEngine::load_template("sdd", None).expect("should load sdd");

    let mut prompts = HashMap::new();
    prompts.insert("tdd_contract".to_string(), "contract".to_string());
    prompts.insert("architecture".to_string(), "arch".to_string());
    prompts.insert("guardrails".to_string(), "guards".to_string());

    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: "/project".to_string(),
        validate_script: "/validate.sh".to_string(),
        quality_gate_script: "/qg.sh".to_string(),
        prompts,
        models_config: config.models,
    };

    let dag =
        pidag::workflow::WorkflowEngine::expand(&template, 3, context).expect("should expand sdd");

    let iter1 = dag
        .nodes
        .iter()
        .find(|n| n.id == "implement-iter1")
        .unwrap();
    assert!(
        iter1.prompt.contains("Implement from scratch"),
        "implement-iter1 should say 'Implement from scratch'"
    );
    assert!(
        !iter1.prompt.contains("{{validate-"),
        "implement-iter1 should not have {{validate- syntax"
    );
}

// ============================================================================
// R8b: Test implement_iter2 repairs from prev output
// ============================================================================
#[test]
fn test_implement_iter2_repairs_from_prev_output() {
    let config = Config::default();
    let template =
        pidag::workflow::WorkflowEngine::load_template("sdd", None).expect("should load sdd");

    let mut prompts = HashMap::new();
    prompts.insert("tdd_contract".to_string(), "contract".to_string());
    prompts.insert("architecture".to_string(), "arch".to_string());
    prompts.insert("guardrails".to_string(), "guards".to_string());

    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: "/project".to_string(),
        validate_script: "/validate.sh".to_string(),
        quality_gate_script: "/qg.sh".to_string(),
        prompts,
        models_config: config.models,
    };

    let dag =
        pidag::workflow::WorkflowEngine::expand(&template, 3, context).expect("should expand sdd");

    let iter2 = dag
        .nodes
        .iter()
        .find(|n| n.id == "implement-iter2")
        .unwrap();
    assert!(
        iter2.prompt.contains("Fix the failures reported in"),
        "implement-iter2 should say 'Fix the failures reported in'"
    );
    assert!(
        iter2.prompt.contains("{{validate-iter1.output}}"),
        "implement-iter2 should reference {{validate-iter1.output}}"
    );
}

// ============================================================================
// R8c: Test implement_iter3 references iter2 output
// ============================================================================
#[test]
fn test_implement_iter3_references_iter2_output() {
    let config = Config::default();
    let template =
        pidag::workflow::WorkflowEngine::load_template("sdd", None).expect("should load sdd");

    let mut prompts = HashMap::new();
    prompts.insert("tdd_contract".to_string(), "contract".to_string());
    prompts.insert("architecture".to_string(), "arch".to_string());
    prompts.insert("guardrails".to_string(), "guards".to_string());

    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: "/project".to_string(),
        validate_script: "/validate.sh".to_string(),
        quality_gate_script: "/qg.sh".to_string(),
        prompts,
        models_config: config.models,
    };

    let dag =
        pidag::workflow::WorkflowEngine::expand(&template, 3, context).expect("should expand sdd");

    let iter3 = dag
        .nodes
        .iter()
        .find(|n| n.id == "implement-iter3")
        .unwrap();
    assert!(
        iter3.prompt.contains("{{validate-iter2.output}}"),
        "implement-iter3 should reference {{validate-iter2.output}}"
    );
}

// ============================================================================
// R8d: Test repeat start offsets iterations
// ============================================================================
#[test]
fn test_repeat_start_offsets_iterations() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/repeat_start_offset");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    let workflows_dir = tmpdir.join(".pidag").join("workflows");
    std::fs::create_dir_all(&workflows_dir).ok();

    let template_toml = r#"
name = "offset_test"
description = "Template with offset repeat sections"
iterations = 3

[[repeat]]
start = 1

  [[repeat.nodes]]
  id = "early-{n}"
  type = "shell"
  command = "echo early"

[[repeat]]
start = 3

  [[repeat.nodes]]
  id = "late-{n}"
  type = "shell"
  command = "echo late"
"#;

    std::fs::write(workflows_dir.join("offset_test.toml"), template_toml).ok();

    let template = pidag::workflow::WorkflowEngine::load_template("offset_test", Some(&tmpdir))
        .expect("should load offset_test template");

    let config = Config::default();
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models,
    };

    let dag = pidag::workflow::WorkflowEngine::expand(&template, 3, context)
        .expect("should expand offset_test template");

    // start=3 section should produce exactly 1 node
    let late_count = dag.nodes.iter().filter(|n| n.id.contains("late-")).count();
    assert_eq!(late_count, 1, "should have exactly 1 late-* node");
    assert!(dag.nodes.iter().any(|n| n.id == "late-3"));

    // start=1 section should produce 3 nodes
    let early_count = dag.nodes.iter().filter(|n| n.id.contains("early-")).count();
    assert_eq!(early_count, 3, "should have exactly 3 early-* nodes");
}

// ============================================================================
// R8e: Test repeat start defaults to one
// ============================================================================
#[test]
fn test_repeat_start_defaults_to_one() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/repeat_default_start");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    let workflows_dir = tmpdir.join(".pidag").join("workflows");
    std::fs::create_dir_all(&workflows_dir).ok();

    let template_toml = r#"
name = "default_start"
description = "Template with default repeat start"
iterations = 2

[[repeat]]

  [[repeat.nodes]]
  id = "item-{n}"
  type = "shell"
  command = "echo item"
"#;

    std::fs::write(workflows_dir.join("default_start.toml"), template_toml).ok();

    let template = pidag::workflow::WorkflowEngine::load_template("default_start", Some(&tmpdir))
        .expect("should load default_start template");

    let config = Config::default();
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models,
    };

    let dag = pidag::workflow::WorkflowEngine::expand(&template, 2, context)
        .expect("should expand default_start template");

    // Should have 2 nodes (item-1, item-2) since start defaults to 1
    assert_eq!(dag.nodes.len(), 2, "should have exactly 2 nodes");
    assert!(dag.nodes.iter().any(|n| n.id == "item-1"));
    assert!(dag.nodes.iter().any(|n| n.id == "item-2"));
}

// ============================================================================
// R6b: Test spec sections with braces do not break generation (P0 regression)
// ============================================================================
#[test]
fn test_spec_sections_with_braces_do_not_break_generation() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/spec_with_braces");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    // Create a spec with literal braces (JSON, Rust generics, etc.)
    let spec_content = r#"## TDD Contract
{ "json": 1 }
{n}
Vec<T> { }

## Architecture
Test

## Exit Criteria
Test

## Guardrails
Test"#;
    std::fs::write(tmpdir.join("spec.md"), spec_content).ok();

    let config = Config::default();
    let result = SddGenerator::from_spec(
        &tmpdir.join("spec.md"),
        &tmpdir,
        &config.models,
        &config.sdd,
    );

    // Generation must SUCCEED
    assert!(
        result.is_ok(),
        "generation should succeed with literal braces in spec: {:?}",
        result.err()
    );

    let dag = result.unwrap();

    // implement-iter1.prompt must contain the spec text verbatim
    let iter1 = dag
        .nodes
        .iter()
        .find(|n| n.id == "implement-iter1")
        .unwrap();

    assert!(
        iter1.prompt.contains("{ \"json\": 1 }"),
        "prompt should contain JSON: {}",
        iter1.prompt
    );
    assert!(
        iter1.prompt.contains("{n}"),
        "prompt should contain {{n}}: {}",
        iter1.prompt
    );
    assert!(
        iter1.prompt.contains("Vec<T> { }"),
        "prompt should contain Vec<T> {{ }}: {}",
        iter1.prompt
    );
}

// ============================================================================
// R6c: Test double-brace with inner placeholder is valid
// ============================================================================
#[test]
fn test_double_brace_with_inner_placeholder_is_valid() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/double_brace_inner");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    let workflows_dir = tmpdir.join(".pidag").join("workflows");
    std::fs::create_dir_all(&workflows_dir).ok();

    // Template with {{...}} containing inner {n-1}
    let template_toml = r#"
name = "double_brace_test"
description = "Template with double-brace and inner placeholder"
iterations = 2

[[repeat]]
start = 2

  [[repeat.nodes]]
  id = "test-{n}"
  type = "shell"
  command = "echo process"
  prompt = "Reference output: {{validate-iter{n-1}.output}}"
"#;

    std::fs::write(workflows_dir.join("double_brace_test.toml"), template_toml).ok();

    let template =
        pidag::workflow::WorkflowEngine::load_template("double_brace_test", Some(&tmpdir))
            .expect("should load double_brace_test template");

    let config = Config::default();
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models,
    };

    // Should succeed - {{...}} masks the outer braces, but {n-1} is recognized as valid
    let result = pidag::workflow::WorkflowEngine::expand(&template, 2, context);
    assert!(
        result.is_ok(),
        "should succeed with double-brace containing inner placeholder: {:?}",
        result.err()
    );
}

// ============================================================================
// R6d: Test unknown placeholder in every field
// ============================================================================
#[test]
fn test_unknown_placeholder_in_every_field() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/unknown_in_fields");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    let workflows_dir = tmpdir.join(".pidag").join("workflows");
    std::fs::create_dir_all(&workflows_dir).ok();

    let config = Config::default();

    // Test unknown placeholder in id
    let template_toml_id = r#"
name = "test_id_unknown"
description = "Test unknown in id"
iterations = 1

[[nodes]]
id = "test-{bogus}"
type = "shell"
command = "echo test"
"#;
    std::fs::write(workflows_dir.join("test_id_unknown.toml"), template_toml_id).ok();
    let template = pidag::workflow::WorkflowEngine::load_template("test_id_unknown", Some(&tmpdir))
        .expect("should load template");
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models.clone(),
    };
    assert!(
        pidag::workflow::WorkflowEngine::expand(&template, 1, context).is_err(),
        "should fail with unknown in id"
    );

    // Test unknown placeholder in prompt
    let template_toml_prompt = r#"
name = "test_prompt_unknown"
description = "Test unknown in prompt"
iterations = 1

[[nodes]]
id = "test"
type = "llm"
prompt = "Test {bogus} prompt"
"#;
    std::fs::write(
        workflows_dir.join("test_prompt_unknown.toml"),
        template_toml_prompt,
    )
    .ok();
    let template =
        pidag::workflow::WorkflowEngine::load_template("test_prompt_unknown", Some(&tmpdir))
            .expect("should load template");
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models.clone(),
    };
    assert!(
        pidag::workflow::WorkflowEngine::expand(&template, 1, context).is_err(),
        "should fail with unknown in prompt"
    );

    // Test unknown placeholder in gate
    let template_toml_gate = r#"
name = "test_gate_unknown"
description = "Test unknown in gate"
iterations = 1

[[nodes]]
id = "test"
type = "shell"
command = "echo test"
gate = "node:{bogus}"
"#;
    std::fs::write(
        workflows_dir.join("test_gate_unknown.toml"),
        template_toml_gate,
    )
    .ok();
    let template =
        pidag::workflow::WorkflowEngine::load_template("test_gate_unknown", Some(&tmpdir))
            .expect("should load template");
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models.clone(),
    };
    assert!(
        pidag::workflow::WorkflowEngine::expand(&template, 1, context).is_err(),
        "should fail with unknown in gate"
    );

    // Test unknown placeholder in verify
    let template_toml_verify = r#"
name = "test_verify_unknown"
description = "Test unknown in verify"
iterations = 1

[[nodes]]
id = "test"
type = "shell"
command = "echo test"
verify = "check {bogus}"
"#;
    std::fs::write(
        workflows_dir.join("test_verify_unknown.toml"),
        template_toml_verify,
    )
    .ok();
    let template =
        pidag::workflow::WorkflowEngine::load_template("test_verify_unknown", Some(&tmpdir))
            .expect("should load template");
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models.clone(),
    };
    assert!(
        pidag::workflow::WorkflowEngine::expand(&template, 1, context).is_err(),
        "should fail with unknown in verify"
    );

    // Test unknown placeholder in depends_on
    let template_toml_depends = r#"
name = "test_depends_unknown"
description = "Test unknown in depends_on"
iterations = 1

[[nodes]]
id = "test"
type = "shell"
command = "echo test"
depends_on = ["node-{bogus}"]
"#;
    std::fs::write(
        workflows_dir.join("test_depends_unknown.toml"),
        template_toml_depends,
    )
    .ok();
    let template =
        pidag::workflow::WorkflowEngine::load_template("test_depends_unknown", Some(&tmpdir))
            .expect("should load template");
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models.clone(),
    };
    assert!(
        pidag::workflow::WorkflowEngine::expand(&template, 1, context).is_err(),
        "should fail with unknown in depends_on"
    );

    // Test unknown placeholder in after
    let template_toml_after = r#"
name = "test_after_unknown"
description = "Test unknown in after"
iterations = 1

[[nodes]]
id = "test"
type = "shell"
command = "echo test"
after = ["node-{bogus}"]
"#;
    std::fs::write(
        workflows_dir.join("test_after_unknown.toml"),
        template_toml_after,
    )
    .ok();
    let template =
        pidag::workflow::WorkflowEngine::load_template("test_after_unknown", Some(&tmpdir))
            .expect("should load template");
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models,
    };
    assert!(
        pidag::workflow::WorkflowEngine::expand(&template, 1, context).is_err(),
        "should fail with unknown in after"
    );
}

// ============================================================================
// R6e: Test multibyte template does not panic
// ============================================================================
#[test]
fn test_multibyte_template_does_not_panic() {
    let tmpdir = PathBuf::from("_tmp/workflow_tests/multibyte_template");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).ok();

    let workflows_dir = tmpdir.join(".pidag").join("workflows");
    std::fs::create_dir_all(&workflows_dir).ok();

    // Template with em dash (multibyte) followed by unknown placeholder
    let template_toml = r#"
name = "multibyte_test"
description = "Template with multibyte character and unknown placeholder"
iterations = 1

[[nodes]]
id = "test"
type = "llm"
prompt = "Process this—{bogus} placeholder"
"#;

    std::fs::write(workflows_dir.join("multibyte_test.toml"), template_toml).ok();

    let template = pidag::workflow::WorkflowEngine::load_template("multibyte_test", Some(&tmpdir))
        .expect("should load multibyte_test template");

    let config = Config::default();
    let context = pidag::workflow::TemplateContext {
        n: 0,
        spec_path: "spec.md".to_string(),
        project_root: tmpdir.display().to_string(),
        validate_script: config.sdd.validate_script.display().to_string(),
        quality_gate_script: config.sdd.quality_gate_script.display().to_string(),
        prompts: HashMap::new(),
        models_config: config.models,
    };

    // Should return a clean error, not panic
    let result = pidag::workflow::WorkflowEngine::expand(&template, 1, context);
    assert!(
        result.is_err(),
        "should return error for unknown placeholder"
    );
    // If we got here without panicking, the test passes
}
